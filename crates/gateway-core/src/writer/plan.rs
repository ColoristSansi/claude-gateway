//! 写入计划的数据形状：**接管键清单是调用方传入的数据**，引擎不认任何客户端的键名。
//!
//! 一个 [`FileSpec`] = 一个「客户端 × 文件」的接管键清单：写要动哪些键（`write_ops`）、
//! 还原时这些键怎么办（`restore_ops`）。票 46 / 47 / 48 各拼自己那份，引擎只负责
//! 「按格式保序改写 + 冲突/幂等/备份/原子替换」。
//!
//! 路径用**段数组**（`["env", "ANTHROPIC_BASE_URL"]`），不用点分字符串：TOML 的键里
//! 本来就允许出现 `.`，点分串会有歧义。

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::path::PathBuf;

/// 目标文件的格式。三种都按「保序改写」处理，解析失败一律即停、绝不退回空文档。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// 严格 JSON。
    Json,
    /// 带注释与尾逗号容忍的 JSON（Claude Desktop 的 `configLibrary/*.json` 实测是 JSONC）。
    Jsonc,
    /// TOML（Codex 的 `config.toml`）：保序、保留用户注释与别的段。
    Toml,
}

/// 一条接管键上的动作。JSON / TOML 通用；TOML 没有 null 字面值，`Set` 到 `null` 会报错。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OwnedOp {
    /// 设值 / 建值；中间对象不存在就建（`env` 块、`model_providers.<id>` 表都靠它）。
    /// 路径上的中间节点已是别的类型（字符串 / 数组）→ 报错停写，不摧毁用户的数据。
    Set { path: Vec<String>, value: Json },
    /// 摘键；不存在即空操作。
    Remove { path: Vec<String> },
    /// 摘键；摘掉后若这一层空了，就把空壳一层层也摘掉（Claude Code 的 `env` 摘空即摘）。
    RemoveIfEmpty { path: Vec<String> },
    /// 数组里按元素的 `field` 值 upsert（Desktop `_meta.json` 的 `entries` 条目）。
    SetElement {
        path: Vec<String>,
        field: String,
        value: Json,
    },
    /// 数组里按元素的 `field` 值摘掉元素（同上，还原用）。
    RemoveElement {
        path: Vec<String>,
        field: String,
        matches: Json,
    },
    /// 仅当现值等于 `equals` 时替换成 `value`（`None` = 摘掉）；不等则一个字节都不动。
    /// 指针键的「回退」用它：`appliedId` 还指着网关就换掉，指着别人就别碰。
    ReplaceIfEquals {
        path: Vec<String>,
        equals: Json,
        value: Option<Json>,
    },
}

impl OwnedOp {
    /// 这条动作的键路径（冲突比对、还原遍历用）。
    pub fn path(&self) -> &[String] {
        match self {
            OwnedOp::Set { path, .. }
            | OwnedOp::Remove { path }
            | OwnedOp::RemoveIfEmpty { path }
            | OwnedOp::SetElement { path, .. }
            | OwnedOp::RemoveElement { path, .. }
            | OwnedOp::ReplaceIfEquals { path, .. } => path,
        }
    }

    /// 记录进 `client_config_state.owned_keys_json` 的标签：路径 + 动作区别，用来判「清单变没变」。
    /// 只做相等比较，不给用户看。
    pub fn label(&self) -> String {
        let base = serde_json::to_string(self.path()).unwrap_or_default();
        match self {
            OwnedOp::Set { .. } => base,
            OwnedOp::Remove { .. } => format!("-{base}"),
            OwnedOp::RemoveIfEmpty { .. } => format!("-?{base}"),
            OwnedOp::SetElement { field, value, .. } => format!(
                "[]+{base}#{field}={}",
                serde_json::to_string(value.get(field).unwrap_or(&Json::Null)).unwrap_or_default()
            ),
            OwnedOp::RemoveElement { field, matches, .. } => format!(
                "[]-{base}#{field}={}",
                serde_json::to_string(matches).unwrap_or_default()
            ),
            OwnedOp::ReplaceIfEquals { equals, .. } => format!(
                "?{base}#if={}",
                serde_json::to_string(equals).unwrap_or_default()
            ),
        }
    }
}

/// 一个「客户端 × 文件」的接管键清单 + 落点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileSpec {
    /// 客户端标识：备份子目录名与 `client_config_state.client` 都用它（如 `claude-desktop`）。
    pub client: String,
    /// 目标文件，**相对 [`crate::writer::WriteRoots::config_root`]**。真实根由调用方注入。
    pub rel_path: PathBuf,
    pub format: Format,
    /// 写：只动这些键。
    pub write_ops: Vec<OwnedOp>,
    /// 还原（默认摘键）：这些键的反向动作。与 `write_ops` 是一份清单的两面。
    pub restore_ops: Vec<OwnedOp>,
    /// 依赖序：同一份计划里 `order` 小的先写（先被引用者、后指针）。
    pub order: u32,
}

impl FileSpec {
    /// 只写不还原的文件（写清单即接管清单）。
    pub fn new(
        client: impl Into<String>,
        rel_path: impl Into<PathBuf>,
        format: Format,
        write_ops: Vec<OwnedOp>,
    ) -> FileSpec {
        FileSpec {
            client: client.into(),
            rel_path: rel_path.into(),
            format,
            write_ops,
            restore_ops: Vec::new(),
            order: 0,
        }
    }

    /// 补上还原动作（链式写法，票 46/47/48 拼清单时用）。
    pub fn with_restore(mut self, restore_ops: Vec<OwnedOp>) -> FileSpec {
        self.restore_ops = restore_ops;
        self
    }

    pub fn with_order(mut self, order: u32) -> FileSpec {
        self.order = order;
        self
    }

    /// 写清单的标签序列（记录/比对用）。
    pub fn labels(&self) -> Vec<String> {
        self.write_ops.iter().map(OwnedOp::label).collect()
    }
}

/// 冲突怎么办：默认放弃本次；`Claim` = 用户显式「认领并覆盖」（票 31 的显式第二选项）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictMode {
    /// 放弃本次 + 提示，不覆盖、不自动合并（ADR-0006 第 4 条）。
    #[default]
    Abort,
    /// 认领并覆盖：接管键以网关为准，仍然备份、仍然只动接管键。
    Claim,
}

/// 一次切换要改的多个文件（按 [`FileSpec::order`] 依赖序逐个原子替换）。
#[derive(Clone, Debug)]
pub struct WritePlan {
    pub files: Vec<FileSpec>,
    pub mode: ConflictMode,
}

impl WritePlan {
    pub fn new(files: Vec<FileSpec>) -> WritePlan {
        WritePlan {
            files,
            mode: ConflictMode::Abort,
        }
    }

    pub fn claim(mut self) -> WritePlan {
        self.mode = ConflictMode::Claim;
        self
    }
}

/// 写入根：**全部参数化**，测试只指临时目录（真实 `%LOCALAPPDATA%\Claude-3p`、
/// `~/.claude`、`~/.codex` 一个字节都不碰）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteRoots {
    /// 配置根：`FileSpec::rel_path` 相对它解析。真实值由票 46/47/48 从环境变量拼。
    pub config_root: PathBuf,
    /// 备份根：`<backup_root>\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`（spec §8.4 第 7 条）。
    pub backup_root: PathBuf,
    /// 元数据日志根（`writer-YYYYMMDD.log`，7 天保留）。
    pub log_root: PathBuf,
}

impl WriteRoots {
    /// 三个根都挂在同一个目录下（测试与便携场景用）。
    pub fn under(root: impl Into<PathBuf>) -> WriteRoots {
        let root = root.into();
        WriteRoots {
            config_root: root.clone(),
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        }
    }

    /// 真实布局：备份 / 日志挂数据目录（`%APPDATA%\com.claudegateway.desktop\`），配置根由调用方给。
    pub fn with_data_dir(
        config_root: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
    ) -> WriteRoots {
        let data_dir = data_dir.into();
        WriteRoots {
            config_root: config_root.into(),
            backup_root: data_dir.join("backups"),
            log_root: data_dir.join("logs"),
        }
    }
}

/// 一个文件这一步的结果。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FileOutcome {
    /// 语义相同：**完全不写**，连 mtime 都不变（spec §8.4 第 3 条）。
    Unchanged,
    /// 写了（或还原了）。
    Written {
        /// 本次写前留的备份（首写时是首写快照 / marker）。
        backup: PathBuf,
        /// 备份是「首写时文件不存在」的 marker（spec §8.4 第 7 条）。
        marker_only: bool,
        /// 这是该文件的第一次网关写入（记录缺失 = 首次接管）。
        first_write: bool,
        /// 给界面的一句话（首次接管 / 认领覆盖 / 清单变了）。
        notice: Option<String>,
    },
    /// 冲突：放弃本次 + 提示（ADR-0006 第 4 条）。
    Conflict {
        /// 出问题的接管键（标签里的路径段）。
        path: Vec<String>,
        /// 网关记录的上次写入值。
        recorded: Json,
        /// 磁盘现值。
        current: Json,
    },
    /// 还原时把「网关新建、且只剩网关内容」的文件整个删了。
    Removed { backup: PathBuf, marker_only: bool },
}

/// 一次多文件写入的结果（每文件一条）。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WriteReport {
    pub files: Vec<(PathBuf, FileOutcome)>,
}

impl WriteReport {
    /// 有没有文件因冲突被放弃（界面据此提示）。
    pub fn conflicts(&self) -> Vec<&PathBuf> {
        self.files
            .iter()
            .filter(|(_, o)| matches!(o, FileOutcome::Conflict { .. }))
            .map(|(p, _)| p)
            .collect()
    }
}

/// 整份回滚（显式第二选项）的结果。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "rollback", rename_all = "snake_case")]
pub enum RollbackOutcome {
    /// 按首写快照整份恢复（或整个删掉网关新建的文件）。
    Rolled { backup: PathBuf, deleted: bool },
    /// 没有记录 / 快照不在 → 拒绝并说清原因。
    Refused { reason: String },
    /// 文件已经与快照一致，不用动。
    Unchanged,
}
