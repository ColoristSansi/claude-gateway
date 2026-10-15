//! 自动升级的接口面（票 61）：`/api/update/status|check|install`。
//!
//! 真实的检查 / 下载 / 安装是 Tauri updater 插件的事（在 `src-tauri` 里，要 AppHandle 与真装机），
//! 所以这里用一个**假的** [`UpdateSource`] 把接口面钉住：接线对不对、错误怎么回、503 有没有说实话。
//! 真装机的闭环（装 v1 → 发 v2 → 应用内升级成功）在票 61 的 `## Answer` 里逐条贴输出。

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gateway_core::dist::update::{
    AvailableUpdate, Rollback, UpdateFuture, UpdateSource, UpdateStatus,
};
use gateway_core::service::{self, GatewayConfig};
use gateway_core::store::Store;

/// 假升级源：把三个动作记下来，结果按测试需要给。
#[derive(Debug)]
struct FakeUpdater {
    current: String,
    check_result: Mutex<Result<UpdateStatus, String>>,
    checks: AtomicUsize,
    installs: AtomicUsize,
    install_error: Option<String>,
}

impl FakeUpdater {
    fn new(current: &str) -> Arc<FakeUpdater> {
        Arc::new(FakeUpdater {
            current: current.to_string(),
            check_result: Mutex::new(Ok(status(current, Some("0.2.0")))),
            checks: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            install_error: None,
        })
    }

    /// 一装就失败的那种（用来钉住「失败原因要原样回给界面」）。
    fn failing_install(current: &str, message: &str) -> Arc<FakeUpdater> {
        Arc::new(FakeUpdater {
            current: current.to_string(),
            check_result: Mutex::new(Ok(status(current, Some("0.2.0")))),
            checks: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            install_error: Some(message.to_string()),
        })
    }
}

fn status(current: &str, available: Option<&str>) -> UpdateStatus {
    UpdateStatus {
        current_version: current.to_string(),
        endpoints: vec!["https://example.invalid/latest.json".to_string()],
        checking: false,
        installing: false,
        last_checked_at: Some("2026-10-15T15:00:00Z".to_string()),
        available: available.map(|v| AvailableUpdate {
            version: v.to_string(),
            notes: Some("修了几处".to_string()),
            pub_date: None,
            download_url: format!("https://example.invalid/Claude-Gateway_{v}_x64-setup.exe"),
        }),
        downloaded_bytes: 0,
        total_bytes: None,
        installing_package: None,
        error: None,
        rollback: Rollback {
            guide: "回退：退出程序 → 双击旧版安装包（数据 / 端口 / 客户端配置都不动）".to_string(),
            installers_dir: r"C:\tmp\updates\installers".to_string(),
            packages: vec![],
            previous: None,
            last_update: None,
        },
    }
}

impl UpdateSource for FakeUpdater {
    fn snapshot(&self) -> UpdateStatus {
        status(&self.current, None)
    }

    fn check(self: Arc<Self>) -> UpdateFuture<Result<UpdateStatus, String>> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let result = self.check_result.lock().expect("锁").clone();
        Box::pin(async move { result })
    }

    fn start_install(self: Arc<Self>) -> Result<UpdateStatus, String> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = &self.install_error {
            return Err(e.clone());
        }
        let mut s = status(&self.current, Some("0.2.0"));
        s.installing = true;
        s.installing_package = Some("Claude Gateway_0.2.0_x64-setup.exe".to_string());
        Ok(s)
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("挑端口")
        .local_addr()
        .expect("地址")
        .port()
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cg61-update-api-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

async fn start(tag: &str, updater: Option<Arc<dyn UpdateSource>>) -> (u16, service::Gateway) {
    let root = tmp(tag);
    let store = Store::open(&root).expect("开库");
    let port = free_port();
    let gateway = service::start(GatewayConfig {
        port,
        db_path: store.db_path().to_path_buf(),
        version: "0.1.0".to_string(),
        forwarding: None,
        maintenance: None,
        // 管理面聚合（票 52）在这一格里不挂：只测升级接口面。
        admin: None,
        updater,
    })
    .await
    .expect("起服务");
    (port, gateway)
}

#[tokio::test]
async fn update_endpoints_are_503_when_the_shell_did_not_wire_them() {
    let (port, gateway) = start("nowire", None).await;

    let res = reqwest::get(format!("http://127.0.0.1:{port}/api/update/status"))
        .await
        .expect("可达");
    assert_eq!(res.status(), 503, "没挂升级面就要明说");
    let body: serde_json::Value = res.json().await.expect("错误体");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("升级面不可用"),
        "{body}"
    );

    for path in ["/api/update/check", "/api/update/install"] {
        let res = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}{path}"))
            .send()
            .await
            .expect("可达");
        assert_eq!(res.status(), 503, "{path} 没挂升级面就要明说");
    }
    gateway.stop().await;
}

#[tokio::test]
async fn check_and_install_go_through_the_wired_source() {
    let fake = FakeUpdater::new("0.1.0");
    let (port, gateway) = start("wired", Some(fake.clone() as Arc<dyn UpdateSource>)).await;
    let client = reqwest::Client::new();

    // 查一次：拿到可用版本（字段是 camelCase，前端直接用）
    let res = client
        .post(format!("http://127.0.0.1:{port}/api/update/check"))
        .send()
        .await
        .expect("可达");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("JSON");
    assert_eq!(body["currentVersion"], "0.1.0");
    assert_eq!(body["available"]["version"], "0.2.0");
    assert_eq!(
        body["rollback"]["installersDir"],
        r"C:\tmp\updates\installers"
    );
    assert_eq!(fake.checks.load(Ordering::SeqCst), 1, "check 要真打到源上");

    // 装：立刻应答（不等安装器），回执里带上「正在装哪个包」
    let res = client
        .post(format!("http://127.0.0.1:{port}/api/update/install"))
        .send()
        .await
        .expect("可达");
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("JSON");
    assert_eq!(body["installing"], true);
    assert_eq!(
        body["installingPackage"],
        "Claude Gateway_0.2.0_x64-setup.exe"
    );
    assert_eq!(fake.installs.load(Ordering::SeqCst), 1);

    // 轮询 status
    let body: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/api/update/status"))
        .send()
        .await
        .expect("可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["currentVersion"], "0.1.0");
    gateway.stop().await;
}

#[tokio::test]
async fn a_failing_check_comes_back_as_502_with_the_readable_message() {
    let fake = FakeUpdater::new("0.1.0");
    *fake.check_result.lock().expect("锁") = Err(
        "检查更新失败：端点不是 https（The configured updater endpoint must use a secure protocol like `https`.）"
            .to_string(),
    );
    let (port, gateway) = start("badcheck", Some(fake as Arc<dyn UpdateSource>)).await;
    let res = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/update/check"))
        .send()
        .await
        .expect("可达");
    assert_eq!(res.status(), 502, "查不动要说清原因，不能假装没有更新");
    let body: serde_json::Value = res.json().await.expect("错误体");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("不是 https"), "{message}");
    assert_eq!(body["error"]["type"], "api_error");
    gateway.stop().await;
}

#[tokio::test]
async fn a_failing_install_is_a_400_with_the_reason() {
    let fake = FakeUpdater::failing_install(
        "0.1.0",
        "签名校验失败：The signature was not created by a trusted key",
    );
    let (port, gateway) = start("badinstall", Some(fake as Arc<dyn UpdateSource>)).await;

    let res = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/update/install"))
        .send()
        .await
        .expect("可达");
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.expect("错误体");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("签名校验失败"),
        "{body}"
    );
    gateway.stop().await;
}
