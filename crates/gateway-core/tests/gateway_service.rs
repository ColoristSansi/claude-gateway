//! 网关服务生命周期与管理 API 骨架的验收（票 43）。
//!
//! 测试面是 `gateway_core::service` 的公开接口：起服务、读 `/api/status`、占端口、停服务。
//! 不碰真实 `%APPDATA%`，库路径一律用临时目录。

use std::net::TcpListener;
use std::path::PathBuf;

use gateway_core::service::{self, GatewayConfig};

fn config(port: u16, db_path: &str) -> GatewayConfig {
    GatewayConfig {
        port,
        db_path: PathBuf::from(db_path),
        version: "0.1.0-test".to_string(),
        // 票 43 的验收只测管理面与生命周期；转发面由票 41 的回放套件起 harness 单测（见
        // tests/replay.rs），票 51 接了真绑定表之后再从这里挂上。
        forwarding: None,
        // 维护面（票 62）在 tests/dist_api.rs 里单独挂：那一格要真库与真数据目录。
        maintenance: None,
    }
}

/// 让操作系统给一个此刻空闲的端口，再让网关去绑它。
fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").expect("绑探测端口");
    let port = probe.local_addr().expect("读探测地址").port();
    drop(probe);
    port
}

#[tokio::test]
async fn api_status_reports_version_port_start_time_and_db_path() {
    let port = free_port();
    let gateway = service::start(config(port, r"C:\tmp\cg-test\gateway.db"))
        .await
        .expect("空闲端口应当起得来");

    assert_eq!(gateway.port(), port, "实例化端口 = 配置端口");

    let body: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{port}/api/status"))
        .await
        .expect("管理 API 可达")
        .json()
        .await
        .expect("返回 JSON");

    assert_eq!(body["version"], "0.1.0-test");
    assert_eq!(body["port"], port);
    assert_eq!(body["running"], true);
    assert_eq!(body["dbPath"], r"C:\tmp\cg-test\gateway.db");
    let started_at = body["startedAt"].as_str().expect("startedAt 是字符串");
    assert!(
        started_at.ends_with('Z') && started_at.len() == 20 && started_at.contains('T'),
        "startedAt 应当是 RFC3339 UTC，实际 {started_at:?}"
    );
    assert!(body["uptimeSeconds"].as_u64().is_some());

    gateway.stop().await;
}

#[tokio::test]
async fn occupied_port_is_a_loud_error_naming_the_port() {
    let squatter = TcpListener::bind("127.0.0.1:0").expect("先占一个端口");
    let port = squatter.local_addr().expect("读地址").port();

    let err = service::start(config(port, r"C:\tmp\cg-test\gateway.db"))
        .await
        .err()
        .expect("端口被占必须起不来（不静默换端口）");

    let msg = err.to_string();
    assert!(msg.contains(&port.to_string()), "报错要带端口号：{msg}");
    assert!(msg.contains("占用"), "报错要点明占用：{msg}");
    assert!(
        msg.contains("不会自动换端口"),
        "报错要说清不静默换端口：{msg}"
    );
}

#[tokio::test]
async fn management_api_and_forwarding_surface_do_not_alias() {
    let port = free_port();
    let gateway = service::start(config(port, r"C:\tmp\cg-test\gateway.db"))
        .await
        .expect("起得来");

    let client = reqwest::Client::new();

    // 转发面（/v1/*）此刻是空的，但绝不能顺手把管理端点暴露在 /v1 下。
    let leaked = client
        .get(format!("http://127.0.0.1:{port}/v1/status"))
        .send()
        .await
        .expect("请求可达");
    assert_eq!(leaked.status(), 404, "/v1/status 不该存在");

    // 反向同理：转发用的 POST 不该在 /api 下被接受。
    let wrong = client
        .post(format!("http://127.0.0.1:{port}/api/messages"))
        .send()
        .await
        .expect("请求可达");
    assert_eq!(wrong.status(), 404, "/api/messages 不该存在");

    let ok = client
        .get(format!("http://127.0.0.1:{port}/api/status"))
        .send()
        .await
        .expect("请求可达");
    assert_eq!(ok.status(), 200);

    gateway.stop().await;
}

#[tokio::test]
async fn stop_closes_the_listener_and_flips_running_to_false() {
    let port = free_port();
    let gateway = service::start(config(port, r"C:\tmp\cg-test\gateway.db"))
        .await
        .expect("起得来");

    let before = gateway.status();
    assert!(before.running, "停之前是运行中");

    let after = gateway.stop().await;
    assert!(!after.running, "停之后是未运行");

    let refused = reqwest::get(format!("http://127.0.0.1:{port}/api/status")).await;
    assert!(refused.is_err(), "停止后监听应当已关闭（端口不再应答）");
}
