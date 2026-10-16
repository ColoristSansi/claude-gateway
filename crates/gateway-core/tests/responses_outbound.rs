//! 票 65 验收：上游 OpenAI Responses 出站 wire（spec §1 目标 3 / §4.2 / §6）——
//! **上游 Responses 帧 → IR → 客户端面** 的流式、非流式、工具往返、`text.format` 三层，
//! 外加「上游非 2xx 时按**入站**协议给信封」。
//!
//! 全部走**真实 HTTP**：转发面路由（`ingress::proxy::router()`）挂在本进程里，
//! 上游是本进程里的假 Responses 面。期望值来自独立来源：spec §4.2 的字段口径、
//! OpenAI Responses 的公开帧词表、以及 `tests/fixtures/responses-outbound/` 里手写的黄金文件
//! （不拿被测代码反算）。

#[path = "common/golden.rs"]
mod golden;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::binding::{default_profiles, Binding, BindingTable, Profile};
use gateway_core::egress::provider::{self, AuthScheme, Upstream, Wire};
use gateway_core::ingress::proxy::Gateway;
use gateway_core::store::Store;
use serde_json::{json, Value};

/// 假上游吐的帧：纯文本（含 `output_item.done` 与终态 usage）。
const MOCK_TEXT_SSE: &str = include_str!("fixtures/responses-outbound/mock-text.sse");
/// 假上游吐的帧：`function_call` 项 + 分片的 `arguments`。
const MOCK_TOOLS_SSE: &str = include_str!("fixtures/responses-outbound/mock-tools.sse");

// ───────────────────────── 夹具 ─────────────────────────

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg65-responses-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// 上游只声明 Responses 面（与 `live_upstream` 同一套：baseUrl 已带 `/v1`，模板不带）。
fn upstream(id: &str, base: &str, with_path: bool) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    if with_path {
        up.set_path(Wire::OpenAiResponses, "/responses");
    } else {
        // `Upstream::new` 会给三条 wire 都铺默认模板；这里模拟「库里这一面没有模板」。
        up.path_templates.clear();
    }
    up
}

/// 一条绑定：虚拟模型 `claude-haiku` → 假上游的 `mock-text`，出站走 **openai-responses**。
fn table() -> BindingTable {
    BindingTable::new(
        vec![Binding::new(
            Profile::Global,
            "claude-haiku",
            "mock",
            "mock-text",
            Wire::OpenAiResponses,
        )],
        default_profiles(),
    )
}

#[derive(Clone, Default)]
struct Mock {
    seen: Arc<Mutex<Vec<Value>>>,
    script: Arc<Mutex<String>>,
}

impl Mock {
    fn set(&self, name: &str) {
        *self.script.lock().unwrap() = name.to_string();
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
        .route("/responses", post(responses))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), mock)
}

async fn responses(State(m): State<Mock>, Json(v): Json<Value>) -> Response {
    m.seen.lock().unwrap().push(v);
    let script = m.script.lock().unwrap().clone();
    match script.as_str() {
        "500" => (StatusCode::INTERNAL_SERVER_ERROR, "上游炸了").into_response(),
        "tools" => sse(MOCK_TOOLS_SSE),
        _ => sse(MOCK_TEXT_SSE),
    }
}

fn sse(body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// 起一份网关 + 假上游，返回 (网关 base, mock)。
async fn harness(tag: &str) -> (String, Mock) {
    harness_with_path(tag, true).await
}

async fn harness_with_path(tag: &str, with_path: bool) -> (String, Mock) {
    let store = Store::open(&tmp_root(tag)).expect("开库");
    let (mock_base, mock) = mock_upstream().await;
    provider::save_upstream(&store, &upstream("mock", &mock_base, with_path))
        .await
        .unwrap();
    store
        .put_credential("mock", "default", "mock-key")
        .await
        .unwrap();
    let mut upstreams = HashMap::new();
    upstreams.insert("mock".to_string(), upstream("mock", &mock_base, with_path));
    let app = Arc::new(Gateway::new(store, upstreams, table()));
    let base = spawn(gateway_core::ingress::proxy::router().with_state(app)).await;
    (base, mock)
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

/// POST `/v1/messages`（客户端面 = Anthropic），返回 (状态码, content-type, 原始文本)。
async fn post_messages(base: &str, body: &Value) -> (u16, String, String) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .json(body)
        .send()
        .await
        .expect("POST /v1/messages 失败");
    read(resp).await
}

/// POST `/v1/responses`（客户端面 = Responses，Codex 那条）。
async fn post_responses(base: &str, body: &Value) -> (u16, String, String) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .json(body)
        .send()
        .await
        .expect("POST /v1/responses 失败");
    read(resp).await
}

async fn read(resp: reqwest::Response) -> (u16, String, String) {
    let status = resp.status().as_u16();
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = resp.text().await.unwrap_or_default();
    (status, ctype, text)
}

fn golden_bytes(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("responses-outbound")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("读不到黄金文件 {}: {e}", p.display()))
}

/// 与黄金文件逐帧逐字段比（`message.id` 是**上游给的**响应 id，照实比，不掩码）。
fn assert_golden(name: &str, got: &str) -> Vec<golden::Frame> {
    let frames = golden::parse(got.as_bytes());
    let want = golden::parse(&golden_bytes(name));
    let diffs = golden::compare(&frames, &want, &[]);
    assert!(
        diffs.is_empty(),
        "与黄金 {name} 不一致：{diffs:#?}\n实际：{got}"
    );
    frames
}

// ───────────────────────── ① 流式：上游 Responses 帧 → 客户端 Anthropic SSE ─────────────────────────

/// 上游 `/responses` 的 `response.*` 帧 → IR → 客户端 Anthropic SSE：
/// `message_start` 打头、`content_block_delta` 逐片、`message_delta` 带上游 usage、
/// `message_stop` 收尾。出站请求体逐字（`instructions` / `input` / `max_output_tokens`）。
#[tokio::test(flavor = "multi_thread")]
async fn upstream_responses_streams_into_anthropic_frames() {
    let (base, mock) = harness("stream-text").await;
    let (status, ctype, text) = post_messages(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_tokens": 128,
            "stream": true,
            "system": "你是助手",
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    assert!(ctype.contains("text/event-stream"), "流式要 SSE：{ctype}");
    let frames = assert_golden("text.stream.sse", &text);
    assert_eq!(
        frames.last().unwrap().event.as_deref(),
        Some("message_stop"),
        "{text}"
    );

    // 出站请求体：系统提示走 `instructions`（**不是** Codex 入站那套 role:"developer"），
    // 走的是上游 `/responses` 那条路径。
    let sent = mock.request(0);
    assert_eq!(sent["model"], "mock-text", "{sent}");
    assert_eq!(sent["instructions"], "你是助手", "{sent}");
    assert_eq!(sent["input"][0]["role"], "user", "{sent}");
    assert_eq!(
        sent["input"][0]["content"][0]["type"], "input_text",
        "{sent}"
    );
    assert_eq!(sent["input"][0]["content"][0]["text"], "你好", "{sent}");
    assert_eq!(sent["max_output_tokens"], 128, "{sent}");
    assert_eq!(sent["stream"], true, "出站永远是流式：{sent}");
}

// ───────────────────────── ② 非流式：缓冲到终态再组装 ─────────────────────────

/// `stream:false` 只是把同一条流缓到终态再组装成一条完整 message（同一批 IR 事件）。
#[tokio::test(flavor = "multi_thread")]
async fn upstream_responses_non_streaming_buffers_to_one_message() {
    let (base, _mock) = harness("nonstream-text").await;
    let (status, ctype, text) = post_messages(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_tokens": 128,
            "stream": false,
            "system": "你是助手",
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    assert!(ctype.contains("application/json"), "非流式回 JSON：{ctype}");
    let got: Value = serde_json::from_str(&text).expect("非流式体是 JSON");
    let want: Value = serde_json::from_slice(&golden_bytes("text.json")).unwrap();
    assert_eq!(got, want, "与黄金 text.json 不一致：{text}");
}

// ───────────────────────── ③ 工具往返：function_call 成对 ─────────────────────────

/// 两个方向都成对：**出站**把 Anthropic 的 `tool_use` / `tool_result` 编成
/// `function_call` / `function_call_output`（配对键 `call_id` 一致）；**回程**把上游的
/// `function_call` 项编回客户端的 `tool_use`（arguments 分片原样转发）。
#[tokio::test(flavor = "multi_thread")]
async fn upstream_responses_round_trips_function_calls() {
    let (base, mock) = harness("tools-roundtrip").await;
    mock.set("tools");
    let (status, _ctype, text) = post_messages(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_tokens": 128,
            "stream": true,
            "messages": [
                {"role":"user","content":"上海天气"},
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"toolu_prev","name":"get_weather","input":{"city":"上海"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"toolu_prev","content":"晴 26 度"}
                ]}
            ],
            "tools":[{"name":"get_weather","description":"查天气",
                      "input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}],
            "tool_choice":"auto",
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    let frames = assert_golden("tools.stream.sse", &text);
    // arguments 分片要原样转发给客户端（客户端自己拼）。
    let args: String = frames
        .iter()
        .filter_map(|f| f.data["delta"]["partial_json"].as_str())
        .collect();
    assert_eq!(args, "{\"city\":\"上海\"}", "{text}");

    // 出站：`tool_use` → `function_call`、`tool_result` → `function_call_output`，`call_id` 成对。
    let sent = mock.request(0);
    assert_eq!(sent["input"][1]["type"], "function_call", "{sent}");
    assert_eq!(sent["input"][1]["call_id"], "toolu_prev", "{sent}");
    assert_eq!(sent["input"][1]["name"], "get_weather", "{sent}");
    assert_eq!(
        sent["input"][1]["arguments"], "{\"city\":\"上海\"}",
        "arguments 是字符串：{sent}"
    );
    assert_eq!(sent["input"][2]["type"], "function_call_output", "{sent}");
    assert_eq!(
        sent["input"][2]["call_id"], "toolu_prev",
        "配对键必须一一对上：{sent}"
    );
    assert_eq!(sent["input"][2]["output"], "晴 26 度", "{sent}");
    // 工具定义：IR 的 `input_schema` 编回 Responses 的 `parameters`。
    assert_eq!(sent["tools"][0]["type"], "function", "{sent}");
    assert_eq!(
        sent["tools"][0]["parameters"]["properties"]["city"]["type"], "string",
        "{sent}"
    );
    assert_eq!(sent["tool_choice"], "auto", "{sent}");
}

// ───────────────────────── ④ `text.format` 三层逐字 ─────────────────────────

/// 票 57 定的三层形状在**真出站请求体**里逐字：`text.format = {type, name, schema}`，
/// `name` 是常量 `"response"`；`strict` **只透传客户端显式值**（没给就整键不出现）。
#[tokio::test(flavor = "multi_thread")]
async fn outbound_text_format_is_three_layers_verbatim() {
    let (base, mock) = harness("text-format").await;
    let schema = json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"]});

    // ① Anthropic 入站（`output_config.format`，本来就没有 `strict`）→ 出站三层、无 `strict` 键。
    let (status, _, text) = post_messages(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_tokens": 64,
            "messages": [{"role":"user","content":"给我 JSON"}],
            "output_config": {"format":{"type":"json_schema","schema":schema}},
        }),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let sent = mock.request(0);
    assert_eq!(
        sent["text"],
        json!({"format":{"type":"json_schema","name":"response","schema":schema}}),
        "三层逐字（`name` 常量、没给的 `strict` 不许补）：{sent}"
    );

    // ② Responses 入站显式给了 `strict:true` → 原样透传。
    let (status, _, text) = post_responses(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_output_tokens": 64,
            "input": [{"role":"user","content":[{"type":"input_text","text":"给我 JSON"}]}],
            "text": {"format":{"type":"json_schema","name":"客户端起的名字","schema":schema,"strict":true}},
        }),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let sent = mock.request(1);
    assert_eq!(
        sent["text"],
        json!({"format":{"type":"json_schema","name":"response","schema":schema,"strict":true}}),
        "`strict` 显式值要透传、`name` 一律换成常量：{sent}"
    );
}

// ───────────────────────── ⑤ 错误信封：按入站协议给形状 ─────────────────────────

/// 上游非 2xx 时**按入站协议**给信封（spec §4.2）：Anthropic 面是
/// `{"type":"error","error":{…}}`，Responses 面才是 `{"error":{message,type,code}}`
/// ——**不是**把上游 Responses 的错误形状原样回给 Anthropic 客户端。
#[tokio::test(flavor = "multi_thread")]
async fn upstream_failure_keeps_the_inbound_envelope_shape() {
    let (base, mock) = harness("error-envelope").await;
    mock.set("500");

    let body = json!({
        "model": "claude-haiku",
        "max_tokens": 64,
        "messages": [{"role":"user","content":"你好"}],
    });
    let (status, ctype, text) = post_messages(&base, &body).await;
    assert_eq!(status, 500, "上游的状态码原样带给客户端：{text}");
    assert!(ctype.contains("application/json"), "{ctype}");
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert_eq!(v["type"], "error", "Anthropic 面的信封：{text}");
    assert_eq!(v["error"]["type"], "api_error", "{text}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("上游炸了"),
        "上游原话要带上：{text}"
    );
    assert!(
        v["error"].get("code").is_none(),
        "Anthropic 信封没有 code：{text}"
    );

    // 同一条上游、同一个模型，客户端的脸换一张，信封就跟着换。
    let (status, _, text) = post_responses(
        &base,
        &json!({
            "model": "claude-haiku",
            "stream": true,
            "input": [{"role":"user","content":[{"type":"input_text","text":"你好"}]}],
        }),
    )
    .await;
    assert_eq!(status, 500, "{text}");
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert!(v.get("type").is_none(), "不是 Anthropic 的形状：{text}");
    assert_eq!(v["error"]["type"], "api_error", "{text}");
    assert_eq!(v["error"]["code"], Value::Null, "{text}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("上游炸了"),
        "{text}"
    );
}

// ───────────────────────── ⑥ 上游没配这条路径模板 ─────────────────────────

/// `pathTemplates` 缺 `openai-responses` 一面（票 45–48 允许这么存）→ 早报 502 并说清是哪一面，
/// 不是静默当 Chat 打。
#[tokio::test(flavor = "multi_thread")]
async fn missing_path_template_is_reported_against_that_face() {
    let (base, _mock) = harness_with_path("no-path", false).await;
    let (status, _, text) = post_messages(
        &base,
        &json!({
            "model": "claude-haiku",
            "max_tokens": 64,
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;
    assert_eq!(status, 502, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("openai-responses"),
        "要说清是哪一面缺路径：{text}"
    );
}
