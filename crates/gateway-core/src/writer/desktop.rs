//! Claude Desktop 写入器（票 46）：`%LOCALAPPDATA%\Claude-3p\configLibrary\` 的 profile JSON 与
//! `_meta.json`，1P ↔ 3P 一键快切。
//!
//! 这一层是**客户端逻辑**，不是引擎：它知道 Desktop 的键名、`_meta.json` 的形状、§8.2 的模型 ID
//! 判据与「先 profile、后指针」的依赖序；真正落盘（保序改写 / 冲突 / 幂等 / 备份 / 原子替换 /
//! 记账）全交给[写入引擎](super::Writer)，引擎一行都不用改。
//!
//! # 写什么
//!
//! profile JSON 的六个接管键（[`GATEWAY_KEYS`]，spec §8 表 + 票 46）：`inferenceProvider="gateway"`
//! 是**激活动词**；`inferenceGatewayBaseUrl` **不带 `/v1`**（§8.1 第 1 条：Desktop 自己追加，写了
//! `/v1` 会得到 `/v1/v1/messages`）；`inferenceModels` 填**完整模型 ID**（§8.1 第 2 条：于是 app
//! 跳过 `GET /v1/models`）；`inferenceGatewayApiKey` 允许占位（§8.3）；`inferenceCredentialKind`
//! 取 `static`（[`CREDENTIAL_KIND_STATIC`] 的裁决与风险见该常量的文档）。`_meta.json` 侧只动网关那
//! 一条 `entries` 与 `appliedId`，**别的 profile 一个字节都不碰**（票 46 / §8.4 第 8 条）。
//!
//! # 1P ↔ 3P
//!
//! - [`switch_to_3p`]：写 profile → 再写 `_meta.json`（先被引用者、后指针，§8.4 第 6 条）。
//! - [`switch_to_1p`]：**摘键**，不还原备份（§8 规则 5）——`inferenceProvider` 一摘即回标准模式，
//!   而 1P/3P 的 userData 与登录态本来就隔离（§8.1 第 3 条）。唯一的例外是
//!   `inferenceCredentialKind`：它会覆盖用户原有的凭据模式，还原时按首写快照**写回旧值**
//!   （[`OwnedOp::RestoreFirstWrite`]，票 46:69 的风险②）。
//! - [`rollback`]：显式第二选项，按首写快照整份回滚；「文件被外部改过就拒绝」由引擎判。
//!
//! # 根是参数
//!
//! [`config_library_root`] 从 `%LOCALAPPDATA%` 拼真实根，**只有产品路径用它**：测试一律把
//! [`super::WriteRoots::config_root`] 指向临时目录，真实 `Claude-3p` 一个字节都不碰（票 45 遗留 2）。

use anyhow::{Context, Result};
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};

use super::doc::Doc;
use super::plan::{FileSpec, Format, OwnedOp, RollbackOutcome, WritePlan, WriteReport};
use super::Writer;

/// 客户端标识：备份子目录名与 `client_config_state.client` 都用它。
pub const CLIENT: &str = "claude-desktop";

/// 网关 profile 的固定 id：**固定才幂等**，否则每次切换都会在 `_meta.json` 的 `entries` 里堆一条
/// （原型 `desktop-3p.ps1:18-19` 的做法；本机既有的 3P profile 就是它）。
pub const PROFILE_ID: &str = "9f1c2b3a-4d5e-4f60-8a71-000000000001";

/// `entries` 里给用户看的名字。
pub const PROFILE_NAME: &str = "Claude Gateway";

/// `_meta.json`：激活链的指针文件（`appliedId` → `<id>.json` 的 `inferenceProvider`）。
pub const META_FILE: &str = "_meta.json";

/// `inferenceGatewayAuthScheme` 默认值（`client-integration.md:24`）。
pub const AUTH_SCHEME_BEARER: &str = "bearer";

/// `inferenceCredentialKind = "static"`。
///
/// 票 46 的接管键清单漏列了这个键，原型 `desktop-3p.ps1:25,62` 会写它，票 15 有记录。**裁决：进清单。**
/// 依据：装机版 app.asar 的凭据层只有 `inferenceCredentialKind` 取交互式（`interactive`）时才抛
/// `signInRequiredMessage`（`research/client-login-state.md`），而 §8.3 的「3P 不需要官方登录态、
/// key 允许占位」正是靠静态路径成立；票 15 的端到端验收也是在写了这个键的配置上跑通的。
/// 它会覆盖用户原有的取值（例如 SSO 的 `interactive`），所以**还原时写回首写快照里的旧值**
/// （[`OwnedOp::RestoreFirstWrite`]）——评审「高」第 4 条修的就是这条：从前摘键 = 静默丢掉用户的选择。
pub const CREDENTIAL_KIND_STATIC: &str = "static";

/// 四个家族槽位名（spec §7）：网关侧的路由名，也是写进 `inferenceModels` 的完整模型 ID。
/// 四个名字都含 `claude`，天然通过 §8.2 判据。
pub const FAMILY_MODELS: [&str; 4] = [
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
];

/// profile JSON 的接管键清单（顺序 = 写入顺序 = 冲突比对顺序）。
pub const GATEWAY_KEYS: [&str; 6] = [
    "inferenceProvider",
    "inferenceGatewayBaseUrl",
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
    "inferenceCredentialKind",
    "inferenceModels",
];

const ORDER_PROFILE: u32 = 0;
const ORDER_META: u32 = 1;

const TIERS: [&str; 5] = ["sonnet", "opus", "haiku", "fable", "mythos"];

/// `qSe` 的纯子串部分（票 16 §1 逐字抄；`\b` / `\d` / 转义的那几条见下面两张表）。
const VENDOR_SUBSTRINGS: [&str; 50] = [
    "ark-code",
    "astron",
    "command-r",
    "deepseek",
    "doubao",
    "gemini",
    "gemma",
    "glm",
    "gpt",
    "grok",
    "hermes",
    "hy3",
    "kimi",
    "lfm",
    "llama",
    "longcat",
    "mimo",
    "minimax",
    "mistral",
    "mixtral",
    "moonshot",
    "nemotron",
    "openai",
    "phi-",
    "qianfan",
    "qwen",
    "tc-code",
    "yi-",
    "stepfun",
    "step-3",
    "seed-",
    "bytedance",
    "hunyuan",
    "granite",
    "amazon.nova",
    "nova-",
    "devstral",
    "ministral",
    "ernie",
    "codex",
    "arcee",
    "trinity",
    "abab",
    "jamba",
    "arctic",
    "solar",
    "mercury",
    "zamba",
    "kat-coder",
    "dpsk/",
];

/// `qSe` 里 `\b<词>\b` 的两头词界条（`\bling\b`、`\bunic\b`）。
const VENDOR_WORDS_BOTH: [&str; 2] = ["ling", "unic"];

/// `qSe` 里只有**前**词界的条（`\bds-`、`\bk2\.`、`\bm2\.`）。
const VENDOR_WORDS_START: [&str; 3] = ["ds-", "k2.", "m2."];

/// 子串表 `KSe`（票 16）。
const ANTHROPIC_SUBSTRINGS: [&str; 7] = [
    "claude",
    "sonnet",
    "opus",
    "haiku",
    "fable",
    "mythos",
    "anthropic",
];

/// 写一份 3P 配置需要的东西（其余键都有默认值，见 [`DesktopConfig::new`]）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopConfig {
    /// 网关的 base URL，**不带 `/v1`**（[`DesktopConfig::new`] 会把多余的 `/v1` 与结尾 `/` 去掉）。
    pub base_url: String,
    /// `inferenceGatewayApiKey`：3P 允许占位（§8.3），真实 key 由调用方给。
    pub api_key: String,
    /// `inferenceGatewayAuthScheme`：默认 `bearer`。
    pub auth_scheme: String,
    /// `inferenceCredentialKind`：默认 `static`。
    pub credential_kind: String,
    /// `inferenceModels` 的完整模型 ID（默认四个家族名）。
    pub models: Vec<String>,
    /// `_meta.json` 的 `entries` 里网关那一条的 `name`。
    pub profile_name: String,
}

impl DesktopConfig {
    /// 默认：四个家族名、`bearer`、`static`、名字 [`PROFILE_NAME`]。
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> DesktopConfig {
        DesktopConfig {
            base_url: normalize_base_url(&base_url.into()),
            api_key: api_key.into(),
            auth_scheme: AUTH_SCHEME_BEARER.to_string(),
            credential_kind: CREDENTIAL_KIND_STATIC.to_string(),
            models: FAMILY_MODELS.map(str::to_string).to_vec(),
            profile_name: PROFILE_NAME.to_string(),
        }
    }

    /// 换模型列表（完整模型 ID）。**不做静默剔除**：非法名写下去 app 会逐条剔掉并留一条 warn
    /// （票 16 §2），要提前发现就用 [`filter_anthropic_models`] 预检（spec §13 的可选预检）。
    pub fn with_models(mut self, models: Vec<String>) -> DesktopConfig {
        self.models = models;
        self
    }

    fn models_json(&self) -> Json {
        Json::Array(self.models.iter().map(|m| json!({ "name": m })).collect())
    }
}

/// 真实配置根：`%LOCALAPPDATA%\Claude-3p\configLibrary`（**只有产品路径用它**）。
pub fn config_library_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| config_library_root_under(Path::new(&d)))
}

/// 纯函数版（好断言）：`<local_app_data>\Claude-3p\configLibrary`。
pub fn config_library_root_under(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Claude-3p").join("configLibrary")
}

/// 网关 profile 的文件名（相对配置根）：`<固定 id>.json`。
pub fn profile_rel_path() -> PathBuf {
    PathBuf::from(format!("{PROFILE_ID}.json"))
}

// ── §8.2 模型 ID 判据（票 16 逐字） ─────────────────────────────────────────

/// Desktop 会不会收下这个模型 ID（票 16 §1 的 `Io(e)`）：
/// 先过**非 Anthropic 厂商黑名单**（命中即非法，含 `claude` 也救不回），再过「裸档位名」或
/// 「子串含 `claude` / 档位名 / `anthropic`」。大小写不敏感。
pub fn is_anthropic_model_id(name: &str) -> bool {
    let lower = name.to_lowercase();
    if vendor_blacklisted(&lower) {
        return false;
    }
    is_bare_tier(&lower) || ANTHROPIC_SUBSTRINGS.iter().any(|s| lower.contains(s))
}

/// 写出前的本地预检：**逐条剔除**、不做 fail-all（票 16 §2 的处置），返回（保留, 剔除）。
/// app 自己也会再剔一遍；这里只是省一次重启，并在界面上早一步告诉用户哪条名字不可用。
pub fn filter_anthropic_models(names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for name in names {
        if is_anthropic_model_id(name) {
            kept.push(name.clone());
        } else {
            dropped.push(name.clone());
        }
    }
    (kept, dropped)
}

fn vendor_blacklisted(lower: &str) -> bool {
    VENDOR_SUBSTRINGS.iter().any(|w| lower.contains(w))
        || VENDOR_WORDS_BOTH
            .iter()
            .any(|w| bounded(lower, w, true))
        || VENDOR_WORDS_START
            .iter()
            .any(|w| bounded(lower, w, false))
        // `phi\d`
        || lower
            .match_indices("phi")
            .any(|(i, _)| lower.as_bytes().get(i + 3).is_some_and(u8::is_ascii_digit))
}

/// `word` 命中且词界满足（`both_ends` = 前后都要是词界，对应 `\bling\b` / `\bds-` 两类）。
fn bounded(hay: &str, word: &str, both_ends: bool) -> bool {
    hay.match_indices(word).any(|(i, _)| {
        let start_ok = i == 0 || !is_word_byte(hay.as_bytes()[i - 1]);
        let end = i + word.len();
        let end_ok = end == hay.len() || !is_word_byte(hay.as_bytes()[end]);
        start_ok && (!both_ends || end_ok)
    })
}

/// JS 正则里 `\w` 的 ASCII 定义（`\b` 因此只在 `[A-Za-z0-9_]` 与其它字符之间成立）。
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 裸档位名：`^(sonnet|opus|haiku|fable|mythos)(-[\d.]+)?$`。
fn is_bare_tier(lower: &str) -> bool {
    TIERS.iter().any(|tier| match lower.strip_prefix(tier) {
        None => false,
        Some("") => true,
        Some(rest) => rest.strip_prefix('-').is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        }),
    })
}

// ── 接管键清单 → FileSpec ──────────────────────────────────────────────────

fn keys(segments: &[&str]) -> Vec<String> {
    segments.iter().map(|s| (*s).to_string()).collect()
}

fn profile_write_ops(cfg: &DesktopConfig) -> Vec<OwnedOp> {
    GATEWAY_KEYS
        .iter()
        .map(|key| OwnedOp::Set {
            path: vec![(*key).to_string()],
            value: match *key {
                "inferenceProvider" => json!("gateway"),
                "inferenceGatewayBaseUrl" => json!(cfg.base_url),
                "inferenceGatewayApiKey" => json!(cfg.api_key),
                "inferenceGatewayAuthScheme" => json!(cfg.auth_scheme),
                "inferenceCredentialKind" => json!(cfg.credential_kind),
                "inferenceModels" => cfg.models_json(),
                other => unreachable!("[`GATEWAY_KEYS`] 与这里的取值表不同步：{other}"),
            },
        })
        .collect()
}

fn profile_restore_ops() -> Vec<OwnedOp> {
    GATEWAY_KEYS
        .iter()
        .map(|key| {
            let path = vec![(*key).to_string()];
            if *key == "inferenceCredentialKind" {
                // 这个键会覆盖用户原有的凭据模式（SSO 的 `interactive`）：还原要写回旧值，
                // 摘键等于静默丢掉用户的选择（票 46:69 的风险②）。
                OwnedOp::RestoreFirstWrite { path }
            } else {
                OwnedOp::Remove { path }
            }
        })
        .collect()
}

/// `_meta.json` 还原时的两个动态判据：都要看**当前磁盘内容**，由 [`meta_restore`] 先算好。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaRestore {
    /// `appliedId` 还指着网关时回退到哪：`Some(id)` 换成它；`None` 摘掉这个键。
    /// 「回退到剩下条目里的第一条」——本机 pristine 1P 的 `_meta.json` 正是
    /// `appliedId = <Default 的 id>`（`configLibrary.backup-20261014-191819`），所以这是能回到 1P 原状的值。
    pub applied_fallback: Option<Json>,
    /// 摘掉网关条目后 `entries` 会空 → 连 `entries` 键一起摘：网关新建的 `_meta.json` 才能整份删掉
    /// （引擎的「只剩网关内容就删文件」判据看的是顶层是否空）。
    pub drop_entries_when_empty: bool,
}

/// profile 的接管键清单（写 + 还原两面）。还原对 `inferenceCredentialKind` 写回首写快照里的旧值，
/// 其余接管键摘掉。
pub fn profile_spec(cfg: &DesktopConfig) -> FileSpec {
    FileSpec::new(
        CLIENT,
        profile_rel_path(),
        Format::Jsonc,
        profile_write_ops(cfg),
    )
    .with_restore(profile_restore_ops())
    .with_order(ORDER_PROFILE)
}

/// `_meta.json` 的接管键清单：写侧**只** upsert 网关那条 `entries` + 置 `appliedId`，还原侧摘条目、
/// 条件回退 `appliedId`（指向别处就一个字节不动）。
pub fn meta_spec(cfg: &DesktopConfig, restore: &MetaRestore) -> FileSpec {
    let mut restore_ops = vec![
        OwnedOp::RemoveElement {
            path: keys(&["entries"]),
            field: "id".to_string(),
            matches: json!(PROFILE_ID),
        },
        OwnedOp::ReplaceIfEquals {
            path: keys(&["appliedId"]),
            equals: json!(PROFILE_ID),
            value: restore.applied_fallback.clone(),
        },
    ];
    if restore.drop_entries_when_empty {
        restore_ops.push(OwnedOp::Remove {
            path: keys(&["entries"]),
        });
    }
    FileSpec::new(
        CLIENT,
        META_FILE,
        Format::Jsonc,
        vec![
            OwnedOp::SetElement {
                path: keys(&["entries"]),
                field: "id".to_string(),
                value: json!({ "id": PROFILE_ID, "name": cfg.profile_name }),
            },
            OwnedOp::Set {
                path: keys(&["appliedId"]),
                value: json!(PROFILE_ID),
            },
        ],
    )
    .with_restore(restore_ops)
    .with_order(ORDER_META)
}

/// 读当前 `_meta.json` 算还原判据。文件不在 = 空现场；**解析不过即报错**（形状不对不猜）。
pub fn meta_restore(config_root: &Path) -> Result<MetaRestore> {
    let path = config_root.join(META_FILE);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MetaRestore {
                applied_fallback: None,
                drop_entries_when_empty: true,
            })
        }
        Err(e) => return Err(e).with_context(|| format!("读 {} 失败", path.display())),
    };
    let doc = Doc::parse(Format::Jsonc, Some(&bytes))
        .with_context(|| format!("解析 {} 失败（形状不对不猜）", path.display()))?;
    let entries = doc
        .get(&keys(&["entries"]))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let survivors: Vec<&Json> = entries
        .iter()
        .filter(|e| e.get("id").and_then(Json::as_str) != Some(PROFILE_ID))
        .collect();
    Ok(MetaRestore {
        applied_fallback: survivors.first().and_then(|e| e.get("id")).cloned(),
        drop_entries_when_empty: survivors.is_empty(),
    })
}

/// 一次快切要改的两个文件，按依赖序：先 profile、后 `_meta.json`。
pub fn specs(config_root: &Path, cfg: &DesktopConfig) -> Result<Vec<FileSpec>> {
    let restore = meta_restore(config_root)?;
    Ok(vec![profile_spec(cfg), meta_spec(cfg, &restore)])
}

/// 一次快切的写入计划（交给引擎 `write` / `restore` / `claim`）。
pub fn plan(config_root: &Path, cfg: &DesktopConfig) -> Result<WritePlan> {
    Ok(WritePlan::new(specs(config_root, cfg)?))
}

/// 切到 3P：写 profile → 写 `_meta.json`。
pub async fn switch_to_3p(writer: &Writer, cfg: &DesktopConfig) -> Result<WriteReport> {
    writer
        .write(&plan(&writer.roots().config_root, cfg)?)
        .await
        .context("切到 Claude Desktop 3P 失败")
}

/// 切回 1P：**摘键**（不还原备份）。`_meta.json` 同时摘掉网关条目、`appliedId` 条件回退。
pub async fn switch_to_1p(writer: &Writer, cfg: &DesktopConfig) -> Result<WriteReport> {
    writer
        .restore(&plan(&writer.roots().config_root, cfg)?)
        .await
        .context("切回 Claude Desktop 1P 失败")
}

/// 「整份回滚」第二选项：按首写快照逐字节恢复两个文件（引擎判「被外部改过就拒绝」）。
///
/// 顺序与写入相反——**指针先回退、被引用者后回退**，回滚路径上不留悬空引用。
pub async fn rollback(
    writer: &Writer,
    cfg: &DesktopConfig,
) -> Result<Vec<(PathBuf, RollbackOutcome)>> {
    let specs = specs(&writer.roots().config_root, cfg)?;
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs.iter().rev() {
        out.push((spec.rel_path.clone(), writer.rollback(spec).await?));
    }
    Ok(out)
}

/// 去掉结尾 `/` 与 `/v1`：Desktop 自己追加 `/v1/messages`（§8.1 第 1 条）。
fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}
