//! OpenAI Chat Completions 编解码：入站请求 -> IR、IR -> 出站请求、上游 SSE -> IR 事件、
//! IR 事件 -> 客户端 Chat SSE（spec §4.3）。两侧都在这个文件里（ADR-0007 第 2 条：
//! 三个编解码文件各自保持完整，不硬拆成两半）。
//! 只做 Anthropic 保真能力需要的那部分映射；映射不回来的按「有损」处理并记在 README。

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::ir::{BlockKind, IrBlock, IrEvent, IrMessage, IrRequest};
use crate::sse;

static SEQ: AtomicU64 = AtomicU64::new(1);

// ---------- 请求：IR -> OpenAI Chat JSON ----------

pub fn encode_request(ir: &IrRequest, stream: bool) -> Value {
    let mut msgs: Vec<Value> = Vec::new();

    let sys: String = ir
        .system
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if !sys.is_empty() {
        msgs.push(json!({"role":"system","content":sys}));
    }

    for m in &ir.messages {
        match m.role.as_str() {
            "assistant" => {
                let text: String = m
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        IrBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let calls: Vec<Value> = m
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        IrBlock::ToolUse { id, name, input } => Some(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": input.to_string()}
                        })),
                        _ => None,
                    })
                    .collect();
                let mut o = json!({"role":"assistant"});
                if text.is_empty() && !calls.is_empty() {
                    o["content"] = Value::Null;
                } else {
                    o["content"] = json!(text);
                }
                if !calls.is_empty() {
                    o["tool_calls"] = json!(calls);
                }
                msgs.push(o);
            }
            _ => {
                // tool_result 在 Anthropic 里挂在 user 轮里，OpenAI 侧必须提升成独立 role:"tool" 行，
                // 且要排在同轮文本之前。
                for b in &m.blocks {
                    if let IrBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                    {
                        let mut c = tool_result_text(content);
                        if is_error == &Some(true) {
                            c = format!("[tool_error] {c}");
                        }
                        msgs.push(json!({"role":"tool","tool_call_id":tool_use_id,"content":c}));
                    }
                }
                let parts: Vec<Value> = m
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        IrBlock::Text { text, .. } => Some(json!({"type":"text","text":text})),
                        IrBlock::Image { source, .. } => Some(json!({
                            "type":"image_url",
                            "image_url":{"url": data_url(source)}
                        })),
                        _ => None,
                    })
                    .collect();
                if !parts.is_empty() {
                    let content = if parts.len() == 1
                        && parts[0].get("type").and_then(|t| t.as_str()) == Some("text")
                    {
                        parts[0]["text"].clone()
                    } else {
                        json!(parts)
                    };
                    msgs.push(json!({"role":"user","content":content}));
                }
            }
        }
    }

    let mut body = json!({
        "model": ir.model,
        "messages": msgs,
        "stream": stream,
    });
    if !ir.tools.is_empty() {
        body["tools"] = json!(ir
            .tools
            .iter()
            .map(|t| {
                json!({"type":"function","function":{
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "description": t.get("description").cloned().unwrap_or(json!("")),
                    "parameters": t.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))
                }})
            })
            .collect::<Vec<_>>());
    }
    if let Some(tc) = &ir.tool_choice {
        body["tool_choice"] = match tc.get("type").and_then(|t| t.as_str()).unwrap_or("auto") {
            "any" => json!("required"),
            "tool" => {
                json!({"type":"function","function":{"name":tc.get("name").cloned().unwrap_or(Value::Null)}})
            }
            "none" => json!("none"),
            _ => json!("auto"),
        };
    }
    if let Some(t) = ir.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = ir.top_p {
        body["top_p"] = json!(t);
    }
    if !ir.stop.is_empty() {
        body["stop"] = json!(ir.stop);
    }
    // 上限字段名各家不一（max_tokens / max_completion_tokens）；原型固定 max_tokens。
    body["max_tokens"] = json!(ir.max_tokens);
    // 缺 usage 是 OpenAI 兼容端的常见坑：无脑要求带上。
    if stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    // 有损项：thinking（OpenAI chat 无对应字段）、cache_control、metadata。
    body
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

fn data_url(source: &Value) -> String {
    let media = source
        .get("media_type")
        .and_then(|m| m.as_str())
        .unwrap_or("image/png");
    let data = source.get("data").and_then(|d| d.as_str()).unwrap_or("");
    format!("data:{media};base64,{data}")
}

// ---------- 请求：OpenAI Chat JSON -> IR ----------

/// Chat 面没有给出上限时的兜底（票 26 / 票 32 的先例 4096）。`max_tokens` 官方已可省，
/// 新接口叫 `max_completion_tokens`，两个都认。
const DEFAULT_MAX_TOKENS: u64 = 4096;

/// `POST /v1/chat/completions` 的请求 → IR（spec §4.3）。
///
/// 与 `anthropic::decode_request` / `responses::decode_request` 并列；`model` 由调用方先经
/// 绑定层解析（这里只做协议映射）。
///
/// **不透传 Chat 专有字段**（`n` / `logprobs` / `stream_options` / `user` /
/// `parallel_tool_calls` …）：出站可能是 Anthropic，严格的上游会直接 400
/// ——与 `responses::decode_request` 同一个判断。
pub fn decode_request(v: &Value) -> Result<IrRequest> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("请求体不是 JSON 对象"))?;
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("缺少 model"))?
        .to_string();

    let mut system: Vec<Value> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();
    match obj.get("messages") {
        Some(Value::Array(items)) => {
            for m in items {
                decode_message(m, &mut system, &mut messages);
            }
        }
        _ => return Err(anyhow!("messages 必须是数组")),
    }
    if messages.is_empty() {
        return Err(anyhow!("messages 为空"));
    }

    Ok(IrRequest {
        model,
        system,
        messages,
        tools: decode_chat_tools(obj.get("tools")),
        tool_choice: decode_chat_tool_choice(obj.get("tool_choice")),
        max_tokens: obj
            .get("max_tokens")
            .or_else(|| obj.get("max_completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: obj.get("temperature").and_then(Value::as_f64),
        top_p: obj.get("top_p").and_then(Value::as_f64),
        stop: decode_chat_stop(obj.get("stop")),
        thinking: None,
        metadata: None,
        // 票 57 已就位：`response_format` → IR 的 `ResponseFormat`（4 层的 `json_schema` 包裹）。
        format: crate::ir::ResponseFormat::from_chat_request(obj),
        extra: Map::new(),
        // Chat 没有「自由格式工具」这个概念（票 32 只从 Responses 侧引入）。
        freeform_tools: Vec::new(),
    })
}

/// 一行 Chat 消息 → IR。`system`/`developer` 归一到 IR 的 system 块（与 Responses 面同一口径）；
/// `tool` 轮的 `tool_result` 挂进 user 轮，连续多条并成同一轮，`tool_use_id` 才配得上对。
fn decode_message(m: &Value, system: &mut Vec<Value>, msgs: &mut Vec<IrMessage>) {
    match m.get("role").and_then(Value::as_str).unwrap_or("user") {
        "system" | "developer" => {
            for b in decode_content(m.get("content")) {
                if let IrBlock::Text { text, .. } = b {
                    if !text.trim().is_empty() {
                        system.push(json!({"type":"text","text":text}));
                    }
                }
            }
        }
        "assistant" => {
            let mut blocks = decode_content(m.get("content"));
            blocks.extend(decode_tool_calls(m.get("tool_calls")));
            if blocks.is_empty() {
                return;
            }
            if let Some(last) = msgs.last_mut() {
                if last.role == "assistant" {
                    last.blocks.extend(blocks);
                    return;
                }
            }
            msgs.push(IrMessage {
                role: "assistant".into(),
                blocks,
            });
        }
        "tool" | "function" => {
            let tool_use_id = m
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let content = m.get("content").cloned().unwrap_or(Value::Null);
            crate::responses::push_block(
                msgs,
                "user",
                IrBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error: None,
                },
            );
        }
        _ => {
            for b in decode_content(m.get("content")) {
                crate::responses::push_block(msgs, "user", b);
            }
        }
    }
}

/// `content` 是字符串或 parts 数组（`{type:"text"}` / `{type:"image_url"}`）。
fn decode_content(v: Option<&Value>) -> Vec<IrBlock> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => vec![IrBlock::Text {
            text: s.clone(),
            cache_control: None,
        }],
        Some(Value::Array(parts)) => parts.iter().filter_map(decode_part).collect(),
        _ => Vec::new(),
    }
}

fn decode_part(p: &Value) -> Option<IrBlock> {
    match p.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" | "input_text" | "output_text" => {
            p.get("text")
                .and_then(Value::as_str)
                .map(|t| IrBlock::Text {
                    text: t.to_string(),
                    cache_control: None,
                })
        }
        "image_url" | "input_image" => Some(decode_image(p)),
        // 不认识的零件原样留着：IR 的 `Unknown` 就是为「不认识就丢是保真度事故」留的。
        _ => Some(IrBlock::Unknown(p.clone())),
    }
}

/// `image_url.url` 是 `data:` URL 时解成 IR 的 base64 `source`（Anthropic 那侧的形状）。
/// 别种 URL（`https://…`）IR 表达不了 → `Unknown` 原样带过，总比悄悄丢一张图好。
fn decode_image(p: &Value) -> IrBlock {
    let url = p.pointer("/image_url/url").and_then(Value::as_str);
    let Some(rest) = url.and_then(|u| u.strip_prefix("data:")) else {
        return IrBlock::Unknown(p.clone());
    };
    let (media_type, data) = rest.split_once(";base64,").unwrap_or(("image/png", rest));
    IrBlock::Image {
        source: json!({"type":"base64","media_type":media_type,"data":data}),
        cache_control: None,
    }
}

/// `tool_calls[]`（assistant 轮里回传的调用）→ IR 的 `ToolUse`。`arguments` 是**字符串**
/// （OpenAI Chat 的取值），复用 Responses 面那个「字符串 / 对象都吃」的解析。
fn decode_tool_calls(v: Option<&Value>) -> Vec<IrBlock> {
    let Some(arr) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|c| {
            let f = c.get("function")?;
            let name = f.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            // 没有 id 的客户端（手写的 OpenAI 兼容调用）给一个稳定合成 id，别让配对失败。
            let id = c
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed)));
            Some(IrBlock::ToolUse {
                id,
                name: name.to_string(),
                input: crate::responses::parse_arguments(f.get("arguments")),
            })
        })
        .collect()
}

/// `tools[]` 是 `{type:"function",function:{name,description,parameters}}`；IR 统一存 Anthropic
/// 形状（`parameters` → `input_schema`），与 Ollama 面同一套改名映射。
fn decode_chat_tools(v: Option<&Value>) -> Vec<Value> {
    let mut tools = Vec::new();
    let Some(arr) = v.and_then(Value::as_array) else {
        return tools;
    };
    for t in arr {
        let f = t.get("function").unwrap_or(t);
        let name = f.get("name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        tools.push(json!({
            "name": name,
            "description": f.get("description").and_then(Value::as_str).unwrap_or(""),
            "input_schema": f.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
        }));
    }
    tools
}

/// Chat 的 `tool_choice`（三态字符串 / `{type:"function",function:{name}}`）→ IR 的 Anthropic
/// 词表（`responses::decode_tool_choice` 是同一个映射，只是字段位置不同）。
fn decode_chat_tool_choice(v: Option<&Value>) -> Option<Value> {
    match v? {
        Value::String(s) => match s.as_str() {
            "auto" => Some(json!({"type":"auto"})),
            "required" => Some(json!({"type":"any"})),
            "none" => Some(json!({"type":"none"})),
            _ => None,
        },
        Value::Object(o) => o
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|n| json!({"type":"tool","name":n})),
        _ => None,
    }
}

/// `stop` 是 **string 或 string[]**（官方 oneOf）——与 Ollama 面同一套归一。
fn decode_chat_stop(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------- 流：上游 OpenAI SSE -> IR 事件 ----------

#[derive(Default)]
pub struct OpenAiDecoder {
    next_index: u32,
    open: Option<(u32, BlockKind)>,
    /// OpenAI 的 tool_calls 自带 index，需要映射到我们顺序分配的 content block index。
    tool_map: Vec<(u64, u32)>,
}

impl OpenAiDecoder {
    fn close_open(&mut self, out: &mut Vec<IrEvent>) {
        if let Some((idx, _)) = self.open.take() {
            out.push(IrEvent::BlockStop { index: idx });
        }
    }

    fn ensure(&mut self, kind: BlockKind, out: &mut Vec<IrEvent>) -> u32 {
        if let Some((idx, k)) = self.open {
            if k == kind {
                return idx;
            }
            out.push(IrEvent::BlockStop { index: idx });
        }
        let idx = self.next_index;
        self.next_index += 1;
        self.open = Some((idx, kind));
        out.push(IrEvent::BlockStart {
            index: idx,
            kind,
            id: None,
            name: None,
        });
        idx
    }

    pub fn decode(&mut self, f: &sse::Frame) -> Vec<IrEvent> {
        let mut out = Vec::new();
        if f.data.trim() == "[DONE]" {
            self.close_open(&mut out);
            out.push(IrEvent::Eos);
            return out;
        }
        let Ok(v) = serde_json::from_str::<Value>(&f.data) else {
            return out;
        };
        // 有些兼容端只在 SSE 事件名上标 error，data 里也是错误体。
        if v.get("error").is_some() || f.event.as_deref() == Some("error") {
            let e = v.get("error").cloned().unwrap_or(json!({}));
            out.push(IrEvent::Error {
                kind: e
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("api_error")
                    .to_string(),
                message: e
                    .get("message")
                    .and_then(|t| t.as_str())
                    .unwrap_or("上游返回 error 帧")
                    .to_string(),
            });
            return out;
        }

        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            out.push(IrEvent::Usage {
                input_tokens: u.get("prompt_tokens").and_then(|t| t.as_u64()),
                output_tokens: u.get("completion_tokens").and_then(|t| t.as_u64()),
                cache_read_input_tokens: u
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(|t| t.as_u64()),
                // 缓存写（票 58）：真录帧里它报在 usage **顶层**的 `cache_creation_input_tokens`
                // （`tests/fixtures/cache-write/openai.sse`，2026-10-15 实测 2623），名字与 Anthropic
                // 侧同名——OpenAI 官方词表里根本没有这个字段，所以按实测落，不照抄文档猜。
                // 同一上游的 deepseek 面另有 `prompt_tokens_details.cache_write_tokens`
                // （实测两次都恒为 0，拿不出「非零」的证据）→ **不读它**，免得把没验证过的形状当口径。
                cache_creation_input_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64()),
            });
        }

        let Some(choice) = v.pointer("/choices/0") else {
            return out;
        };
        let d = choice.get("delta").cloned().unwrap_or(json!({}));

        // 推理内容：DeepSeek 系叫 reasoning_content，OpenAI 系叫 reasoning。
        let reasoning = d
            .get("reasoning_content")
            .or_else(|| d.get("reasoning"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !reasoning.is_empty() {
            let idx = self.ensure(BlockKind::Thinking, &mut out);
            out.push(IrEvent::ThinkingDelta {
                index: idx,
                text: reasoning.to_string(),
            });
        }

        if let Some(text) = d.get("content").and_then(|t| t.as_str()) {
            if !text.is_empty() {
                let idx = self.ensure(BlockKind::Text, &mut out);
                out.push(IrEvent::TextDelta {
                    index: idx,
                    text: text.to_string(),
                });
            }
        }

        if let Some(calls) = d.get("tool_calls").and_then(|c| c.as_array()) {
            for c in calls {
                let oa_idx = c.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                let mapped = self
                    .tool_map
                    .iter()
                    .find(|(o, _)| *o == oa_idx)
                    .map(|(_, m)| *m);
                let idx = match mapped {
                    Some(m) => m,
                    None => {
                        self.close_open(&mut out);
                        let idx = self.next_index;
                        self.next_index += 1;
                        self.open = Some((idx, BlockKind::ToolUse));
                        self.tool_map.push((oa_idx, idx));
                        out.push(IrEvent::BlockStart {
                            index: idx,
                            kind: BlockKind::ToolUse,
                            id: c.get("id").and_then(|i| i.as_str()).map(str::to_string),
                            name: c
                                .pointer("/function/name")
                                .and_then(|n| n.as_str())
                                .map(str::to_string),
                        });
                        idx
                    }
                };
                if let Some(args) = c.pointer("/function/arguments").and_then(|a| a.as_str()) {
                    if !args.is_empty() {
                        out.push(IrEvent::ToolInputDelta {
                            index: idx,
                            partial_json: args.to_string(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
            self.close_open(&mut out);
            out.push(IrEvent::Stop {
                reason: map_finish_reason(reason).to_string(),
            });
        }
        out
    }
}

fn map_finish_reason(r: &str) -> &str {
    match r {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "content_filter" => "end_turn",
        other => other,
    }
}

// ---------- 流：IR 事件 -> 发给客户端的 OpenAI Chat SSE ----------

/// IR 事件 → `data: {…}` 帧 + `data: [DONE]`（spec §4.3；Chat 的 SSE **没有事件名**）。
///
/// **usage 的落点**（spec §4.3）：DeepSeek 把 usage 附在最后一个内容块上、不单独发 usage
/// chunk，所以这里也**不单发 usage chunk** —— usage 跟着它之后的第一帧走；若直到终态都没等到
/// 下一帧，就附在 `finish_reason` 那一帧上。
pub struct ChatEncoder {
    id: String,
    /// 对客户端暴露的模型名（虚拟模型名），不回显上游真实 ID。
    model: String,
    created: u64,
    started: bool,
    done: bool,
    /// 已归一化成 Chat 词表的 usage（上游没报就是 `None`）。
    usage: Option<Value>,
    usage_sent: bool,
    stop_reason: Option<String>,
    /// 缓冲中的工具调用：IR 的 block index → Chat 的 `tool_calls[].index`。
    open_tool: Option<(u32, u64)>,
    tool_seq: u64,
}

impl ChatEncoder {
    pub fn new(model: String) -> Self {
        Self {
            id: new_chat_id(),
            model,
            created: created_now(),
            started: false,
            done: false,
            usage: None,
            usage_sent: false,
            stop_reason: None,
            open_tool: None,
            tool_seq: 0,
        }
    }

    pub fn push(&mut self, ev: IrEvent) -> String {
        match ev {
            // Chat 帧的 id 是编码器自己发的（上游 Chat 的 `chatcmpl-…` 不进 IR），Start 只说明流开了。
            IrEvent::Start { .. } => String::new(),
            IrEvent::BlockStart {
                index,
                kind: BlockKind::ToolUse,
                id,
                name,
            } => {
                let n = self.tool_seq;
                self.tool_seq += 1;
                self.open_tool = Some((index, n));
                self.delta(json!({"tool_calls":[{
                    "index": n,
                    "id": id
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("call_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed))),
                    "type": "function",
                    "function": {"name": name.unwrap_or_default(), "arguments": ""}
                }]}))
            }
            IrEvent::BlockStart { .. } => String::new(),
            IrEvent::TextDelta { text, .. } => self.delta(json!({"content": text})),
            // Chat 词表里没有 thinking：DeepSeek 系叫 `reasoning_content`（出站解码器也读它），
            // 原样回给客户端，别把推理内容吞了。
            IrEvent::ThinkingDelta { text, .. } => self.delta(json!({"reasoning_content": text})),
            IrEvent::SignatureDelta { .. } => String::new(),
            IrEvent::ToolInputDelta {
                index,
                partial_json,
            } => match self.open_tool {
                Some((i, n)) if i == index => self.delta(
                    json!({"tool_calls":[{"index": n, "function": {"arguments": partial_json}}]}),
                ),
                _ => String::new(),
            },
            IrEvent::BlockStop { .. } => {
                self.open_tool = None;
                String::new()
            }
            // Chat 的 SSE 既没有 ping 帧型也没有事件名：发一帧 SSE 注释，客户端一律忽略。
            IrEvent::Ping => sse::comment("keep-alive"),
            IrEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => {
                self.usage = chat_usage(
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                );
                String::new()
            }
            IrEvent::Stop { reason } => {
                self.stop_reason = Some(reason);
                String::new()
            }
            IrEvent::Error { kind, message } => {
                // 流中错误：状态码已经发出去了，只能追加一帧错误体（帧已开，没有 `[DONE]`）。
                self.done = true;
                data_frame(&json!({"error":{"message":message,"type":kind,"code":Value::Null}}))
            }
            IrEvent::Eos => self.finish(),
        }
    }

    fn delta(&mut self, delta: Value) -> String {
        let pre = self.ensure_start();
        pre + &self.chunk(json!([{"index":0,"delta":delta,"finish_reason":Value::Null}]))
    }

    /// 首帧必须带 `role:"assistant"`（客户端靠它建 assistant 消息），而上游 Chat 帧里没有这个
    /// 信号，所以由编码器在第一次真要发帧之前补一帧（范式同 `ResponsesEncoder::write_start`）。
    fn ensure_start(&mut self) -> String {
        if self.started {
            return String::new();
        }
        self.started = true;
        self.chunk(json!([{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":Value::Null}]))
    }

    fn chunk(&mut self, choices: Value) -> String {
        let mut o = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": choices,
        });
        // usage 只在**已经知道**的那一帧上出现，且只出现一次（见类型上的那段说明）。
        if !self.usage_sent {
            if let Some(u) = self.usage.clone() {
                o["usage"] = u;
                self.usage_sent = true;
            }
        }
        data_frame(&o)
    }

    fn finish(&mut self) -> String {
        if self.done {
            return String::new();
        }
        self.done = true;
        let reason = self
            .stop_reason
            .as_deref()
            .map(finish_reason)
            .unwrap_or("stop");
        let mut out = self.ensure_start();
        out.push_str(&self.chunk(json!([{"index":0,"delta":{},"finish_reason":reason}])));
        out.push_str("data: [DONE]\n\n");
        out
    }
}

/// IR 的 `Stop.reason` 已归一化到 Anthropic 词表，这里反归一化回 Chat 的取值。
fn finish_reason(reason: &str) -> &'static str {
    match reason {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    }
}

/// Chat 的 usage 词表：`prompt_tokens` / `completion_tokens` / `total_tokens` +
/// `prompt_tokens_details.cached_tokens`；缓存写（票 58）在**顶层** `cache_creation_input_tokens`
/// ——与出站解码器读的是同一处（真录帧实测）。上游没报的键一律不填。
fn chat_usage(
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
) -> Option<Value> {
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    let mut o = Map::new();
    if let Some(n) = input {
        o.insert("prompt_tokens".into(), json!(n));
    }
    if let Some(n) = output {
        o.insert("completion_tokens".into(), json!(n));
    }
    if let (Some(i), Some(t)) = (input, output) {
        o.insert("total_tokens".into(), json!(i + t));
    }
    if let Some(n) = cache_read {
        o.insert("prompt_tokens_details".into(), json!({"cached_tokens": n}));
    }
    if let Some(n) = cache_write {
        o.insert("cache_creation_input_tokens".into(), json!(n));
    }
    Some(Value::Object(o))
}

/// 一帧 = `data: {…}` + 空行；Chat 的 SSE **没有事件名**，`[DONE]` 也没有。
fn data_frame(v: &Value) -> String {
    format!("data: {v}\n\n")
}

fn new_chat_id() -> String {
    format!("chatcmpl-gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

fn created_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------- 非流式：IR 事件 -> 一个 chat.completion ----------

/// 组装中的一次工具调用：`BlockStart(ToolUse)` 到 `BlockStop` 之间只攒 JSON 片段。
struct AsmTool {
    id: String,
    name: String,
    payload: String,
}

/// `stream:false` 的组装器（spec §4.3）：与 `ChatEncoder` 同一套块语义与 usage 取值，
/// 区别只是不发帧 —— 攒到终态一次组装成 `chat.completion`
/// （范式同 `AnthropicAssembler` / `ResponsesAssembler`）。
#[derive(Default)]
pub struct ChatAssembler {
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    /// 缓存写（票 58）：Chat 词表里没有这一项、**不回给客户端**，但日志行要记（上游报了才有）。
    cache_write: Option<u64>,
    ended: bool,
    error: Option<(String, String)>,
    stop_reason: Option<String>,
    text: String,
    thinking: String,
    tools: Vec<AsmTool>,
    open: Option<AsmTool>,
}

impl ChatAssembler {
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
                self.open = Some(AsmTool {
                    id: id.filter(|s| !s.is_empty()).unwrap_or_default(),
                    name: name.unwrap_or_default(),
                    payload: String::new(),
                });
            }
            IrEvent::BlockStart { .. } => {}
            IrEvent::TextDelta { text, .. } => self.text.push_str(&text),
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
                cache_creation_input_tokens,
            } => {
                self.usage_in = input_tokens.or(self.usage_in);
                self.usage_out = output_tokens.or(self.usage_out);
                self.cache_read = cache_read_input_tokens.or(self.cache_read);
                self.cache_write = cache_creation_input_tokens.or(self.cache_write);
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

    /// 上游给过终态才算完整应答（断流 / 无 `[DONE]` 时调用方回错误信封）。
    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn error(&self) -> Option<(String, String)> {
        self.error.clone()
    }

    /// 四项 usage：`(输入, 输出, 缓存读, 缓存写)`（票 58）；Chat 的**响应体**只回前三项
    /// 的 Chat 词表（缓存写只进日志行）。
    pub fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        (
            self.usage_in,
            self.usage_out,
            self.cache_read,
            self.cache_write,
        )
    }

    /// 一个完整的 `chat.completion`（官方非流式示例的字段与顺序）。
    pub fn body(&self, model: &str) -> Value {
        let mut msg = Map::new();
        msg.insert("role".into(), json!("assistant"));
        // 有 tool_calls 且没有正文时官方回 `content: null`（不是空串）。
        let has_tools = !self.tools.is_empty() || self.open.is_some();
        if self.text.is_empty() && has_tools {
            msg.insert("content".into(), Value::Null);
        } else {
            msg.insert("content".into(), json!(self.text));
        }
        if !self.thinking.is_empty() {
            msg.insert("reasoning_content".into(), json!(self.thinking));
        }
        let calls: Vec<Value> = self
            .tools
            .iter()
            .chain(self.open.iter())
            .map(|o| {
                // `arguments` 在 Chat 里是**字符串**（与 Ollama 面的对象相反）。
                let args = if o.payload.is_empty() {
                    "{}".to_string()
                } else {
                    o.payload.clone()
                };
                json!({
                    "id": o.id,
                    "type": "function",
                    "function": {"name": o.name, "arguments": args},
                })
            })
            .collect();
        if !calls.is_empty() {
            msg.insert("tool_calls".into(), json!(calls));
        }

        let mut o = Map::new();
        o.insert("id".into(), json!(new_chat_id()));
        o.insert("object".into(), json!("chat.completion"));
        o.insert("created".into(), json!(created_now()));
        o.insert("model".into(), json!(model));
        o.insert(
            "choices".into(),
            json!([{
                "index": 0,
                "message": Value::Object(msg),
                "finish_reason": self
                    .stop_reason
                    .as_deref()
                    .map(finish_reason)
                    .unwrap_or("stop"),
            }]),
        );
        o.insert(
            "usage".into(),
            chat_usage(
                self.usage_in,
                self.usage_out,
                self.cache_read,
                self.cache_write,
            )
            .unwrap_or_else(|| json!({})),
        );
        Value::Object(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ResponseFormat;

    /// 真录帧的字节（录制工具 `tests/record.ps1` 原样落盘；只读、不许改）。
    const REAL_CACHE_WRITE_OPENAI: &str = include_str!("../tests/fixtures/cache-write/openai.sse");

    /// 票 64：Chat 入站请求 → IR。system / 多部分 content / 图片 / 工具调用与结果 / tools 改名 /
    /// `max_completion_tokens` / `stop` 数组 / `response_format`（票 57 的归一化）逐条钉住。
    #[test]
    fn decodes_a_chat_request_into_ir() {
        let body = json!({
            "model": "chat-model",
            "messages": [
                {"role":"system","content":"你是助手"},
                {"role":"user","content":[
                    {"type":"text","text":"看图"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,AA"}}
                ]},
                {"role":"assistant","content":"","tool_calls":[{
                    "id":"call_1","type":"function",
                    "function":{"name":"f","arguments":"{\"a\":1}"}
                }]},
                {"role":"tool","tool_call_id":"call_1","content":"ok"}
            ],
            "tools":[{"type":"function","function":{
                "name":"f","description":"d","parameters":{"type":"object"}
            }}],
            "tool_choice":{"type":"function","function":{"name":"f"}},
            "max_completion_tokens":77,
            "temperature":0.25,
            "stop":["x"],
            "response_format":{"type":"json_object"},
            "stream":true
        });
        let ir = decode_request(&body).unwrap();

        assert_eq!(ir.model, "chat-model");
        assert_eq!(ir.system, vec![json!({"type":"text","text":"你是助手"})]);
        assert_eq!(ir.max_tokens, 77, "新字段名 max_completion_tokens 也认");
        assert_eq!(ir.temperature, Some(0.25));
        assert_eq!(ir.stop, vec!["x".to_string()]);
        assert_eq!(ir.tool_choice, Some(json!({"type":"tool","name":"f"})));
        assert_eq!(ir.format, Some(ResponseFormat::Json));

        let roles: Vec<&str> = ir.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
        match &ir.messages[0].blocks[1] {
            IrBlock::Image { source, .. } => {
                assert_eq!(source["type"], "base64");
                assert_eq!(source["media_type"], "image/png");
                assert_eq!(source["data"], "AA");
            }
            other => panic!("期望 image，得到 {other:?}"),
        }
        match &ir.messages[1].blocks[0] {
            IrBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "f");
                assert_eq!(input["a"], 1, "arguments 是字符串，要解成对象");
            }
            other => panic!("期望 tool_use，得到 {other:?}"),
        }
        match &ir.messages[2].blocks[0] {
            IrBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                assert_eq!(tool_use_id, "call_1", "配对键就是 tool_call_id");
                assert_eq!(content, "ok");
            }
            other => panic!("期望 tool_result，得到 {other:?}"),
        }
        assert_eq!(ir.tools[0]["input_schema"]["type"], "object");
        // Chat 专有字段一个都不许出站。
        assert!(ir.extra.is_empty());
    }

    /// 缺 `model` / 缺 `messages` / 空 `messages` / 非对象体：都是请求期 400（由调用方给信封）。
    #[test]
    fn rejects_chat_bodies_without_model_or_messages() {
        assert!(decode_request(&json!("不是对象")).is_err());
        assert!(decode_request(&json!({"messages":[{"role":"user","content":"."}]})).is_err());
        assert!(decode_request(&json!({"model":"m"})).is_err());
        assert!(decode_request(&json!({"model":"m","messages":[]})).is_err());
    }

    /// 票 58：OpenAI Chat 侧的缓存写字段，**用真录帧**
    /// （`tests/fixtures/cache-write/openai.sse`，2026-10-15 真打 commandcode `/chat/completions`
    /// + `gpt-6-luna`，system 是 >1024 token 的前缀 → 触发缓存创建）。
    ///
    /// 实测（帧文件就是独立来源）：`usage.cache_creation_input_tokens` = **2623**（顶层），
    /// `prompt_tokens` 2626、`completion_tokens` 6、`prompt_tokens_details.cached_tokens` 0。
    /// ——**不是** OpenAI 文档里的名字（那边没有缓存写这一项），名字与 Anthropic 侧同名。
    #[test]
    fn openai_cache_creation_input_tokens_lands_in_cache_write() {
        let mut parser = sse::SseParser::default();
        let mut frames = parser.push(REAL_CACHE_WRITE_OPENAI.as_bytes());
        frames.extend(parser.finish());

        let mut dec = OpenAiDecoder::default();
        let mut usage = None;
        for f in &frames {
            for ev in dec.decode(f) {
                if let IrEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } = ev
                {
                    usage = Some((
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    ));
                }
            }
        }
        assert_eq!(
            usage,
            Some((Some(2626), Some(6), Some(0), Some(2623))),
            "(输入, 输出, 缓存读, 缓存写)"
        );
    }

    /// 上游没报缓存写时是 `None`（**不编 0**）：这一帧没有 `cache_creation_input_tokens` 键。
    #[test]
    fn absent_cache_creation_stays_none() {
        let mut dec = OpenAiDecoder::default();
        let evs = dec.decode(&sse::Frame {
            event: None,
            data: r#"{"usage":{"prompt_tokens":10,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":4}}}"#
                .into(),
        });
        let usage = evs
            .into_iter()
            .find_map(|e| match e {
                IrEvent::Usage {
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                    ..
                } => Some((cache_read_input_tokens, cache_creation_input_tokens)),
                _ => None,
            })
            .expect("这一帧该给出 Usage 事件");
        assert_eq!(usage, (Some(4), None));
    }
}
