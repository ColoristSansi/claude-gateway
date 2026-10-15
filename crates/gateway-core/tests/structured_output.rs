//! 票 57 验收单测：IR 归一化槽位、四条入站解码、三条出站映射的逐字形状、
//! 请求期 400 的两条路径、能力位落库与自检探针。
//!
//! 测试面 = `gateway_core::ir::ResponseFormat`（归一化）+ `gateway_core::egress::structured`
//! （出站映射 / 早拒 / 探针）+ `egress::provider` 的能力位列；最后几条走**真实 HTTP**
//! （`ingress::proxy::router()` 在本进程里挂起来），验的是接进真实路由之后的入站状态码与信封。
//! 一律临时目录，不碰真实 `%APPDATA%`。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::binding::{default_profiles, Binding, BindingTable, Profile};
use gateway_core::egress::provider::{
    self, save_manual_model, save_upstream, AuthScheme, StructuredOutput, StructuredSource,
    Upstream, Wire,
};
use gateway_core::egress::structured::{self, HOSTILE_PROMPT, RESPONSE_NAME};
use gateway_core::ingress::proxy::Gateway;
use gateway_core::ir::ResponseFormat;
use gateway_core::store::Store;
use serde_json::{json, Map, Value};

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg57-structured-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

async fn open_store(tag: &str) -> Store {
    Store::open(&tmp_root(tag)).expect("开库")
}

fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/v1/messages");
    up.set_path(Wire::OpenAiChat, "/v1/chat/completions");
    up.set_path(Wire::OpenAiResponses, "/v1/responses");
    up
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("应当是 JSON 对象")
}

/// 一份两层嵌套的 schema：顶层 object + 一个嵌套 object + 一个数组（数组里的 object 也要补）。
fn nested_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "a": {"type": "string"},
            "nested": {"type": "object", "properties": {"b": {"type": "integer"}}},
            "list": {"type": "array", "items": {"type": "object", "properties": {"c": {"type": "string"}}}},
            "free": {"type": "object", "additionalProperties": true}
        },
        "required": ["a"]
    })
}

fn schema_format(strict: Option<bool>) -> ResponseFormat {
    ResponseFormat::JsonSchema {
        schema: nested_schema(),
        name: Some("入站给的名字".into()),
        strict,
    }
}

// ───────────────────────── ① 入站：四种形状解成 IR ─────────────────────────

#[test]
fn anthropic_inbound_reads_output_config_format_without_name_or_strict() {
    let req = obj(json!({
        "model": "claude-haiku",
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "."}],
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}}
    }));
    assert_eq!(
        ResponseFormat::from_anthropic_request(&req),
        Some(ResponseFormat::JsonSchema {
            schema: json!({"type": "object"}),
            name: None,
            strict: None,
        })
    );

    // 没传 / 只传了 output_config 的兄弟键 → 没要结构化输出（`undefined`）。
    assert_eq!(
        ResponseFormat::from_anthropic_request(&obj(json!({}))),
        None
    );
    assert_eq!(
        ResponseFormat::from_anthropic_request(&obj(json!({"output_config": {"effort": "high"}}))),
        None
    );
    // 认不出的形状原样兜底，带的是整份 `output_config`（这样「原样带」就是放回同一个字段）。
    assert_eq!(
        ResponseFormat::from_anthropic_request(&obj(
            json!({"output_config": {"format": {"type": "json_schema"}}})
        )),
        Some(ResponseFormat::Other(
            json!({"format": {"type": "json_schema"}})
        ))
    );
}

#[test]
fn chat_inbound_reads_response_format_four_layers() {
    let req = obj(json!({"response_format": {
        "type": "json_schema",
        "json_schema": {"name": "ticket", "schema": {"type": "object"}, "strict": false}
    }}));
    assert_eq!(
        ResponseFormat::from_chat_request(&req),
        Some(ResponseFormat::JsonSchema {
            schema: json!({"type": "object"}),
            name: Some("ticket".into()),
            // 显式 `false` 与「没给」必须分开（spec §5 硬规则 1）。
            strict: Some(false),
        })
    );
    // 没给 strict → `None`（吃上游缺省），不是 `Some(false)`。
    let no_strict = obj(json!({"response_format": {
        "type": "json_schema", "json_schema": {"name": "t", "schema": {"type": "object"}}
    }}));
    assert_eq!(
        ResponseFormat::from_chat_request(&no_strict),
        Some(ResponseFormat::JsonSchema {
            schema: json!({"type": "object"}),
            name: Some("t".into()),
            strict: None,
        })
    );
    assert_eq!(
        ResponseFormat::from_chat_request(&obj(
            json!({"response_format": {"type": "json_object"}})
        )),
        Some(ResponseFormat::Json)
    );
    // `{"type":"text"}` 是官方缺省值：与没传同一个下场。
    assert_eq!(
        ResponseFormat::from_chat_request(&obj(json!({"response_format": {"type": "text"}}))),
        None
    );
    assert_eq!(ResponseFormat::from_chat_request(&obj(json!({}))), None);
}

#[test]
fn responses_inbound_reads_text_format_three_layers() {
    let req = obj(json!({"text": {"format": {
        "type": "json_schema", "name": "ticket", "schema": {"type": "object"}, "strict": true
    }}}));
    assert_eq!(
        ResponseFormat::from_responses_request(&req),
        Some(ResponseFormat::JsonSchema {
            schema: json!({"type": "object"}),
            name: Some("ticket".into()),
            strict: Some(true),
        })
    );
    assert_eq!(
        ResponseFormat::from_responses_request(&obj(
            json!({"text": {"format": {"type": "json_object"}}})
        )),
        Some(ResponseFormat::Json)
    );
    assert_eq!(
        ResponseFormat::from_responses_request(&obj(json!({"text": {"format": {"type": "text"}}}))),
        None
    );
    // 缺 schema 的 json_schema：原样兜底，带整份 `text`。
    assert_eq!(
        ResponseFormat::from_responses_request(&obj(
            json!({"text": {"format": {"type": "json_schema", "name": "x"}}})
        )),
        Some(ResponseFormat::Other(
            json!({"format": {"type": "json_schema", "name": "x"}})
        ))
    );
}

#[test]
fn ollama_inbound_reads_a_bare_schema_or_the_string_json() {
    assert_eq!(
        ResponseFormat::from_ollama_request(&obj(json!({"format": {"type": "object"}}))),
        Some(ResponseFormat::JsonSchema {
            schema: json!({"type": "object"}),
            name: None,
            strict: None,
        })
    );
    assert_eq!(
        ResponseFormat::from_ollama_request(&obj(json!({"format": "json"}))),
        Some(ResponseFormat::Json)
    );
    // 空串 / null = 没要结构化输出（官方 `format` 缺省即无）。
    assert_eq!(
        ResponseFormat::from_ollama_request(&obj(json!({"format": ""}))),
        None
    );
    assert_eq!(
        ResponseFormat::from_ollama_request(&obj(json!({"format": null}))),
        None
    );
    assert_eq!(ResponseFormat::from_ollama_request(&obj(json!({}))), None);
}

// ───────────────────────── ② 出站：三条映射的逐字形状 ─────────────────────────

/// 黄金用例 1：Anthropic —— 3 层、无 `name` / `strict` / `json_schema` 包裹，
/// 且**递归**补 `additionalProperties:false`（只在缺失时补，显式给了值一律不动）。
#[test]
fn anthropic_outbound_is_three_layers_with_recursive_additional_properties() {
    let mut body = json!({"model": "real-model", "max_tokens": 16});
    structured::apply(
        &mut body,
        Some(&schema_format(Some(true))),
        Wire::Anthropic,
        StructuredOutput::Native,
        Some(Wire::Anthropic),
    )
    .expect("Anthropic 面表达得了 json_schema");

    assert_eq!(
        body,
        json!({
            "model": "real-model",
            "max_tokens": 16,
            "output_config": {"format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "a": {"type": "string"},
                        "nested": {"type": "object", "additionalProperties": false,
                                   "properties": {"b": {"type": "integer"}}},
                        "list": {"type": "array", "items": {
                            "type": "object", "additionalProperties": false,
                            "properties": {"c": {"type": "string"}}}},
                        // 显式 `true` 不改成 `false`：那是客户端的语义（真给 true 就让上游自己 400）。
                        "free": {"type": "object", "additionalProperties": true}
                    },
                    "required": ["a"],
                    "additionalProperties": false
                }
            }}
        })
    );
    // `name` / `strict` 一个字都不发（Anthropic 的 format 里只有两个键）。
    let f = &body["output_config"]["format"];
    assert_eq!(f.as_object().unwrap().len(), 2, "{f}");
    assert!(f.get("name").is_none() && f.get("strict").is_none());
}

/// 黄金用例 2：OpenAI Chat —— 4 层（`response_format` → `json_schema` → `{name,schema,strict}`），
/// `name` 一律常量 `"response"`，`strict` 只透传客户端显式值，schema **不**补 additionalProperties。
#[test]
fn chat_outbound_is_four_layers_with_the_constant_name() {
    let mut body = json!({"model": "real-model"});
    structured::apply(
        &mut body,
        Some(&schema_format(Some(true))),
        Wire::OpenAiChat,
        StructuredOutput::Native,
        Some(Wire::OpenAiChat),
    )
    .expect("Chat 面表达得了 json_schema");
    assert_eq!(
        body["response_format"],
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": RESPONSE_NAME,
                "schema": nested_schema(),
                "strict": true
            }
        })
    );
    assert_eq!(RESPONSE_NAME, "response");

    // 没给 strict：整个键不出现（吃上游缺省 false），schema 逐字不动。
    let mut body = json!({});
    structured::apply(
        &mut body,
        Some(&schema_format(None)),
        Wire::OpenAiChat,
        StructuredOutput::Native,
        Some(Wire::OpenAiChat),
    )
    .unwrap();
    let js = &body["response_format"]["json_schema"];
    assert_eq!(js["name"], "response", "name 一律常量，不追入站那个名字");
    assert!(js.get("strict").is_none(), "{js}");
    assert_eq!(js["schema"], nested_schema());
}

/// 黄金用例 3：OpenAI Responses —— `text.format` 3 层（少一层 `json_schema` 包裹），`name` 必填。
#[test]
fn responses_outbound_is_three_layers_under_text_format() {
    let mut body = json!({"model": "real-model"});
    structured::apply(
        &mut body,
        Some(&schema_format(Some(false))),
        Wire::OpenAiResponses,
        StructuredOutput::Native,
        Some(Wire::OpenAiResponses),
    )
    .expect("Responses 面表达得了 json_schema");
    assert_eq!(
        body,
        json!({"model": "real-model", "text": {"format": {
            "type": "json_schema",
            "name": RESPONSE_NAME,
            "schema": nested_schema(),
            "strict": false
        }}})
    );
    // 字段名互斥：Responses 没有 `response_format`、Chat 没有 `text`。
    assert!(body.get("response_format").is_none(), "{body}");
}

#[test]
fn json_mode_maps_to_json_object_on_both_openai_wires() {
    let mut chat = json!({});
    structured::apply(
        &mut chat,
        Some(&ResponseFormat::Json),
        Wire::OpenAiChat,
        StructuredOutput::Native,
        Some(Wire::OpenAiChat),
    )
    .unwrap();
    assert_eq!(chat, json!({"response_format": {"type": "json_object"}}));

    let mut resp = json!({});
    structured::apply(
        &mut resp,
        Some(&ResponseFormat::Json),
        Wire::OpenAiResponses,
        StructuredOutput::Native,
        Some(Wire::OpenAiResponses),
    )
    .unwrap();
    assert_eq!(resp, json!({"text": {"format": {"type": "json_object"}}}));
}

// ───────────────────────── ③ 请求期 400 的两条路径 ─────────────────────────

/// 路径一：无 schema 的 JSON mode 打到 Anthropic wire —— 一律 400（`output_config.format`
/// 只有 json_schema 一种类型，写不出「任意 JSON」）。
#[test]
fn json_mode_on_the_anthropic_wire_is_rejected_with_400() {
    let mut body = json!({});
    let err = structured::apply(
        &mut body,
        Some(&ResponseFormat::Json),
        Wire::Anthropic,
        StructuredOutput::Native,
        Some(Wire::Anthropic),
    )
    .expect_err("Anthropic 写不出任意 JSON");
    assert_eq!(err.status, 400);
    assert_eq!(err.error_type, "invalid_request_error");
    assert!(err.message.contains("json_schema"), "{}", err.message);
    assert!(body.get("output_config").is_none(), "不许静默丢弃后照发");
}

/// 路径二：能力位标 `ignore`（该面会静默忽略结构化输出字段）→ 客户端要结构化输出就 400 并说明。
#[test]
fn ignore_capability_rejects_any_structured_output_with_400() {
    for wire in Wire::ALL {
        let mut body = json!({});
        let err = structured::apply(
            &mut body,
            Some(&schema_format(None)),
            wire,
            StructuredOutput::Ignore,
            Some(wire),
        )
        .expect_err("ignore 面必须 400");
        assert_eq!(err.status, 400, "{wire:?}");
        assert_eq!(err.error_type, "invalid_request_error");
        assert!(err.message.contains(wire.as_str()), "{}", err.message);
        assert!(err.message.contains("ignore"), "{}", err.message);
        assert!(body.as_object().unwrap().is_empty(), "{body}");
    }
    // 客户端**没要**结构化输出时能力位无关：照常放行。
    let mut body = json!({"model": "m"});
    structured::apply(
        &mut body,
        None,
        Wire::Anthropic,
        StructuredOutput::Ignore,
        Some(Wire::Anthropic),
    )
    .expect("没要结构化输出就不该拦");
    assert_eq!(body, json!({"model": "m"}));
}

/// `Other` 只在**与入站同形状**的 wire 上原样带，其余 400。
#[test]
fn other_only_passes_through_on_the_same_shaped_wire() {
    let raw = json!({"format": {"type": "没见过的类型", "whatever": 1}});
    let mut body = json!({});
    structured::apply(
        &mut body,
        Some(&ResponseFormat::Other(raw.clone())),
        Wire::Anthropic,
        StructuredOutput::Native,
        Some(Wire::Anthropic),
    )
    .expect("入站 Anthropic → 出站 Anthropic：原样带");
    assert_eq!(body, json!({"output_config": raw}));

    let mut body = json!({});
    let err = structured::apply(
        &mut body,
        Some(&ResponseFormat::Other(raw)),
        Wire::OpenAiChat,
        StructuredOutput::Native,
        Some(Wire::Anthropic),
    )
    .expect_err("换了形状就表达不了");
    assert_eq!(err.status, 400);
    assert_eq!(err.error_type, "invalid_request_error");
}

/// `output_config` 的兄弟键（`effort`）不能因为本票而丢：Anthropic 直通时仍原样带。
#[test]
fn anthropic_passthrough_keeps_output_config_siblings() {
    let mut body = json!({"output_config": {"effort": "high"}});
    structured::apply(
        &mut body,
        Some(&schema_format(None)),
        Wire::Anthropic,
        StructuredOutput::Native,
        Some(Wire::Anthropic),
    )
    .unwrap();
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(
        body["output_config"]["format"]["type"], "json_schema",
        "{body}"
    );
}

// ───────────────────────── ④ 能力位：落库与迁移 ─────────────────────────

#[tokio::test]
async fn capability_bit_is_per_upstream_and_per_wire_and_defaults_to_native() {
    let store = open_store("cap-bit").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:9"))
        .await
        .unwrap();

    // 缺省 = native（没有记录就是没判过 / 认）。
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        up.structured_output(Wire::Anthropic),
        StructuredOutput::Native
    );
    assert_eq!(up.structured_capability(Wire::Anthropic), None);

    provider::set_structured_output(
        &store,
        "cc",
        Wire::Anthropic,
        StructuredOutput::Ignore,
        StructuredSource::Selfcheck,
    )
    .await
    .unwrap();

    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        up.structured_output(Wire::Anthropic),
        StructuredOutput::Ignore
    );
    assert_eq!(
        up.structured_output(Wire::OpenAiChat),
        StructuredOutput::Native
    );
    assert_eq!(
        up.structured_capability(Wire::Anthropic),
        Some(provider::StructuredCapability {
            mode: StructuredOutput::Ignore,
            source: StructuredSource::Selfcheck,
        })
    );
    // 改回 native 也记得住（手工标的来源）。
    provider::set_structured_output(
        &store,
        "cc",
        Wire::Anthropic,
        StructuredOutput::Native,
        StructuredSource::Manual,
    )
    .await
    .unwrap();
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        up.structured_capability(Wire::Anthropic),
        Some(provider::StructuredCapability {
            mode: StructuredOutput::Native,
            source: StructuredSource::Manual,
        })
    );
    // 能力位是**运行期事实**：改声明式字段（路径）不会把它冲掉。
    let mut edited = up.clone();
    edited.set_path(Wire::OpenAiChat, "/v2/chat");
    save_upstream(&store, &edited).await.unwrap();
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        up.structured_output(Wire::Anthropic),
        StructuredOutput::Native
    );
    assert_eq!(
        up.structured_capability(Wire::Anthropic).map(|c| c.source),
        Some(StructuredSource::Manual)
    );
}

// ───────────────────────── ⑤ 接进真实路由：两条 400 ─────────────────────────

/// 把转发面挂在本进程里，回 base_url。
async fn spawn_router(app: Gateway) -> String {
    let state = Arc::new(app);
    let router = gateway_core::ingress::proxy::router().with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

/// 一条绑好的上游：`(上游 id, 真实模型, wire)` + 目录条目 + 凭据。
///
/// 回的是**从库里读回来**的那份 `Upstream`：能力位是列里的事实，装配 `Gateway` 的那条路径
/// （外壳走 `list_upstreams`）也必须读它——拿一份现搭的 `Upstream::new` 会看不到能力位。
async fn wire_up(
    tag: &str,
    upstream_id: &str,
    real_model: &str,
    wire: Wire,
    cap: Option<StructuredOutput>,
) -> (Store, Upstream, Arc<Mutex<Vec<Value>>>) {
    let store = open_store(tag).await;
    let (base, seen) = mock_upstream().await;
    save_upstream(&store, &upstream(upstream_id, &base))
        .await
        .unwrap();
    store
        .put_credential(upstream_id, "default", "test-key")
        .await
        .unwrap();
    save_manual_model(&store, upstream_id, real_model, &[wire])
        .await
        .unwrap();
    if let Some(mode) = cap {
        provider::set_structured_output(
            &store,
            upstream_id,
            wire,
            mode,
            StructuredSource::Selfcheck,
        )
        .await
        .unwrap();
    }
    let up = provider::load_upstream(&store, upstream_id)
        .await
        .unwrap()
        .unwrap();
    (store, up, seen)
}

/// mock 上游：Anthropic 面回一段最小的 SSE（非流式入站也会被网关缓冲到终态）。
async fn mock_upstream() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                move |State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(v): Json<Value>| async move {
                    seen.lock().unwrap().push(v);
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        concat!(
                            "event: message_start\n",
                            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":1}}}\n\n",
                            "event: content_block_start\n",
                            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                            "event: content_block_delta\n",
                            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                            "event: content_block_stop\n",
                            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                            "event: message_delta\n",
                            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
                            "event: message_stop\n",
                            "data: {\"type\":\"message_stop\"}\n\n"
                        ),
                    )
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

fn gateway(store: Store, upstreams: Vec<Upstream>, bindings: Vec<Binding>) -> Gateway {
    Gateway::new(
        store,
        upstreams.into_iter().map(|u| (u.id.clone(), u)).collect(),
        BindingTable::new(bindings, default_profiles()),
    )
}

/// 路径一接进真实路由：Responses 入站的 `text.format = {"type":"json_object"}` 绑到 Anthropic
/// 面 → 400 + OpenAI 信封（Codex 读不懂 Anthropic 那份）。
#[tokio::test]
async fn json_mode_on_an_anthropic_binding_is_400_on_the_real_route() {
    let (store, up, _seen) = wire_up(
        "route-json",
        "cc",
        "claude-sonnet-5-5",
        Wire::Anthropic,
        None,
    )
    .await;
    let app = gateway(
        store,
        vec![up],
        vec![Binding::new(
            Profile::Global,
            "claude-sonnet",
            "cc",
            "claude-sonnet-5-5",
            Wire::Anthropic,
        )],
    );
    let url = spawn_router(app).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/v1/responses"))
        .json(&json!({
            "model": "claude-sonnet",
            "input": [{"role": "user", "content": "."}],
            "text": {"format": {"type": "json_object"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["type"], "invalid_request_error", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("json_schema"),
        "{v}"
    );
}

/// 路径二接进真实路由：能力位 `ignore` 时客户端要结构化输出 → 400 + Anthropic 信封。
#[tokio::test]
async fn ignore_capability_is_400_on_the_real_route_with_the_anthropic_envelope() {
    let (store, up, seen) = wire_up(
        "route-ignore",
        "cc",
        "claude-sonnet-5-5",
        Wire::Anthropic,
        Some(StructuredOutput::Ignore),
    )
    .await;
    let app = gateway(
        store,
        vec![up],
        vec![Binding::new(
            Profile::Global,
            "claude-sonnet",
            "cc",
            "claude-sonnet-5-5",
            Wire::Anthropic,
        )],
    );
    let url = spawn_router(app).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/v1/messages"))
        .json(&json!({
            "model": "claude-sonnet",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "."}],
            "output_config": {"format": {"type": "json_schema",
                "schema": {"type": "object", "properties": {"a": {"type": "string"}}}}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["type"], "error", "{v}");
    assert_eq!(v["error"]["type"], "invalid_request_error", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("ignore"),
        "{v}"
    );
    // 400 是**请求期早拒**：上游一个字节都不该收到。
    assert!(seen.lock().unwrap().is_empty(), "早拒不该打到上游");
}

/// 正常路径：Anthropic 入站 → Anthropic 面，上游收到的逐字就是映射出来的 3 层形状。
#[tokio::test]
async fn anthropic_inbound_lands_as_output_config_format_on_the_upstream() {
    let (store, up, seen) =
        wire_up("route-ok", "cc", "claude-sonnet-5-5", Wire::Anthropic, None).await;
    let app = gateway(
        store,
        vec![up],
        vec![Binding::new(
            Profile::Global,
            "claude-sonnet",
            "cc",
            "claude-sonnet-5-5",
            Wire::Anthropic,
        )],
    );
    let url = spawn_router(app).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/v1/messages"))
        .json(&json!({
            "model": "claude-sonnet",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "."}],
            "output_config": {"effort": "high", "format": {"type": "json_schema",
                "schema": {"type": "object", "properties": {"a": {"type": "string"}}}}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let sent = seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("上游该收到一发");
    assert_eq!(
        sent["output_config"],
        json!({"effort": "high", "format": {"type": "json_schema",
            "schema": {"type": "object", "additionalProperties": false,
                       "properties": {"a": {"type": "string"}}}}})
    );
}

// ───────────────────────── ⑥ 自检探针：敌意提示词 + 判定 ─────────────────────────

/// 起一个「照 prompt 直说」的 mock：敌意提示词下回 `hello`（= 没被 schema 约束住）。
async fn mock_reply(body: Value) -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                move |State((seen, body)): State<(Arc<Mutex<Vec<Value>>>, Value)>,
                      Json(v): Json<Value>| async move {
                    seen.lock().unwrap().push(v);
                    (StatusCode::OK, Json(body))
                },
            ),
        )
        .with_state((seen.clone(), body));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// 探针认「回答被 schema 约束住」= native；认「照敌意提示词直说」= ignore 并落库。
#[tokio::test]
async fn probe_marks_ignore_when_the_hostile_prompt_wins_and_native_when_the_schema_wins() {
    // ① 上游忽略 schema：逐字照 prompt 回 hello。
    let store1 = open_store("probe-ignore").await;
    let (base, seen) = mock_reply(json!({
        "type": "message",
        "content": [{"type": "text", "text": "hello"}]
    }))
    .await;
    save_upstream(&store1, &upstream("cc", &base))
        .await
        .unwrap();
    store1.put_credential("cc", "default", "k").await.unwrap();
    let up = provider::load_upstream(&store1, "cc")
        .await
        .unwrap()
        .unwrap();
    let r = structured::probe(
        &reqwest::Client::new(),
        &store1,
        &up,
        Wire::Anthropic,
        "claude-sonnet-5-5",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.mode, Some(StructuredOutput::Ignore), "{r:?}");
    assert_eq!(r.answer, "hello");
    assert_eq!(r.prompt, HOSTILE_PROMPT);
    // 探针形状：最短请求 + 带 schema 的字段 + 敌意提示词。
    let sent = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent["messages"][0]["content"], HOSTILE_PROMPT);
    assert_eq!(
        sent["output_config"]["format"]["schema"]["additionalProperties"],
        false
    );
    let after = provider::load_upstream(&store1, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.structured_capability(Wire::Anthropic),
        Some(provider::StructuredCapability {
            mode: StructuredOutput::Ignore,
            source: StructuredSource::Selfcheck,
        })
    );

    // ② 上游真强制 schema：回一个合法 JSON 对象。
    let store2 = open_store("probe-native").await;
    let (base, _seen) = mock_reply(json!({
        "type": "message",
        "content": [{"type": "text", "text": "{\"a\":\"hello\"}"}]
    }))
    .await;
    save_upstream(&store2, &upstream("cc", &base))
        .await
        .unwrap();
    store2.put_credential("cc", "default", "k").await.unwrap();
    let up = provider::load_upstream(&store2, "cc")
        .await
        .unwrap()
        .unwrap();
    let r = structured::probe(
        &reqwest::Client::new(),
        &store2,
        &up,
        Wire::Anthropic,
        "claude-sonnet-5-5",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(r.mode, Some(StructuredOutput::Native), "{r:?}");
    let after = provider::load_upstream(&store2, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.structured_output(Wire::Anthropic),
        StructuredOutput::Native
    );
}

/// 判不出来（没凭据 / 连不上 / 超时）就不落库：能力位保持原样，别把「没测出」说成 native。
#[tokio::test]
async fn probe_without_a_verdict_does_not_touch_the_capability_bit() {
    // ① 没凭据：502（与转发面同一套口径），不落库。
    let store = open_store("probe-nocred").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:1"))
        .await
        .unwrap();
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    let r = structured::probe(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "claude-sonnet-5-5",
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(r.mode, None, "{r:?}");
    assert_eq!(r.status, 502, "{r:?}");
    assert!(r.detail.contains("凭据"), "{r:?}");

    // ② 有凭据但连不上 / 超时：502 或 504 都算「判不出来」，同样不落库。
    let store = open_store("probe-noverdict").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:1"))
        .await
        .unwrap();
    store.put_credential("cc", "default", "k").await.unwrap();
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    let r = structured::probe(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "claude-sonnet-5-5",
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(r.mode, None, "{r:?}");
    assert!(
        r.status == 502 || r.status == 504,
        "连不上只该是 502 / 504：{r:?}"
    );
    let after = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.structured_capability(Wire::Anthropic), None);
}

/// 探针在 Chat 面上编的是 4 层 `response_format`（同一个探针三面通用）。
#[tokio::test]
async fn probe_uses_the_chat_shape_on_the_chat_wire() {
    let store = open_store("probe-chat").await;
    let (base, seen) = mock_reply(json!({
        "choices": [{"message": {"content": "{\"a\":\"hello\"}"}, "finish_reason": "stop"}]
    }))
    .await;
    let mut up = upstream("cc", &base);
    up.set_path(Wire::OpenAiChat, "/v1/messages"); // mock 只挂了一个路由
    save_upstream(&store, &up).await.unwrap();
    store.put_credential("cc", "default", "k").await.unwrap();
    let up = provider::load_upstream(&store, "cc")
        .await
        .unwrap()
        .unwrap();
    let r = structured::probe(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::OpenAiChat,
        "deepseek/deepseek-v4.1-flash",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(r.mode, Some(StructuredOutput::Native), "{r:?}");
    let sent = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        sent["response_format"]["json_schema"]["name"],
        RESPONSE_NAME
    );
    assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
    assert!(sent.get("output_config").is_none(), "{sent}");
}

// 迁移（v6 给 `upstreams` 加能力位列）的用例在 `store/tests.rs` 里：那里才够得着
// `schema::migrate_upto`（内部接口），能真造一份 v5 老库再升上来。
