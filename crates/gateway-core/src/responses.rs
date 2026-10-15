//! OpenAI Responses 编解码（票 32）。Codex 的 `wire_api` 唯一合法值是 `responses`，
//! 所以这一层只做「Responses ⇄ IR」：入站请求解成 IR，IR 事件重新编成 Responses SSE。
//!
//! 三条硬约束（spec §4.2）：必须以 `response.completed` 收尾；`function_call` 与
//! `function_call_output` 靠 `call_id` 配对；客户端固定发 `store:false` / `stream:true` /
//! `include:["reasoning.encrypted_content"]`。

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ir::{BlockKind, IrBlock, IrEvent, IrMessage, IrRequest};
use crate::sse;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// Responses 里没有给出上限时的兜底（Codex 通常不带 `max_output_tokens`）。
const DEFAULT_MAX_TOKENS: u64 = 4096;

// ---------- 请求：Responses JSON -> IR ----------

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
    // Responses 的系统提示叫 `instructions`（没有独立的 system 消息角色）。
    if let Some(s) = obj.get("instructions").and_then(Value::as_str) {
        if !s.trim().is_empty() {
            system.push(json!({"type":"text","text":s}));
        }
    }

    let mut messages: Vec<IrMessage> = Vec::new();
    decode_input(obj.get("input"), &mut messages, &mut system);
    if messages.is_empty() {
        return Err(anyhow!("input 为空"));
    }

    let (tools, freeform) = decode_tools(obj.get("tools"));

    Ok(IrRequest {
        model,
        system,
        messages,
        tools,
        tool_choice: decode_tool_choice(obj.get("tool_choice")),
        max_tokens: obj
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: obj.get("temperature").and_then(Value::as_f64),
        top_p: obj.get("top_p").and_then(Value::as_f64),
        stop: Vec::new(),
        thinking: None,
        metadata: None,
        // 票 57：`text.format` 归一化进 IR。`text` 的兄弟键（`verbosity`）与其余 Responses
        // 专有字段一样不透传（见上面那段注释）。
        format: crate::ir::ResponseFormat::from_responses_request(obj),
        // 刻意**不**透传 Responses 专有字段（`store` / `include` / `reasoning` /
        // `previous_response_id` …）：出站是 Anthropic 或 OpenAI Chat，这些字段对它们
        // 没有意义，严格的上游会直接 400。
        extra: Map::new(),
        freeform_tools: freeform,
    })
}

fn decode_input(input: Option<&Value>, msgs: &mut Vec<IrMessage>, system: &mut Vec<Value>) {
    let items: &[Value] = match input {
        Some(Value::Array(a)) => a,
        // 简写形式：`input` 直接是一个字符串。
        Some(Value::String(s)) => {
            if !s.is_empty() {
                push_block(
                    msgs,
                    "user",
                    IrBlock::Text {
                        text: s.clone(),
                        cache_control: None,
                    },
                );
            }
            return;
        }
        _ => return,
    };

    for it in items {
        let ty = it.get("type").and_then(Value::as_str).unwrap_or("");
        // 带 `role` 的一律是 message 项（有的版本连 `type` 都省了）。
        if let Some(role) = it.get("role").and_then(Value::as_str) {
            let blocks = decode_content(it.get("content"));
            match role {
                "assistant" => {
                    for b in blocks {
                        push_block(msgs, "assistant", b);
                    }
                }
                "system" | "developer" => {
                    for b in blocks {
                        if let IrBlock::Text { text, .. } = b {
                            system.push(json!({"type":"text","text":text}));
                        }
                    }
                }
                _ => {
                    for b in blocks {
                        push_block(msgs, "user", b);
                    }
                }
            }
            continue;
        }
        match ty {
            "function_call" => {
                let id = call_id(it);
                push_block(
                    msgs,
                    "assistant",
                    IrBlock::ToolUse {
                        id,
                        name: it
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: parse_arguments(it.get("arguments")),
                    },
                );
            }
            // 自由格式工具（`type:"custom"`，如 Codex 的 apply_patch / exec）：
            // 载荷是一整段原文而不是 JSON，按统一约定包成 `{"input": 原文}`。
            "custom_tool_call" => {
                let id = call_id(it);
                push_block(
                    msgs,
                    "assistant",
                    IrBlock::ToolUse {
                        id,
                        name: it
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: json!({"input": it.get("input").and_then(Value::as_str).unwrap_or_default()}),
                    },
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                push_block(
                    msgs,
                    "user",
                    IrBlock::ToolResult {
                        tool_use_id: call_id(it),
                        content: it.get("output").cloned().unwrap_or(Value::Null),
                        is_error: None,
                    },
                );
            }
            // `reasoning` 项：上游不是 OpenAI，拿不到可验证的 encrypted_content，
            // 我们的上游也吃不下 CoT 回放 —— 丢掉（票 32 的处置决定）。
            _ => {}
        }
    }
}

fn decode_content(content: Option<&Value>) -> Vec<IrBlock> {
    match content {
        Some(Value::String(s)) => vec![IrBlock::Text {
            text: s.clone(),
            cache_control: None,
        }],
        Some(Value::Array(parts)) => parts.iter().filter_map(part_block).collect(),
        _ => Vec::new(),
    }
}

fn part_block(p: &Value) -> Option<IrBlock> {
    match p.get("type").and_then(Value::as_str).unwrap_or("") {
        "input_text" | "output_text" | "text" => Some(IrBlock::Text {
            text: p
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            cache_control: None,
        }),
        "input_image" => {
            let url = p.get("image_url").and_then(Value::as_str).unwrap_or("");
            Some(IrBlock::Image {
                source: image_source(url),
                cache_control: None,
            })
        }
        _ => None,
    }
}

/// `data:image/png;base64,AAA` 拆成 Anthropic 的 source；http(s) 直接当 url。
fn image_source(url: &str) -> Value {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((media, data)) = rest.split_once(";base64,") {
            return json!({"type":"base64","media_type":media,"data":data});
        }
    }
    json!({"type":"url","url":url})
}

fn call_id(it: &Value) -> String {
    it.get("call_id")
        .or_else(|| it.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_arguments(a: Option<&Value>) -> Value {
    match a {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({})),
        Some(v) => v.clone(),
        None => json!({}),
    }
}

/// 工具表：IR 里统一存 Anthropic 形状的 `input_schema`；自由格式工具额外记在
/// `freeform_tools` 里（不往工具定义里塞私有键，免得漏给出站上游）。
fn decode_tools(v: Option<&Value>) -> (Vec<Value>, Vec<String>) {
    let mut tools = Vec::new();
    let mut freeform = Vec::new();
    let Some(arr) = v.and_then(Value::as_array) else {
        return (tools, freeform);
    };
    for t in arr {
        let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
        match t.get("type").and_then(Value::as_str).unwrap_or("function") {
            "function" => tools.push(json!({
                "name": name,
                "description": desc,
                "input_schema": t.get("parameters").cloned().unwrap_or(json!({"type":"object"})),
            })),
            // 自由格式工具在 JSON-schema wire 上没有对应形状：对上游声明成
            // 「一个名叫 input 的字符串参数」，回来时再把 `input` 拆出来还原成原文。
            "custom" => {
                freeform.push(name.to_string());
                tools.push(json!({
                    "name": name,
                    "description": desc,
                    "input_schema": {
                        "type":"object",
                        "properties":{"input":{"type":"string","description":"原始自由格式载荷"}},
                        "required":["input"]
                    },
                }));
            }
            // web_search / file_search 这类本原型不支持的工具：丢掉，不出站。
            _ => {}
        }
    }
    (tools, freeform)
}

/// Responses 的 tool_choice → IR（Anthropic 词表，OpenAI 侧由 openai.rs 再映射一次）。
fn decode_tool_choice(v: Option<&Value>) -> Option<Value> {
    match v {
        Some(Value::String(s)) => match s.as_str() {
            "auto" => Some(json!({"type":"auto"})),
            "required" => Some(json!({"type":"any"})),
            "none" => Some(json!({"type":"none"})),
            _ => None,
        },
        Some(Value::Object(o)) => o
            .get("name")
            .and_then(Value::as_str)
            .map(|n| json!({"type":"tool","name":n})),
        _ => None,
    }
}

fn push_block(msgs: &mut Vec<IrMessage>, role: &str, b: IrBlock) {
    if let Some(last) = msgs.last_mut() {
        if last.role == role {
            last.blocks.push(b);
            return;
        }
    }
    msgs.push(IrMessage {
        role: role.into(),
        blocks: vec![b],
    });
}

// ---------- 共用的响应对象与 output item 形状 ----------

fn created_at() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_response_id() -> String {
    format!("resp_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Responses 的 usage 词表与 Anthropic 不同（`input_tokens` / `output_tokens` / details）。
fn usage_json(input: Option<u64>, output: Option<u64>, cache_read: Option<u64>) -> Value {
    let i = input.unwrap_or(0);
    let o = output.unwrap_or(0);
    json!({
        "input_tokens": i,
        "input_tokens_details": {"cached_tokens": cache_read.unwrap_or(0)},
        "output_tokens": o,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": i + o,
    })
}

fn message_item(id: &str, text: &str) -> Value {
    json!({
        "type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}],
    })
}

fn reasoning_item(id: &str, text: &str) -> Value {
    json!({
        "type":"reasoning","id":id,
        "summary": if text.is_empty() { json!([]) } else { json!([{"type":"summary_text","text":text}]) },
    })
}

fn tool_item(id: &str, name: &str, payload: &str, freeform: bool) -> Value {
    if freeform {
        json!({"type":"custom_tool_call","id":id,"call_id":id,"name":name,"input":payload,"status":"completed"})
    } else {
        json!({"type":"function_call","id":id,"call_id":id,"name":name,"arguments":payload,"status":"completed"})
    }
}

/// 响应对象骨架。两条路径（流式与非流式）共用，免得两边字段漂移。
fn response_object(id: &str, model: &str, status: &str, output: Value, usage: Value) -> Value {
    json!({
        "id": id,
        "object": "response",
        "created_at": created_at(),
        "status": status,
        "model": model,
        "output": output,
        "usage": usage,
        "error": Value::Null,
        "incomplete_details": Value::Null,
        "instructions": Value::Null,
        "metadata": {},
        "parallel_tool_calls": false,
        "previous_response_id": Value::Null,
        "reasoning": {"effort": Value::Null, "summary": Value::Null},
        "store": false,
        "temperature": 1,
        "text": {"format": {"type": "text"}},
        "tool_choice": "auto",
        "tools": [],
        "top_p": 1,
        "truncation": "disabled",
    })
}

// ---------- 流：IR 事件 -> 发给客户端的 Responses SSE ----------

struct Open {
    index: u32,
    kind: BlockKind,
    id: String,
    name: String,
    payload: String,
}

pub struct ResponsesEncoder {
    id: String,
    /// 对客户端暴露的模型名（虚拟模型名），不回显上游真实 ID。
    model: String,
    /// 自由格式工具名：决定工具调用回给客户端的是 `custom_tool_call` 还是 `function_call`。
    freeform: Vec<String>,
    seq: u64,
    item_seq: u64,
    started: bool,
    done: bool,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    items: Vec<Value>,
    open: Option<Open>,
}

impl ResponsesEncoder {
    pub fn new(model: String, freeform: Vec<String>) -> Self {
        Self {
            id: new_response_id(),
            model,
            freeform,
            seq: 0,
            item_seq: 0,
            started: false,
            done: false,
            usage_in: None,
            usage_out: None,
            cache_read: None,
            items: Vec::new(),
            open: None,
        }
    }

    pub fn push(&mut self, ev: IrEvent) -> String {
        // 上游（OpenAI Chat）没有 response.created 的对应事件，但它是客户端认 ID 的唯一来源，
        // 所以任何事件先于 Start 到达时补发一次。
        let pre = if !self.started {
            self.write_start()
        } else {
            String::new()
        };
        pre + &self.push_inner(ev)
    }

    fn write_start(&mut self) -> String {
        self.started = true;
        let mut out = self.frame(
            "response.created",
            json!({"response": response_object(&self.id, &self.model, "in_progress", json!([]), Value::Null)}),
        );
        out.push_str(&self.frame(
            "response.in_progress",
            json!({"response": response_object(&self.id, &self.model, "in_progress", json!([]), Value::Null)}),
        ));
        out
    }

    /// 每帧都带 `type` 与自增的 `sequence_number`（Responses 的契约字段）。
    fn frame(&mut self, ty: &str, body: Value) -> String {
        self.seq += 1;
        let mut o = body.as_object().cloned().unwrap_or_default();
        o.insert("type".into(), json!(ty));
        o.insert("sequence_number".into(), json!(self.seq));
        sse::frame(ty, &Value::Object(o).to_string())
    }

    fn push_inner(&mut self, ev: IrEvent) -> String {
        match ev {
            IrEvent::Start { .. } => String::new(),
            IrEvent::BlockStart {
                index: _,
                kind,
                id,
                name,
            } => self.open_item(kind, id, name),
            IrEvent::TextDelta { text, .. } => self.delta_text(&text),
            IrEvent::ThinkingDelta { text, .. } => self.delta_thinking(&text),
            // 没有可验证的 encrypted_content，签名不回给客户端（Codex 对非 OpenAI
            // provider 本来也会清掉加密推理项）。
            IrEvent::SignatureDelta { .. } => String::new(),
            IrEvent::ToolInputDelta { partial_json, .. } => self.delta_tool(&partial_json),
            IrEvent::BlockStop { .. } => self.close_item(),
            // Responses 没有 keep-alive 事件：发一帧 SSE 注释，客户端一律忽略。
            IrEvent::Ping => sse::comment("ping"),
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
            IrEvent::Stop { .. } => String::new(),
            IrEvent::Error { kind, message } => {
                self.done = true;
                let mut base = response_object(
                    &self.id,
                    &self.model,
                    "failed",
                    json!(self.items),
                    Value::Null,
                );
                base["error"] = json!({"code": kind, "message": message});
                self.frame("response.failed", json!({"response": base}))
            }
            IrEvent::Eos => self.finish(),
        }
    }

    fn open_item(&mut self, kind: BlockKind, id: Option<String>, name: Option<String>) -> String {
        // 同一时刻只允许一个 output item：上一个没关就先关掉。
        let mut out = self.close_item();
        self.item_seq += 1;
        let index = self.items.len() as u32;
        let name = name.unwrap_or_default();
        let n = self.item_seq;
        let freeform = kind == BlockKind::ToolUse && self.freeform.contains(&name);
        let (id, item, part) = match kind {
            BlockKind::Text => {
                let id = format!("msg_gw_{n}");
                (
                    id.clone(),
                    json!({"type":"message","id":id,"role":"assistant","status":"in_progress","content":[]}),
                    Some(json!({"type":"output_text","text":"","annotations":[]})),
                )
            }
            BlockKind::Thinking => {
                let id = format!("rs_gw_{n}");
                (
                    id.clone(),
                    json!({"type":"reasoning","id":id,"summary":[]}),
                    None,
                )
            }
            _ => {
                let id = id.unwrap_or_else(|| format!("call_gw_{n}"));
                let item = if freeform {
                    json!({"type":"custom_tool_call","id":id,"call_id":id,"name":name,"input":""})
                } else {
                    json!({"type":"function_call","id":id,"call_id":id,"name":name,"arguments":""})
                };
                (id, item, None)
            }
        };
        out.push_str(&self.frame(
            "response.output_item.added",
            json!({"output_index": index, "item": item}),
        ));
        if let Some(part) = part {
            out.push_str(&self.frame(
                "response.content_part.added",
                json!({"item_id":id,"output_index":index,"content_index":0,"part":part}),
            ));
        }
        self.open = Some(Open {
            index,
            kind,
            id,
            name,
            payload: String::new(),
        });
        out
    }

    fn delta_text(&mut self, text: &str) -> String {
        let Some(o) = self.open.as_mut() else {
            return String::new();
        };
        o.payload.push_str(text);
        let (id, index) = (o.id.clone(), o.index);
        self.frame(
            "response.output_text.delta",
            json!({"item_id":id,"output_index":index,"content_index":0,"delta":text,"logprobs":[]}),
        )
    }

    fn delta_thinking(&mut self, text: &str) -> String {
        let Some(o) = self.open.as_mut() else {
            return String::new();
        };
        o.payload.push_str(text);
        let (id, index) = (o.id.clone(), o.index);
        self.frame(
            "response.reasoning_summary_text.delta",
            json!({"item_id":id,"output_index":index,"summary_index":0,"delta":text}),
        )
    }

    fn delta_tool(&mut self, partial_json: &str) -> String {
        let Some(o) = self.open.as_mut() else {
            return String::new();
        };
        o.payload.push_str(partial_json);
        let (id, index) = (o.id.clone(), o.index);
        if self.freeform.contains(&o.name) {
            self.frame(
                "response.custom_tool_call_input.delta",
                json!({"item_id":id,"output_index":index,"delta":partial_json}),
            )
        } else {
            self.frame(
                "response.function_call_arguments.delta",
                json!({"item_id":id,"output_index":index,"delta":partial_json}),
            )
        }
    }

    /// 关掉当前 output item：先把 `.done` 帧发完，再把完整 item 记进 `output`。
    fn close_item(&mut self) -> String {
        let Some(o) = self.open.take() else {
            return String::new();
        };
        let Open {
            index,
            kind,
            id,
            name,
            payload,
        } = o;
        let freeform = self.freeform.contains(&name);
        let mut out = String::new();
        let item = match kind {
            BlockKind::Text => {
                out.push_str(&self.frame(
                    "response.output_text.done",
                    json!({"item_id":id,"output_index":index,"content_index":0,"text":payload}),
                ));
                out.push_str(&self.frame(
                    "response.content_part.done",
                    json!({"item_id":id,"output_index":index,"content_index":0,
                           "part":{"type":"output_text","text":payload,"annotations":[]}}),
                ));
                message_item(&id, &payload)
            }
            BlockKind::Thinking => {
                out.push_str(&self.frame(
                    "response.reasoning_summary_text.done",
                    json!({"item_id":id,"output_index":index,"summary_index":0,"text":payload}),
                ));
                reasoning_item(&id, &payload)
            }
            _ if freeform => {
                out.push_str(&self.frame(
                    "response.custom_tool_call_input.done",
                    json!({"item_id":id,"output_index":index,"input":payload}),
                ));
                tool_item(&id, &name, &payload, true)
            }
            _ => {
                out.push_str(&self.frame(
                    "response.function_call_arguments.done",
                    json!({"item_id":id,"output_index":index,"arguments":payload}),
                ));
                tool_item(&id, &name, &payload, false)
            }
        };
        out.push_str(&self.frame(
            "response.output_item.done",
            json!({"output_index":index,"item":item}),
        ));
        self.items.push(item);
        out
    }

    fn finish(&mut self) -> String {
        if self.done {
            return String::new();
        }
        self.done = true;
        let mut out = self.close_item();
        let usage = usage_json(self.usage_in, self.usage_out, self.cache_read);
        let base = response_object(&self.id, &self.model, "completed", json!(self.items), usage);
        out.push_str(&self.frame("response.completed", json!({"response": base})));
        out
    }
}

// ---------- 非流式：IR 事件 -> 完整 response ----------

#[derive(Default)]
struct AsmBlock {
    kind: Option<BlockKind>,
    text: String,
    thinking: String,
    payload: String,
    id: Option<String>,
    name: Option<String>,
}

/// 非流式 `/v1/responses`（`stream:false`）：与 `ResponsesEncoder` 同一套块语义与 usage 取值，
/// 区别只是不发 SSE —— 攒到 `response()` 一次性给出完整对象。
#[derive(Default)]
pub struct ResponsesAssembler {
    id: Option<String>,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    ended: bool,
    error: Option<(String, String)>,
    blocks: Vec<AsmBlock>,
    freeform: Vec<String>,
}

impl ResponsesAssembler {
    pub fn new(freeform: Vec<String>) -> Self {
        Self {
            freeform,
            ..Default::default()
        }
    }

    pub fn push(&mut self, ev: IrEvent) {
        match ev {
            IrEvent::Start { id, input_tokens } => {
                self.id = id.or(self.id.take());
                self.usage_in = input_tokens.or(self.usage_in);
            }
            IrEvent::BlockStart {
                index,
                kind,
                id,
                name,
            } => {
                let b = self.slot(index);
                b.kind = Some(kind);
                b.id = id;
                b.name = name;
            }
            IrEvent::TextDelta { index, text } => self.slot(index).text.push_str(&text),
            IrEvent::ThinkingDelta { index, text } => self.slot(index).thinking.push_str(&text),
            IrEvent::SignatureDelta { .. } => {}
            IrEvent::ToolInputDelta {
                index,
                partial_json,
            } => self.slot(index).payload.push_str(&partial_json),
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
            IrEvent::Error { kind, message } => {
                if self.error.is_none() {
                    self.error = Some((kind, message));
                }
            }
            IrEvent::Eos => self.ended = true,
            IrEvent::Stop { .. } | IrEvent::Ping | IrEvent::BlockStop { .. } => {}
        }
    }

    fn slot(&mut self, index: u32) -> &mut AsmBlock {
        while self.blocks.len() <= index as usize {
            self.blocks.push(AsmBlock::default());
        }
        &mut self.blocks[index as usize]
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn error(&self) -> Option<(String, String)> {
        self.error.clone()
    }

    pub fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>) {
        (self.usage_in, self.usage_out, self.cache_read)
    }

    pub fn response(&self, model: &str) -> Value {
        let mut output = Vec::new();
        for b in &self.blocks {
            let id =
                b.id.clone()
                    .unwrap_or_else(|| format!("out_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed)));
            match b.kind {
                Some(BlockKind::Text) => output.push(message_item(&id, &b.text)),
                Some(BlockKind::Thinking) => output.push(reasoning_item(&id, &b.thinking)),
                Some(BlockKind::ToolUse) => {
                    let name = b.name.clone().unwrap_or_default();
                    output.push(tool_item(
                        &id,
                        &name,
                        &b.payload,
                        self.freeform.contains(&name),
                    ));
                }
                _ => {}
            }
        }
        let id = self.id.clone().unwrap_or_else(new_response_id);
        response_object(
            &id,
            model,
            "completed",
            json!(output),
            usage_json(self.usage_in, self.usage_out, self.cache_read),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真的 Codex 请求形状（票 25 实测 + 票 32 复录）的缩样：instructions + 回放输入 +
    /// function 工具 + 自由格式工具 + 推理档位。
    fn codex_body() -> Value {
        json!({
            "model": "claude-haiku",
            "instructions": "You are Codex.",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"列出当前目录"}]},
                {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"gAAAAA"},
                {"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}","call_id":"call_1"},
                {"type":"function_call_output","call_id":"call_1","output":"a.txt"},
                {"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** End Patch","call_id":"call_2"},
                {"type":"custom_tool_call_output","call_id":"call_2","output":"Done!"},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"再做一次"}]}
            ],
            "tools": [
                {"type":"function","name":"shell","description":"跑命令","parameters":{"type":"object","properties":{"command":{"type":"array"}}}},
                {"type":"custom","name":"apply_patch","description":"改文件","format":{"type":"grammar","syntax":"lark"}}
            ],
            "tool_choice":"auto","parallel_tool_calls":false,"store":false,"stream":true,
            "include":["reasoning.encrypted_content"],"reasoning":{"effort":"low"}
        })
    }

    #[test]
    fn decodes_codex_request_into_ir() {
        let ir = decode_request(&codex_body()).unwrap();
        assert_eq!(ir.model, "claude-haiku");
        assert_eq!(ir.system.len(), 1);
        // reasoning 项丢掉，call/output 合并进相邻轮次。
        let roles: Vec<&str> = ir.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["user", "assistant", "user", "assistant", "user"]
        );
        assert_eq!(ir.freeform_tools, vec!["apply_patch".to_string()]);
        assert_eq!(ir.tools.len(), 2);
        // 自由格式工具对上游声明成「一个字符串参数」。
        assert_eq!(ir.tools[1]["input_schema"]["required"][0], "input");
        // Responses 专有字段一个都不许出站。
        assert!(ir.extra.is_empty());
        assert_eq!(ir.max_tokens, 4096);
        match &ir.messages[1].blocks[0] {
            IrBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "shell");
                assert_eq!(input["command"][0], "ls");
            }
            other => panic!("期望 tool_use，得到 {other:?}"),
        }
        // 自由格式调用按约定包成 {"input": 原文}。
        match &ir.messages[3].blocks[0] {
            IrBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "apply_patch");
                assert!(input["input"].as_str().unwrap().contains("Begin Patch"));
            }
            other => panic!("期望 tool_use，得到 {other:?}"),
        }
    }

    fn text_events() -> Vec<IrEvent> {
        vec![
            IrEvent::Start {
                id: None,
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
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            IrEvent::Eos,
        ]
    }

    #[test]
    fn stream_ends_with_response_completed() {
        let mut e = ResponsesEncoder::new("claude-haiku".into(), Vec::new());
        let mut out = String::new();
        for ev in text_events() {
            out.push_str(&e.push(ev));
        }
        let types: Vec<&str> = out
            .lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect();
        assert_eq!(types.first().copied(), Some("response.created"));
        assert_eq!(types.last().copied(), Some("response.completed"));
        assert!(out.contains("\"delta\":\"你好\""));
        // 终态对象里必须带上完整的 output 与 usage。
        let last = out
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .next_back()
            .unwrap();
        let v: Value = serde_json::from_str(last).unwrap();
        assert_eq!(v["type"], "response.completed");
        assert_eq!(v["response"]["status"], "completed");
        assert_eq!(v["response"]["output"][0]["content"][0]["text"], "你好");
        assert_eq!(v["response"]["usage"]["input_tokens"], 11);
        assert_eq!(v["response"]["usage"]["total_tokens"], 13);
        assert_eq!(v["response"]["model"], "claude-haiku");
    }

    #[test]
    fn freeform_tool_call_comes_back_as_custom_tool_call() {
        let mut e = ResponsesEncoder::new("claude-haiku".into(), vec!["apply_patch".into()]);
        let mut out = String::new();
        for ev in [
            IrEvent::Start {
                id: None,
                input_tokens: None,
            },
            IrEvent::BlockStart {
                index: 0,
                kind: BlockKind::ToolUse,
                id: Some("toolu_1".into()),
                name: Some("apply_patch".into()),
            },
            IrEvent::ToolInputDelta {
                index: 0,
                partial_json: "*** Begin Patch".into(),
            },
            IrEvent::BlockStop { index: 0 },
            IrEvent::Eos,
        ] {
            out.push_str(&e.push(ev));
        }
        assert!(out.contains("response.custom_tool_call_input.delta"));
        assert!(out.contains("\"type\":\"custom_tool_call\""));
        // call_id 与上游 tool_use id 一致，客户端才能配对 function_call_output。
        assert!(out.contains("\"call_id\":\"toolu_1\""));
        assert!(!out.contains("function_call"));
    }

    #[test]
    fn plain_tool_call_stays_function_call() {
        let mut e = ResponsesEncoder::new("claude-haiku".into(), Vec::new());
        let mut out = String::new();
        for ev in [
            IrEvent::Start {
                id: None,
                input_tokens: None,
            },
            IrEvent::BlockStart {
                index: 0,
                kind: BlockKind::ToolUse,
                id: Some("call_9".into()),
                name: Some("shell".into()),
            },
            IrEvent::ToolInputDelta {
                index: 0,
                partial_json: "{\"command\":[\"ls\"]}".into(),
            },
            IrEvent::BlockStop { index: 0 },
            IrEvent::Eos,
        ] {
            out.push_str(&e.push(ev));
        }
        assert!(out.contains("response.function_call_arguments.delta"));
        assert!(out.contains("\"call_id\":\"call_9\""));
        assert!(!out.contains("custom_tool_call"));
    }

    #[test]
    fn assembler_matches_non_streaming_contract() {
        let mut a = ResponsesAssembler::new(vec!["apply_patch".into()]);
        for ev in [
            IrEvent::Start {
                id: Some("resp_1".into()),
                input_tokens: Some(3),
            },
            IrEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text,
                id: None,
                name: None,
            },
            IrEvent::TextDelta {
                index: 0,
                text: "好".into(),
            },
            IrEvent::BlockStop { index: 0 },
            IrEvent::BlockStart {
                index: 1,
                kind: BlockKind::ToolUse,
                id: Some("call_2".into()),
                name: Some("apply_patch".into()),
            },
            IrEvent::ToolInputDelta {
                index: 1,
                partial_json: "*** End Patch".into(),
            },
            IrEvent::BlockStop { index: 1 },
            IrEvent::Usage {
                input_tokens: Some(3),
                output_tokens: Some(5),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
            IrEvent::Eos,
        ] {
            a.push(ev);
        }
        assert!(a.ended() && a.error().is_none());
        let r = a.response("claude-haiku");
        assert_eq!(r["id"], "resp_1");
        assert_eq!(r["status"], "completed");
        assert_eq!(r["output"][0]["type"], "message");
        assert_eq!(r["output"][1]["type"], "custom_tool_call");
        assert_eq!(r["output"][1]["input"], "*** End Patch");
        assert_eq!(r["usage"]["output_tokens"], 5);
    }

    /// 上游在终态前断流：非流式路径必须能看出「没结束」，让调用方回 502 而不是假成功。
    #[test]
    fn assembler_reports_incomplete_stream() {
        let mut a = ResponsesAssembler::new(Vec::new());
        a.push(IrEvent::TextDelta {
            index: 0,
            text: "半句".into(),
        });
        assert!(!a.ended());
    }
}
