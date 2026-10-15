//! Codex 这一条：`%USERPROFILE%\.codex\config.toml` 的接管键 + 模型目录文件（票 48）。
//!
//! 依据：spec §7 末段 + §8 表 + §8.3 / §8.4、[票 31]（目录内容与并存）、[票 32]（原型实测），
//! ADR-0006（接管键清单）。**引擎（票 45）不认客户端的键名**，这一条只做两件事：
//!
//! 1. 拼出 Codex 的 [`FileSpec`]（`config.toml` + `claudegateway-model-catalog.json`），
//!    交给 [`Writer`] 去备份 / 判冲突 / 原子替换 / 记库。
//! 2. 补上引擎表达不了的两件 Codex 专属小事：**哨兵注释行的逐行认领**（见下）与
//!    **目录文件的「逐字节一致才删」**判据。
//!
//! # 接管键（写进 TOML 的只有这些）
//!
//! 顶层 `model_provider` / `model` / `model_catalog_json` + `[model_providers.claudegateway]`
//! 的 `name` / `base_url` / `wire_api` / `http_headers`。**不写** `env_key`（声明了那个变量
//! 就必须真存在，否则 Codex 直接拒发；网关入站不鉴权，实测不带它也能跑——票 24/32），
//! **不写** `requires_openai_auth`，**绝不碰** `auth.json`（票 25 第 4 条）。
//!
//! # 哨兵行为什么要单独认领
//!
//! 原型按「哨兵围出的区域」删，被实测打穿：Codex 桌面应用会把 `[windows] sandbox = "elevated"`
//! 之类的表**插进哨兵之间**（票 32），按区域删就会连它一起删掉。所以这里也是**逐行**：
//! [`mark_managed`] 只增删自己那四行注释，别的一个字节不动（[`strip_managed`] 摘干净）。
//! 哨兵只是给人的记号，判据仍然是引擎的接管键清单。
//!
//! # 模型目录
//!
//! `<CODEX_HOME>/claudegateway-model-catalog.json`（指针写**相对文件名**）；只装网关暴露的
//! 虚拟模型（四个家族名在前 + 普通模型按名排），用中性行模板（`shell_type = "shell_command"`、
//! 无 freeform `apply_patch`）。写前自检不过就**不写**（[`check_catalog`]）；还原时**只在磁盘
//! 内容就是网关最后写的那份字节**才交给引擎删文件，被外部改过则一个字节不动（[`CodexWriter::restore`]）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};

use crate::binding::ExposedModel;
use crate::store::Store;

use super::doc::Doc;
use super::plan::{
    ConflictMode, FileOutcome, FileSpec, Format, OwnedOp, RollbackOutcome, WritePlan, WriteReport,
    WriteRoots,
};
use super::state;
use super::Writer;

/// 客户端标识：备份子目录名与 `client_config_state.client` 都用它。
pub const CLIENT: &str = "codex";
/// provider 表名：`[model_providers.claudegateway]`。
pub const PROVIDER_ID: &str = "claudegateway";
/// 模型目录文件名（指针写**相对文件名**，Codex 按 `CODEX_HOME` 解析；票 31 第 3 条）。
pub const CATALOG_FILE: &str = "claudegateway-model-catalog.json";
/// 目标配置文件名。
pub const CONFIG_FILE: &str = "config.toml";
/// 四个家族槽位与它们在目录里的固定顺序（spec §7、票 31 第 8 条）。
pub const FAMILY_SLOTS: [&str; 4] = [
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
];
/// 上下文窗口：模板默认，不做用户可配（票 31 第 4 条）。
pub const CONTEXT_WINDOW: u64 = 262144;
/// `http_headers` 里带的客户端标识头（静态、网关不校验；见 Answer 的裁决）。
pub const CLIENT_HEADER: &str = "X-Claude-Gateway-Client";

/// 两对哨兵行（逐字取自原型 `codex-gateway.ps1:36-39`）。
pub const TOP_BEGIN: &str = "# >>> claudegateway managed (top-level) >>>";
pub const TOP_END: &str = "# <<< claudegateway managed (top-level) <<<";
pub const TBL_BEGIN: &str = "# >>> claudegateway managed (provider) >>>";
pub const TBL_END: &str = "# <<< claudegateway managed (provider) <<<";

const SENTINELS: [&str; 4] = [TOP_BEGIN, TOP_END, TBL_BEGIN, TBL_END];
/// 中性模板的 `base_instructions`（cc-switch 的 native-responses 模板原文）。
const BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

/// 写入根：配置根 = `$CODEX_HOME`，没设就 `%USERPROFILE%\.codex`。
///
/// 参数化到这一步就够了：测试一律把 `WriteRoots.config_root` 指到临时目录，本函数只在
/// 「真的要写真机」时用（[`CodexWriter::from_home`]）。空白值当没设。
pub fn home_from(codex_home: Option<&str>, user_profile: Option<&str>) -> Result<PathBuf> {
    fn non_blank(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(h) = non_blank(codex_home) {
        return Ok(PathBuf::from(h));
    }
    let profile =
        non_blank(user_profile).context("既没设 CODEX_HOME 也没设 USERPROFILE，拼不出 ~/.codex")?;
    Ok(PathBuf::from(profile).join(".codex"))
}

/// [`home_from`] 的环境变量版。
pub fn home() -> Result<PathBuf> {
    home_from(
        std::env::var("CODEX_HOME").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
    )
}

// ── 模型目录 ───────────────────────────────────────────────────────────────────

/// 一行目录条目的中性模板（字段与取值逐字见原型 `codex-gateway.ps1:154-181`）。
fn neutral_row() -> Json {
    json!({
        "slug": "",
        "display_name": "",
        "description": "",
        "base_instructions": BASE_INSTRUCTIONS,
        "default_reasoning_level": "high",
        "supported_reasoning_levels": [
            { "effort": "none", "description": "Disable Thinking" },
            { "effort": "high", "description": "Enabled Thinking" }
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 0,
        "supports_reasoning_summaries": true,
        "default_reasoning_summary": "none",
        "support_verbosity": false,
        "truncation_policy": { "mode": "bytes", "limit": 10000 },
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": CONTEXT_WINDOW,
        "max_context_window": CONTEXT_WINDOW,
        "effective_context_window_percent": 95,
        "experimental_supported_tools": [],
        "input_modalities": ["text", "image"],
        "supports_search_tool": false
    })
}

/// 目录值：`{"models":[...]}`，四个家族名在前（fable / opus / sonnet / haiku），
/// 普通虚拟模型按名排序；每个条目 = 中性模板 + 覆盖（票 31 第 1、4、8 条）。
///
/// 入参 = 网关暴露的清单（票 51 的 [`ExposedModel`]；`GET /v1/models` 与它同一份来源）。
pub fn catalog_value(models: &[ExposedModel]) -> Json {
    let mut ordered: Vec<&ExposedModel> = models.iter().collect();
    ordered.sort_by_key(|m| family_rank(&m.name));
    let models: Vec<Json> = ordered
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut row = neutral_row();
            row["slug"] = json!(m.name);
            row["display_name"] = json!(m.name);
            row["description"] = json!(match &m.binding {
                Some(b) => format!("Claude Gateway -> {} / {}", b.upstream_id, b.real_model),
                // 家族槽位可以还没绑（票 51 的 `exposed` 照样暴露名字）；目录不能因此不写。
                None => "Claude Gateway（尚未绑定）".to_string(),
            });
            // 原型的 `priority = 1000 + index`（票 31 没定字面值，这里沿用原型）。
            row["priority"] = json!(1000 + i as u64);
            row
        })
        .collect();
    json!({ "models": models })
}

/// 排序键：家族槽位按固定顺序排在最前，其余按名字。
fn family_rank(name: &str) -> (u8, usize, String) {
    match FAMILY_SLOTS.iter().position(|slot| *slot == name) {
        Some(i) => (0, i, String::new()),
        None => (1, 0, name.to_string()),
    }
}

/// 写前自检（票 31 第 7 条）：任一项不过就**不写**，免得 Codex 因为半份目录看不到任何模型。
pub fn check_catalog(catalog: &Json) -> Result<()> {
    let models = catalog
        .get("models")
        .and_then(|m| m.as_array())
        .context("目录自检失败：models 不是数组")?;
    if models.is_empty() {
        bail!("目录自检失败：models 为空（Codex 会看不到任何网关模型）");
    }
    if !models
        .iter()
        .any(|m| m.get("visibility").and_then(Json::as_str) == Some("list"))
    {
        bail!("目录自检失败：没有一条 visibility=list（官方模型会全被藏起来）");
    }
    let non_empty = |v: Option<&Json>| {
        v.and_then(Json::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    };
    let mut seen = HashSet::new();
    for m in models {
        let slug = m.get("slug").and_then(Json::as_str).unwrap_or_default();
        if slug.trim().is_empty() {
            bail!("目录自检失败：slug 有空值");
        }
        if !seen.insert(slug.to_string()) {
            bail!("目录自检失败：slug 重复（{slug}）");
        }
        let has_instructions = non_empty(m.get("base_instructions"))
            || non_empty(m.pointer("/model_messages/instructions_template"));
        if !has_instructions {
            bail!(
                "目录自检失败：{slug} 缺 base_instructions / model_messages.instructions_template"
            );
        }
        for field in [
            "supports_reasoning_summaries",
            "supports_parallel_tool_calls",
        ] {
            if !m.get(field).is_some_and(Json::is_boolean) {
                bail!("目录自检失败：{slug} 缺 {field}");
            }
        }
    }
    Ok(())
}

// ── 哨兵：逐行认领 ─────────────────────────────────────────────────────────────

fn is_sentinel(line: &str) -> bool {
    SENTINELS.contains(&line.trim())
}

/// 顶层接管键行：`^(model_provider|model|model_catalog_json)\s*=`（只认第一个 `[表]` 之前）。
fn is_owned_top(line: &str) -> bool {
    let t = line.trim_start();
    ["model_provider", "model", "model_catalog_json"]
        .iter()
        .any(|k| {
            t.strip_prefix(k)
                .map(|rest| {
                    rest.trim_start().starts_with('=') && !rest.trim_start().starts_with("==")
                })
                .unwrap_or(false)
        })
}

fn is_table_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// 拆行：CRLF 与 LF 都认，末尾换行不算一行内容。
fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn newline_and_trailing(text: &str) -> (&'static str, bool) {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    (nl, text.ends_with('\n'))
}

fn join_lines(lines: &[String], nl: &str, trailing: bool) -> String {
    let mut out = lines.join(nl);
    if trailing && !out.is_empty() {
        out.push_str(nl);
    }
    out
}

/// 摘掉自己那四行哨兵注释（别的一个字节不动）。
pub fn strip_managed(text: &str) -> String {
    let (nl, trailing) = newline_and_trailing(text);
    let lines: Vec<String> = split_lines(text)
        .into_iter()
        .filter(|l| !is_sentinel(l))
        .collect();
    join_lines(&lines, nl, trailing)
}

/// 给接管键加上/修正哨兵行：先摘掉自己那四行，再按当前接管键位置重插。
///
/// 幂等（`mark_managed(mark_managed(x)) == mark_managed(x)`），且**只动这四行**——
/// 插进哨兵之间的别人的表（`[windows]` 之类）原样留在原地（票 32 的实测教训）。
pub fn mark_managed(text: &str) -> String {
    let (nl, trailing) = newline_and_trailing(text);
    let mut lines: Vec<String> = split_lines(text)
        .into_iter()
        .filter(|l| !is_sentinel(l))
        .collect();

    // 顶层接管键：只认第一个 `[表]` 之前那几行。
    let first_table = lines
        .iter()
        .position(|l| is_table_header(l))
        .unwrap_or(lines.len());
    let top: Vec<usize> = (0..first_table)
        .filter(|&i| is_owned_top(&lines[i]))
        .collect();
    // provider 表：从表头到下一个表头；紧挨着的那行空的 `[model_providers]` 父表也是引擎
    // 顺手建的壳（摘键时由 `RemoveIfEmpty` 摘掉），一起圈进哨兵区域内。
    let header = lines
        .iter()
        .position(|l| l.trim() == format!("[{PROVIDER_PREFIX}{PROVIDER_ID}]"));
    let region = header.map(|head| {
        let mut start = head;
        while start > 0 && lines[start - 1].trim().is_empty() {
            start -= 1;
        }
        if start > 0 && lines[start - 1].trim() == "[model_providers]" {
            start -= 1;
        }
        let end = (head + 1..lines.len())
            .find(|&i| is_table_header(&lines[i]))
            .unwrap_or(lines.len());
        (start, end)
    });

    let mut inserts: Vec<(usize, &str)> = Vec::new();
    if let (Some(&first), Some(&last)) = (top.first(), top.last()) {
        inserts.push((first, TOP_BEGIN));
        inserts.push((last + 1, TOP_END));
    }
    if let Some((start, end)) = region {
        inserts.push((start, TBL_BEGIN));
        inserts.push((end, TBL_END));
    }
    // 从后往前插（同一下标时保持「先上后下」的先后：TOP_END 先插、TBL_BEGIN 后插）。
    inserts.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (i, line) in inserts {
        lines.insert(i, line.to_string());
    }
    join_lines(&lines, nl, trailing)
}

/// `[model_providers.` 前缀（拼表头用）。
const PROVIDER_PREFIX: &str = "model_providers.";

// ── 写入器 ─────────────────────────────────────────────────────────────────────

/// Codex 这一条的写入器：把 [`Writer`]（引擎）与 Codex 专属的两件小事绑在一起。
#[derive(Clone)]
pub struct CodexWriter {
    writer: Writer,
    /// 网关监听地址（不带 `/v1`；Codex 侧必须带，与 Desktop 相反）。
    base_url: String,
}

impl CodexWriter {
    pub fn new(writer: Writer, base_url: impl Into<String>) -> CodexWriter {
        CodexWriter {
            writer,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// 真实布局：配置根 = `$CODEX_HOME`（没设就 `%USERPROFILE%\.codex`），
    /// 备份 / 日志挂网关数据目录。
    pub fn from_home(store: Store, base_url: impl Into<String>) -> Result<CodexWriter> {
        let roots = WriteRoots::with_data_dir(home()?, crate::store::default_data_dir()?);
        Ok(CodexWriter::new(Writer::new(store, roots), base_url))
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    pub fn config_path(&self) -> PathBuf {
        self.writer.roots().config_root.join(CONFIG_FILE)
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.writer.roots().config_root.join(CATALOG_FILE)
    }

    /// 目录文件的接管键清单。`models` 只在写的时候有意义；还原 / 回滚只用到路径标签。
    fn catalog_spec(&self, models: Json) -> FileSpec {
        FileSpec::new(
            CLIENT,
            CATALOG_FILE,
            Format::Json,
            vec![OwnedOp::Set {
                path: vec!["models".into()],
                value: models,
            }],
        )
        .with_restore(vec![OwnedOp::Remove {
            path: vec!["models".into()],
        }])
        .with_order(0)
    }

    /// `config.toml` 的接管键清单（`model` 的值只在写的时候有意义）。
    fn config_spec(&self, model: &str) -> FileSpec {
        let provider = |key: &str, value: Json| OwnedOp::Set {
            path: vec!["model_providers".into(), PROVIDER_ID.into(), key.into()],
            value,
        };
        let top = |key: &str, value: Json| OwnedOp::Set {
            path: vec![key.into()],
            value,
        };
        FileSpec::new(
            CLIENT,
            CONFIG_FILE,
            Format::Toml,
            vec![
                top("model_provider", json!(PROVIDER_ID)),
                top("model", json!(model)),
                top("model_catalog_json", json!(CATALOG_FILE)),
                provider("name", json!("Claude Gateway")),
                provider("base_url", json!(format!("{}/v1", self.base_url))),
                provider("wire_api", json!("responses")),
                provider("http_headers", json!({ CLIENT_HEADER: "codex" })),
            ],
        )
        .with_restore(vec![
            OwnedOp::Remove {
                path: vec!["model_provider".into()],
            },
            OwnedOp::Remove {
                path: vec!["model".into()],
            },
            OwnedOp::Remove {
                path: vec!["model_catalog_json".into()],
            },
            // 表 + 空壳：引擎建 `[model_providers.claudegateway]` 时会顺手建出父表 `[model_providers]`，
            // 摘键时父表空了也要摘掉，否则文件回不到原样（别人的 provider 还在就不会摘）。
            OwnedOp::RemoveIfEmpty {
                path: vec!["model_providers".into(), PROVIDER_ID.into()],
            },
        ])
        .with_order(1)
    }

    /// 一次切换的写入计划：目录文件（order 0）→ `config.toml`（order 1，先被引用者后指针）。
    /// 自检不过就返回 `Err`，调用方什么都不写。
    pub fn plan(&self, models: &[ExposedModel], model: &str) -> Result<WritePlan> {
        let catalog = catalog_value(models);
        // 自检走一遍「序列化 → 再解析」：真写下去的就是这份字节（票 31 第 7 条）。
        let text = serde_json::to_string(&catalog).context("目录自检失败：序列化不了")?;
        let reparsed: Json =
            serde_json::from_str(&text).context("目录自检失败：序列化结果不是合法 JSON")?;
        check_catalog(&reparsed)?;
        Ok(WritePlan::new(vec![
            self.catalog_spec(reparsed["models"].clone()),
            self.config_spec(model),
        ]))
    }

    /// 写：目录 + `config.toml`，再整理哨兵行。冲突（引擎放弃本次）时一个字节都不动。
    pub async fn write(&self, models: &[ExposedModel], model: &str) -> Result<WriteReport> {
        let plan = self.plan(models, model)?;
        self.apply(&plan, ConflictMode::Abort).await
    }

    /// 「认领并覆盖」（用户显式确认后）：接管键以网关为准，仍然备份、仍然只动接管键
    /// （ADR-0006 第 4 条、票 31 第 6 条）。
    pub async fn claim(&self, models: &[ExposedModel], model: &str) -> Result<WriteReport> {
        let plan = self.plan(models, model)?;
        self.apply(&plan, ConflictMode::Claim).await
    }

    /// 按依赖序**逐个**写（先目录、后指针）：第一个冲突或失败即停，后面的文件一个字节不动
    /// ——免得留下悬空指针（spec §8.4 第 6 条）。每个文件仍然走引擎（备份 / 判冲突 / 原子替换 /
    /// 记库 / 日志）；之所以不用引擎的多文件计划，是因为集成分支上那一版遇到冲突还会往下走。
    async fn apply(&self, plan: &WritePlan, mode: ConflictMode) -> Result<WriteReport> {
        let mut specs: Vec<&FileSpec> = plan.files.iter().collect();
        specs.sort_by_key(|s| s.order);
        let mut files: Vec<(PathBuf, FileOutcome)> = Vec::new();
        let mut conflicted = false;
        for spec in specs {
            match self.writer.write_one(spec, mode).await {
                Ok(outcome) => {
                    conflicted = matches!(outcome, FileOutcome::Conflict { .. });
                    files.push((spec.rel_path.clone(), outcome));
                    if conflicted {
                        break;
                    }
                }
                Err(e) => {
                    return Err(
                        e.context(format!("Codex 写入中途失败；已改动：{}", changed(&files)))
                    )
                }
            }
        }
        // config.toml 冲突（放弃本次）时一个字节都不动；没走到它也就不用整理哨兵行。
        let config_done = files.iter().any(|(p, _)| p == Path::new(CONFIG_FILE));
        if config_done && !conflicted {
            self.mark_config()?;
        }
        Ok(WriteReport { files })
    }

    /// 还原：默认摘键。`config.toml` 交给引擎（网关新建且摘空则整个删）；目录文件只在
    /// **磁盘内容就是网关最后写的那份字节**时才交给引擎摘键并删文件（票 31 第 3 条 /
    /// 票 48:19），被外部改过则一个字节不动，报 `Conflict` 让界面提示。最后摘掉残留哨兵行。
    pub async fn restore(&self) -> Result<WriteReport> {
        let drift = self.catalog_drift().await?;
        let mut files = vec![
            self.catalog_spec(Json::Array(Vec::new())),
            self.config_spec(""),
        ];
        if drift.is_some() {
            files.remove(0);
        }
        let mut report = self.writer.restore(&WritePlan::new(files)).await?;
        if let Some((recorded, current)) = drift {
            report.files.insert(
                0,
                (
                    PathBuf::from(CATALOG_FILE),
                    FileOutcome::Conflict {
                        path: vec!["models".into()],
                        recorded,
                        current,
                    },
                ),
            );
        }
        self.mark_config()?;
        Ok(report)
    }

    /// 整份回滚（显式第二选项）：目录 → `config.toml`，各自按首写快照逐字节恢复。
    /// 回滚只用 `write_ops` 的**路径标签**判「清单变过没」，值不参与 → 用占位值即可。
    pub async fn rollback(&self) -> Result<Vec<RollbackOutcome>> {
        let mut out = Vec::new();
        for spec in [
            self.catalog_spec(Json::Array(Vec::new())),
            self.config_spec(""),
        ] {
            out.push(self.writer.rollback(&spec).await?);
        }
        Ok(out)
    }

    /// 目录文件被外部改过吗（`Some((记录值, 磁盘值))` = 改过 / 没记录，别动它）。
    ///
    /// 判据是**逐字节**：用当前文件的排版保真 + 记录下来的 `models` 重新序列化一遍，
    /// 与磁盘比。只看语义会漏掉「用户手改过排版」这种改法。
    async fn catalog_drift(&self) -> Result<Option<(Json, Json)>> {
        let path = self.catalog_path();
        let Some(bytes) = read_optional(&path)? else {
            return Ok(None);
        };
        let doc = Doc::parse(Format::Json, Some(&bytes))?;
        let current = doc.get(&["models".into()]).unwrap_or(Json::Null);
        // 没有记录 = 不是网关写的（首次运行 / 库被清 / 换机）：不认领它，也不删它。
        let Some(rec) = state::load(self.writer.store(), CLIENT, CATALOG_FILE).await? else {
            return Ok(Some((Json::Null, current)));
        };
        let recorded = rec.snapshot.values.first().cloned().unwrap_or(Json::Null);
        let mut expected = Doc::empty(Format::Json);
        expected.fidelity = doc.fidelity.clone();
        expected.apply(&OwnedOp::Set {
            path: vec!["models".into()],
            value: recorded.clone(),
        })?;
        if expected.to_bytes()? == bytes {
            return Ok(None);
        }
        Ok(Some((recorded, current)))
    }

    /// 整理 `config.toml` 的哨兵行（内容没变就不写，幂等）。
    ///
    /// 紧跟引擎的写入 / 还原之后跑：引擎刚留过备份、刚记过账，这里只加 / 删四行注释，
    /// 所以不再单独留备份，也不动 `client_config_state`（接管键值没变）。
    fn mark_config(&self) -> Result<()> {
        let path = self.config_path();
        let Some(bytes) = read_optional(&path)? else {
            return Ok(());
        };
        let (bom, text) = decode(&bytes)?;
        let marked = mark_managed(&text);
        if marked == text {
            return Ok(());
        }
        super::atomic::replace(&path, &encode(bom, &marked))
            .with_context(|| format!("整理哨兵行失败：{}", path.display()))?;
        Ok(())
    }
}

/// 读文件；不存在给 `None`（引擎的同名判据在 `engine` 模块里，这里是模块内小工具）。
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("读目标文件失败：{}", path.display())),
    }
}

/// 已改动的文件清单（中途失败时报给用户）。
fn changed(done: &[(PathBuf, FileOutcome)]) -> String {
    let changed: Vec<String> = done
        .iter()
        .filter(|(_, o)| !matches!(o, FileOutcome::Unchanged | FileOutcome::Conflict { .. }))
        .map(|(p, _)| p.display().to_string())
        .collect();
    if changed.is_empty() {
        "（无）".to_string()
    } else {
        changed.join("、")
    }
}

/// UTF-8 BOM 与正文分开处理（哨兵行不该动文件的 BOM）。
fn decode(bytes: &[u8]) -> Result<(bool, String)> {
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body)
        .map_err(|e| anyhow::anyhow!("配置文件不是合法 UTF-8（偏移 {}）", e.valid_up_to()))?;
    Ok((bom, text.to_string()))
}

fn encode(bom: bool, text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(text.as_bytes());
    out
}
