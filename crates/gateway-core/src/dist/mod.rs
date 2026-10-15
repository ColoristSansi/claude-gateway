//! 分发收尾（票 62；spec §12 M4、[票 18](../../../../../.scratch/model-gateway/issues/18-grilling-packaging-and-updates.md)）：
//! 开机自启、卸载时还原客户端配置、导出 / 导入。
//!
//! 三块都**不依赖 Tauri**（ADR-0007 第 1 条）：外壳（托盘 / 命令行）与卸载器都只是调用方，
//! 逻辑与断言留在这里裸跑。
//!
//! - [`autostart`]：`HKCU\...\Run` 一个值，默认关、关掉即删（不留卸载残留）。
//! - [`bundle`]：导出 / 导入的 JSON 形状与口令加密；默认不含明文密钥（spec §9）。
//! - [`restore`]：卸载时的「还原客户端配置」——**复用** `writer` 引擎的摘键还原，
//!   不另写一套还原逻辑（票 45/46/47 的 `FileSpec` 就是还原清单）。
//! - [`cli`]：脚本可调用的参数面；外壳的 bin 认它，卸载器的 `--restore-clients --silent` 也走它。
//! - [`update`]（票 61）：升级留包（`updates\`）+ 回退指引 + 更新状态形状；真正的
//!   「检查 / 下载 / 安装」在 Tauri 的 updater 插件里，这里只放能裸跑的那半。

pub mod autostart;
pub mod bundle;
pub mod cli;
pub mod restore;
pub mod update;

/// DPAPI 密文**换机解不开**（spec §13）：导出 / 导入两处都给这句原文，界面（票 59）直接显示。
///
/// 依据：`CryptProtectData` 默认 CurrentUser 范围——密文绑到当前用户，换机 / 换用户都解不开
/// （`crates/gateway-core/src/store/dpapi.rs`）。跨机迁移必须用「带口令的加密导出」重新加密。
pub const DPAPI_MIGRATION_WARNING: &str =
    "数据库里的上游密钥是用 Windows DPAPI 按「当前用户」加密的，\
换一台机器（或换一个 Windows 用户）就解不开。要跨机迁移，请用「带口令的加密导出」：\
导出文件里的密钥用你设的口令重新加密，在新机器上凭同一口令导入即可；\
不带口令的导出默认**不含明文密钥**，导入后需要重新填一次密钥。";
