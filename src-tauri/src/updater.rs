//! 自动升级的外壳侧（票 61）：把 Tauri 的 updater 插件接进管理 API（`/api/update/*`）。
//!
//! 分工（照 ADR-0007 第 1 条）：检查 / 下载 / 验签 / 静默安装是插件的活（要 AppHandle 与真装机），
//! 留包与回退指引在 `gateway_core::dist::update`（能裸跑、有断言）；本模块只做「把两边的形状对上」
//! 与「把过程记下来」。
//!
//! 三条不可关的安全线（票 18 §3，配置在 `src-tauri/tauri.conf.json` 的 `plugins.updater`）：
//! 端点必须 https、签名校验由插件强制（`download` 先验签再给字节，验不过就没有然后）、
//! `requireSignedVersion` 要求签名里带它签的那个版本（挡住「拿旧包的合法签名配新版本号」的降级攻击）。
//! 本地闭环验收用的 http 端点只在 `src-tauri/tauri.conf.test.json`（`--config` 覆盖）里出现。
//!
//! 升级**不改端口、不改数据、不改客户端配置**：这条由 NSIS 的 `/UPDATE` 模式保证（installer.nsi
//! 在 update 模式下跳过数据删除、快捷方式与 Run 键清理；`nsis/hooks.nsh` 的 PREUNINSTALL 整段
//! 用 `$UpdateMode <> 1` 跳过），本模块做的事只有三件：下载到内存 → 在 `updates\installers\` 留一份
//! → 交给插件跑安装器。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gateway_core::dist::update::{self, AvailableUpdate, UpdateFuture, UpdateSource, UpdateStatus};
use gateway_core::ingress::api::rfc3339_utc;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// 检查 / 安装这条路上的状态（`snapshot` 就是它的投影）。
#[derive(Debug, Default)]
struct Inner {
    checking: bool,
    installing: bool,
    last_checked_at: Option<String>,
    available: Option<AvailableUpdate>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    installing_package: Option<String>,
    error: Option<String>,
}

/// 外壳的升级句柄。实现 [`UpdateSource`] 后交给 `gateway_core::service`（`GatewayConfig.updater`）。
#[derive(Debug)]
pub struct ShellUpdater {
    app: AppHandle,
    data_dir: PathBuf,
    current_version: String,
    endpoints: Vec<String>,
    inner: Mutex<Inner>,
}

impl ShellUpdater {
    /// `data_dir` 与 `current_version` 由外壳在 `setup` 里递进来（数据目录只有一处定义）。
    pub fn new(app: AppHandle, data_dir: PathBuf, current_version: impl Into<String>) -> Arc<Self> {
        // 端点只用于「界面上显示我们其实在问谁」——真正的请求由插件按同一份配置发。
        let endpoints = app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|v| v.get("endpoints"))
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Arc::new(ShellUpdater {
            app,
            data_dir,
            current_version: current_version.into(),
            endpoints,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// 启动后静默检查一次（票 18 §3）：延后一点再问，别跟启动抢；离线 / 端点不在只记进状态，
    /// **不弹窗**（用户下次打开界面时能看到失败原因）。
    pub fn spawn_startup_check(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(15));
            let _ = tauri::async_runtime::block_on(me.clone().check());
        });
    }

    /// 把一件事写进 `logs\gateway.log`（与外壳同一份日志）。
    fn log(&self, line: &str) {
        let logs = crate::shell::logs_dir(&self.data_dir);
        let _ = crate::shell::append_log(&logs, line);
    }

    /// 状态快照（拿在手上的那把锁）。
    fn status_with(&self, inner: &Inner) -> UpdateStatus {
        UpdateStatus {
            current_version: self.current_version.clone(),
            endpoints: self.endpoints.clone(),
            checking: inner.checking,
            installing: inner.installing,
            last_checked_at: inner.last_checked_at.clone(),
            available: inner.available.clone(),
            downloaded_bytes: inner.downloaded_bytes,
            total_bytes: inner.total_bytes,
            installing_package: inner.installing_package.clone(),
            error: inner.error.clone(),
            rollback: update::rollback(&self.data_dir, &self.current_version),
        }
    }

    /// 去端点问一次（真发请求）。`Ok(None)` = 已经是最新。
    async fn do_check(&self) -> anyhow::Result<Option<tauri_plugin_updater::Update>> {
        let updater = self
            .app
            .updater()
            .map_err(|e| anyhow::anyhow!("updater 没配好：{e}"))?;
        Ok(updater.check().await?)
    }

    /// 下载 → 留包 → 记回执 → 静默安装。
    ///
    /// 最后一步在 Windows 上会 `exit(0)`（插件把 NSIS 安装器 `ShellExecute` 起来后退出本进程，
    /// 安装器用 `/R` 把新版拉起），所以这里正常**不会返回**；一切失败都返回 `Err`。
    async fn do_install(self: Arc<Self>) -> anyhow::Result<()> {
        let updater = self
            .app
            .updater()
            .map_err(|e| anyhow::anyhow!("updater 没配好：{e}"))?;
        let Some(update) = updater.check().await? else {
            anyhow::bail!(
                "端点没宣布比 {} 更新的版本（可能是刚被撤下，或清单里的版本没变）",
                self.current_version
            );
        };
        let version = update.version.clone();
        let file_name = update::file_name_from_url(update.download_url.as_str());
        self.log(&format!(
            "开始升级：{} → {}（端点 {}）",
            self.current_version, version, update.download_url
        ));

        // 进度回调只更新状态，锁在回调里拿了就放，不跨 await 持有。
        let progress = self.clone();
        let bytes = update
            .download(
                move |chunk, total| {
                    if let Ok(mut inner) = progress.inner.lock() {
                        inner.downloaded_bytes += chunk as u64;
                        if total.is_some() {
                            inner.total_bytes = total;
                        }
                    }
                },
                || {},
            )
            .await
            // 验签不过 = 包不是应用配置里那个公钥对应的私钥签的：**拒绝安装**，并把原因说清。
            // 这一步在 `save_package` 之前，所以被拒的包一个字节都不会落盘。
            .map_err(|e| match e {
                tauri_plugin_updater::Error::Minisign(inner) => anyhow::anyhow!(
                    "签名校验不过，已拒绝安装：这个包不是用「应用配置里那个公钥」对应的私钥签的（{inner}）。\
                     请检查更新源是不是用错了密钥——不要绕过校验。"
                ),
                other => anyhow::anyhow!("下载更新包失败：{other}"),
            })?;
        // 走到这里签名已经验过了（`download` 先 `verify` 再给字节）。

        let saved = update::save_package(&self.data_dir, &file_name, &bytes)?;
        let at = rfc3339_utc(SystemTime::now());
        update::record_update(
            &self.data_dir,
            &self.current_version,
            &version,
            &file_name,
            &at,
        )?;
        {
            let mut inner = self.inner.lock().expect("升级状态锁");
            inner.installing_package = Some(file_name.clone());
        }
        self.log(&format!(
            "包已下好并留档：{}（{} 字节）；开始静默安装（installMode = passive，装完自动重启）",
            saved.display(),
            bytes.len()
        ));

        update.install(bytes)?;
        Ok(())
    }
}

impl UpdateSource for ShellUpdater {
    fn snapshot(&self) -> UpdateStatus {
        self.status_with(&self.inner.lock().expect("升级状态锁"))
    }

    fn check(self: Arc<Self>) -> UpdateFuture<Result<UpdateStatus, String>> {
        Box::pin(async move {
            {
                let mut inner = self.inner.lock().expect("升级状态锁");
                if inner.checking {
                    // 已经有人在查（比如启动后那次静默检查正好在跑）：把当前状态原样回给他，
                    // 别报错——「检查更新」点两下不该是一个错误。
                    return Ok(self.status_with(&inner));
                }
                inner.checking = true;
            }
            let result = self.do_check().await;
            let mut inner = self.inner.lock().expect("升级状态锁");
            inner.checking = false;
            inner.last_checked_at = Some(rfc3339_utc(SystemTime::now()));
            match result {
                Ok(Some(update)) => {
                    inner.available = Some(AvailableUpdate {
                        version: update.version.clone(),
                        notes: update.body.clone(),
                        pub_date: update.date.map(|d| d.to_string()),
                        download_url: update.download_url.to_string(),
                    });
                    inner.error = None;
                    self.log(&format!(
                        "检查更新：发现 {}（当前 {}）",
                        update.version, self.current_version
                    ));
                    Ok(self.status_with(&inner))
                }
                Ok(None) => {
                    inner.available = None;
                    inner.error = None;
                    Ok(self.status_with(&inner))
                }
                Err(e) => {
                    let message = format!("检查更新失败：{e}");
                    inner.error = Some(message.clone());
                    inner.available = None;
                    drop(inner);
                    self.log(&message);
                    Err(message)
                }
            }
        })
    }

    fn start_install(self: Arc<Self>) -> Result<UpdateStatus, String> {
        {
            let mut inner = self.inner.lock().expect("升级状态锁");
            if inner.installing {
                // 幂等：界面重复点「下载并安装」不该报错，回当前进度就行。
                return Ok(self.status_with(&inner));
            }
            if inner.checking {
                // 这一次是真冲突：检查还没回来，不知道要装哪个版本。
                return Err("正在检查更新，稍等一下再装".to_string());
            }
            inner.installing = true;
            inner.error = None;
            inner.downloaded_bytes = 0;
            inner.total_bytes = None;
            inner.installing_package = None;
        }
        let me = self.clone();
        // 后台跑：安装器起来会杀掉本进程，HTTP 那一头不能等它。
        tauri::async_runtime::spawn(async move {
            if let Err(e) = me.clone().do_install().await {
                let message = format!("安装更新失败：{e:#}");
                if let Ok(mut inner) = me.inner.lock() {
                    inner.installing = false;
                    inner.error = Some(message.clone());
                }
                me.log(&message);
            }
        });
        Ok(self.status_with(&self.inner.lock().expect("升级状态锁")))
    }
}
