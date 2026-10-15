//! 配置写入引擎（旁路，不在请求路径上）：三个客户端配置文件的写入与还原（spec §8、
//! [ADR-0006](../../../../../docs/adr/0006-config-writer-owned-keys.md)）。
//!
//! # 分工
//!
//! 引擎**不认任何客户端的键名**：接管键清单是调用方（票 46 Claude Desktop / 票 47 Claude
//! Code / 票 48 Codex）拼出来的 [`FileSpec`] 数据。引擎只做与客户端无关的那五件事：
//!
//! 1. **保序改写**（JSON / JSONC / TOML）：只动清单里的键，清单外的一切原样保留；
//!    解析失败即停，绝不退回空文档重写（[`doc`]）。
//! 2. **原子替换**：同目录临时文件 + fsync + `ReplaceFileW`（退 `fs::rename`）；占用/权限
//!    退避重试 3 次后报错，错误带目标路径、备份路径与占用提示（[`atomic`]）。
//! 3. **备份**：`<备份根>\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`，首写快照常驻、滚动 5 份 /
//!    90 天，首写时文件不存在只写 marker（[`backup`]）。
//! 4. **冲突 / 幂等 / 还原判据**：唯一真相是 `client_config_state`（[`state`]）。
//! 5. **记账**：每次写入 / 还原记库 + 元数据日志（7 天）（[`log`]）。
//!
//! # 写入根是参数
//!
//! [`WriteRoots`] 的 `config_root` / `backup_root` / `log_root` 全部由调用方给：测试只指
//! 临时目录，真实 `%LOCALAPPDATA%\Claude-3p`、`~/.claude`、`~/.codex` 一个字节都不碰。
//!
//! # 例子
//!
//! ```no_run
//! use gateway_core::writer::{ConflictMode, FileSpec, Format, OwnedOp, WritePlan, WriteRoots, Writer};
//! use gateway_core::store::Store;
//! # async fn demo(store: Store, config_root: std::path::PathBuf) -> anyhow::Result<()> {
//! let spec = FileSpec::new(
//!     "claude-code",
//!     ".claude/settings.json",
//!     Format::Jsonc,
//!     vec![OwnedOp::Set {
//!         path: vec!["env".into(), "ANTHROPIC_BASE_URL".into()],
//!         value: serde_json::json!("http://127.0.0.1:8787"),
//!     }],
//! )
//! .with_restore(vec![OwnedOp::RemoveIfEmpty {
//!     path: vec!["env".into(), "ANTHROPIC_BASE_URL".into()],
//! }]);
//! let writer = Writer::new(store, WriteRoots::under(config_root));
//! let report = writer.write(&WritePlan::new(vec![spec])).await?;
//! # let _ = (report, ConflictMode::Abort);
//! # Ok(())
//! # }
//! ```

mod atomic;
mod backup;
mod doc;
mod engine;
pub mod instance;
mod log;
mod plan;
mod state;

#[cfg(test)]
mod tests;

pub use engine::Writer;
pub use plan::{
    ConflictMode, FileOutcome, FileSpec, Format, OwnedOp, RollbackOutcome, WritePlan, WriteReport,
    WriteRoots,
};
