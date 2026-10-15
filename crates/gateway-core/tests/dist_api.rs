//! 票 62 的管理 API 验收：开机自启开关、导出、导入、给票 59 的常量格。
//!
//! 起真服务、发真 HTTP、落真文件；库与数据目录都在临时目录里。
//! **自启用带进程号的临时 Run 值名**（`Maintenance::with_autostart_value`）：真实注册表往返，
//! 但绝不碰用户真正的 `Claude Gateway` 值。

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use gateway_core::dist::autostart::{self, HkcuRun, RunKey};
use gateway_core::ingress::api::Maintenance;
use gateway_core::service::{self, GatewayConfig};
use gateway_core::store::Store;

const KNOWN_SECRET: &str = "sk-or-v1-CG62-API-CHECK-0123456789abcdef-NOT-IN-FILE";

fn tmp(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cg62-api-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn free_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").expect("绑探测端口");
    let port = probe.local_addr().expect("读探测地址").port();
    drop(probe);
    port
}

async fn seeded_store(root: &Path) -> Store {
    let store = Store::open(root).expect("开库");
    store
        .write(|c| {
            c.execute(
                "INSERT INTO upstreams(id,name,base_url) VALUES('openrouter','OpenRouter','https://openrouter.ai/api')",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("种上游");
    store
        .put_credential("openrouter", "anthropic", KNOWN_SECRET)
        .await
        .expect("存密钥");
    store
}

struct Env {
    port: u16,
    root: PathBuf,
    /// 临时 Run 值名（测试自己造、自己删）。
    value_name: String,
    _gateway: service::Gateway,
}

async fn start(tag: &str) -> Env {
    let root = tmp(tag);
    let store = seeded_store(&root).await;
    let value_name = format!("Claude Gateway (t62 api {}-{tag})", std::process::id());
    let port = free_port();
    let gateway = service::start(GatewayConfig {
        port,
        db_path: store.db_path().to_path_buf(),
        version: "0.1.0-test".to_string(),
        forwarding: None,
        maintenance: Some(Maintenance::with_autostart_value(
            store,
            root.clone(),
            value_name.clone(),
        )),
    })
    .await
    .expect("起服务");
    Env {
        port,
        root,
        value_name,
        _gateway: gateway,
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

#[tokio::test]
async fn autostart_endpoint_reads_writes_and_clears_the_run_value() {
    let env = start("autostart").await;
    let client = reqwest::Client::new();

    // 初始：关闭（票面：默认关闭）。
    let body: serde_json::Value = client
        .get(env.url("/api/settings/autostart"))
        .send()
        .await
        .expect("GET 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["enabled"], false, "默认必须是关闭：{body}");
    assert_eq!(body["valueName"], env.value_name.as_str());
    assert_eq!(
        body["runKey"],
        r"Software\Microsoft\Windows\CurrentVersion\Run"
    );
    assert!(
        body["commandLine"]
            .as_str()
            .expect("命令行")
            .contains("--startup"),
        "{body}"
    );

    // 打开：真写 HKCU Run 值（临时名），注册表里查得到。
    let body: serde_json::Value = client
        .put(env.url("/api/settings/autostart"))
        .json(&serde_json::json!({ "enabled": true }))
        .send()
        .await
        .expect("PUT 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["enabled"], true, "{body}");
    let key = HkcuRun::named(env.value_name.clone());
    assert!(
        key.get().expect("读注册表").is_some(),
        "PUT 之后注册表里必须有这个值"
    );

    // 关闭：删值且无残留。
    let body: serde_json::Value = client
        .put(env.url("/api/settings/autostart"))
        .json(&serde_json::json!({ "enabled": false }))
        .send()
        .await
        .expect("PUT 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["enabled"], false, "{body}");
    assert_eq!(key.get().expect("读注册表"), None, "关闭后不许留残留");
}

#[tokio::test]
async fn export_endpoint_writes_a_file_without_plaintext_secrets_then_import_reads_it_back() {
    let env = start("export-import").await;
    let client = reqwest::Client::new();
    let out = env.root.join("导出.json");

    let body: serde_json::Value = client
        .post(env.url("/api/export"))
        .json(&serde_json::json!({ "path": out.to_string_lossy() }))
        .send()
        .await
        .expect("POST 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["secrets"], false, "默认不含明文密钥：{body}");
    assert_eq!(body["encrypted"], false);
    assert!(
        body["dpapiWarning"]
            .as_str()
            .expect("换机提示")
            .contains("DPAPI"),
        "导出回执必须带上换机提示：{body}"
    );

    let text = fs::read_to_string(&out).expect("读导出文件");
    assert!(!text.contains(KNOWN_SECRET), "导出文件里不许有明文密钥");
    assert!(!text.contains("NOT-IN-FILE"), "连片段都不许有");

    // 导入到**另一个**数据目录（同一进程的库没法换，这里用 CLI 同款的核心函数在临时库上验）。
    let other = Store::open(&env.root.join("imported")).expect("开导入库");
    let report = gateway_core::dist::bundle::import(
        &other,
        &text,
        None,
        gateway_core::dist::bundle::ImportMode::SkipExisting,
    )
    .await
    .expect("导入");
    assert_eq!(report.upstreams_created, 1, "{report:?}");
    assert_eq!(report.credentials_skipped, 1, "没有密钥的导出 → 凭据跳过");

    // 加密导出走接口：回执说清加了密，文件里照样没有明文。
    let with_pw = env.root.join("加密导出.json");
    let body: serde_json::Value = client
        .post(env.url("/api/export"))
        .json(&serde_json::json!({
            "path": with_pw.to_string_lossy(),
            "password": "口令-62",
            "includeSecrets": true
        }))
        .send()
        .await
        .expect("POST 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["secrets"], true);
    assert_eq!(body["encrypted"], true, "{body}");
    let text = fs::read_to_string(&with_pw).expect("读加密导出");
    assert!(!text.contains(KNOWN_SECRET), "加密文件里不许有明文");

    // 导入那个加密文件：密钥回来了（这才是「导入回来网关可用」）。
    let third = Store::open(&env.root.join("imported-pw")).expect("开第三个库");
    let report = gateway_core::dist::bundle::import(
        &third,
        &text,
        Some("口令-62"),
        gateway_core::dist::bundle::ImportMode::SkipExisting,
    )
    .await
    .expect("带口令导入");
    assert_eq!(report.credentials_imported, 1, "{report:?}");
    assert_eq!(
        third
            .credential_secret("openrouter", "anthropic")
            .await
            .expect("查密钥")
            .as_deref(),
        Some(KNOWN_SECRET)
    );

    // 导入接口本身：读文件、给回执（版本 + 重复上游都在回执里）。
    let body: serde_json::Value = client
        .post(env.url("/api/import"))
        .json(&serde_json::json!({ "path": out.to_string_lossy() }))
        .send()
        .await
        .expect("POST 可达")
        .json()
        .await
        .expect("JSON");
    assert_eq!(body["version"], 1, "{body}");
    assert_eq!(
        body["duplicates"][0], "openrouter",
        "重复上游要列出来：{body}"
    );
    assert_eq!(body["upstreamsSkipped"], 1, "{body}");

    // 坏文件 / 缺参数：明确的 400，不是 500，也不是静默成功。
    let res = client
        .post(env.url("/api/import"))
        .json(&serde_json::json!({ "path": env.root.join("不存在.json").to_string_lossy() }))
        .send()
        .await
        .expect("POST 可达");
    assert_eq!(res.status(), 400, "读不到文件要 400");
    let body: serde_json::Value = res.json().await.expect("JSON");
    assert!(body["error"]
        .as_str()
        .expect("错误文案")
        .contains("读导入文件失败"));
}

#[tokio::test]
async fn dist_info_hands_the_settings_page_the_warning_and_the_usage() {
    let env = start("info").await;
    let body: serde_json::Value = reqwest::get(env.url("/api/dist/info"))
        .await
        .expect("GET 可达")
        .json()
        .await
        .expect("JSON");

    assert_eq!(body["exportFormat"], "claude-gateway-export");
    assert_eq!(body["exportVersion"], 1);
    assert_eq!(body["autostartDefault"], false);
    assert!(body["dpapiWarning"]
        .as_str()
        .expect("提示")
        .contains("DPAPI"));
    assert!(body["usage"]
        .as_str()
        .expect("用法")
        .contains("--restore-clients"));
    assert!(body["restoreCommand"]
        .as_str()
        .expect("命令")
        .contains("--silent"));
}

#[tokio::test]
async fn maintenance_endpoints_are_503_when_the_shell_did_not_wire_them() {
    let port = free_port();
    let root = tmp("no-maint");
    let store = Store::open(&root).expect("开库");
    let gateway = service::start(GatewayConfig {
        port,
        db_path: store.db_path().to_path_buf(),
        version: "0.1.0-test".to_string(),
        forwarding: None,
        maintenance: None,
    })
    .await
    .expect("起服务");

    let res = reqwest::get(format!("http://127.0.0.1:{port}/api/settings/autostart"))
        .await
        .expect("可达");
    assert_eq!(res.status(), 503, "没挂维护面就要明说，不许假装成功");
    gateway.stop().await;
}

/// 清理临时 Run 值（万一某个断言提前失败，别把测试值留在用户注册表里）。
#[test]
fn cleanup_helpers_are_no_ops_for_unknown_names() {
    let key = HkcuRun::named("Claude Gateway (t62 api never-written)");
    key.delete().expect("删不存在的值也要成功");
    assert!(key.get().expect("读").is_none());
    let _ = autostart::is_enabled(&key).expect("读开关");
}
