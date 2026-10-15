//! 票 56 验收：Ollama 兼容面五个端点、两处怪癖（NDJSON 终态、按名反查补 `tool_use_id`）、
//! 装载 / 卸载与错误信封。全部走**真实 HTTP**：`ingress::proxy::router()` 在本进程里挂起来，
//! 上游是本进程里的假 Anthropic 面。
//!
//! 断言的期望值来自独立来源：NDJSON 帧形状来自 spec §4.4 与本机 Ollama 0.40.4 的实测输出，
//! `digest` 的 hex 取自 `sha256sum` 的公开结果，不拿被测代码反算。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::binding::{self, default_profiles, Binding, BindingTable, Profile};
use gateway_core::egress::provider::{self, AuthScheme, Upstream, Wire};
use gateway_core::ingress::api::ApiState;
use gateway_core::ingress::proxy::Gateway;
use gateway_core::store::Store;
use serde_json::{json, Value};

// ───────────────────────── 夹具 ─────────────────────────

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg56-ollama-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/anthropic/v1/messages");
    up
}

/// 两条绑定：一个家族槽位（`claude-haiku`）与一个普通虚拟模型（`my-model`），都打假上游。
fn table() -> BindingTable {
    BindingTable::new(
        vec![
            Binding::new(
                Profile::Global,
                "claude-haiku",
                "mock",
                "mock-text",
                Wire::Anthropic,
            ),
            Binding::new(
                Profile::Global,
                "my-model",
                "mock",
                "mock-text",
                Wire::Anthropic,
            ),
        ],
        default_profiles(),
    )
}

// ── 假上游：按脚本回一段 Anthropic SSE，并把收到的出站请求体记下来 ──

/// 纯文本：一帧文本 delta + usage，正常收尾。
const TEXT_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":11}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你好\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"！\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":11,\"output_tokens\":2,\"cache_read_input_tokens\":80}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// 工具调用：入参分两片发（Ollama 面必须缓冲到 block stop 才发整通）。
const TOOLS_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_t\",\"usage\":{\"input_tokens\":9}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"上海\\\"}\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"input_tokens\":9,\"output_tokens\":5}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// 断流：200 开着，但走到一半就没了（没有 `message_stop`）。
const ABORT_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_a\",\"usage\":{\"input_tokens\":3}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"半\"}}\n\n",
);

#[derive(Clone, Default)]
struct Mock {
    seen: Arc<Mutex<Vec<Value>>>,
    script: Arc<Mutex<String>>,
}

impl Mock {
    fn set(&self, name: &str) {
        *self.script.lock().unwrap() = name.to_string();
    }

    fn len(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// 第 `i` 次出站请求体（0 起）。
    fn request(&self, i: usize) -> Value {
        self.seen
            .lock()
            .unwrap()
            .get(i)
            .cloned()
            .unwrap_or(Value::Null)
    }
}

async fn mock_upstream() -> (String, Mock) {
    let mock = Mock::default();
    let app = Router::new()
        .route("/anthropic/v1/messages", post(anthropic))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), mock)
}

async fn anthropic(State(m): State<Mock>, Json(v): Json<Value>) -> Response {
    m.seen.lock().unwrap().push(v);
    let script = m.script.lock().unwrap().clone();
    match script.as_str() {
        "500" => (StatusCode::INTERNAL_SERVER_ERROR, "上游炸了").into_response(),
        "tools" => sse(TOOLS_SSE),
        "abort" => sse(ABORT_SSE),
        _ => sse(TEXT_SSE),
    }
}

fn sse(body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// 起一份网关 + 假上游，返回 (网关 base, mock, 网关运行态)。
async fn harness(tag: &str) -> (String, Mock, Arc<Gateway>) {
    let store = Store::open(&tmp_root(tag)).expect("开库");
    let (mock_base, mock) = mock_upstream().await;
    provider::save_upstream(&store, &upstream("mock", &mock_base))
        .await
        .unwrap();
    store
        .put_credential("mock", "default", "mock-key")
        .await
        .unwrap();
    let mut upstreams = HashMap::new();
    upstreams.insert("mock".to_string(), upstream("mock", &mock_base));
    let app = Arc::new(Gateway::new(store, upstreams, table()));
    let base = spawn(gateway_core::ingress::proxy::router().with_state(app.clone())).await;
    (base, mock, app)
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

// ── HTTP 小件 ──

async fn post_json(base: &str, path: &str, body: &Value) -> (u16, String, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .expect("POST 失败");
    let status = resp.status().as_u16();
    let ctype = content_type(&resp);
    let v = resp.json().await.unwrap_or(Value::Null);
    (status, ctype, v)
}

/// POST 一个原始 body（空体 / 非法 JSON 两条 400 用它）。
async fn post_raw(base: &str, path: &str, body: &'static str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .expect("POST 失败");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// POST `/api/chat` 并把 NDJSON 逐行解析出来。
async fn chat(base: &str, body: &Value) -> (u16, String, Vec<Value>) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/chat"))
        .json(body)
        .send()
        .await
        .expect("POST /api/chat 失败");
    let status = resp.status().as_u16();
    let ctype = content_type(&resp);
    let text = resp.text().await.unwrap_or_default();
    let lines = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or(Value::Null))
        .collect();
    (status, ctype, lines)
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .send()
        .await
        .expect("GET 失败");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn head_status(base: &str, path: &str) -> u16 {
    reqwest::Client::new()
        .head(format!("{base}{path}"))
        .send()
        .await
        .expect("HEAD 失败")
        .status()
        .as_u16()
}

fn content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 出站请求体里最后一条 `tool_result` 块。
fn tool_result(sent: &Value) -> Value {
    sent["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .find(|b| b["type"] == "tool_result")
        .unwrap_or(Value::Null)
}

// ───────────────────────── ① `/api/chat`：NDJSON 流 ─────────────────────────

/// 怪癖①：一行一帧、末帧 `done:true` 才是终态、统计字段在帧顶层、无 `data:` / `[DONE]`。
#[tokio::test(flavor = "multi_thread")]
async fn chat_streams_ndjson_and_ends_with_a_terminal_done_frame() {
    let (base, _mock, _app) = harness("chat-stream").await;
    let (status, ctype, lines) = chat(
        &base,
        &json!({"model":"claude-haiku","messages":[{"role":"user","content":"你好"}]}),
    )
    .await;

    assert_eq!(status, 200, "{lines:?}");
    assert!(
        ctype.contains("application/x-ndjson"),
        "流式必须是 NDJSON：{ctype}"
    );
    assert_eq!(lines.len(), 3, "两帧文本 + 一帧终态：{lines:?}");
    // 增量帧：带 model 与 created_at，`done:false`，content 是**字符串**。
    assert_eq!(lines[0]["model"], "claude-haiku");
    assert_eq!(lines[0]["done"], false);
    assert_eq!(lines[0]["message"]["role"], "assistant");
    assert_eq!(lines[0]["message"]["content"], "你好");
    assert!(lines[0]["created_at"].is_string());
    // 末帧：content 空串、done:true、统计与 done_reason 在**帧顶层**。
    let last = &lines[2];
    assert_eq!(last["done"], true);
    assert_eq!(last["message"]["content"], "");
    assert_eq!(last["done_reason"], "stop");
    assert_eq!(last["prompt_eval_count"], 11);
    assert_eq!(last["eval_count"], 2);
    assert_eq!(last["prompt_eval_cached_count"], 80);
    assert!(
        last["message"].get("prompt_eval_count").is_none(),
        "统计不在 message 里：{last}"
    );
    // 传输不是 SSE：没有 `data:`、没有 `[DONE]`。
    let raw = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    for banned in ["data:", "[DONE]", "event:"] {
        assert!(!raw.contains(banned), "NDJSON 里不该出现 {banned}");
    }
}

/// `stream` **缺省为 true**（与 OpenAI 面相反）；`:latest` 被接受并剥掉。
#[tokio::test(flavor = "multi_thread")]
async fn chat_stream_defaults_to_true_and_accepts_the_latest_tag() {
    let (base, _mock, _app) = harness("chat-latest").await;
    let (status, ctype, lines) = chat(
        &base,
        &json!({"model":"claude-haiku:latest","messages":[{"role":"user","content":"你好"}]}),
    )
    .await;
    assert_eq!(status, 200, "{lines:?}");
    assert!(ctype.contains("application/x-ndjson"), "{ctype}");
    // 回给客户端的 `model` 是**请求里那个名字**（官方回显 `req.Model`；与另两张脸的
    // 「回显入站虚拟模型名」口径一致）。`:latest` 只在查表时被剥掉。
    assert_eq!(lines[0]["model"], "claude-haiku:latest");
    assert_eq!(lines.last().unwrap()["done"], true);
}

/// `stream:false`：单个 JSON 对象（`application/json`），同一批事件攒到终态再组装。
#[tokio::test(flavor = "multi_thread")]
async fn chat_non_streaming_returns_one_json_object() {
    let (base, _mock, _app) = harness("chat-nonstream").await;
    let (status, ctype, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"my-model","stream":false,"messages":[{"role":"user","content":"你好"}]}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(ctype.contains("application/json"), "{ctype}");
    assert_eq!(v["model"], "my-model");
    assert_eq!(v["done"], true);
    assert_eq!(v["message"]["role"], "assistant");
    assert_eq!(v["message"]["content"], "你好！");
    assert_eq!(v["done_reason"], "stop");
    assert_eq!(v["prompt_eval_count"], 11);
    assert_eq!(v["eval_count"], 2);
}

// ───────────────────────── ② 工具调用两处怪癖 ─────────────────────────

/// 怪癖②：`arguments` 是**对象**、`BlockStop` 才发整通；回传只有 `tool_name` 时
/// 必须按名反查补上 `tool_use_id`（空串会让 Anthropic 出站 400）。
#[tokio::test(flavor = "multi_thread")]
async fn tool_call_is_buffered_whole_and_tool_results_pair_by_name() {
    let (base, mock, _app) = harness("chat-tools").await;

    // ① 模型产出工具调用：一帧、`arguments` 是对象、`index` 恒发。
    mock.set("tools");
    let (status, _ctype, lines) = chat(
        &base,
        &json!({"model":"claude-haiku","messages":[{"role":"user","content":"天气"}],
                "tools":[{"type":"function","function":{"name":"get_weather","description":"d",
                          "parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}]}),
    )
    .await;
    assert_eq!(status, 200, "{lines:?}");
    assert_eq!(lines.len(), 2, "一帧工具调用 + 一帧终态：{lines:?}");
    let call = &lines[0]["message"]["tool_calls"][0];
    assert_eq!(lines[0]["done"], false);
    assert_eq!(call["id"], "toolu_1");
    assert_eq!(call["function"]["index"], 0);
    assert_eq!(call["function"]["name"], "get_weather");
    assert_eq!(call["function"]["arguments"], json!({"city":"上海"}));
    assert!(
        call["function"]["arguments"].is_object(),
        "arguments 必须是对象，不是字符串：{call}"
    );
    assert_eq!(lines[1]["done"], true);
    assert!(
        lines[1]["message"].get("tool_calls").is_none(),
        "别在终帧里重复给一通工具调用（客户端会累加）：{}",
        lines[1]
    );
    // 工具定义出站是 Anthropic 形状（`parameters` → `input_schema` 的改名映射）。
    let sent = mock.request(0);
    assert_eq!(sent["tools"][0]["name"], "get_weather");
    assert_eq!(sent["tools"][0]["input_schema"]["type"], "object");
    assert!(sent["tools"][0].get("parameters").is_none(), "{sent}");

    // ② 客户端只回 `tool_name`（Ollama 原生客户端就是这样）：id 必须补上。
    mock.set("text");
    let (status, _, _) = chat(
        &base,
        &json!({"model":"claude-haiku","stream":false,"messages":[
            {"role":"user","content":"天气"},
            {"role":"assistant","content":"","tool_calls":[
                {"id":"toolu_1","function":{"name":"get_weather","arguments":{"city":"上海"}}}]},
            {"role":"tool","content":"晴 26 度","tool_name":"get_weather"}
        ]}),
    )
    .await;
    assert_eq!(status, 200);
    let sent = mock.request(1);
    let result = tool_result(&sent);
    assert_eq!(result["tool_use_id"], "toolu_1", "{sent}");
    assert_eq!(result["content"], "晴 26 度");
    // 成对：assistant 轮的 `tool_use` 也在同一个出站体里。
    let uses: Vec<Value> = sent["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .filter(|b| b["type"] == "tool_use")
        .collect();
    assert_eq!(uses.len(), 1, "{sent}");
    assert_eq!(uses[0]["id"], "toolu_1");

    // ③ 客户端连 assistant 轮的 id 都没回放：我们合成的 id 也必须配上对（不许空串）。
    let (status, _, _) = chat(
        &base,
        &json!({"model":"claude-haiku","stream":false,"messages":[
            {"role":"assistant","content":"","tool_calls":[
                {"function":{"name":"get_weather","arguments":{"city":"上海"}}}]},
            {"role":"tool","content":"晴","tool_name":"get_weather"}
        ]}),
    )
    .await;
    assert_eq!(status, 200);
    let sent = mock.request(2);
    let result = tool_result(&sent);
    let id = result["tool_use_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!id.is_empty(), "补不出 id 也不能留空串：{sent}");
    let used = sent["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .find(|b| b["type"] == "tool_use")
        .map(|b| b["id"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    assert_eq!(id, used, "tool_result 与 tool_use 必须成对：{sent}");
}

// ───────────────────────── ③ 装载 / 卸载 ─────────────────────────

/// 空 `messages` = 装载 / 卸载：本地假终态、**不打上游**；`keep_alive:0` 才是卸载。
#[tokio::test(flavor = "multi_thread")]
async fn empty_messages_load_and_unload_without_touching_the_upstream() {
    let (base, mock, _app) = harness("chat-load").await;

    let (status, ctype, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"claude-haiku","messages":[]}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(
        ctype.contains("application/json"),
        "官方这里也是单对象：{ctype}"
    );
    assert_eq!(v["model"], "claude-haiku");
    assert_eq!(v["message"]["content"], "");
    assert_eq!(v["done"], true);
    assert_eq!(v["done_reason"], "load");
    assert!(v["prompt_eval_count"].is_null(), "统计字段一律不填：{v}");

    let (status, _, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"claude-haiku","messages":[],"keep_alive":0}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["done_reason"], "unload");
    // 字符串形状的 keep_alive 也认。
    let (status, _, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"claude-haiku","messages":[],"keep_alive":"0s"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v["done_reason"], "unload");
    // `-1`（永不卸载）不是卸载。
    let (status, _, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"claude-haiku","messages":[],"keep_alive":-1}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(v["done_reason"], "load");

    assert_eq!(mock.len(), 0, "装载 / 卸载绝不打上游");
}

/// `messages` 非空时 `keep_alive` 一律忽略（网关不持有模型，它只影响驻留）。
#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_is_ignored_when_messages_are_present() {
    let (base, mock, _app) = harness("chat-keepalive").await;
    let (status, _, lines) = chat(
        &base,
        &json!({"model":"claude-haiku","keep_alive":0,"messages":[{"role":"user","content":"你好"}]}),
    )
    .await;
    assert_eq!(status, 200, "{lines:?}");
    assert_eq!(lines.last().unwrap()["done"], true);
    assert_eq!(mock.len(), 1, "有 messages 就该真打上游");
}

// ───────────────────────── ④ 辅助端点 ─────────────────────────

/// `/api/tags` 与 `/v1/models` 同一集合；`digest` 是名字的确定性 sha256（裸 hex）；
/// 不谎报 `format:"gguf"`；`size` 是唯一的诚实值 0。
#[tokio::test(flavor = "multi_thread")]
async fn tags_mirrors_v1_models_with_honest_fields() {
    let (base, _mock, _app) = harness("tags").await;
    let (status, tags) = get(&base, "/api/tags").await;
    assert_eq!(status, 200, "{tags}");
    let models = tags["models"].as_array().cloned().unwrap_or_default();
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_string))
        .collect();
    // 四个家族槽位恒在且在最前，其余按名排序（与 `/v1/models` 同一份来源）。
    assert_eq!(
        names,
        vec![
            "claude-fable",
            "claude-opus",
            "claude-sonnet",
            "claude-haiku",
            "my-model"
        ]
    );
    let (_, v1) = get(&base, "/v1/models").await;
    let v1_names: Vec<String> = v1["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, v1_names, "两张脸必须是同一集合");

    for m in &models {
        assert_eq!(m["name"], m["model"], "name / model 填同一个裸名：{m}");
        assert!(!m["name"].as_str().unwrap_or("").contains(':'), "{m}");
        assert_eq!(m["size"], 0, "没有权重文件：{m}");
        let digest = m["digest"].as_str().unwrap_or_default();
        assert_eq!(digest.len(), 64, "digest 是裸 hex sha256：{m}");
        assert!(!digest.starts_with("sha256:"), "不带前缀：{m}");
        assert_eq!(m["details"]["format"], "", "不许谎报 gguf：{m}");
        assert_eq!(m["details"]["parent_model"], "");
        assert_eq!(m["details"]["parameter_size"], "");
        assert_eq!(m["details"]["quantization_level"], "");
        assert_eq!(m["capabilities"], json!(["completion", "tools"]));
        assert!(
            m["modified_at"].as_str().map(|s| s.len() >= 20) == Some(true),
            "{m}"
        );
    }
    // digest 的期望值来自 `sha256sum`（独立于被测代码）。
    let by_name = |n: &str| {
        models
            .iter()
            .find(|m| m["name"] == n)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        by_name("my-model")["digest"],
        "98ad223a32fd2352c0463042ae03ae1fbfe11c67ce2ef5dc62b8a7b6ca9ead4e"
    );
    assert_eq!(
        by_name("claude-haiku")["digest"],
        "6f28ac55ee2ffcc0e5d1a48d893b7752d683ea90d31caac598abdf09bf924c54"
    );
    // 家族槽位填槽位名；普通虚拟模型空串 / 空数组。
    assert_eq!(by_name("claude-haiku")["details"]["family"], "claude-haiku");
    assert_eq!(
        by_name("claude-haiku")["details"]["families"],
        json!(["claude-haiku"])
    );
    assert_eq!(by_name("my-model")["details"]["family"], "");
    assert_eq!(by_name("my-model")["details"]["families"], json!([]));
    // 未绑定的家族槽位也照列（名字照样暴露，请求期才 404）。
    assert_eq!(by_name("claude-opus")["details"]["family"], "claude-opus");
}

/// `/api/version`（GET + 未文档化的 HEAD）与 `/api/ps`（空数组、不是 `null`；官方没有 HEAD）。
#[tokio::test(flavor = "multi_thread")]
async fn version_and_ps_are_shape_exact() {
    let (base, _mock, _app) = harness("version-ps").await;
    assert_eq!(head_status(&base, "/api/version").await, 200, "官方有 HEAD");
    let (status, v) = get(&base, "/api/version").await;
    assert_eq!(status, 200);
    assert_eq!(v, json!({"version": "0.13.3"}), "唯一键 version");

    let (status, v) = get(&base, "/api/ps").await;
    assert_eq!(status, 200);
    assert_eq!(v, json!({"models": []}), "空数组、不是 null");
    assert_eq!(head_status(&base, "/api/ps").await, 405, "官方只有 GET");
    // HEAD /api/tags 官方也有（axum 的 get() 自带）。
    assert_eq!(head_status(&base, "/api/tags").await, 200);
}

/// `/api/show`：必给 `details` 六子键 + `model_info:{}` + `modified_at`，其余整键省略；
/// 请求体用 `model`（`name` 只是废弃兜底）；`verbose` / `runner` / `all_manifests` 接受并忽略。
#[tokio::test(flavor = "multi_thread")]
async fn show_fills_only_the_honest_keys() {
    let (base, _mock, _app) = harness("show").await;
    let (status, _, v) = post_json(&base, "/api/show", &json!({"model":"claude-haiku"})).await;
    assert_eq!(status, 200, "{v}");
    let details = v["details"].as_object().cloned().unwrap_or_default();
    let mut keys: Vec<&str> = details.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "families",
            "family",
            "format",
            "parameter_size",
            "parent_model",
            "quantization_level"
        ],
        "六个子键恒在"
    );
    assert_eq!(v["details"]["family"], "claude-haiku");
    assert_eq!(v["details"]["families"], json!(["claude-haiku"]));
    assert_eq!(v["details"]["format"], "");
    assert_eq!(v["model_info"], json!({}), "必须给空对象，别给 null");
    assert!(v["modified_at"].is_string(), "{v}");
    // 整键省略名单（一编就是伪造）。
    for absent in [
        "modelfile",
        "parameters",
        "template",
        "license",
        "system",
        "renderer",
        "parser",
        "thinking",
        "tensors",
        "projector_info",
        "requires",
        "manifests",
        "capabilities",
        "messages",
    ] {
        assert!(v.get(absent).is_none(), "{absent} 该整键省略：{v}");
    }

    // 普通虚拟模型：family 空串 / families 空数组。
    let (_, _, v) = post_json(&base, "/api/show", &json!({"model":"my-model"})).await;
    assert_eq!(v["details"]["family"], "");
    assert_eq!(v["details"]["families"], json!([]));

    // `name` 是已废弃的兜底（两者都给时 `model` 胜出）。
    let (status, _, v) = post_json(&base, "/api/show", &json!({"name":"claude-haiku"})).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["details"]["family"], "claude-haiku");
    let (status, _, v) = post_json(
        &base,
        "/api/show",
        &json!({"name":"claude-haiku","model":"my-model","verbose":true,"runner":"ggml","all_manifests":true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "verbose / runner / all_manifests 接受并忽略：{v}"
    );
    assert_eq!(v["details"]["family"], "", "model 胜出 → my-model");
    assert!(
        v.get("manifests").is_none(),
        "绝不返回另一套 ShowManifestsResponse"
    );
}

// ───────────────────────── ⑤ 错误信封 ─────────────────────────

/// 三条 404 文案 + 两条 400 文案（逐字），错误体是**单层** `{"error":"…"}`；
/// 上游断流时追加一行 `{"error":…}` 且状态码不变。
#[tokio::test(flavor = "multi_thread")]
async fn error_envelopes_are_ollama_shaped() {
    let (base, mock, _app) = harness("errors").await;

    for (path, body) in [
        (
            "/api/chat",
            json!({"model":"nope","messages":[{"role":"user","content":"."}]}),
        ),
        ("/api/chat", json!({"model":"nope:latest","messages":[]})),
        ("/api/show", json!({"model":"no-such-model:latest"})),
    ] {
        let (status, _ctype, v) = post_json(&base, path, &body).await;
        assert_eq!(status, 404, "{path} {body} → {v}");
        let want = format!("model '{}' not found", body["model"].as_str().unwrap());
        assert_eq!(v, json!({"error": want}), "逐字复用官方文案");
        assert!(v.get("type").is_none(), "不是 OpenAI 的嵌套形状");
    }

    // 空 body / 非法 JSON / 缺 model（chat 与 show 同一套文案）。
    for path in ["/api/chat", "/api/show"] {
        let (status, v) = post_raw(&base, path, "").await;
        assert_eq!(status, 400, "{path} 空 body → {v}");
        assert_eq!(v, json!({"error": "missing request body"}));
        let (status, v) = post_raw(&base, path, "{}").await;
        assert_eq!(status, 400, "{path} 缺 model → {v}");
        assert_eq!(v, json!({"error": "model is required"}));
        let (status, v) = post_raw(&base, path, "{").await;
        assert_eq!(status, 400, "{path} 非法 JSON → {v}");
        assert!(v["error"].is_string(), "{v}");
    }

    // 上游 500：状态码透传 + 单层信封。
    mock.set("500");
    let (status, _, v) = post_json(
        &base,
        "/api/chat",
        &json!({"model":"claude-haiku","stream":false,"messages":[{"role":"user","content":"."}]}),
    )
    .await;
    assert_eq!(status, 500, "{v}");
    assert!(v["error"].as_str().unwrap_or("").contains("500"), "{v}");

    // 上游断流（无 message_stop）：流已经开着，末行追加错误、状态码不变。
    mock.set("abort");
    let (status, _ctype, lines) = chat(
        &base,
        &json!({"model":"claude-haiku","messages":[{"role":"user","content":"."}]}),
    )
    .await;
    assert_eq!(status, 200, "帧已经写出去了，状态码改不了：{lines:?}");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["message"]["content"], "半");
    assert!(
        lines[1]["error"].as_str().unwrap_or("").contains("断流"),
        "{lines:?}"
    );
    assert!(lines[1].get("done").is_none(), "断流不发终帧");
}

/// 其余 `/api/*` 一律 404（Ollama 面**没有**这些端点），但**管理面的路由优先于兜底**。
#[tokio::test(flavor = "multi_thread")]
async fn unknown_api_paths_are_404_and_the_management_plane_survives() {
    let (base, _mock, app) = harness("fallback").await;

    for (path, body) in [
        ("/api/pull", json!({"model":"x"})),
        ("/api/create", json!({"model":"x"})),
        ("/api/copy", json!({"source":"a","destination":"b"})),
        ("/api/delete", json!({"model":"x"})),
        ("/api/blobs/sha256:abc", json!({})),
        ("/api/generate", json!({"model":"x","prompt":"hi"})),
        ("/api/embed", json!({"model":"x","input":"hi"})),
        ("/api/embeddings", json!({"model":"x","prompt":"hi"})),
        ("/api/whatever", json!({})),
    ] {
        let (status, _ctype, v) = post_json(&base, path, &body).await;
        assert_eq!(status, 404, "{path} → {v}");
        assert!(v["error"].is_string(), "{path} 要 Ollama 信封：{v}");
    }
    // 兜底不吃别的脸。
    assert_eq!(get(&base, "/v1/models").await.0, 200);
    let (status, _, v) = post_json(&base, "/v1/messages", &json!({"model":"nope"})).await;
    assert_eq!(status, 404, "{v}");
    assert_eq!(
        v["error"]["type"], "not_found_error",
        "`/v1/*` 仍是 OpenAI 信封：{v}"
    );

    // 管理面与 Ollama 面同挂一份 router（`ingress::router`）：`/api/status` 是具体路由，
    // 兜底只吃没被匹配的 `/api/*`。
    let full = gateway_core::ingress::router(ApiState::new("0.1.0", ":memory:", 0), Some(app));
    let mgmt = spawn(full).await;
    let (status, v) = get(&mgmt, "/api/status").await;
    assert_eq!(status, 200, "管理面被兜底吃了：{v}");
    assert_eq!(v["running"], true);
    assert_eq!(get(&mgmt, "/v1/models").await.0, 200);
    let (status, _, v) = post_json(&mgmt, "/api/pull", &json!({"model":"x"})).await;
    assert_eq!(status, 404, "{v}");
    assert!(v["error"].is_string(), "{v}");
    assert_eq!(binding::FAMILY_SLOTS.len(), 4, "夹具前提：四个家族槽位");
}
