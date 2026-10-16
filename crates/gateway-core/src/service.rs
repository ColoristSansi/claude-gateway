//! 网关服务：绑监听、对外服务、优雅停止（spec §3）。
//!
//! 「网关由谁起」已定案：**同进程 axum**，不是 sidecar——外壳在 `setup` 里调
//! [`start`]，关窗不停止，只有退出才 [`Gateway::stop`]（票 18 §2）。
//!
//! 端口：默认 [`DEFAULT_PORT`]，被占时返回 [`StartError::PortInUse`]，**不静默换端口**
//! ——地址已经写进客户端配置（spec §3）。

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::ingress::api::{AdminCtx, ApiState, GatewayStatus};

/// 只监听回环：网关是本机客户端的私有上游（spec §3）。
pub const HOST: &str = "127.0.0.1";

/// 默认端口，写死在客户端配置里，改动要同步三处客户端配置。
pub const DEFAULT_PORT: u16 = 8787;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub port: u16,
    pub db_path: PathBuf,
    pub version: String,
    /// 转发面 `/v1/*` 的状态（票 41 的 [`crate::ingress::proxy::Gateway`]）。
    /// `None` = 只起管理面：外壳还没接出站转发（供应商表与绑定表来自票 49 / 51）时就是这一档。
    pub forwarding: Option<Arc<crate::ingress::proxy::Gateway>>,
    /// 维护面（票 62）：开机自启开关 / 导出 / 导入要动数据目录与库。
    /// `None` = 这些端点是 503（只起管理面 / 测试不带）。
    pub maintenance: Option<crate::ingress::api::Maintenance>,
    /// 升级面（票 61）：检查更新 / 下载 / 静默安装走 Tauri 的 updater 插件，由外壳实现
    /// [`crate::dist::update::UpdateSource`] 后递进来。`None` = `/api/update/*` 是 503。
    pub updater: Option<Arc<dyn crate::dist::update::UpdateSource>>,
    /// 管理面 `/api/*` 的业务上下文（库 + 三个客户端的配置根 + 外壳句柄，票 52）。
    /// `None` = 只挂 `/api/status` 那一条（M1 单测那档）。
    pub admin: Option<Arc<AdminCtx>>,
}

#[derive(Debug)]
pub enum StartError {
    /// 地址已被占用。地址已进客户端配置，所以这里只报错、不换端口。
    PortInUse {
        port: u16,
    },
    Bind {
        port: u16,
        source: std::io::Error,
    },
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::PortInUse { port } => write!(
                f,
                "端口 {port} 已被占用（{HOST}:{port} 绑定失败：Address already in use）。\
                 该地址已写进客户端配置，网关不会自动换端口；\
                 请先关掉占用 {port} 的程序，或在设置页改用空闲端口后重启。"
            ),
            StartError::Bind { port, source } => {
                write!(f, "端口 {port} 绑定失败（{HOST}:{port}）：{source}")
            }
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StartError::PortInUse { .. } => None,
            StartError::Bind { source, .. } => Some(source),
        }
    }
}

/// 跑起来的网关。持有它 = 有人负责在退出时 [`Gateway::stop`]。
pub struct Gateway {
    state: Arc<ApiState>,
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    /// 保留期清理循环（票 53）：**挂在这里**，[`Gateway::stop`] 一并停——否则换端口 / 手动重启
    /// 每起一次就多一个常驻循环清同一个库（评审「中」第 5 条）。
    sweeper: Option<crate::obs::PurgeSweeper>,
}

impl Gateway {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// `/api/status` 的同一份快照。
    pub fn status(&self) -> GatewayStatus {
        self.state.snapshot()
    }

    /// 优雅停止：关监听、等 serve 任务收尾，**并把清理循环一并停掉**，之后 `/api/status` 不再可达。
    /// 返回停止后的快照（`running == false`），省得再留一个可观测句柄。
    pub async fn stop(mut self) -> GatewayStatus {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.task.await;
        if let Some(sweeper) = self.sweeper.take() {
            sweeper.stop().await;
        }
        self.state.mark_stopped();
        self.state.snapshot()
    }
}

/// 绑 127.0.0.1:`cfg.port` 并开始服务。
///
/// `port = 0` 交给操作系统挑（只给测试用）；真实端口以 [`Gateway::port`] 为准。
pub async fn start(cfg: GatewayConfig) -> Result<Gateway, StartError> {
    let want = SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = TcpListener::bind(want).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            StartError::PortInUse { port: cfg.port }
        } else {
            StartError::Bind {
                port: cfg.port,
                source: e,
            }
        }
    })?;
    let addr = listener.local_addr().map_err(|e| StartError::Bind {
        port: cfg.port,
        source: e,
    })?;

    let mut api_state = ApiState::new(
        cfg.version,
        cfg.db_path.to_string_lossy().into_owned(),
        addr.port(),
    );
    // 票 62：维护面（导出 / 导入 / 自启开关）跟着库走，不管转发面挂没挂。
    if let Some(maint) = cfg.maintenance {
        api_state = api_state.with_maintenance(maint);
    }
    // 票 61：升级面（检查更新 / 静默安装）同样由外壳递进来；没递进来就是 503。
    if let Some(updater) = cfg.updater {
        api_state = api_state.with_updater(updater);
    }
    if let Some(admin) = cfg.admin.clone() {
        // 绑定成功之后才知道真端口（`port = 0` 是操作系统挑的）：写回管理面上下文。
        admin.set_real_port(addr.port());
        api_state = api_state.with_admin(admin);
    }
    let state = Arc::new(api_state);
    // 票 53：保留期清理表跟着网关起——启动立刻扫一次（把上次退出后攒下的老行清掉），之后每小时一次。
    // 只在转发面挂了的时候起（没有库就没有日志可清）；句柄挂进 `Gateway`，停网关时一并停。
    let sweeper = cfg
        .forwarding
        .as_ref()
        .map(|gateway| crate::obs::spawn_purge_sweeper(gateway.store.clone()));
    let app = crate::ingress::router((*state).clone(), cfg.forwarding);
    let (tx, rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
        if let Err(e) = served {
            eprintln!("[网关] 服务异常退出：{e}");
        }
    });

    Ok(Gateway {
        state,
        addr,
        shutdown: Some(tx),
        task,
        sweeper,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::BindingTable;
    use crate::ingress::proxy;
    use crate::store::Store;
    use std::collections::HashMap;

    /// 转发面挂了就必须挂上清理表，且**停网关时一并停掉**：否则换端口 / 手动重启每起一次就多一个
    /// 常驻循环清同一个库（评审「中」第 5 条）。
    #[tokio::test]
    async fn stop_takes_the_purge_sweeper_with_it() {
        let root = std::env::temp_dir().join(format!("cg-review-sweeper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).expect("开库");
        let forwarding = Arc::new(proxy::Gateway::new(
            store,
            HashMap::new(),
            BindingTable::default(),
        ));
        let gateway = start(GatewayConfig {
            port: 0,
            db_path: root.join("gateway.db"),
            version: "0.1.0-test".into(),
            forwarding: Some(forwarding),
            maintenance: None,
            updater: None,
            admin: None,
        })
        .await
        .expect("起网关");

        let sweeper = gateway.sweeper.as_ref().expect("转发面挂了就必须挂清理表");
        let probe = sweeper.abort_handle();
        assert!(!probe.is_finished(), "刚起的清理循环不该已经结束");

        gateway.stop().await;

        assert!(
            probe.is_finished(),
            "stop 必须把清理循环也停掉（否则重启一次多一个常驻循环）"
        );
    }
}
