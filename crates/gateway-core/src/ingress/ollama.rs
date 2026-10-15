//! Ollama 兼容面（spec §4.4 / 票 56）：`/api/chat`、`/api/tags`、`/api/version`、`/api/ps`、
//! `/api/show`，外加「装载 / 卸载」这条本地假终态。
//!
//! 与 Anthropic / OpenAI 三个入站**共用同一条 IR 与出站链路**（spec §3：绝不做协议直转），
//! 只有三处不同：
//!
//! 1. **传输是 NDJSON，不是 SSE**：`application/x-ndjson`、一行一帧、无 `data:`、无 `[DONE]`；
//!    终态是末帧 `done:true`，统计字段全在帧顶层。
//! 2. **工具调用必须缓冲到 `BlockStop`**：`tool_calls[].function.arguments` 是**对象**，
//!    半个 JSON 表示不出来（这是 Ollama 面在工具调用上的结构性限制）。
//! 3. **错误信封是单层的** `{"error":"<字符串>"}`，不是 OpenAI 的嵌套形状。
//!
//! 装载 / 卸载（空 `messages`）是本地假终态、**不打上游**：网关不持有权重、也没有驻留概念
//! （spec §4.4）。
//!
//! `/api/*` 被两张脸共用：本模块只挂 5 个具体路由 + 一条兜底，兜底只吃**没被任何具体路由
//! 匹配**且以 `/api/` 开头的路径（管理面的 `/api/status` / `/api/logs*` 是具体路由，永远优先）。
//!
//! 三条 404 / 两条 400 文案逐字复用官方（2026-10-15 本机 Ollama 0.40.4 实测）：
//! `model '<name>' not found` / `missing request body` / `model is required`。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use crate::egress::upstream::Inbound;
use crate::ingress::api::rfc3339_utc;
use crate::ingress::proxy::{self, Gateway};
use crate::ir::{BlockKind, IrBlock, IrEvent, IrMessage, IrRequest};
use crate::responses::{parse_arguments, push_block};

/// `/api/version` 的版本串：这个值等于声明「实现了到 0.13.3 为止的 API 契约」
/// （spec §4.4 / 票 36 定案 5）。官方该 handler **恒 200、无错误分支**，另有未文档化的 `HEAD`。
const OLLAMA_VERSION: &str = "0.13.3";

/// `options.num_predict` 缺省（`-1` = 无上限）时给 IR 的上限：票 26 / 票 32 的先例 4096。
const DEFAULT_MAX_TOKENS: u64 = 4096;

// ───────────────────────── 路由 ─────────────────────────

/// Ollama 面的五个具体路由（挂进转发面的 `Router<Arc<Gateway>>`）。
///
/// `/api/version` 的 `HEAD` 由 axum 的 `get()` 自动兜住（官方也是同一个 handler）；
/// `/api/ps` 官方**没有** HEAD，所以显式压掉，不让它落到 GET 上。
pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/api/chat", post(chat))
        .route("/api/tags", get(tags))
        .route("/api/version", get(version))
        .route("/api/ps", get(ps).head(ps_no_head))
        .route("/api/show", post(show))
}

/// `/api/*` 兜底：没被任何具体路由匹配、且落在 `/api/` 下的路径 → 404。
///
/// ⚠️ 口径差（写进票 56 的 Answer）：spec §4.4 那句「其余 `/api/*` 一律 404」**只就 Ollama 面
/// 而言**，不是覆盖管理面。`/api/status`（票 43）、`/api/logs*`（票 53）以及原型自用的
/// `/api/routes`、`/api/echo` 都是**具体注册的路由**，在 axum 里优先于兜底，不会被这条吃掉。
///
/// 文案是本网关自己定的：真 Ollama 对未知路径回的是 gin 的纯文本 `404 page not found`，
/// 对已实现端点（`pull` / `create` / …）根本没有「未实现」这一态可比。这里统一用本面的
/// 错误信封，客户端解析错误时拿到的至少是结构化体。
pub async fn api_fallback(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        return error(
            StatusCode::NOT_FOUND,
            &format!("{} is not implemented by this gateway", uri.path()),
        );
    }
    // 其余路径保持 axum 原本的行为（空体 404），别把 `/v1/*` 的未知路径也换成 Ollama 信封。
    StatusCode::NOT_FOUND.into_response()
}

/// Ollama 的错误信封：**单层** `{"error":"<字符串>"}`（spec §4.4；不是 OpenAI 的嵌套形状）。
pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

// ───────────────────────── 请求：Ollama JSON -> IR ─────────────────────────

/// `/api/chat` 请求 → IR（逐字段映射见 `research/ollama-inbound.md` §1.1 / §4）。
///
/// 与 `anthropic.rs` / `responses.rs` 的两个 decoder 并列；`model` 已被调用方经绑定层解析过，
/// 这里只负责消息、工具与 `options`。
pub fn decode_request(v: &Value) -> Result<IrRequest> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("请求体不是 JSON 对象"))?;
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut system: Vec<Value> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();
    // 已经配对过的 `ToolUse.id`：同名工具调两次时不能两次都反查到最近那一次（下面按名反查）。
    let mut used: HashSet<String> = HashSet::new();
    let mut synth = 0usize;

    if let Some(arr) = obj.get("messages").and_then(Value::as_array) {
        for m in arr {
            let role = m
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .to_ascii_lowercase();
            let content = m.get("content").and_then(Value::as_str).unwrap_or_default();

            if role == "system" {
                if !content.is_empty() {
                    system.push(json!({"type":"text","text":content}));
                }
                continue;
            }

            // 工具结果轮：`content` 就是结果原文，别再多建一个文本块（IR 是 `Value`，字符串即可）。
            if role == "tool" {
                let name = m.get("tool_name").and_then(Value::as_str).unwrap_or("");
                let id = match m
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    // 源码有 `tool_call_id`（文档没记载）：有就直接用，它是首选。
                    Some(id) => id.to_string(),
                    // 官方原生客户端只发 `tool_name`：按名反查前面 assistant 轮里最近一次同名
                    // `ToolUse.id` 补上——放着空串，出站到 Anthropic 会 400（spec §4.4）。
                    None => pair_by_name(&messages, name, &used),
                };
                used.insert(id.clone());
                push_block(
                    &mut messages,
                    "user",
                    IrBlock::ToolResult {
                        tool_use_id: id,
                        content: json!(content),
                        // Ollama 没有错误标记字段：别拿 content 文本猜。
                        is_error: None,
                    },
                );
                continue;
            }

            if !content.is_empty() {
                push_block(
                    &mut messages,
                    &role,
                    IrBlock::Text {
                        text: content.to_string(),
                        cache_control: None,
                    },
                );
            }
            if let Some(thinking) = m
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                push_block(
                    &mut messages,
                    &role,
                    IrBlock::Thinking {
                        thinking: thinking.to_string(),
                        signature: None,
                    },
                );
            }
            for img in m
                .get("images")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(data) = img.as_str() {
                    push_block(
                        &mut messages,
                        &role,
                        IrBlock::Image {
                            source: json!({
                                "type": "base64",
                                "media_type": media_type(data),
                                "data": data,
                            }),
                            cache_control: None,
                        },
                    );
                }
            }
            for call in m
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let f = call.get("function").unwrap_or(call);
                let name = f.get("name").and_then(Value::as_str).unwrap_or_default();
                // `id` 官方恒有、文档示例却缺 ⇒ 两种都要容忍；缺了自己生成（不能留空串，
                // IR 就靠它配 `ToolResult`）。
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        synth += 1;
                        format!("call_gw_{synth}")
                    });
                push_block(
                    &mut messages,
                    "assistant",
                    IrBlock::ToolUse {
                        id,
                        name: name.to_string(),
                        // `arguments` 是对象（与 OpenAI Chat 的字符串相反）；`parse_arguments`
                        // 两种都吃得下，字符串分支留着给不守形状的客户端。
                        input: parse_arguments(f.get("arguments").filter(|a| !a.is_null())),
                    },
                );
            }
        }
    }

    if messages.is_empty() {
        return Err(anyhow!("messages 为空"));
    }

    let opts = obj.get("options").and_then(Value::as_object);

    Ok(IrRequest {
        model,
        system,
        messages,
        tools: decode_tools(obj.get("tools")),
        // Ollama 没有 `tool_choice`（官方 OpenAI 兼容面也明确不支持）。
        tool_choice: None,
        max_tokens: max_tokens(opts),
        temperature: opts
            .and_then(|o| o.get("temperature"))
            .and_then(Value::as_f64),
        top_p: opts.and_then(|o| o.get("top_p")).and_then(Value::as_f64),
        stop: decode_stop(opts),
        // `think`（bool 或模型自定义档位）**不往 IR 的 `thinking` 里塞**：IR 那个槽位是
        // Anthropic 的形状（`{"type":"enabled","budget_tokens":N}`），bool 转过去必然是上游 400；
        // 与其编一个 budget，不如如实忽略（thinking 仍由上游模型自己决定）。
        thinking: None,
        metadata: None,
        // `options` 里剩下的（`seed` / `top_k` / `min_p` / `repeat_*` / `presence_penalty` /
        // `frequency_penalty`）**不进 `extra`**：Anthropic 出站会把 `extra` 原样塞进请求体，
        // 这些键在那里是非法顶层字段（400）；本端口岸也没有任何 wire 读它们（`openai.rs` 不读 extra）。
        // 本机装载参数（`num_ctx` / `num_gpu` / …）与已废弃的 `typical_p` 同理丢弃。
        extra: Map::new(),
        freeform_tools: Vec::new(),
        // `format`（JSON mode / 裸 JSON schema）→ IR 的归一化槽位（票 57 的
        // [`ResponseFormat::from_ollama_request`]）：Ollama 面**只解不编**，出站仍走三条 wire
        // 的逐字映射，表达不了就在请求期 400（spec §4.4）。
        format: crate::ir::ResponseFormat::from_ollama_request(obj),
    })
}

/// 工具结果只有 `tool_name`：在**前面的 assistant 轮**里按名反查最近一次同名且还没配过对的
/// `ToolUse.id`（spec §4.4）。查不到给一个稳定合成 id 并留一行 warn——**不 400**，
/// 因为 Ollama 原生客户端根本不会带 id。
fn pair_by_name(messages: &[IrMessage], name: &str, used: &HashSet<String>) -> String {
    for m in messages.iter().rev().filter(|m| m.role == "assistant") {
        for b in m.blocks.iter().rev() {
            if let IrBlock::ToolUse { id, name: n, .. } = b {
                if n == name && !id.is_empty() && !used.contains(id) {
                    return id.clone();
                }
            }
        }
    }
    let synth = if name.is_empty() {
        "call_gw_unknown".to_string()
    } else {
        format!("call_name_{name}")
    };
    eprintln!("[warn] /api/chat 的 tool 轮只有 tool_name={name:?}，前面找不到同名 tool_use，按 {synth} 配对（可能配不上）");
    synth
}

/// `messages[].images` 是**裸 base64**（无 `data:`、无 media_type）；Anthropic 出站要求
/// `media_type` 合法，所以按 base64 前缀认常见格式，认不出按 PNG。
fn media_type(b64: &str) -> &'static str {
    if b64.starts_with("iVBORw0KGgo") {
        "image/png"
    } else if b64.starts_with("/9j/") {
        "image/jpeg"
    } else if b64.starts_with("R0lGOD") {
        "image/gif"
    } else if b64.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/png"
    }
}

/// `tools[]` 与 OpenAI Chat **逐字相同**（`{type,function:{name,description,parameters}}`）；
/// IR 里统一存 Anthropic 形状，所以这是一次**改名映射**（`parameters` → `input_schema`），不是新形状。
fn decode_tools(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let f = t.get("function").unwrap_or(t);
            let name = f.get("name").and_then(Value::as_str)?;
            Some(json!({
                "name": name,
                "description": f.get("description").and_then(Value::as_str).unwrap_or(""),
                "input_schema": f.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
            }))
        })
        .collect()
}

/// `options.stop` 是 **string 或 string[]**（官方 oneOf），归一成数组。
fn decode_stop(opts: Option<&Map<String, Value>>) -> Vec<String> {
    match opts.and_then(|o| o.get("stop")) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// `options.num_predict` → `max_tokens`；`-1`（默认）= 无上限 → 给缺省（票 26 / 32 的先例）。
fn max_tokens(opts: Option<&Map<String, Value>>) -> u64 {
    match opts
        .and_then(|o| o.get("num_predict"))
        .and_then(Value::as_f64)
    {
        Some(n) if n > 0.0 => n as u64,
        _ => DEFAULT_MAX_TOKENS,
    }
}

// ───────────────────────── 流：IR 事件 -> NDJSON 帧 ─────────────────────────

/// IR 事件 → `application/x-ndjson` 的一行一帧（对应 `ResponsesEncoder` 的角色）。
///
/// `created_at` 由编码器自己盖、一帧一个（实测 0.40.4：首帧与终帧时间戳不同）；
/// `model` 回客户端看到的**虚拟模型名**，不回显上游真实 ID。
pub struct OllamaEncoder {
    model: String,
    done: bool,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    stop_reason: Option<String>,
    open: Option<OpenTool>,
}

/// 缓冲中的一次工具调用：`BlockStart(ToolUse)` 到 `BlockStop` 之间只攒，不发帧。
struct OpenTool {
    id: String,
    name: String,
    payload: String,
}

impl OllamaEncoder {
    pub fn new(model: String) -> Self {
        Self {
            model,
            done: false,
            usage_in: None,
            usage_out: None,
            cache_read: None,
            stop_reason: None,
            open: None,
        }
    }

    pub fn push(&mut self, ev: IrEvent) -> String {
        match ev {
            // 响应里没有 message id 字段，`Start` 无处安放（只取 input_tokens）。
            IrEvent::Start { input_tokens, .. } => {
                self.usage_in = input_tokens.or(self.usage_in);
                String::new()
            }
            IrEvent::BlockStart {
                kind: BlockKind::ToolUse,
                id,
                name,
                ..
            } => {
                // 上游没发上一个 `BlockStop` 就开了新块：先把整通发出去，别把它吞了。
                let pre = self.close_tool();
                self.open = Some(OpenTool {
                    id: id
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("call_gw_{:x}", std::process::id())),
                    name: name.unwrap_or_default(),
                    payload: String::new(),
                });
                pre
            }
            IrEvent::BlockStart { .. } => String::new(),
            IrEvent::TextDelta { text, .. } => {
                self.frame(json!({"role":"assistant","content":text}))
            }
            // 实测 0.40.4：thinking 逐帧发，且 `content` 键**照发空串**（Message.Content 无 omitempty）。
            IrEvent::ThinkingDelta { text, .. } => {
                self.frame(json!({"role":"assistant","content":"","thinking":text}))
            }
            IrEvent::SignatureDelta { .. } => String::new(),
            IrEvent::ToolInputDelta { partial_json, .. } => {
                if let Some(o) = self.open.as_mut() {
                    o.payload.push_str(&partial_json);
                }
                String::new()
            }
            IrEvent::BlockStop { .. } => self.close_tool(),
            // NDJSON 没有 keep-alive 帧型：静默就是静默（超时判定交给客户端）。
            IrEvent::Ping => String::new(),
            IrEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                ..
            } => {
                self.usage_in = input_tokens.or(self.usage_in);
                self.usage_out = output_tokens.or(self.usage_out);
                self.cache_read = cache_read_input_tokens.or(self.cache_read);
                String::new()
            }
            IrEvent::Stop { reason } => {
                self.stop_reason = Some(reason);
                String::new()
            }
            IrEvent::Error { message, .. } => {
                // 流中错误：帧已经写出去了，状态码改不了 ⇒ 追加一行错误（spec §4.4）。
                self.done = true;
                line(&json!({ "error": message }))
            }
            IrEvent::Eos => {
                if self.done {
                    return String::new();
                }
                self.done = true;
                line(&self.terminal())
            }
        }
    }

    /// `BlockStop(ToolUse)`：到这里 `arguments` 已经是完整 JSON，一次给整通。
    fn close_tool(&mut self) -> String {
        let Some(o) = self.open.take() else {
            return String::new();
        };
        let args = serde_json::from_str::<Value>(&o.payload).unwrap_or_else(|_| json!({}));
        self.frame(json!({
            "role":"assistant",
            "content":"",
            "tool_calls":[{
                "id": o.id,
                "function": {
                    // `index` 无 omitempty（官方恒发）：我们一帧只带一通完整调用，ARRAY 下标即 0。
                    "index": 0,
                    "name": o.name,
                    "arguments": args,
                }
            }],
        }))
    }

    fn frame(&self, message: Value) -> String {
        line(&json!({
            "model": self.model,
            "created_at": created_at(),
            "message": message,
            "done": false,
        }))
    }

    /// 终帧：`done:true`、`message.content` 是空串，统计字段在**帧顶层**
    /// （官方 `Metrics` 内嵌提升；字段顺序也照官方结构体）。
    fn terminal(&self) -> Value {
        let mut o = Map::new();
        o.insert("model".into(), json!(self.model));
        o.insert("created_at".into(), json!(created_at()));
        o.insert("message".into(), json!({"role":"assistant","content":""}));
        o.insert("done".into(), json!(true));
        if let Some(reason) = self.stop_reason.as_deref().map(done_reason) {
            o.insert("done_reason".into(), json!(reason));
        }
        // `Metrics` 各字段都带 omitempty ⇒ 0 不出现；但 `prompt_eval_cached_count` 是 `*int`，
        // 指针非 nil 时**连 0 也照发**（实测 0.40.4 回的就是 `"prompt_eval_cached_count":0`），
        // 所以它按「上游给没给」判，不按值判。
        if let Some(n) = self.usage_in.filter(|n| *n > 0) {
            o.insert("prompt_eval_count".into(), json!(n));
        }
        if let Some(n) = self.cache_read {
            o.insert("prompt_eval_cached_count".into(), json!(n));
        }
        if let Some(n) = self.usage_out.filter(|n| *n > 0) {
            o.insert("eval_count".into(), json!(n));
        }
        Value::Object(o)
    }
}

/// IR 的 `Stop.reason` 已归一化到 Anthropic 词表，这里反归一化回 Ollama 的取值。
///
/// 2026-10-15 本机 Ollama 0.40.4 实测到的取值域：`stop`（正常收尾 / 工具调用）、
/// `length`（`num_predict` 截断）、`load` / `unload`（空 `messages`，本地假终态自己给）。
/// Anthropic 那侧不是 `stop` 的只有 `max_tokens`。
fn done_reason(reason: &str) -> &'static str {
    match reason {
        "max_tokens" => "length",
        _ => "stop",
    }
}

/// 一帧 = 一个 JSON 对象 + `'\n'`（官方源码注释：delineate chunks with new-line delimiter）。
fn line(v: &Value) -> String {
    let mut s = v.to_string();
    s.push('\n');
    s
}

/// `created_at` 一帧一个（官方示例首帧与终帧不同），RFC3339 UTC。
fn created_at() -> String {
    rfc3339_utc(SystemTime::now())
}

// ───────────────────────── 非流式：IR 事件 -> 一个 ChatResponse ─────────────────────────

/// `stream:false` 的组装器：与 `OllamaEncoder` 同一套块语义与 usage 取值，区别只是不发帧
/// —— 攒到终态一次给出（范式同 `AnthropicAssembler` / `ResponsesAssembler`）。
#[derive(Default)]
pub struct OllamaAssembler {
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    ended: bool,
    error: Option<(String, String)>,
    stop_reason: Option<String>,
    content: String,
    thinking: String,
    tools: Vec<OpenTool>,
    open: Option<OpenTool>,
}

impl OllamaAssembler {
    pub fn push(&mut self, ev: IrEvent) {
        match ev {
            IrEvent::Start { input_tokens, .. } => self.usage_in = input_tokens.or(self.usage_in),
            IrEvent::BlockStart {
                kind: BlockKind::ToolUse,
                id,
                name,
                ..
            } => {
                if let Some(o) = self.open.take() {
                    self.tools.push(o);
                }
                self.open = Some(OpenTool {
                    id: id.filter(|s| !s.is_empty()).unwrap_or_default(),
                    name: name.unwrap_or_default(),
                    payload: String::new(),
                });
            }
            IrEvent::BlockStart { .. } => {}
            IrEvent::TextDelta { text, .. } => self.content.push_str(&text),
            IrEvent::ThinkingDelta { text, .. } => self.thinking.push_str(&text),
            IrEvent::SignatureDelta { .. } | IrEvent::Ping => {}
            IrEvent::ToolInputDelta { partial_json, .. } => {
                if let Some(o) = self.open.as_mut() {
                    o.payload.push_str(&partial_json);
                }
            }
            IrEvent::BlockStop { .. } => {
                if let Some(o) = self.open.take() {
                    self.tools.push(o);
                }
            }
            IrEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                ..
            } => {
                self.usage_in = input_tokens.or(self.usage_in);
                self.usage_out = output_tokens.or(self.usage_out);
                self.cache_read = cache_read_input_tokens.or(self.cache_read);
            }
            IrEvent::Stop { reason } => self.stop_reason = Some(reason),
            IrEvent::Error { kind, message } => {
                if self.error.is_none() {
                    self.error = Some((kind, message));
                }
            }
            IrEvent::Eos => self.ended = true,
        }
    }

    /// 上游给过终态才算完整应答（断流 / 无 `message_stop` 时调用方回错误信封）。
    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn error(&self) -> Option<(String, String)> {
        self.error.clone()
    }

    pub fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>) {
        (self.usage_in, self.usage_out, self.cache_read)
    }

    /// 一个完整的 `ChatResponse`（官方非流式示例的字段与顺序）。
    pub fn body(&self, model: &str) -> Value {
        let mut msg = Map::new();
        msg.insert("role".into(), json!("assistant"));
        msg.insert("content".into(), json!(self.content));
        if !self.thinking.is_empty() {
            msg.insert("thinking".into(), json!(self.thinking));
        }
        if !self.tools.is_empty() {
            let calls: Vec<Value> = self
                .tools
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let args =
                        serde_json::from_str::<Value>(&o.payload).unwrap_or_else(|_| json!({}));
                    json!({
                        "id": o.id,
                        "function": {"index": i, "name": o.name, "arguments": args},
                    })
                })
                .collect();
            msg.insert("tool_calls".into(), json!(calls));
        }

        let mut o = Map::new();
        o.insert("model".into(), json!(model));
        o.insert("created_at".into(), json!(created_at()));
        o.insert("message".into(), Value::Object(msg));
        o.insert("done".into(), json!(true));
        if let Some(reason) = self.stop_reason.as_deref().map(done_reason) {
            o.insert("done_reason".into(), json!(reason));
        }
        if let Some(n) = self.usage_in.filter(|n| *n > 0) {
            o.insert("prompt_eval_count".into(), json!(n));
        }
        if let Some(n) = self.cache_read {
            o.insert("prompt_eval_cached_count".into(), json!(n));
        }
        if let Some(n) = self.usage_out.filter(|n| *n > 0) {
            o.insert("eval_count".into(), json!(n));
        }
        Value::Object(o)
    }
}

// ───────────────────────── 端点 ─────────────────────────

/// `POST /api/chat`：问答、装载 / 卸载（空 `messages`）。
async fn chat(State(app): State<Arc<Gateway>>, headers: HeaderMap, body: Bytes) -> Response {
    let v = match parse_body(&body) {
        Ok(v) => v,
        Err(msg) => return error(StatusCode::BAD_REQUEST, &msg),
    };
    let obj = v.as_object().unwrap_or(&Map::new()).clone();
    let raw = obj
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        return error(StatusCode::BAD_REQUEST, "model is required");
    }

    let identity = proxy::client_identity(&headers);
    // 日志里的档案是**客户端档案**（与票 51/53 的口径一致）：命中那条绑定的 profile 可能是
    // 落回 global 的那一行，不能拿它当客户端归属。
    let profile = app.bindings.profile_for(&identity);
    let name = bare_name(&raw);
    let rt = match app.bindings.resolve(name, &identity) {
        Ok(rt) => rt,
        Err(_) => return not_found(&raw),
    };
    proxy::dump_inbound(&app, &v);

    // 空 `messages` = 装载 / 卸载：**本地假终态，不打上游**（网关不持有权重）。
    // 模型名先解析：本机 0.40.4 实测「未知名 + 空 messages」回的是 404，不是 load。
    if obj
        .get("messages")
        .and_then(Value::as_array)
        .map(|m| m.is_empty())
        .unwrap_or(true)
    {
        let reason = if keep_alive_is_zero(obj.get("keep_alive")) {
            "unload"
        } else {
            "load"
        };
        println!(
            "[in] /api/chat model={raw} profile={} → {reason}（本地假终态）",
            profile.id()
        );
        // 实测：官方连 `stream` 缺省（true）时也是**单个 JSON 对象 + application/json**。
        return Json(json!({
            "model": raw,
            "created_at": created_at(),
            "message": {"role":"assistant","content":""},
            "done": true,
            "done_reason": reason,
        }))
        .into_response();
    }

    // `stream` **缺省 true**（与 OpenAI 面相反，两个 decoder 不共用取 stream 的助手）。
    let streaming = obj.get("stream").and_then(Value::as_bool).unwrap_or(true);
    println!(
        "[in] /api/chat model={raw} profile={} wire={} stream={streaming}",
        profile.id(),
        rt.wire.as_str()
    );
    proxy::forward_ir(
        app,
        &headers,
        rt,
        raw,
        decode_request(&v),
        streaming,
        Inbound::Ollama,
    )
    .await
}

/// `GET /api/tags`：与 `GET /v1/models` **同一集合**（spec §4.4）。`HEAD` 由 `get()` 兜住（官方也有）。
async fn tags(State(app): State<Arc<Gateway>>) -> Response {
    let models: Vec<Value> = app
        .bindings
        .exposed()
        .into_iter()
        .map(|m| {
            json!({
                // `name` 与 `model` 填同一个值：**裸虚拟模型名**，与 `/v1/models` 逐字一致。
                "name": m.name,
                "model": m.name,
                // 该绑定的最后修改时间我们没存 ⇒ 网关启动时间（spec §4.4）。
                "modified_at": rfc3339_utc(app.started_at),
                // 没有权重文件：0 是唯一诚实的值，且这个键没有 omitempty、必须给。
                "size": 0,
                "digest": digest(&m.name),
                "details": {
                    "parent_model": "",
                    // **不谎报 `gguf`**：网关不托管权重。
                    "format": "",
                    // 家族槽位填槽位名；普通虚拟模型空串 / 空数组。
                    "family": if m.is_family { m.name.clone() } else { String::new() },
                    "families": if m.is_family { json!([m.name]) } else { json!([]) },
                    "parameter_size": "",
                    "quantization_level": "",
                },
                // 四条出站链路都支持工具调用，客户端真会读这个键。
                "capabilities": ["completion", "tools"],
            })
        })
        .collect();
    Json(json!({"models": models})).into_response()
}

/// `GET|HEAD /api/version`：唯一键 `version`；恒 200、无错误分支。
async fn version() -> Json<Value> {
    Json(json!({"version": OLLAMA_VERSION}))
}

/// `GET /api/ps`：网关不装载任何模型 ⇒ 恒空数组（**不是 `null`**：官方在什么都没装载时就是这个形状）。
async fn ps() -> Json<Value> {
    Json(json!({"models": []}))
}

/// 官方 `/api/ps` 只有 `GET`：`HEAD` 回 405（实测 0.40.4），别让 axum 把它落到 GET 上。
async fn ps_no_head() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

/// `POST /api/show`：只填能诚实填的键（spec §4.4 的整键省略名单）。
async fn show(State(app): State<Arc<Gateway>>, headers: HeaderMap, body: Bytes) -> Response {
    let v = match parse_body(&body) {
        Ok(v) => v,
        Err(msg) => return error(StatusCode::BAD_REQUEST, &msg),
    };
    let obj = v.as_object().cloned().unwrap_or_default();
    let raw = obj
        .get("model")
        .and_then(Value::as_str)
        // `name` 只是已废弃的兜底（两者都给时 `model` 胜出）。
        .or_else(|| obj.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string();
    if raw.is_empty() {
        return error(StatusCode::BAD_REQUEST, "model is required");
    }

    let identity = proxy::client_identity(&headers);
    let name = bare_name(&raw);
    let rt = match app.bindings.resolve(name, &identity) {
        Ok(rt) => rt,
        Err(_) => return not_found(&raw),
    };
    let _ = &rt;

    // `verbose`（只影响 tensors 与 tokenizer 大数组）、`runner` / `all_manifests`（另一套
    // `ShowManifestsResponse` 形状，spec §4.4 明确不实现）、`system` / `options` / `template`
    // 一律**接受并忽略**：网关永远没有这些数据，也绝不改输出形状。
    Json(json!({
        // `details` 对 struct 的 omitempty 无效 ⇒ 键恒在；六个子键都没有 omitempty，必须全给。
        "details": {
            "parent_model": "",
            "format": "",
            "family": if rt.is_family() { rt.name.clone() } else { String::new() },
            "families": if rt.is_family() { json!([rt.name]) } else { json!([]) },
            "parameter_size": "",
            "quantization_level": "",
        },
        // 源码点名的兼容性硬要求：nil / 空 map 会让集成崩溃 ⇒ 恒 `{}`（别给 null、别编 kv）。
        "model_info": {},
        // `time.Time` 的 omitempty 也无效 ⇒ 键恒在；填网关启动时间，不填零值（否则界面显示 0001 年）。
        "modified_at": rfc3339_utc(app.started_at),
    }))
    .into_response()
}

/// 未知模型：404 + `model '<name>' not found`（逐字复用官方文案；工具的 show-then-pull 依赖它）。
fn not_found(name: &str) -> Response {
    error(StatusCode::NOT_FOUND, &format!("model '{name}' not found"))
}

/// 请求体的两道 400：空 body（逐字）与非法 JSON（用解析器的文本，官方那边是 gin bind 的文本）。
fn parse_body(bytes: &Bytes) -> Result<Value, String> {
    if bytes.is_empty() {
        return Err("missing request body".to_string());
    }
    serde_json::from_slice::<Value>(bytes).map_err(|e| e.to_string())
}

/// 入站解析**接受并剥掉** `:latest`（官方：tag 缺省即 latest），暴露的名字仍是裸名。
/// 别的 tag 不剥：`foo:7b` 不是我们的模型名，照原样查表、查不到就 404。
fn bare_name(raw: &str) -> &str {
    raw.strip_suffix(":latest").unwrap_or(raw)
}

/// `keep_alive: 0`（数字或 `"0"` / `"0s"` 这类时长串）= 卸载；`-1` 是「永不卸载」，不是 0。
fn keep_alive_is_zero(v: Option<&Value>) -> bool {
    let head = |s: &str| -> Option<f64> {
        let t: String = s
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
            .collect();
        t.parse().ok()
    };
    match v {
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::String(s)) => head(s) == Some(0.0),
        _ => false,
    }
}

/// `digest`：虚拟模型名的**确定性 sha256**，裸 hex。
///
/// 不带 `sha256:` 前缀——2026-10-15 本机 0.40.4 实测：`/api/tags` 与 `/api/ps` 的 `digest`
/// 都是裸 hex。确定性让客户端去重 / 缓存稳定（同为空串会互相碰撞）。
fn digest(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<Value> {
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Value>(l).expect("每一行都得是 JSON"))
            .collect()
    }

    fn drain(enc: &mut OllamaEncoder, evs: Vec<IrEvent>) -> Vec<Value> {
        let mut out = String::new();
        for ev in evs {
            out.push_str(&enc.push(ev));
        }
        lines(&out)
    }

    fn text_stream() -> Vec<IrEvent> {
        vec![
            IrEvent::Start {
                id: Some("msg_1".into()),
                input_tokens: Some(11),
            },
            IrEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text,
                id: None,
                name: None,
            },
            IrEvent::TextDelta {
                index: 0,
                text: "你好".into(),
            },
            IrEvent::BlockStop { index: 0 },
            IrEvent::Usage {
                input_tokens: Some(11),
                output_tokens: Some(2),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: None,
            },
            IrEvent::Stop {
                reason: "end_turn".into(),
            },
            IrEvent::Eos,
        ]
    }

    // ── 请求 ──

    #[test]
    fn decodes_messages_options_and_tools_positionally() {
        let ir = decode_request(&json!({
            "model": "claude-haiku",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "你好", "images": ["iVBORw0KGgoAAAANSUhEUg=="]},
                {"role": "assistant", "content": "答", "thinking": "想"},
                {"role": "tool", "content": "晴", "tool_name": "get_weather"},
            ],
            "tools": [{"type":"function","function":{
                "name":"get_weather","description":"d",
                "parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],
            "options": {"num_predict": 7, "temperature": 0.3, "top_p": 0.9,
                        "stop": "STOP", "seed": 42, "top_k": 20},
        }))
        .unwrap();

        assert_eq!(ir.model, "claude-haiku");
        assert_eq!(ir.max_tokens, 7);
        assert_eq!(ir.temperature, Some(0.3));
        assert_eq!(ir.top_p, Some(0.9));
        assert_eq!(ir.stop, vec!["STOP".to_string()]);
        // system 出 IR 的 system 槽位（Anthropic 形状的块）。
        assert_eq!(ir.system, vec![json!({"type":"text","text":"sys"})]);
        // system 轮不进 messages。
        assert_eq!(ir.messages.len(), 3);
        assert_eq!(ir.messages[0].role, "user");
        assert!(matches!(
            &ir.messages[0].blocks[0],
            IrBlock::Text { text, .. } if text == "你好"
        ));
        assert!(matches!(ir.messages[0].blocks[1], IrBlock::Image { .. }));
        // 工具结果轮：IR 的 role 是 user，块是 ToolResult，content 就是原文。
        assert_eq!(ir.messages[2].role, "user");
        assert!(matches!(
            &ir.messages[2].blocks[0],
            IrBlock::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "call_name_get_weather"
                    && content == &json!("晴")
                    && is_error.is_none()
        ));
        // tools：`parameters` → `input_schema` 的改名映射。
        assert_eq!(ir.tools[0]["name"], "get_weather");
        assert_eq!(ir.tools[0]["input_schema"]["type"], "object");
        // `seed` / `top_k` 不进 extra：Anthropic 出站会把 extra 原样塞进请求体（非法顶层字段）。
        assert!(ir.extra.is_empty(), "{:?}", ir.extra);
        assert!(ir.thinking.is_none());
    }

    #[test]
    fn image_media_type_is_sniffed_from_base64() {
        let cases = [
            ("iVBORw0KGgoAAAANSUhEUg==", "image/png"),
            ("/9j/4AAQSkZJRg==", "image/jpeg"),
            ("R0lGODlhAQABAAAAACw=", "image/gif"),
            ("UklGRiQAAABXRUJQVlA4IBg=", "image/webp"),
            ("AAAA", "image/png"),
        ];
        for (b64, want) in cases {
            assert_eq!(media_type(b64), want, "{b64}");
        }
    }

    #[test]
    fn stop_accepts_both_shapes_and_num_predict_minus_one_is_the_default() {
        let one = decode_request(&json!({
            "model":"m","messages":[{"role":"user","content":"."}],
            "options":{"stop":["a","b"]}}))
        .unwrap();
        assert_eq!(one.stop, vec!["a".to_string(), "b".to_string()]);
        let none = decode_request(&json!({
            "model":"m","messages":[{"role":"user","content":"."}],
            "options":{"num_predict":-1}}))
        .unwrap();
        assert_eq!(none.max_tokens, DEFAULT_MAX_TOKENS);
        let absent = decode_request(&json!({
            "model":"m","messages":[{"role":"user","content":"."}]}))
        .unwrap();
        assert_eq!(absent.max_tokens, DEFAULT_MAX_TOKENS);
    }

    /// `format`：裸 schema → `JsonSchema`（`name`/`strict` 留空）、`"json"` → `Json`、
    /// 缺省 / 空串 → 没要结构化输出（spec §4.4；解成 IR 后由票据 57 的三条出站映射接手）。
    #[test]
    fn format_is_normalized_into_the_ir_slot() {
        let with = |format: Value| {
            decode_request(
                &json!({"model":"m","messages":[{"role":"user","content":"."}],"format":format}),
            )
            .unwrap()
            .format
        };
        assert_eq!(
            with(json!({"type":"object","properties":{"city":{"type":"string"}}})),
            Some(crate::ir::ResponseFormat::JsonSchema {
                schema: json!({"type":"object","properties":{"city":{"type":"string"}}}),
                name: None,
                strict: None,
            })
        );
        assert_eq!(with(json!("json")), Some(crate::ir::ResponseFormat::Json));
        assert_eq!(with(json!("")), None);
        assert_eq!(
            decode_request(&json!({"model":"m","messages":[{"role":"user","content":"."}]}))
                .unwrap()
                .format,
            None
        );
    }

    /// 怪癖③：只有 `tool_name` 时按名反查前面 assistant 轮里的 `ToolUse.id`；
    /// 同名调两次要配到**两次不同的** id 上，查不到才用合成 id。
    #[test]
    fn tool_results_pair_by_name_with_distinct_tool_use_ids() {
        let req = |tool_name: &str, extra_history: Vec<Value>| {
            let mut msgs = extra_history;
            msgs.push(json!({"role":"tool","content":"r","tool_name": tool_name}));
            decode_request(&json!({"model":"m","messages": msgs})).unwrap()
        };

        // 有 tool_call_id：直接用（首选），不看名字。
        let ir = req(
            "get_weather",
            vec![json!({"role":"tool","content":"x","tool_call_id":"call_explicit"})],
        );
        let last = ir.messages.last().unwrap();
        assert!(
            matches!(&last.blocks[0], IrBlock::ToolResult { tool_use_id, .. }
            if tool_use_id == "call_explicit")
        );

        // 两次同名调用 → 两条结果配到不同的 id（最近的在后面）。
        let ir = decode_request(&json!({
        "model":"m",
        "messages":[
            {"role":"assistant","content":"","tool_calls":[
                {"id":"call_a","function":{"name":"get_weather","arguments":{"city":"上海"}}}]},
            {"role":"tool","content":"晴","tool_name":"get_weather"},
            {"role":"assistant","content":"","tool_calls":[
                {"id":"call_b","function":{"name":"get_weather","arguments":{"city":"北京"}}}]},
            {"role":"tool","content":"阴","tool_name":"get_weather"},
        ]}))
        .unwrap();
        let ids: Vec<String> = ir
            .messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                IrBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["call_a".to_string(), "call_b".to_string()]);

        // 找不到同名 tool_use：合成稳定 id，**不 400**（原生客户端不会带 id）。
        let ir = req("never_called", vec![]);
        assert!(
            matches!(&ir.messages[0].blocks[0], IrBlock::ToolResult { tool_use_id, .. }
            if tool_use_id == "call_name_never_called")
        );

        // assistant 轮的 tool_calls 缺 id ⇒ 自己生成，不留空串。
        let ir = decode_request(&json!({
            "model":"m",
            "messages":[{"role":"assistant","content":"","tool_calls":[
                {"function":{"name":"t","arguments":{}}}]}]}))
        .unwrap();
        assert!(
            matches!(&ir.messages[0].blocks[0], IrBlock::ToolUse { id, .. }
            if !id.is_empty())
        );
    }

    // ── 流：NDJSON ──

    /// 怪癖①：一行一帧、无 `data:` / `[DONE]`，终态是末帧 `done:true` 且 content 空串。
    #[test]
    fn ndjson_frames_end_with_a_terminal_done_frame() {
        let mut enc = OllamaEncoder::new("claude-haiku".into());
        let frames = drain(&mut enc, text_stream());
        assert_eq!(frames.len(), 2, "{frames:?}");
        let first = &frames[0];
        assert_eq!(first["model"], "claude-haiku");
        assert_eq!(first["message"]["role"], "assistant");
        assert_eq!(first["message"]["content"], "你好");
        assert_eq!(first["done"], false);
        assert!(first.get("done_reason").is_none(), "{first}");

        let last = &frames[1];
        assert_eq!(last["done"], true);
        assert_eq!(last["message"]["content"], "");
        assert_eq!(last["done_reason"], "stop");
        // 统计字段在帧顶层，不在 message 里。
        assert_eq!(last["prompt_eval_count"], 11);
        assert_eq!(last["eval_count"], 2);
        // `prompt_eval_cached_count` 是 *int：给了 0 就照发 0。
        assert_eq!(last["prompt_eval_cached_count"], 0);
        assert_eq!(
            last["created_at"].as_str().map(|s| s.len() > 10),
            Some(true)
        );
    }

    #[test]
    fn max_tokens_becomes_length_and_missing_reason_omits_the_key() {
        let mut enc = OllamaEncoder::new("m".into());
        let last = drain(
            &mut enc,
            vec![
                IrEvent::Stop {
                    reason: "max_tokens".into(),
                },
                IrEvent::Eos,
            ],
        )
        .pop()
        .unwrap();
        assert_eq!(last["done_reason"], "length");

        // 上游没给 Stop：`done_reason` 带 omitempty ⇒ 整键缺失（spec §4.4）。
        let mut enc = OllamaEncoder::new("m".into());
        let last = drain(&mut enc, vec![IrEvent::Eos]).pop().unwrap();
        assert!(last.get("done_reason").is_none(), "{last}");
    }

    /// 实测 0.40.4 的形状：thinking 逐帧，`content` 键照发空串。
    #[test]
    fn thinking_frames_carry_an_empty_content_string() {
        let mut enc = OllamaEncoder::new("m".into());
        let frames = drain(
            &mut enc,
            vec![
                IrEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::Thinking,
                    id: None,
                    name: None,
                },
                IrEvent::ThinkingDelta {
                    index: 0,
                    text: "想".into(),
                },
            ],
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["message"]["content"], "");
        assert_eq!(frames[0]["message"]["thinking"], "想");
        assert_eq!(frames[0]["done"], false);
    }

    /// 怪癖②：工具调用在 `BlockStart`→`BlockStop` 之间只缓冲，`BlockStop` 才发整通，
    /// 且 `arguments` 是**对象**（不是 OpenAI 那样的字符串）。
    #[test]
    fn tool_calls_buffer_until_block_stop_and_arguments_is_an_object() {
        let mut enc = OllamaEncoder::new("m".into());
        let frames = drain(
            &mut enc,
            vec![
                IrEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::ToolUse,
                    id: Some("call_1".into()),
                    name: Some("get_weather".into()),
                },
                IrEvent::ToolInputDelta {
                    index: 0,
                    partial_json: "{\"city\":".into(),
                },
                IrEvent::ToolInputDelta {
                    index: 0,
                    partial_json: "\"上海\"}".into(),
                },
                IrEvent::BlockStop { index: 0 },
                IrEvent::Stop {
                    reason: "tool_use".into(),
                },
                IrEvent::Eos,
            ],
        );
        assert_eq!(frames.len(), 2, "{frames:?}");
        let call = &frames[0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "call_1");
        assert_eq!(call["function"]["index"], 0);
        assert_eq!(call["function"]["name"], "get_weather");
        assert_eq!(call["function"]["arguments"], json!({"city":"上海"}));
        assert_eq!(frames[0]["message"]["content"], "");
        assert_eq!(frames[0]["done"], false);
        // 工具调用场景的 done_reason 也是 stop（实测 0.40.4 与 D1 一致）。
        assert_eq!(frames[1]["done_reason"], "stop");
        assert!(frames[1]["message"].get("tool_calls").is_none(), "别重复给");
    }

    #[test]
    fn mid_stream_error_appends_one_error_line_and_no_terminal_frame() {
        let mut enc = OllamaEncoder::new("m".into());
        let frames = drain(
            &mut enc,
            vec![
                IrEvent::TextDelta {
                    index: 0,
                    text: "半".into(),
                },
                IrEvent::Error {
                    kind: "upstream_abort".into(),
                    message: "上游在终态事件前断流".into(),
                },
                IrEvent::Eos,
            ],
        );
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!(frames[1]["error"], "上游在终态事件前断流");
        assert!(frames[1].get("done").is_none(), "错误行不是终帧");
    }

    // ── 非流式 ──

    #[test]
    fn assembler_builds_one_chat_response_with_tool_calls() {
        let mut a = OllamaAssembler::default();
        for ev in [
            IrEvent::Start {
                id: None,
                input_tokens: Some(9),
            },
            IrEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text,
                id: None,
                name: None,
            },
            IrEvent::TextDelta {
                index: 0,
                text: "你好".into(),
            },
            IrEvent::BlockStop { index: 0 },
            IrEvent::BlockStart {
                index: 1,
                kind: BlockKind::ToolUse,
                id: Some("call_1".into()),
                name: Some("get_weather".into()),
            },
            IrEvent::ToolInputDelta {
                index: 1,
                partial_json: "{\"city\":\"上海\"}".into(),
            },
            IrEvent::BlockStop { index: 1 },
            IrEvent::Usage {
                input_tokens: Some(9),
                output_tokens: Some(4),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            IrEvent::Stop {
                reason: "end_turn".into(),
            },
            IrEvent::Eos,
        ] {
            a.push(ev);
        }
        assert!(a.ended() && a.error().is_none());
        assert_eq!(a.usage(), (Some(9), Some(4), None));
        let body = a.body("claude-haiku");
        assert_eq!(body["model"], "claude-haiku");
        assert_eq!(body["done"], true);
        assert_eq!(body["done_reason"], "stop");
        assert_eq!(body["message"]["content"], "你好");
        assert_eq!(
            body["message"]["tool_calls"][0]["function"]["arguments"]["city"],
            "上海"
        );
        assert_eq!(body["prompt_eval_count"], 9);
        assert_eq!(body["eval_count"], 4);
        assert!(body.get("prompt_eval_cached_count").is_none());
    }

    // ── 小件 ──

    #[test]
    fn digest_is_deterministic_hex_sha256_of_the_bare_name() {
        // 期望值取自 sha256("abc") 的公开向量（独立于本函数）。
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest("claude-haiku"), digest("claude-haiku"));
        assert_ne!(digest("claude-haiku"), digest("claude-sonnet"));
    }

    #[test]
    fn latest_suffix_is_stripped_but_other_tags_are_not() {
        assert_eq!(bare_name("claude-haiku:latest"), "claude-haiku");
        assert_eq!(bare_name("claude-haiku"), "claude-haiku");
        assert_eq!(bare_name("qwen:7b"), "qwen:7b");
    }

    #[test]
    fn keep_alive_zero_means_unload_in_both_shapes() {
        assert!(keep_alive_is_zero(Some(&json!(0))));
        assert!(keep_alive_is_zero(Some(&json!("0"))));
        assert!(keep_alive_is_zero(Some(&json!("0s"))));
        assert!(!keep_alive_is_zero(Some(&json!("5m"))));
        assert!(!keep_alive_is_zero(Some(&json!(-1))));
        assert!(!keep_alive_is_zero(None));
    }

    #[test]
    fn error_envelope_is_single_layer() {
        let r = error(StatusCode::NOT_FOUND, "model 'x' not found");
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn empty_body_and_bad_json_are_the_two_400s() {
        assert_eq!(
            parse_body(&Bytes::new()).unwrap_err(),
            "missing request body"
        );
        assert!(parse_body(&Bytes::from_static(b"{")).is_err());
        assert!(parse_body(&Bytes::from_static(b"{}")).is_ok());
    }
}
