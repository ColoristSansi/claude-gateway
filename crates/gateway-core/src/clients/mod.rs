//! 客户端接入面：三张卡的**实读比对**、切换 / 还原 / 1P↔3P、备份清单（spec §10 第 4 节）。
//!
//! 引擎与会话形状都在别处：[`crate::writer`] 是写入引擎（票 45）、三个客户端的接管键清单在
//! `writer::desktop` / `writer::claude_code` / `writer::codex`（票 46/47/48）。本模块只做
//! **装配**：哪个客户端读哪个根、四态怎么判、一次动作调哪个函数、界面要看的绝对路径与备份路径。
//!
//! - **每次都真读文件、不缓存**（票 17 第 50 条：缓存会撒谎）。
//! - 四态判据：四行接管键全中 = `已生效`；一个网关键都没有且从没写过 = `未配置`；
//!   一个网关键都没有但 `client_config_state.last_op == "restore"` = `已回官方`；其余 = `值不匹配`。
//! - **「未登录」不是阻断性错误**（spec §8.3）：只在 `notes` 里给一句说明。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::binding::ExposedModel;
use crate::binding::Profile;
use crate::store::Store;
use crate::writer::{
    claude_code, codex, desktop, ConflictMode, FileInspect, FileSpec, WriteReport, WriteRoots,
    Writer,
};

/// 三个客户端（逐字，spec §10 第 4 节；Codex 保持**一张卡**，票 25）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientId {
    Desktop,
    ClaudeCode,
    Codex,
}

impl ClientId {
    pub const ALL: [ClientId; 3] = [ClientId::Desktop, ClientId::ClaudeCode, ClientId::Codex];

    /// 与 `writer::*::CLIENT` 同一套串（备份子目录名与 `client_config_state.client` 都用它）。
    pub fn id(self) -> &'static str {
        match self {
            ClientId::Desktop => desktop::CLIENT,
            ClientId::ClaudeCode => claude_code::CLIENT,
            ClientId::Codex => codex::CLIENT,
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            ClientId::Desktop => "Claude Desktop",
            ClientId::ClaudeCode => "Claude Code",
            ClientId::Codex => "Codex",
        }
    }

    /// 这个客户端在绑定表里用的档案（票 51 的每客户端覆盖）。
    pub fn profile(self) -> Profile {
        match self {
            ClientId::Desktop => Profile::ClaudeDesktop,
            ClientId::ClaudeCode => Profile::ClaudeCode,
            ClientId::Codex => Profile::Codex,
        }
    }

    pub fn parse(id: &str) -> Option<ClientId> {
        match id.trim() {
            "claude-desktop" => Some(ClientId::Desktop),
            "claude-code" => Some(ClientId::ClaudeCode),
            "codex" => Some(ClientId::Codex),
            _ => None,
        }
    }
}

/// 三个客户端的配置根 + 备份 / 日志根。**全部参数化**：测试只指临时目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEnv {
    pub desktop_root: PathBuf,
    pub claude_code_root: PathBuf,
    pub codex_root: PathBuf,
    pub backup_root: PathBuf,
    pub log_root: PathBuf,
}

impl ClientEnv {
    /// 三个客户端各用 `<root>\<客户端 id>`（测试与便携场景；真实根见 [`ClientEnv::production`]）。
    pub fn under(root: impl Into<PathBuf>) -> ClientEnv {
        let root = root.into();
        ClientEnv {
            desktop_root: root.join("claude-desktop"),
            claude_code_root: root.join("claude-code"),
            codex_root: root.join("codex"),
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        }
    }

    /// 真实布局（spec §8.1 / §8.3）：Desktop `%LOCALAPPDATA%\Claude-3p\configLibrary`、
    /// Claude Code `%USERPROFILE%\.claude`、Codex `$CODEX_HOME` 或 `%USERPROFILE%\.codex`；
    /// 备份 / 日志挂网关数据目录。**只算路径，不创建。**
    pub fn production(data_dir: &Path) -> Result<ClientEnv> {
        let desktop_root = desktop::config_library_root()
            .ok_or_else(|| anyhow!("LOCALAPPDATA 未设置，无法定位 Claude Desktop 配置目录"))?;
        Ok(ClientEnv {
            desktop_root,
            claude_code_root: claude_code::default_config_root()?,
            codex_root: codex::home()?,
            backup_root: data_dir.join("backups"),
            log_root: data_dir.join("logs"),
        })
    }

    pub fn config_root(&self, id: ClientId) -> &Path {
        match id {
            ClientId::Desktop => &self.desktop_root,
            ClientId::ClaudeCode => &self.claude_code_root,
            ClientId::Codex => &self.codex_root,
        }
    }

    /// 这个客户端的写入引擎（配置根 = 它自己的根，备份 / 日志挂数据目录）。
    pub fn writer(&self, store: &Store, id: ClientId) -> Writer {
        Writer::new(
            store.clone(),
            WriteRoots {
                config_root: self.config_root(id).to_path_buf(),
                backup_root: self.backup_root.clone(),
                log_root: self.log_root.clone(),
            },
        )
    }

    /// 已安装判定（不启客户端、不读注册表）：配置目录在 = 装过。
    /// shortcut: 只看配置目录；「装在别处、配置目录还没建」的机器会显示未检测到安装。
    pub fn installed(&self, id: ClientId) -> bool {
        self.config_root(id).is_dir()
    }
}

/// 网关要写进客户端的那几个值（端口变了就要重建：`base_url` 跟着端口走）。
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayValues {
    /// 网关根地址，如 `http://127.0.0.1:8787`（**不带 `/v1`**；Desktop 与 Codex 各自按需处理）。
    pub base_url: String,
    /// 客户端填的占位 key（3P 允许占位，§8.3；真实上游凭据在网关侧）。
    pub api_key: String,
    /// Claude Code 的四个家族槽位取值（可换成该客户端的覆盖名）。
    pub models: claude_code::ModelSlots,
    /// Codex 目录内容（**按 codex 档案解析**的三元组描述，票 31）。
    pub exposed: Vec<ExposedModel>,
    /// Codex `config.toml` 的 `model`：默认用第一个已绑定的家族槽位。
    pub default_model: String,
}

/// 卡片状态（逐字，spec §10 第 4 节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientState {
    /// 文件里一个网关键都没有，也从没由网关写过。
    NotConfigured,
    /// 三个文件的接管键全部等于网关要写的值。
    Effective,
    /// 有网关的痕迹但值与期望不一致（含被手改过）。
    Mismatch,
    /// 网关写过又被还原（`last_op == restore`），现在回到官方态。
    Official,
}

impl ClientState {
    pub fn label(self) -> &'static str {
        match self {
            ClientState::NotConfigured => "未配置",
            ClientState::Effective => "已生效",
            ClientState::Mismatch => "值不匹配",
            ClientState::Official => "已回官方",
        }
    }
}

/// 卡片上一份文件的实读结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientFileStatus {
    /// 绝对路径（二次确认弹窗与卡片都要列它）。
    pub path: String,
    pub exists: bool,
    pub error: Option<String>,
    pub fields: Vec<crate::writer::FieldDiff>,
    /// 现在是否已是网关要写的样子。
    pub gateway_state: bool,
    /// 这次动作会落到的备份路径（预告，不写盘）。
    pub backup_preview: String,
    /// 上一次动作（`write` / `restore`）与首写快照。
    pub last_op: Option<String>,
    pub first_backup: Option<String>,
}

/// 一张卡。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub id: String,
    pub display: String,
    pub state: ClientState,
    pub state_label: String,
    pub installed: bool,
    /// 检测到的客户端版本；检测不到就是 `None`（界面显示「未检测到版本」，不猜）。
    pub version: Option<String>,
    pub files: Vec<ClientFileStatus>,
    /// 普通说明（未登录不是错误、改完要完全退出重启、Desktop 的 baseUrl 不带 `/v1`）。
    pub notes: Vec<String>,
    /// 阻断性问题（解析失败 / 目录文件被外部改过）。
    pub problem: Option<String>,
}

/// 一份文件实读 + 记录（四次读盘，不缓存）。
async fn inspect_file(writer: &Writer, spec: &FileSpec) -> Result<(FileInspect, ClientFileStatus)> {
    let inspect = writer.inspect(spec).await;
    let rec = writer.recorded(&spec.client, &spec.rel_path).await?;
    let status = ClientFileStatus {
        path: inspect.path.to_string_lossy().to_string(),
        exists: inspect.exists,
        error: inspect.error.clone(),
        fields: inspect.fields.clone(),
        gateway_state: inspect.is_gateway_state(),
        backup_preview: writer.preview_backup(spec).to_string_lossy().to_string(),
        last_op: rec.as_ref().map(|r| r.last_op.clone()),
        first_backup: rec
            .as_ref()
            .and_then(|r| r.backup_path.as_ref())
            .map(|p| p.to_string_lossy().to_string()),
    };
    Ok((inspect, status))
}

/// 一次动作要改的文件清单（与写入引擎用的是同一份清单，不另造一套）。
pub fn specs(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<Vec<FileSpec>> {
    match id {
        ClientId::Desktop => desktop::specs(
            &env.desktop_root,
            &desktop::DesktopConfig::new(&v.base_url, &v.api_key),
        ),
        ClientId::ClaudeCode => Ok(vec![claude_code::file_spec(
            &v.base_url,
            &v.api_key,
            &v.models,
        )]),
        ClientId::Codex => {
            let cw = codex::CodexWriter::new(env.writer(store, id), &v.base_url);
            Ok(cw.plan(&v.exposed, &v.default_model)?.files)
        }
    }
}

/// 实读三张卡（或一张）：**每次进页面都调它**，没有缓存。
pub async fn status(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<ClientStatus> {
    let writer = env.writer(store, id);
    let specs = specs(env, store, id, v)?;

    let mut files = Vec::with_capacity(specs.len());
    let mut inspects = Vec::with_capacity(specs.len());
    for spec in &specs {
        let (inspect, status) = inspect_file(&writer, spec).await?;
        inspects.push(inspect);
        files.push(status);
    }

    let any_exists = inspects.iter().any(|i| i.exists);
    let all_gateway = !inspects.is_empty() && inspects.iter().all(FileInspect::is_gateway_state);
    let no_gateway_keys = inspects.iter().all(FileInspect::has_no_gateway_keys);
    let restored = {
        let mut rows = Vec::new();
        for spec in &specs {
            rows.push(writer.recorded(&spec.client, &spec.rel_path).await?);
        }
        rows.iter().flatten().any(|r| r.last_op == "restore")
    };

    let state = if all_gateway {
        ClientState::Effective
    } else if no_gateway_keys && restored {
        ClientState::Official
    } else if !any_exists || no_gateway_keys {
        ClientState::NotConfigured
    } else {
        ClientState::Mismatch
    };

    let problem = inspects
        .iter()
        .find_map(|i| i.error.clone())
        .map(|e| format!("配置文件读不了 / 解析不了，状态判不了：{e}"));

    let mut notes = Vec::new();
    match id {
        ClientId::Desktop => {
            notes.push(
                "配置改完必须**完全退出并重开** Claude Desktop 才生效（它不做热重载）。"
                    .to_string(),
            );
            notes.push(
                "「未登录」不是错误：3P 用静态 API key，允许占位（§8.3），与 1P 的登录态互不影响。"
                    .to_string(),
            );
            notes.push(
                "inferenceGatewayBaseUrl **不带 `/v1`**：Desktop 自己追加（填 /v1 会得到 /v1/v1/messages）。"
                    .to_string(),
            );
        }
        ClientId::ClaudeCode => {
            notes.push(
                "只写 ANTHROPIC_AUTH_TOKEN（写 ANTHROPIC_API_KEY 需一次交互批准，会静默失灵）。"
                    .to_string(),
            );
            notes.push("「未登录」不是错误：Claude Code 零凭据也能用网关（§8.3）。".to_string());
        }
        ClientId::Codex => {
            notes.push(
                "Codex CLI 与桌面应用**同一张卡**（共用 ~/.codex/config.toml）。".to_string(),
            );
            notes.push(
                "改完要完全退出重开；「未登录」只影响云线程，不影响模型选择器与网关请求。"
                    .to_string(),
            );
            notes.push(
                "目录文件（model_catalog_json）不是网关写的那份时按冲突处理：先「放弃」，确认后才「认领并覆盖」。"
                    .to_string(),
            );
        }
    }

    Ok(ClientStatus {
        id: id.id().to_string(),
        display: id.display().to_string(),
        state,
        state_label: state.label().to_string(),
        installed: env.installed(id) || any_exists,
        version: detect_version(id),
        files,
        notes,
        problem,
    })
}

/// 客户端版本：从装机目录里的版本文件读。
///
/// shortcut: 只认几个已知的版本落点；读不到就回 `None`（界面显示「未检测到版本」，
/// 不猜、不联网）。真要全客户端通用得等有各家的安装清单。
fn detect_version(id: ClientId) -> Option<String> {
    let candidates: [PathBuf; 2] = match id {
        ClientId::Desktop => {
            let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
            [
                base.join("AnthropicClaude").join("version"),
                base.join("Claude-3p").join("version"),
            ]
        }
        ClientId::ClaudeCode => {
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
            [
                home.join(".claude").join("version"),
                home.join(".claude.json"),
            ]
        }
        ClientId::Codex => {
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
            [
                home.join(".codex").join("version"),
                home.join(".codex").join("version.json"),
            ]
        }
    };
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let trimmed = text.trim();
            if !trimmed.is_empty() && trimmed.len() < 64 {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 一次动作的结果（界面直接渲染；每文件的绝对路径 + 备份 + 那句人话）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReport {
    pub client: String,
    pub action: String,
    pub message: String,
    pub files: Vec<ActionFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionFile {
    pub path: String,
    pub outcome: serde_json::Value,
    pub backup: Option<String>,
    pub notice: Option<String>,
}

/// 把引擎回执翻成界面要的形状（绝对路径由 `specs` 对齐）。
fn report(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    action: &str,
    specs: &[FileSpec],
    report: &WriteReport,
) -> ActionReport {
    let writer = env.writer(store, id);
    let mut files = Vec::with_capacity(report.files.len());
    let mut message = String::new();
    for (rel, outcome) in &report.files {
        let spec = specs.iter().find(|s| &s.rel_path == rel);
        let abs = spec
            .map(|s| writer.abs_path(s))
            .unwrap_or_else(|| env.config_root(id).join(rel));
        let (backup, notice, line) = match outcome {
            crate::writer::FileOutcome::Unchanged => (
                None,
                None,
                format!("{}：已是最新，一个字节都没动", abs.display()),
            ),
            crate::writer::FileOutcome::Written {
                backup,
                marker_only,
                first_write,
                notice,
            } => (
                Some(backup.to_string_lossy().to_string()),
                notice.clone(),
                format!(
                    "{}：已写入{}；备份：{}{}",
                    abs.display(),
                    if *first_write {
                        "（首次接管）"
                    } else {
                        ""
                    },
                    backup.display(),
                    if *marker_only {
                        "（原文件不存在，只记 marker）"
                    } else {
                        ""
                    },
                ),
            ),
            crate::writer::FileOutcome::Removed { backup, .. } => (
                Some(backup.to_string_lossy().to_string()),
                None,
                format!("{}：已整个删掉（网关当初新建的）", abs.display()),
            ),
            crate::writer::FileOutcome::Conflict { path, .. } => (
                None,
                None,
                format!(
                    "{}：冲突，本次一个字节都没写（接管键 {} 被外部改过）",
                    abs.display(),
                    path.join(".")
                ),
            ),
        };
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&line);
        files.push(ActionFile {
            path: abs.to_string_lossy().to_string(),
            outcome: serde_json::to_value(outcome).unwrap_or(serde_json::Value::Null),
            backup,
            notice,
        });
    }
    ActionReport {
        client: id.id().to_string(),
        action: action.to_string(),
        message,
        files,
    }
}

/// 一键切换（Desktop = 切到 3P；Codex 冲突时用 [`apply_claim`]）。
pub async fn apply(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<ActionReport> {
    let specs = specs(env, store, id, v)?;
    match id {
        ClientId::Desktop => {
            let cfg = desktop::DesktopConfig::new(&v.base_url, &v.api_key);
            let writer = env.writer(store, id);
            let rep = desktop::switch_to_3p(&writer, &cfg).await?;
            Ok(report(env, store, id, "apply", &specs, &rep))
        }
        ClientId::ClaudeCode => {
            let writer = env.writer(store, id);
            let outcome = writer.write_one(&specs[0], ConflictMode::Abort).await?;
            Ok(report(
                env,
                store,
                id,
                "apply",
                &specs,
                &WriteReport {
                    files: vec![(specs[0].rel_path.clone(), outcome)],
                },
            ))
        }
        ClientId::Codex => {
            let cw = codex::CodexWriter::new(env.writer(store, id), &v.base_url);
            let rep = cw.write(&v.exposed, &v.default_model).await?;
            Ok(report(env, store, id, "apply", &specs, &rep))
        }
    }
}

/// 「认领并覆盖」：只有 Codex 有这条（`model_catalog_json` 被外部改过，spec §10 第 4 节）。
pub async fn apply_claim(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<ActionReport> {
    if id != ClientId::Codex {
        return Err(anyhow!("「认领并覆盖」只对 Codex 的模型目录有定义"));
    }
    let specs = specs(env, store, id, v)?;
    let cw = codex::CodexWriter::new(env.writer(store, id), &v.base_url);
    let rep = cw.claim(&v.exposed, &v.default_model).await?;
    Ok(report(env, store, id, "claim", &specs, &rep))
}

/// 一键还原（默认摘键；Codex 目录被外部改过时那一条会以 `Conflict` 如实回报）。
pub async fn restore(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<ActionReport> {
    let specs = specs(env, store, id, v)?;
    match id {
        ClientId::Desktop => {
            let cfg = desktop::DesktopConfig::new(&v.base_url, &v.api_key);
            let writer = env.writer(store, id);
            let rep = desktop::switch_to_1p(&writer, &cfg).await?;
            Ok(report(env, store, id, "restore", &specs, &rep))
        }
        ClientId::ClaudeCode => {
            let writer = env.writer(store, id);
            let outcome = writer.restore_one(&specs[0]).await?;
            Ok(report(
                env,
                store,
                id,
                "restore",
                &specs,
                &WriteReport {
                    files: vec![(specs[0].rel_path.clone(), outcome)],
                },
            ))
        }
        ClientId::Codex => {
            let cw = codex::CodexWriter::new(env.writer(store, id), &v.base_url);
            let rep = cw.restore().await?;
            Ok(report(env, store, id, "restore", &specs, &rep))
        }
    }
}

/// Desktop 的 1P↔3P 快切（托盘那一项复用同一个动作，spec §10 托盘段）。
pub async fn desktop_mode(
    env: &ClientEnv,
    store: &Store,
    v: &GatewayValues,
    mode: &str,
) -> Result<ActionReport> {
    match mode {
        "3p" => apply(env, store, ClientId::Desktop, v).await,
        "1p" => restore(env, store, ClientId::Desktop, v).await,
        other => Err(anyhow!("未知的 Desktop 模式 {other:?}：只有 1p / 3p")),
    }
}

/// 这个客户端现在有没有网关痕迹（`一键切换全部可用客户端` 跳过「已生效」用它）。
pub async fn is_effective(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<bool> {
    Ok(status(env, store, id, v).await?.state == ClientState::Effective)
}

/// 卡片要列的历史备份（新到旧；含首写快照）。
pub async fn backups(
    env: &ClientEnv,
    store: &Store,
    id: ClientId,
    v: &GatewayValues,
) -> Result<Vec<String>> {
    let writer = env.writer(store, id);
    let mut out = Vec::new();
    for spec in specs(env, store, id, v)? {
        for path in writer.list_backups(&spec) {
            out.push(path.to_string_lossy().to_string());
        }
        if let Some(first) = writer
            .recorded(&spec.client, &spec.rel_path)
            .await?
            .and_then(|r| r.backup_path)
        {
            out.push(first.to_string_lossy().to_string());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}
