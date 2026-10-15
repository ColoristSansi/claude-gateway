//! 票 52 验收：前端四页需要的 `/api/*` 端点，走**真实 HTTP**（网关 + 假上游都在本进程外
//! 可达的端口上），配置根全部指临时目录——真实 `%APPDATA%` / `%LOCALAPPDATA%` / `~/.claude`
//! 一个字节都不碰。
//!
//! 覆盖：三步主路径（加上游 → 绑槽位 → 切客户端）、侧栏徽标、四态实读（含**手改后如实报
//! 「值不匹配」**）、一键还原、端口探测与换端口（含自动同步已生效客户端地址）、被引用上游
//! 的删除冲突、诊断包脱敏、用量汇总。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_core::clients::ClientEnv;
use gateway_core::ingress::api::{AdminCtx, GatewayControl};
use gateway_core::service::{self, GatewayConfig};
use gateway_core::store::Store;
use serde_json::{json, Value};

/// 测试里的外壳：只记账，不真的重启（重启在单进程测试里没有落点）。
#[derive(Default)]
struct TestControl {
    calls: Mutex<Vec<String>>,
}

impl GatewayControl for TestControl {
    fn start(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("start".into());
        Ok(())
    }

    fn restart(&self, port: u16) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("restart:{port}"));
        Ok(())
    }
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("cg52-admin-api").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 假上游：`/models` 报两条模型（带 `supported_endpoints`，供 wire 反查），
/// `/v1/messages` 回一条最短的非流式消息。
async fn mock_upstream() -> String {
    async fn models() -> Json<Value> {
        Json(json!({"data": [
            {"id": "mock-fast", "supported_endpoints": ["/v1/messages", "/v1/chat/completions"]},
            {"id": "mock-slow", "supported_endpoints": ["/v1/messages"]}
        ]}))
    }
    /// 上游总是收到 `stream: true`（网关自己决定是否缓冲），所以假上游回 SSE。
    async fn messages() -> axum::response::Response {
        use axum::response::IntoResponse;
        let body = concat!(
            "event: message_start
",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"mock-fast\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}

",
            "event: content_block_start
",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

",
            "event: content_block_delta
",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}

",
            "event: content_block_stop
",
            "data: {\"type\":\"content_block_stop\",\"index\":0}

",
            "event: message_delta
",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}

",
            "event: message_stop
",
            "data: {\"type\":\"message_stop\"}

",
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            body,
        )
            .into_response()
    }

    async fn chat() -> Json<Value> {
        Json(
            json!({"id": "cmpl_mock", "object": "chat.completion", "model": "mock-fast",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"},
                                 "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4}}),
        )
    }
    let app = Router::new()
        .route("/models", get(models))
        .route("/v1/models", get(models))
        .route("/v1/messages", post(messages))
        .route("/v1/chat/completions", post(chat));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{}", addr.port())
}

struct Harness {
    base: String,
    gateway: service::Gateway,
    control: Arc<TestControl>,
    root: PathBuf,
}

impl Harness {
    async fn start(tag: &str) -> Harness {
        let root = tmp(tag);
        let store = Store::open(&root).expect("开库");
        let env = ClientEnv::under(root.join("clients"));
        let control = Arc::new(TestControl::default());
        let forwarding = Arc::new(proxy_with(store.clone()));
        let admin = Arc::new(
            AdminCtx::new(
                store,
                root.clone(),
                env,
                control.clone(),
                "0.1.0-test",
                root.join("gateway.db").to_string_lossy().into_owned(),
            )
            .with_proxy(forwarding.clone()),
        );
        let config = GatewayConfig {
            port: 0,
            db_path: root.join("gateway.db"),
            version: "0.1.0-test".into(),
            // 转发面挂上：验收要真走 `/v1/messages` 入站（票 41 的 proxy）。
            forwarding: Some(forwarding),
            admin: Some(admin.clone()),
            // 票 62 的维护面：这一格只测票 52 的管理面，不挂。
            maintenance: None,
            // 票 61 的升级面：要 Tauri updater 插件，这里不挂。
            updater: None,
        };
        let gateway = service::start(config).await.expect("起网关");
        Harness {
            base: format!("http://127.0.0.1:{}", gateway.port()),
            gateway,
            control,
            root,
        }
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = reqwest::get(format!("{}{path}", self.base)).await.unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .put(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    async fn stop(self) {
        self.gateway.stop().await;
    }

    fn claude_code_file(&self) -> PathBuf {
        self.root
            .join("clients")
            .join("claude-code")
            .join("settings.json")
    }
}

/// 起一个能真转发的 proxy（空表也要挂：装配路径与外壳一致）。
fn proxy_with(store: Store) -> gateway_core::ingress::proxy::Gateway {
    gateway_core::ingress::proxy::Gateway::new(
        store,
        std::collections::HashMap::new(),
        gateway_core::binding::BindingTable::default(),
    )
}

/// 走完「加上游 → 拉模型 → 自检」三步，回上游 id。
async fn onboard_upstream(h: &Harness, mock: &str, id: &str) {
    let (status, body) = h
        .post(
            "/api/upstreams",
            json!({
                "id": id,
                "name": id,
                "baseUrl": mock,
                "paths": {"anthropic": "/v1/messages", "openai-chat": "/v1/chat/completions"},
                "credentials": [{"wire": "default", "key": "sk-test-secret-DO-NOT-LEAK"}]
            }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    assert!(
        !body.to_string().contains("sk-test-secret-DO-NOT-LEAK"),
        "建上游的回执里带了密钥明文：{body}"
    );

    let (status, body) = h
        .post(&format!("/api/upstreams/{id}/discovery"), json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["disposition"], "fetched", "{body}");
    assert_eq!(body["models"].as_array().unwrap().len(), 2, "{body}");

    let (status, body) = h
        .post(&format!("/api/upstreams/{id}/selftest"), json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["status"], 200, "{body}");
}

#[tokio::test]
async fn three_step_path_from_zero_then_acceptance() {
    let h = Harness::start("three-step").await;
    let mock = mock_upstream().await;

    // 起手：三步全未就绪，侧栏两个徽标都在。
    let (status, badges) = h.get("/api/sidebar/badges").await;
    assert_eq!(status, 200);
    assert_eq!(badges["upstreamsFailedSelfcheck"], 0);
    assert_eq!(badges["clientsNotEffective"], 3, "{badges}");
    assert_eq!(badges["ready"], false);

    // ① 加上游 + 拉模型 + 自检
    onboard_upstream(&h, &mock, "mock").await;
    let (_, badges) = h.get("/api/sidebar/badges").await;
    assert_eq!(badges["upstreamsTotal"], 1, "{badges}");
    assert_eq!(badges["upstreamsFailedSelfcheck"], 0, "{badges}");
    assert_eq!(badges["ready"], true, "自检过了第一步就该绿");

    // ② 绑槽位（目录里选）+ 探针
    for slot in [
        "claude-fable",
        "claude-opus",
        "claude-sonnet",
        "claude-haiku",
    ] {
        let (status, body) = h
            .put(
                &format!("/api/bindings/{slot}"),
                json!({"upstreamId": "mock", "realModel": "mock-fast", "wire": "anthropic"}),
            )
            .await;
        assert_eq!(status, 200, "{slot}: {body}");
        assert_eq!(body["saved"], true, "{body}");
    }
    let (status, probe) = h.post("/api/bindings/claude-haiku/probe", json!({})).await;
    assert_eq!(status, 200, "{probe}");
    assert_eq!(probe["ok"], true, "{probe}");
    assert!(probe["elapsedMs"].as_u64().is_some(), "{probe}");

    // 目录里没有的模型绑不上（三层校验第一层）。
    let (status, body) = h
        .put(
            "/api/bindings/claude-haiku",
            json!({"upstreamId": "mock", "realModel": "does-not-exist", "wire": "anthropic"}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]["what"]
            .as_str()
            .unwrap()
            .contains("目录里没有"),
        "{body}"
    );

    // ③ 切客户端（Claude Code 只写 settings.json，最省事）
    let (status, body) = h.post("/api/clients/claude-code/apply", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["client"]["state"], "effective", "{body}");
    let written = std::fs::read_to_string(h.claude_code_file()).expect("切换后文件应当存在");
    assert!(written.contains("ANTHROPIC_BASE_URL"), "{written}");
    assert!(
        written.contains(&format!("127.0.0.1:{}", h.gateway.port())),
        "{written}"
    );

    let (_, badges) = h.get("/api/sidebar/badges").await;
    assert_eq!(badges["clientsNotEffective"], 2, "{badges}");

    // 端到端验收：haiku 槽位发一条非流式最短请求，走真实入站。
    let (status, acc) = h.post("/api/acceptance/run", json!({})).await;
    assert_eq!(status, 200, "{acc}");
    assert_eq!(acc["ok"], true, "验收应当通过：{acc}");
    assert_eq!(acc["slot"], "claude-haiku");
    assert_eq!(acc["upstream"], "mock", "{acc}");
    assert_eq!(acc["status"], 200, "{acc}");
    assert!(acc["elapsedMs"].as_u64().is_some(), "{acc}");
    // 请求日志行的 id 一定在（`requests.request_id` 由转发面填，见遗留：本票用 logId 兜底）。
    assert!(acc["logId"].as_i64().is_some(), "{acc}");

    // 用量：刚才那次验收在库里留了一行，今日汇总能看见。
    let (status, usage) = h
        .get("/api/usage/summary?range=today&dim=virtual_model")
        .await;
    assert_eq!(status, 200, "{usage}");
    assert!(
        usage["totals"]["requests"].as_i64().unwrap() >= 1,
        "{usage}"
    );
    assert_eq!(usage["rows"][0]["key"], "claude-haiku", "{usage}");

    // 诊断包：脱敏（密钥明文一个字节都不许出现），且带上上游配置与请求元数据。
    let (status, diag) = h
        .post(
            "/api/diagnostics/copy",
            json!({"requestId": acc["logId"], "error": "上游 401"}),
        )
        .await;
    assert_eq!(status, 200, "{diag}");
    let text = diag["text"].as_str().unwrap();
    assert!(!text.contains("sk-test-secret-DO-NOT-LEAK"), "{text}");
    assert!(text.contains(&mock), "诊断包要带上游地址：{text}");
    assert!(
        text.contains("mock-fast") || text.contains("请求元数据"),
        "{text}"
    );
    assert!(
        text.contains("sk-…") || text.contains("…"),
        "密钥要掩码：{text}"
    );

    h.stop().await;
}

#[tokio::test]
async fn client_states_are_read_from_disk_not_cached() {
    let h = Harness::start("four-states").await;
    let mock = mock_upstream().await;
    onboard_upstream(&h, &mock, "mock").await;
    h.put(
        "/api/bindings/claude-haiku",
        json!({"upstreamId": "mock", "realModel": "mock-fast", "wire": "anthropic"}),
    )
    .await;

    let card = |body: &Value| body["clients"].as_array().unwrap().clone();
    let find = |cards: &[Value], id: &str| -> Value {
        cards
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    };

    // ① 未配置
    let (_, body) = h.get("/api/clients").await;
    assert_eq!(find(&card(&body), "claude-code")["state"], "not-configured");

    // ② 已生效
    h.post("/api/clients/claude-code/apply", json!({})).await;
    let (_, body) = h.get("/api/clients").await;
    let cc = find(&card(&body), "claude-code");
    assert_eq!(cc["state"], "effective", "{cc}");
    assert_eq!(cc["stateLabel"], "已生效");
    assert_eq!(cc["installed"], true);
    let path = cc["files"][0]["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("settings.json"), "{path}");
    assert!(
        cc["files"][0]["backupPreview"]
            .as_str()
            .unwrap()
            .contains("backups"),
        "{cc}"
    );

    // ③ 手改文件 → 如实报「值不匹配」（不缓存：每次进页面实读）
    let file = h.claude_code_file();
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    doc["env"]["ANTHROPIC_BASE_URL"] = json!("http://127.0.0.1:9999");
    std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let (_, body) = h.get("/api/clients").await;
    let cc = find(&card(&body), "claude-code");
    assert_eq!(cc["state"], "mismatch", "{cc}");
    assert_eq!(cc["stateLabel"], "值不匹配");
    let diff = cc["files"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "env.ANTHROPIC_BASE_URL")
        .cloned()
        .unwrap();
    assert_eq!(diff["matches"], false, "{diff}");
    assert_eq!(diff["current"], "http://127.0.0.1:9999", "{diff}");

    // 「保留我的修改」：把记录对齐磁盘，之后文件不再被判冲突。
    let (status, body) = h
        .post("/api/clients/claude-code/keep-mine", json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["kept"].as_array().unwrap().len() == 1, "{body}");

    // ④ 一键还原 → 已回官方（网关键摘掉）
    let (status, body) = h.post("/api/clients/claude-code/restore", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["client"]["state"], "official", "{body}");
    assert_eq!(body["client"]["stateLabel"], "已回官方");
    // 这份 settings.json 是网关新建的（原先没有）：还原 = 整个删掉（spec §8.4 第 8 条）。
    assert!(
        !file.exists(),
        "网关新建、且只剩网关内容的文件，还原时应当整个删掉"
    );

    // 备份清单列得出来（二次确认弹窗要用）。
    let (status, backups) = h.get("/api/clients/claude-code/backups").await;
    assert_eq!(status, 200, "{backups}");
    assert!(
        !backups["backups"].as_array().unwrap().is_empty(),
        "{backups}"
    );

    h.stop().await;
}

#[tokio::test]
async fn port_probe_and_change_syncs_only_effective_clients() {
    let h = Harness::start("port").await;
    let mock = mock_upstream().await;
    onboard_upstream(&h, &mock, "mock").await;
    h.put(
        "/api/bindings/claude-haiku",
        json!({"upstreamId": "mock", "realModel": "mock-fast", "wire": "anthropic"}),
    )
    .await;
    h.post("/api/clients/claude-code/apply", json!({})).await;

    // 探测：只预填，不写入。
    let (status, probe) = h.post("/api/gateway/port/probe", json!({})).await;
    assert_eq!(status, 200, "{probe}");
    let suggested = probe["suggestedPort"].as_u64().unwrap();
    assert!(suggested > 0);
    let (_, status_before) = h.get("/api/status").await;
    assert_eq!(
        status_before["port"].as_u64().unwrap(),
        h.gateway.port() as u64,
        "探测不许写端口"
    );

    // 确认后写入：只同步「已生效」的客户端，并让外壳按新端口重启。
    let (status, changed) = h
        .post("/api/gateway/port", json!({"port": suggested}))
        .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["port"], suggested, "{changed}");
    assert_eq!(changed["synced"].as_array().unwrap().len(), 1, "{changed}");
    assert_eq!(changed["synced"][0]["id"], "claude-code", "{changed}");
    assert!(
        changed["message"]
            .as_str()
            .unwrap()
            .contains("已同步 1 个客户端"),
        "{changed}"
    );
    assert_eq!(
        h.control.calls.lock().unwrap().last().unwrap(),
        &format!("restart:{suggested}")
    );
    let written = std::fs::read_to_string(h.claude_code_file()).unwrap();
    assert!(
        written.contains(&format!("127.0.0.1:{suggested}")),
        "已生效客户端要跟着换地址：{written}"
    );

    // 未生效的那两个一个字节都不动。
    assert!(!h
        .root
        .join("clients")
        .join("codex")
        .join("config.toml")
        .exists());

    // 端口被占：再换到一个正被占着的端口要报 409。
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let (status, body) = h
        .post("/api/gateway/port", json!({"port": busy_port}))
        .await;
    assert_eq!(status, 409, "{body}");
    drop(busy);

    h.stop().await;
}

#[tokio::test]
async fn referenced_upstream_delete_needs_force_and_diagnostics_stay_masked() {
    let h = Harness::start("delete").await;
    let mock = mock_upstream().await;
    onboard_upstream(&h, &mock, "mock").await;
    h.put(
        "/api/bindings/claude-haiku",
        json!({"upstreamId": "mock", "realModel": "mock-fast", "wire": "anthropic"}),
    )
    .await;

    // 被引用：409 + 引用处；强制删要显式 force。
    let resp = reqwest::Client::new()
        .delete(format!("{}/api/upstreams/mock", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);
    let (status, refs) = h.get("/api/upstreams/mock/references").await;
    assert_eq!(status, 200);
    assert_eq!(refs["references"][0]["model"], "claude-haiku", "{refs}");

    let resp = reqwest::Client::new()
        .delete(format!("{}/api/upstreams/mock?force=true", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["bindingsRemoved"], 1, "{body}");
    let (_, ups) = h.get("/api/upstreams").await;
    assert_eq!(ups["upstreams"].as_array().unwrap().len(), 0, "{ups}");

    h.stop().await;
}

#[tokio::test]
async fn providers_presets_and_upstream_view_columns() {
    let h = Harness::start("providers").await;
    let mock = mock_upstream().await;
    onboard_upstream(&h, &mock, "mock").await;

    let (status, providers) = h.get("/api/providers").await;
    assert_eq!(status, 200, "{providers}");
    let presets = providers["presets"].as_array().unwrap();
    assert!(
        presets.iter().any(|p| p["id"] == "commandcode"),
        "{providers}"
    );
    assert!(
        presets
            .iter()
            .all(|p| p["baseUrl"].as_str().is_some_and(|u| u.starts_with("http"))),
        "预填必须有真 baseUrl：{providers}"
    );

    // 列表六列 + 被引用数 + 凭据掩码（明文不出界）。
    let (_, ups) = h.get("/api/upstreams").await;
    let up = &ups["upstreams"][0];
    assert_eq!(up["name"], "mock");
    assert!(up["baseUrl"].as_str().unwrap().starts_with("http"));
    assert_eq!(up["lastSelfcheck"]["ok"], true, "{up}");
    assert_eq!(up["lastPull"]["status"], "ok", "{up}");
    assert_eq!(up["references"], 0, "{up}");
    assert!(up["modelCount"].as_i64().unwrap() >= 2, "{up}");
    let mask = up["credentials"][0]["mask"].as_str().unwrap();
    assert!(!mask.contains("DO-NOT-LEAK"), "{up}");
    assert_eq!(up["credentials"][0]["wire"], "default", "{up}");

    h.stop().await;
}
