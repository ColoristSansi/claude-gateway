//! 票 64 验收：OpenAI Chat 入站 `POST /v1/chat/completions`（spec §4.3）——
//! 流式 / 非流式、工具往返、DeepSeek 的 usage 怪癖，外加三个入站家族的**错误信封**
//! 与 `/v1/*` 未匹配路径。
//!
//! 全部走**真实 HTTP**：转发面路由（`ingress::proxy::router()`）挂在本进程里，
//! 上游是本进程里的假 OpenAI Chat 面。期望值来自独立来源：spec §4.3、OpenAI Chat 公开形状、
//! 以及 `tests/fixtures/` 里手写的黄金文件（不拿被测代码反算）。

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

// ───────────────────────── 假上游的脚本 ─────────────────────────

/// 真 fixture（票 30 录的回放字节）：纯文本 + **单独一帧** usage（OpenAI 官方形状）。
const TEXT_SSE: &str = include_str!("fixtures/text/openai.sse");
/// 真 fixture：文本 + 工具调用（arguments 分两片）+ 单独一帧 usage。
const TOOLS_SSE: &str = include_str!("fixtures/tools/openai.sse");
/// 真 fixture：200 开着、走到一半就没了（没有 `finish_reason`、没有 `[DONE]`）。
const ABORT_SSE: &str = include_str!("fixtures/abort/openai.sse");

/// DeepSeek 的怪癖（spec §4.3）：usage **附在最后一个内容块**上，不单独发 usage chunk。
/// 手写而不是录的：本机没有 DeepSeek 的 Chat 录制，spec 那句话就是判据。
const DEEPSEEK_USAGE_SSE: &str = concat!(
    "data: {\"id\":\"chatcmpl-ds\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-ds\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"缓存\"},\"finish_reason\":null}]}\n\n",
    // 最后一个内容块与 usage 同一帧。
    "data: {\"id\":\"chatcmpl-ds\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"命中。\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":12,\"total_tokens\":112,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
    "data: {\"id\":\"chatcmpl-ds\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"deepseek\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

// ───────────────────────── 夹具 ─────────────────────────

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg64-chat-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::OpenAiChat, "/openai/v1/chat/completions");
    up
}

/// 两条绑定：一个走 Chat wire 的普通虚拟模型（上游是假 Chat 面），
/// 另一条走 Anthropic wire —— 错误信封与 `stream:false` 之外只用来证明入站面与出站 wire 解耦。
fn table() -> BindingTable {
    BindingTable::new(
        vec![
            Binding::new(
                Profile::Global,
                "chat-model",
                "mock",
                "mock-text",
                Wire::OpenAiChat,
            ),
            Binding::new(
                Profile::Global,
                "chat-to-anthropic",
                "mock",
                "mock-text",
                Wire::Anthropic,
            ),
        ],
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
        .route("/openai/v1/chat/completions", post(chat))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), mock)
}

async fn chat(State(m): State<Mock>, Json(v): Json<Value>) -> Response {
    m.seen.lock().unwrap().push(v);
    let script = m.script.lock().unwrap().clone();
    match script.as_str() {
        "500" => (StatusCode::INTERNAL_SERVER_ERROR, "上游炸了").into_response(),
        "tools" => sse(TOOLS_SSE),
        "abort" => sse(ABORT_SSE),
        "deepseek-usage" => sse(DEEPSEEK_USAGE_SSE),
        _ => sse(TEXT_SSE),
    }
}

fn sse(body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// 起一份网关 + 假上游，返回 (网关 base, mock)。
async fn harness(tag: &str) -> (String, Mock) {
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

/// POST `/v1/chat/completions`，返回 (状态码, content-type, 原始文本)。
async fn post_chat(base: &str, body: &Value) -> (u16, String, String) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .json(body)
        .send()
        .await
        .expect("POST /v1/chat/completions 失败");
    read(resp).await
}

/// 发原始 body（非法 JSON / 缺 content-type 用它），`ctype` = `None` 时**不带** content-type。
async fn post_raw(
    base: &str,
    path: &str,
    body: &'static str,
    ctype: Option<&'static str>,
) -> (u16, String, String) {
    post_raw_h(base, path, body, ctype, &[]).await
}

/// 同上，另带入站头（`/v1/*` 兜底按客户端家族给信封时用它）。
async fn post_raw_h(
    base: &str,
    path: &str,
    body: &'static str,
    ctype: Option<&'static str>,
    headers: &[(&'static str, &'static str)],
) -> (u16, String, String) {
    let mut rb = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .body(body);
    if let Some(c) = ctype {
        rb = rb.header(header::CONTENT_TYPE, c);
    }
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    read(rb.send().await.expect("POST 失败")).await
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

/// 黄金文件里的字节（期望值的手写来源）。
fn golden_bytes(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("chat-inbound")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("读不到黄金文件 {}: {e}", p.display()))
}

/// 与黄金文件逐帧逐字段比（`id` / `created` 是时间与序号，掩码掉）。
fn assert_golden(name: &str, got: &str) -> Vec<golden::Frame> {
    let frames = golden::parse(got.as_bytes());
    let want = golden::parse(&golden_bytes(name));
    let diffs = golden::compare(&frames, &want, &masks());
    assert!(
        diffs.is_empty(),
        "与黄金 {name} 不一致：{diffs:#?}\n实际：{got}"
    );
    frames
}

/// 黄金比对里要掩掉的字段：`id` 是编码器自造的序号，`created` 是时间戳。
fn masks() -> Vec<String> {
    vec!["id".to_string(), "created".to_string()]
}

// ───────────────────────── ① 流式：`data:` 帧 + `[DONE]` ─────────────────────────

/// spec §4.3：标准实现是 `data:` 帧 + `[DONE]`；帧形状（`object: chat.completion.chunk`、
/// `choices[].delta`、`finish_reason`）与 OpenAI Chat 一致。
#[tokio::test(flavor = "multi_thread")]
async fn chat_streams_data_frames_and_ends_with_done() {
    let (base, _mock) = harness("stream-text").await;
    let (status, ctype, text) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": true,
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    assert!(ctype.contains("text/event-stream"), "流式要 SSE：{ctype}");
    let frames = assert_golden("text.stream.sse", &text);

    // 传输层独立于黄金：最后一行必须是 `data: [DONE]`，且没有 Anthropic 的 `event:` 行。
    assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
    assert!(!text.contains("event:"), "Chat 流没有事件名：{text}");
    // 首帧就声明 role（客户端靠它建 assistant 消息）。
    assert_eq!(frames[0].data["choices"][0]["delta"]["role"], "assistant");
}

// ───────────────────────── ② 非流式：缓冲到终态再组装 ─────────────────────────

/// spec §4.3 / §4.1 同范式：非流式只是把同一条流缓冲到终态再组装成 `chat.completion`；
/// **出站仍然是 `stream:true`**（编解码面只有一套，迟到的 usage 也正好在收敛后拿全）。
#[tokio::test(flavor = "multi_thread")]
async fn chat_non_streaming_buffers_to_one_chat_completion() {
    let (base, mock) = harness("nonstream-text").await;
    let (status, ctype, text) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": false,
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    assert!(ctype.contains("application/json"), "非流式回 JSON：{ctype}");
    let got: Value = serde_json::from_str(&text).expect("非流式体是 JSON");
    let want: Value = serde_json::from_slice(&golden_bytes("text.json")).unwrap();
    assert_eq!(
        golden::masked(&got, &masks()),
        golden::masked(&want, &masks()),
        "与黄金 text.json 不一致：{text}"
    );

    // 出站永远是流式（票 26 决定 3），且要上游带上 usage（缺 usage 是常见坑）。
    let sent = mock.request(0);
    assert_eq!(sent["stream"], true, "出站仍要流式：{sent}");
    assert_eq!(sent["stream_options"]["include_usage"], true, "{sent}");
    assert_eq!(sent["messages"][0]["content"], "你好", "{sent}");
}

// ───────────────────────── ③ 工具往返 ─────────────────────────

/// 工具调用两个方向都要成对：入站把 `assistant.tool_calls` + `role:"tool"` 解成 IR 的
/// `ToolUse` / `ToolResult`（`tool_call_id` 就是配对键），回程把上游的 tool_calls 增量
/// 编回 `delta.tool_calls`（`index` + 分片 `arguments`，spec §4.3）。
#[tokio::test(flavor = "multi_thread")]
async fn chat_round_trips_tool_calls_and_tool_results() {
    let (base, mock) = harness("tools-roundtrip").await;
    mock.set("tools");
    let (status, _ctype, text) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": true,
            "messages": [
                {"role":"user","content":"上海天气"},
                {"role":"assistant","content":null,"tool_calls":[{
                    "id":"call_prev",
                    "type":"function",
                    "function":{"name":"get_weather","arguments":"{\"city\":\"上海\"}"}
                }]},
                {"role":"tool","tool_call_id":"call_prev","content":"晴 26 度"}
            ],
            "tools":[{"type":"function","function":{
                "name":"get_weather",
                "description":"查天气",
                "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}
            }}],
            "tool_choice":"auto",
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    let frames = assert_golden("tools.stream.sse", &text);
    // 工具入参分片必须原样转发（客户端自己拼）。
    let args: String = frames
        .iter()
        .filter_map(|f| {
            f.data["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str()
        })
        .collect();
    assert_eq!(args, "{\"city\":\"上海\"}", "{text}");

    // 出站：assistant.tool_calls 与 role:"tool" 成对（id 一致），工具定义改名成 `input_schema`。
    let sent = mock.request(0);
    assert_eq!(sent["messages"][1]["role"], "assistant", "{sent}");
    assert_eq!(
        sent["messages"][1]["tool_calls"][0]["id"], "call_prev",
        "{sent}"
    );
    assert_eq!(
        sent["messages"][1]["tool_calls"][0]["function"]["arguments"], "{\"city\":\"上海\"}",
        "{sent}"
    );
    assert_eq!(sent["messages"][2]["role"], "tool", "{sent}");
    assert_eq!(
        sent["messages"][2]["tool_call_id"], "call_prev",
        "配对键必须带过去：{sent}"
    );
    assert_eq!(sent["messages"][2]["content"], "晴 26 度", "{sent}");
    assert_eq!(
        sent["tools"][0]["function"]["name"], "get_weather",
        "{sent}"
    );
    assert_eq!(
        sent["tools"][0]["function"]["parameters"]["properties"]["city"]["type"], "string",
        "input_schema 要编回 parameters：{sent}"
    );
    assert_eq!(sent["tool_choice"], "auto", "{sent}");
}

// ───────────────────────── ④ DeepSeek 的 usage 怪癖 ─────────────────────────

/// spec §4.3：DeepSeek 把 usage 附在**最后一个内容块**上、不单独发 usage chunk。
/// 上游这么发时我们也要这么回：usage 只出现一次、就挂在最后一个内容块那一帧上，
/// 且**没有**「`choices: []` + usage」那种单独的 usage chunk（OpenAI 官方那种形状）。
#[tokio::test(flavor = "multi_thread")]
async fn deepseek_usage_rides_the_last_content_chunk() {
    let (base, mock) = harness("deepseek-usage").await;
    mock.set("deepseek-usage");
    let (status, _ctype, text) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": true,
            "messages": [{"role":"user","content":"缓存还在吗"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    let frames = assert_golden("usage-quirk.stream.sse", &text);

    let with_usage: Vec<&golden::Frame> = frames
        .iter()
        .filter(|f| f.data.get("usage").is_some())
        .collect();
    assert_eq!(with_usage.len(), 1, "usage 只该出现一次：{text}");
    assert_eq!(
        with_usage[0].data["choices"][0]["delta"]["content"], "命中。",
        "usage 要跟着最后一个内容块走：{text}"
    );
    assert_eq!(with_usage[0].data["usage"]["prompt_tokens"], 100, "{text}");
    assert_eq!(
        with_usage[0].data["usage"]["completion_tokens"], 12,
        "{text}"
    );
    assert_eq!(with_usage[0].data["usage"]["total_tokens"], 112, "{text}");
    assert_eq!(
        with_usage[0].data["usage"]["prompt_tokens_details"]["cached_tokens"], 80,
        "缓存读走 prompt_tokens_details.cached_tokens：{text}"
    );
    assert!(
        !frames.iter().any(|f| f
            .data
            .get("choices")
            .and_then(Value::as_array)
            .map(|c| c.is_empty())
            .unwrap_or(false)
            && f.data.get("usage").is_some()),
        "不许有单独的 usage chunk：{text}"
    );
    // 非流式也必须拿到同一份 usage（同一批帧缓冲到终态）。
    let (status, _ctype, body) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": false,
            "messages": [{"role":"user","content":"缓存还在吗"}],
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["usage"]["prompt_tokens"], 100, "{body}");
    assert_eq!(
        v["usage"]["prompt_tokens_details"]["cached_tokens"], 80,
        "{body}"
    );
}

// ───────────────────────── ⑤ 错误信封：三家入站各一条 ─────────────────────────

/// 入站解析失败也要给信封（spec §4.2 / §4.3 / §4.4；评审「中」第一条）：
/// **语法错的 JSON** 与**缺 `content-type`** 这两类输入以前会落到 axum 默认的
/// text/plain 400/415/422，客户端读不懂。这里逐家族钉死形状与 content-type。
#[tokio::test(flavor = "multi_thread")]
async fn parse_failures_get_family_shaped_envelopes() {
    let (base, _mock) = harness("envelopes").await;

    // ① Anthropic：`{"type":"error","error":{…}}`。
    let (status, ctype, text) =
        post_raw(&base, "/v1/messages", "{", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    assert!(
        ctype.contains("application/json"),
        "不许退成 axum 的 text/plain：{ctype}"
    );
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert_eq!(v["type"], "error", "{text}");
    assert_eq!(v["error"]["type"], "invalid_request_error", "{text}");
    assert!(v["error"]["message"].is_string(), "{text}");
    // 空 body 同一条路。
    let (status, _, text) = post_raw(&base, "/v1/messages", "", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["type"],
        "error",
        "{text}"
    );

    // ①' 缺 content-type：**不再是** axum 的 415 text/plain，而是照常处理（Ollama 面的做法）。
    let (status, ctype, text) = post_raw(
        &base,
        "/v1/messages",
        r#"{"model":"chat-model","messages":[{"role":"user","content":"."}]}"#,
        None,
    )
    .await;
    assert_ne!(status, 415, "缺 content-type 不该是 415：{text}");
    assert_eq!(status, 200, "缺 content-type 也要能通：{text}");
    assert!(
        ctype.contains("application/json"),
        "没有 stream:true 时走非流式：{ctype} {text}"
    );

    // ② OpenAI Chat：`{"error":{"message","type","code"}}`（嵌套，不是 Ollama 那种单层）。
    let (status, ctype, text) =
        post_raw(&base, "/v1/chat/completions", "{", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    assert!(ctype.contains("application/json"), "{ctype}");
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert!(v["error"]["message"].is_string(), "{text}");
    assert_eq!(v["error"]["type"], "invalid_request_error", "{text}");
    assert_eq!(v["error"]["code"], Value::Null, "{text}");
    assert!(v.get("type").is_none(), "不是 Anthropic 的形状：{text}");
    // 缺 content-type + 未绑定的模型：也要 OpenAI 信封（不是空体、不是 text/plain）。
    let (status, ctype, text) = post_raw(
        &base,
        "/v1/chat/completions",
        r#"{"model":"nope","messages":[{"role":"user","content":"."}]}"#,
        None,
    )
    .await;
    assert_eq!(status, 404, "{text}");
    assert!(ctype.contains("application/json"), "{ctype}");
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["error"]["type"],
        "not_found_error",
        "{text}"
    );

    // ②' Responses 与 Chat 共用同一份 OpenAI 信封（spec §4.2）。
    let (status, _, text) = post_raw(&base, "/v1/responses", "{", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["error"]["message"].is_string(), "{text}");
    assert!(v.get("type").is_none(), "{text}");

    // ②'' 不带 `/v1` 的别名（票 15 的 base URL 两份资料互相矛盾）也走同一个 handler。
    let (status, _, text) =
        post_raw(&base, "/chat/completions", "{", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["error"]["message"].is_string(), "{text}");
    assert!(v.get("type").is_none(), "{text}");

    // ③ Ollama：单层 `{"error":"…"}`（spec §4.4）。
    let (status, ctype, text) = post_raw(&base, "/api/chat", "{", Some("application/json")).await;
    assert_eq!(status, 400, "{text}");
    assert!(ctype.contains("application/json"), "{ctype}");
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert!(v["error"].is_string(), "Ollama 信封是单层的：{text}");
    assert!(v.get("type").is_none(), "{text}");
    // 缺 content-type 同样能吃。
    let (status, _, text) = post_raw(
        &base,
        "/api/chat",
        r#"{"model":"nope","messages":[{"role":"user","content":"."}]}"#,
        None,
    )
    .await;
    assert_eq!(status, 404, "{text}");
    assert!(
        serde_json::from_str::<Value>(&text).unwrap()["error"].is_string(),
        "{text}"
    );
}

// ───────────────────────── ⑥ `/v1/*` 未匹配路径 ─────────────────────────

/// 评审「中」第三条：`/v1/*` 的未知路径以前回**空体 404**（票 56 有意留的），
/// 客户端解析错误时拿不到任何结构化体。现在按入站家族给信封；空体只留给真正无协议的路径。
#[tokio::test(flavor = "multi_thread")]
async fn unknown_v1_paths_get_family_shaped_envelopes() {
    let (base, _mock) = harness("v1-fallback").await;

    // 路径自带家族：`/v1/messages/*` 是 Anthropic 面。
    let (status, ctype, text) = post_raw(&base, "/v1/messages/nope", "", None).await;
    assert_eq!(status, 404, "{text}");
    assert!(ctype.contains("application/json"), "{ctype}");
    let v: Value = serde_json::from_str(&text).expect("信封是 JSON");
    assert_eq!(v["type"], "error", "{text}");
    assert_eq!(v["error"]["type"], "not_found_error", "{text}");

    // `/v1/chat/completions/extra` 是 Chat 面 → OpenAI 信封。
    let (status, _, text) = post_raw(&base, "/v1/chat/completions/extra", "", None).await;
    assert_eq!(status, 404, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["error"]["message"].is_string(), "{text}");
    assert!(v.get("type").is_none(), "不是 Anthropic 的形状：{text}");

    // 没有路径线索时按客户端标识头判：Claude 系默认 Anthropic 信封。
    let (status, _, text) = post_raw(&base, "/v1/whatever", "", None).await;
    assert_eq!(status, 404, "{text}");
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["type"],
        "error",
        "{text}"
    );

    // Codex 的 UA 拿 OpenAI 信封（同一个路径）。
    let (status, _, text) = post_raw_h(
        &base,
        "/v1/whatever",
        "",
        None,
        &[("user-agent", "codex_exec/0.162.1")],
    )
    .await;
    assert_eq!(status, 404, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["error"]["message"].is_string(), "{text}");
    assert!(v.get("type").is_none(), "Codex 要 OpenAI 信封：{text}");

    // 真正无协议的路径仍然是空体 404（别把兜底扩大到整张路由表）。
    let (status, _, text) = post_raw(&base, "/definitely-not-a-protocol", "", None).await;
    assert_eq!(status, 404, "{text}");
    assert!(text.is_empty(), "无协议路径保持空体：{text}");
}

// ───────────────────────── ⑦ 流中出错（上游断流）─────────────────────────

/// 上游断在终态之前：状态码已经发出去了（200），只能追加一帧 OpenAI 错误体，
/// 且**不发 `[DONE]`**（客户端靠「有没有 `[DONE]`」判流是否正常收尾）。
#[tokio::test(flavor = "multi_thread")]
async fn stream_abort_appends_an_error_frame_and_skips_done() {
    let (base, mock) = harness("abort").await;
    mock.set("abort");
    let (status, _ctype, text) = post_chat(
        &base,
        &json!({
            "model": "chat-model",
            "stream": true,
            "messages": [{"role":"user","content":"你好"}],
        }),
    )
    .await;

    assert_eq!(status, 200, "{text}");
    let frames = golden::parse(text.as_bytes());
    let last = frames.last().expect("至少有一帧");
    assert!(
        last.data["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("终态"),
        "末帧要是错误体：{text}"
    );
    assert!(
        !frames
            .iter()
            .any(|f| f.data == Value::String("[DONE]".into())),
        "断流不发 [DONE] 哨兵帧：{text}"
    );
    // 断流前的正文仍然照发（已经流出去的内容不收回）。
    assert!(text.contains("说到一半就断了"), "{text}");
}
