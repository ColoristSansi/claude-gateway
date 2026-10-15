//! 卸载时的「还原客户端配置」（票 62；spec §8.4 第 8 条：还原 = **摘键**）。
//!
//! **复用配置写入引擎，不另写还原逻辑**：三个客户端各自的 [`FileSpec`]（接管键清单 + 反向动作）
//! 就是还原清单，本模块只做三件事——定位配置根、按客户端拼清单、把结果汇总成一句人话。
//!
//! 每个客户端**各自成败**：一个客户端没装 / 文件被占用 / 清单拼不出来，不许把另外两个也带下水
//! （卸载一旦走到这一步，软件已经要删了，能救几个是几个）。汇总进 [`RestoreReport::errors`]，
//! 卸载器据此显示「请手动还原」。
//!
//! 退出码：调用方（外壳的 `--restore-clients`）用 [`RestoreReport::failed`] 决定 0 / 1。

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::service::DEFAULT_PORT;
use crate::store::Store;
use crate::writer::claude_code::{self, ModelSlots};
use crate::writer::codex::{self, CodexWriter};
use crate::writer::desktop::{self, DesktopConfig};
use crate::writer::{FileOutcome, FileSpec, WritePlan, WriteRoots, Writer};

/// 三个客户端的配置根。`None` = 这台机器上定位不到（环境变量缺）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientRoots {
    /// `%LOCALAPPDATA%\Claude-3p\configLibrary`（票 46）。
    pub claude_desktop: Option<PathBuf>,
    /// `%USERPROFILE%\.claude`（票 47）。
    pub claude_code: Option<PathBuf>,
    /// `%USERPROFILE%\.codex`（票 48）。
    pub codex: Option<PathBuf>,
}

impl ClientRoots {
    /// 真实根：只算路径，不创建、不写（定位不到就是 `None`）。
    pub fn real() -> ClientRoots {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
        ClientRoots {
            claude_desktop: desktop::config_library_root(),
            claude_code: home.as_ref().map(|h| claude_code::config_root_from(h)),
            // Codex 认 `$CODEX_HOME`（没设才回落 `%USERPROFILE%\.codex`），所以问票 48 的 home()。
            codex: codex::home().ok(),
        }
    }

    /// 三个根都挂在一个临时根下（测试与「临时现场」用）。
    pub fn under(root: &Path) -> ClientRoots {
        ClientRoots {
            claude_desktop: Some(root.join("Claude-3p").join("configLibrary")),
            claude_code: Some(root.join(".claude")),
            codex: Some(root.join(".codex")),
        }
    }
}

/// 一个客户端这一步的结果（文件级结果来自引擎的 [`FileOutcome`]）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredClient {
    pub client: String,
    pub root: PathBuf,
    pub files: Vec<(PathBuf, FileOutcome)>,
}

/// 整次卸载还原的结果：逐客户端的结果 + 逐客户端的失败原因。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub clients: Vec<RestoredClient>,
    /// 没能还原的（含「这台机器上定位不到配置根」），一条一句人话。
    pub errors: Vec<String>,
}

impl RestoreReport {
    /// 有任何一条失败 → 卸载器要提示「请手动还原」。
    pub fn failed(&self) -> bool {
        !self.errors.is_empty()
    }

    /// 给命令行 / 日志的一行汇总。
    pub fn summary(&self) -> String {
        if self.clients.is_empty() && self.errors.is_empty() {
            return "没有需要还原的客户端配置".to_string();
        }
        let done: Vec<String> = self
            .clients
            .iter()
            .map(|c| format!("{}（{} 个文件）", c.client, c.files.len()))
            .collect();
        let mut out = if done.is_empty() {
            "没有成功还原任何客户端".to_string()
        } else {
            format!("已还原：{}", done.join("、"))
        };
        if !self.errors.is_empty() {
            out.push_str(&format!(
                "；{} 项失败：{}",
                self.errors.len(),
                self.errors.join("；")
            ));
        }
        out
    }
}

/// `settings` 表里的端口（还原清单里的地址只用于记账，不影响摘键动作）。
/// 读不到 / 非法 → [`DEFAULT_PORT`]。
pub async fn configured_port(store: &Store) -> u16 {
    let row = store
        .read(|c| {
            let mut st = c.prepare("SELECT value FROM settings WHERE key='port'")?;
            let mut rows = st.query([])?;
            Ok(match rows.next()? {
                Some(r) => Some(r.get::<_, String>(0)?),
                None => None,
            })
        })
        .await
        .ok()
        .flatten();
    row.and_then(|v| v.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// 还原三个客户端的官方配置（卸载流程的第二步：选「还原」时跑的就是它）。
///
/// 还原动作 = 引擎的 [`Writer::restore`]（默认摘键）；`data_dir` 只用来放备份与日志
/// （与网关写入时同一个数据目录，备份链不断）。
pub async fn restore_clients(
    store: &Store,
    data_dir: &Path,
    port: u16,
    roots: &ClientRoots,
) -> RestoreReport {
    let mut report = RestoreReport::default();
    let base_url = claude_code::gateway_root_url(port);

    // ① Claude Desktop：profile + `_meta.json`（顺序与写入相反由引擎的多文件计划保证）。
    match &roots.claude_desktop {
        None => report.errors.push(
            "定位不到 Claude Desktop 配置目录（LOCALAPPDATA 未设置）：请手动把 3P 配置切回官方态"
                .to_string(),
        ),
        Some(root) => {
            let cfg = DesktopConfig::new(&base_url, "");
            let result = match desktop::specs(root, &cfg) {
                Ok(specs) => restore_with(store, data_dir, root, specs).await,
                Err(e) => Err(e),
            };
            push(&mut report, desktop::CLIENT, root, result);
        }
    }

    // ② Claude Code：`~/.claude/settings.json` 的 env 摘键。
    match &roots.claude_code {
        None => report.errors.push(
            "定位不到 Claude Code 配置目录（USERPROFILE 未设置）：请手动清掉 settings.json 里的网关键"
                .to_string(),
        ),
        Some(root) => {
            let spec = claude_code::file_spec(&base_url, "", &ModelSlots::default());
            let result = restore_with(store, data_dir, root, vec![spec]).await;
            push(&mut report, claude_code::CLIENT, root, result);
        }
    }

    // ③ Codex：`config.toml` + 模型目录文件。清单与还原顺序由票 48 的 [`CodexWriter`] 给
    //    （它还要管目录漂移与哨兵行清理），这里只把根与基址递进去——同样不另写一套还原逻辑。
    match &roots.codex {
        None => report.errors.push(
            "定位不到 Codex 配置目录（USERPROFILE / CODEX_HOME 未设置）：请手动清掉 config.toml 里的网关注册".to_string(),
        ),
        Some(root) => {
            let writer = CodexWriter::new(
                Writer::new(store.clone(), WriteRoots::with_data_dir(root, data_dir)),
                &base_url,
            );
            let result = writer.restore().await.map(|report| RestoredClient {
                client: String::new(),
                root: root.clone(),
                files: report.files,
            });
            push(&mut report, codex::CLIENT, root, result);
        }
    }

    report
}

/// 一个客户端跑一次引擎还原（多文件计划）。
async fn restore_with(
    store: &Store,
    data_dir: &Path,
    config_root: &Path,
    specs: Vec<FileSpec>,
) -> Result<RestoredClient> {
    let writer = Writer::new(
        store.clone(),
        WriteRoots::with_data_dir(config_root, data_dir),
    );
    let report = writer.restore(&WritePlan::new(specs)).await?;
    Ok(RestoredClient {
        client: String::new(), // 由 push 填（清单里的 client 与展示名同源）
        root: config_root.to_path_buf(),
        files: report.files,
    })
}

fn push(report: &mut RestoreReport, client: &str, root: &Path, result: Result<RestoredClient>) {
    match result {
        Ok(mut done) => {
            done.client = client.to_string();
            report.clients.push(done);
        }
        Err(e) => report
            .errors
            .push(format!("还原 {client} 失败（{}）：{e}", root.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cg62-restore-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("建临时根");
        root
    }

    #[tokio::test]
    async fn configured_port_reads_settings_and_falls_back_to_default() {
        let root = tmp_root("port");
        let store = Store::open(&root.join("data")).expect("开库");
        assert_eq!(configured_port(&store).await, DEFAULT_PORT, "默认端口");

        store
            .write(|c| {
                c.execute("UPDATE settings SET value='9123' WHERE key='port'", [])?;
                Ok(())
            })
            .await
            .expect("改端口");
        assert_eq!(configured_port(&store).await, 9123);

        store
            .write(|c| {
                c.execute("UPDATE settings SET value='不是数字' WHERE key='port'", [])?;
                Ok(())
            })
            .await
            .expect("写坏值");
        assert_eq!(
            configured_port(&store).await,
            DEFAULT_PORT,
            "坏值回默认，不 panic"
        );
    }

    #[tokio::test]
    async fn a_client_root_that_does_not_exist_is_not_a_failure() {
        // 三个客户端都没装（空临时现场）：引擎把「文件不在」当空操作，回执里没有错误。
        let root = tmp_root("empty");
        let store = Store::open(&root.join("data")).expect("开库");
        let report = restore_clients(
            &store,
            &root.join("data"),
            DEFAULT_PORT,
            &ClientRoots::under(&root),
        )
        .await;

        assert!(!report.failed(), "空现场不该报失败：{:?}", report.errors);
        assert_eq!(report.clients.len(), 3, "三个客户端各出一条回执");
        for c in &report.clients {
            assert!(
                c.files
                    .iter()
                    .all(|(_, o)| matches!(o, FileOutcome::Unchanged)),
                "{} 应当全是「不用动」",
                c.client
            );
        }
    }

    #[tokio::test]
    async fn missing_roots_are_reported_as_errors_not_silent_success() {
        let root = tmp_root("noroots");
        let store = Store::open(&root.join("data")).expect("开库");
        let roots = ClientRoots {
            claude_desktop: None,
            claude_code: None,
            codex: None,
        };
        let report = restore_clients(&store, &root.join("data"), DEFAULT_PORT, &roots).await;
        assert!(
            report.failed(),
            "定位不到必须算失败（卸载器要提示手动还原）"
        );
        assert_eq!(
            report.errors.len(),
            3,
            "三个客户端各一条定位失败：{:?}",
            report.errors
        );
        assert!(
            report.summary().contains("没有成功还原"),
            "{}",
            report.summary()
        );
    }
}
