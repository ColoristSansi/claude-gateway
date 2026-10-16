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

/// 工具入参的两种形状（字符串 / 对象）都吃得下：Ollama 入站（`ingress::ollama`）也复用它。
pub(crate) fn parse_arguments(a: Option<&Value>) -> Value {
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

/// 同 role 的相邻块并进同一轮：Ollama 的 `role:"tool"` 连续多条必须并成一个 user 轮，
/// 否则 Anthropic 出站会把每个 `tool_result` 拆到不同的消息里（配对失败）。
pub(crate) fn push_block(msgs: &mut Vec<IrMessage>, role: &str, b: IrBlock) {
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

// ---------- 请求：IR -> 上游 OpenAI Responses JSON（票 65 的出站半边） ----------

/// IR → 上游 `/responses` 的请求体（spec §4.2 / §6）。
///
/// 与 [`decode_request`] **方向相反**：那是 Codex 打进来的形状，这是发给上游的形状。
/// 系统提示走**顶层 `instructions`**（上游 Responses 没有 system 消息角色）；
/// 客户端那半边的 `input[].role:"developer"`（票 32 实测的 Codex 形状）不是这里的事。
///
/// 有损项（IR 表达得了、Responses 没有对应字段，**不编不猜**）：`stop`（上游没有 stop 参数）、
/// `thinking`（拿不到可验证的 `encrypted_content`，伪造加密推理只会骗自己）、
/// `IrBlock::Unknown`（形状不认识就不猜）。结构化输出**不在这里**：由
/// [`crate::egress::structured::apply`] 按票 57 的三层形状落 `text.format`
/// （`{name, schema, strict}`，`name` 常量 `"response"`），所以这里一个字都不写 `text`。
pub fn encode_request(ir: &IrRequest, stream: bool) -> Value {
    let instructions: String = ir
        .system
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");

    let mut input: Vec<Value> = Vec::new();
    for m in &ir.messages {
        let role = if m.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let mut parts: Vec<Value> = Vec::new();
        for b in &m.blocks {
            match b {
                IrBlock::Text { text, .. } => parts.push(text_part(role, text)),
                IrBlock::Image { source, .. } => parts.push(json!({
                    "type":"input_image",
                    "image_url": image_url(source),
                })),
                // 工具调用与结果在 Responses 里是**独立 item**，不能塞进 message 的 content：
                // 先收尾当前 message，再按原顺序追加，配对键一路都是 `call_id`。
                IrBlock::ToolUse {
                    id,
                    name,
                    input: args,
                } => {
                    flush_message(&mut parts, role, &mut input);
                    input.push(json!({
                        "type":"function_call",
                        "call_id":id,
                        "name":name,
                        // Responses 的 `arguments` 是**字符串**（与 Chat 同形，不是对象）。
                        "arguments":args.to_string(),
                    }));
                }
                IrBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    flush_message(&mut parts, role, &mut input);
                    let mut out = crate::openai::tool_result_text(content);
                    if is_error == &Some(true) {
                        out = format!("[tool_error] {out}");
                    }
                    input.push(json!({
                        "type":"function_call_output",
                        "call_id":tool_use_id,
                        "output":out,
                    }));
                }
                IrBlock::Thinking { .. } | IrBlock::Unknown(_) => {}
            }
        }
        flush_message(&mut parts, role, &mut input);
    }

    let mut body = json!({
        "model": ir.model,
        "input": input,
        "max_output_tokens": ir.max_tokens,
        "stream": stream,
        // 续轮只做输入回放（spec §4.2：不实现 `previous_response_id`），所以别让上游留这份会话。
        "store": false,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if !ir.tools.is_empty() {
        body["tools"] = json!(ir
            .tools
            .iter()
            .map(|t| {
                json!({"type":"function",
                    "name": t.get("name").cloned().unwrap_or(Value::Null),
                    "description": t.get("description").cloned().unwrap_or(json!("")),
                    // IR 存的是 Anthropic 形状的 `input_schema`，Responses 叫 `parameters`。
                    "parameters": t.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))
                })
            })
            .collect::<Vec<_>>());
    }
    if let Some(tc) = &ir.tool_choice {
        body["tool_choice"] = match tc.get("type").and_then(|t| t.as_str()).unwrap_or("auto") {
            "any" => json!("required"),
            "tool" => {
                json!({"type":"function","name":tc.get("name").cloned().unwrap_or(Value::Null)})
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
    body
}

/// 一端文本零件：assistant 轮是 `output_text`，其余是 `input_text`。
fn text_part(role: &str, text: &str) -> Value {
    let ty = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    json!({"type": ty, "text": text})
}

/// 把攒下的零件收成一个 message item（没有零件就不发这一轮）。
fn flush_message(parts: &mut Vec<Value>, role: &str, input: &mut Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    input.push(json!({"role": role, "content": Value::Array(std::mem::take(parts))}));
}

/// IR 的 image source → Responses 的 `image_url`：base64 → `data:` URL，`url` 形状直接带过去。
fn image_url(source: &Value) -> String {
    match source.get("url").and_then(Value::as_str) {
        Some(u) => u.to_string(),
        None => crate::openai::data_url(source),
    }
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
    /// 缓存写（票 58）：入站是 Responses 的客户端走 Anthropic 上游时，这一项也得进日志行。
    cache_write: Option<u64>,
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
                cache_creation_input_tokens,
            } => {
                self.usage_in = input_tokens.or(self.usage_in);
                self.usage_out = output_tokens.or(self.usage_out);
                self.cache_read = cache_read_input_tokens.or(self.cache_read);
                self.cache_write = cache_creation_input_tokens.or(self.cache_write);
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

    /// 四项 usage：`(输入, 输出, 缓存读, 缓存写)`（票 58）；上游没报的是 `None`。
    pub fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        (
            self.usage_in,
            self.usage_out,
            self.cache_read,
            self.cache_write,
        )
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

// ---------- 流：上游 Responses SSE -> IR 事件（票 65 的出站半边） ----------

/// 上游 `/responses` 的 SSE → IR 事件。
///
/// 上游的 `output_index` 只是它自己的序号：IR 的块序号由本解码器**顺序分配**
/// （与 [`crate::openai::OpenAiDecoder`] 同一套做法），客户端面才拿得到连续的 0,1,2…
///
/// shortcut: `response.output_text.done` / `response.content_part.done` 这类「整段回执」帧
/// **只当收尾用、不取里面的正文** —— 真流式上游的正文都在 `.delta` 帧里；碰到只发 `.done`
/// 不发 `.delta` 的上游再补「没收到 delta 才取 done 正文」。
#[derive(Default)]
pub struct ResponsesDecoder {
    next_index: u32,
    /// 上游 `output_index` → IR 块序号。
    items: Vec<(u64, u32)>,
    open: Option<(u32, BlockKind)>,
    /// 这一发里出现过工具调用：终态的 stop_reason 要报 `tool_use`（客户端靠它决定走工具）。
    saw_tool_use: bool,
    id: Option<String>,
    started: bool,
}

impl ResponsesDecoder {
    fn close_open(&mut self, out: &mut Vec<IrEvent>) {
        if let Some((idx, _)) = self.open.take() {
            out.push(IrEvent::BlockStop { index: idx });
        }
    }

    /// 开流只报一次（`response.created` 与任何更早到达的帧都可能触发它）。
    fn start(&mut self, id: Option<String>, out: &mut Vec<IrEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        self.id = id.or(self.id.take());
        out.push(IrEvent::Start {
            id: self.id.clone(),
            input_tokens: None,
        });
    }

    /// 上游给的 `output_index` → 顺序分配的 IR 块序号。
    fn index_of(&mut self, up_index: u64) -> u32 {
        if let Some((_, i)) = self.items.iter().find(|(o, _)| *o == up_index) {
            return *i;
        }
        let idx = self.next_index;
        self.next_index += 1;
        self.items.push((up_index, idx));
        idx
    }

    fn open_block(
        &mut self,
        up_index: u64,
        kind: BlockKind,
        id: Option<String>,
        name: Option<String>,
        out: &mut Vec<IrEvent>,
    ) -> u32 {
        self.start(None, out);
        self.close_open(out);
        let idx = self.index_of(up_index);
        if kind == BlockKind::ToolUse {
            self.saw_tool_use = true;
        }
        self.open = Some((idx, kind));
        out.push(IrEvent::BlockStart {
            index: idx,
            kind,
            id,
            name,
        });
        idx
    }

    /// 已经开着同类块就用它；否则开一个（上游漏发 `output_item.added` 时的兜底）。
    fn ensure(&mut self, up_index: u64, kind: BlockKind, out: &mut Vec<IrEvent>) -> u32 {
        if let Some((idx, k)) = self.open {
            if k == kind {
                return idx;
            }
        }
        self.open_block(up_index, kind, None, None, out)
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
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        // 上游把错误也发成帧：形状是 `{"type":"error","error":{…}}`（有的只在事件名上标 error）。
        if ty == "error" || f.event.as_deref() == Some("error") {
            out.push(ir_error(v.get("error").unwrap_or(&Value::Null)));
            return out;
        }

        // 每帧都可能带 `output_index`，缺了就按 0 兜底。
        let up_index = v.get("output_index").and_then(Value::as_u64).unwrap_or(0);
        match ty {
            "response.created" | "response.in_progress" => {
                self.start(response_id(&v), &mut out);
            }
            "response.output_item.added" => {
                let item = v.get("item").cloned().unwrap_or(json!({}));
                match item.get("type").and_then(Value::as_str).unwrap_or("") {
                    "message" => {
                        self.open_block(
                            up_index,
                            BlockKind::Text,
                            item.get("id").and_then(Value::as_str).map(str::to_string),
                            None,
                            &mut out,
                        );
                    }
                    // 推理项**不在这里开块**：真录帧（commandcode `/responses`）的 reasoning 项
                    // 只有 `encrypted_content`、`summary` 是空的，先开块就等于给客户端一个
                    // 空 thinking 块（没有 signature，回放时还可能被上游拒）。有 summary 正文时
                    // 由下面那条 `.delta` 懒开块，没有就整块不出现。
                    "reasoning" => {}
                    // 配对键是 `call_id`（`function_call_output` 回的就是它）。
                    "function_call" | "custom_tool_call" => {
                        let id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                        self.open_block(
                            up_index,
                            BlockKind::ToolUse,
                            id,
                            item.get("name").and_then(Value::as_str).map(str::to_string),
                            &mut out,
                        );
                    }
                    _ => {}
                }
            }
            "response.output_item.done" => self.close_open(&mut out),
            "response.output_text.delta" => {
                let idx = self.ensure(up_index, BlockKind::Text, &mut out);
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    if !d.is_empty() {
                        out.push(IrEvent::TextDelta {
                            index: idx,
                            text: d.to_string(),
                        });
                    }
                }
            }
            // 推理内容：汇总（`summary_text`）与原文（`reasoning_text`）都吃，都进 thinking 块。
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let idx = self.ensure(up_index, BlockKind::Thinking, &mut out);
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    if !d.is_empty() {
                        out.push(IrEvent::ThinkingDelta {
                            index: idx,
                            text: d.to_string(),
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let idx = self.ensure(up_index, BlockKind::ToolUse, &mut out);
                if let Some(d) = v.get("delta").and_then(Value::as_str) {
                    if !d.is_empty() {
                        out.push(IrEvent::ToolInputDelta {
                            index: idx,
                            partial_json: d.to_string(),
                        });
                    }
                }
            }
            "response.completed" => {
                let resp = v.get("response").cloned().unwrap_or(json!({}));
                self.start(
                    resp.get("id").and_then(Value::as_str).map(str::to_string),
                    &mut out,
                );
                push_usage(&resp, &mut out);
                self.close_open(&mut out);
                out.push(IrEvent::Stop {
                    reason: if self.saw_tool_use {
                        "tool_use"
                    } else {
                        "end_turn"
                    }
                    .into(),
                });
                out.push(IrEvent::Eos);
            }
            // 撞上限（`incomplete_details.reason = "max_output_tokens"`）：照样是完整的一发，
            // 只是 stop_reason 要报 `max_tokens`，让客户端知道是截断而不是答完。
            "response.incomplete" => {
                let resp = v.get("response").cloned().unwrap_or(json!({}));
                self.start(
                    resp.get("id").and_then(Value::as_str).map(str::to_string),
                    &mut out,
                );
                push_usage(&resp, &mut out);
                self.close_open(&mut out);
                out.push(IrEvent::Stop {
                    reason: "max_tokens".into(),
                });
                out.push(IrEvent::Eos);
            }
            "response.failed" => {
                out.push(ir_error(
                    v.pointer("/response/error").unwrap_or(&Value::Null),
                ));
            }
            // 其余（`content_part.added` / `.done` / 各种 `.done` 回执）不改 IR 状态。
            _ => {}
        }
        out
    }
}

fn response_id(v: &Value) -> Option<String> {
    v.pointer("/response/id")
        .or_else(|| v.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Responses 的 usage → IR（`input_tokens_details.cached_tokens` 就是缓存读；
/// 上游词表里**没有**缓存写这一项，不编 0）。
fn push_usage(resp: &Value, out: &mut Vec<IrEvent>) {
    let Some(u) = resp.get("usage").filter(|u| u.is_object()) else {
        return;
    };
    out.push(IrEvent::Usage {
        input_tokens: u.get("input_tokens").and_then(Value::as_u64),
        output_tokens: u.get("output_tokens").and_then(Value::as_u64),
        cache_read_input_tokens: u
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64),
        cache_creation_input_tokens: None,
    });
}

/// 上游错误体 → IR 的错误事件（`code` 缺省时退 `type`，再缺省退 `api_error`）。
fn ir_error(e: &Value) -> IrEvent {
    IrEvent::Error {
        kind: e
            .get("code")
            .or_else(|| e.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("api_error")
            .to_string(),
        message: e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("上游返回 error 帧")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ResponseFormat;

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

    // ───────────── 票 65：出站方向（IR → 上游 `/responses`、上游 SSE → IR） ─────────────

    /// 一件 Anthropic 入站的请求（**独立来源**：Anthropic 公开形状）解成 IR，喂给出站编码。
    fn anthropic_ir_with_tools() -> IrRequest {
        crate::anthropic::decode_request(&json!({
            "model":"mock-text",
            "max_tokens":77,
            "temperature":0.25,
            "top_p":0.9,
            "stop_sequences":["X"],
            "system":"你是助手",
            "messages":[
                {"role":"user","content":[
                    {"type":"text","text":"看图"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA"}}
                ]},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"想一下","signature":"sig"},
                    {"type":"tool_use","id":"toolu_1","name":"f","input":{"a":1}}
                ]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":true}]}
            ],
            "tools":[{"name":"f","description":"d","input_schema":{"type":"object"}}],
            "tool_choice":{"type":"tool","name":"f"}
        }))
        .unwrap()
    }

    /// IR → 上游 Responses 请求体：**系统提示走顶层 `instructions`**（不是 Codex 入站那套
    /// `input[].role:"developer"`）、工具调用与结果成对、图片编成 `input_image` 的 data URL。
    /// 逐字断言（对象比较不看键序，但多一个键就算不一致）——顺带钉住「有损项不出站」：
    /// `stop` / `thinking` 在 Responses 面上没有对应字段。
    #[test]
    fn encodes_ir_into_an_upstream_responses_request() {
        let ir0 = anthropic_ir_with_tools();
        let body = encode_request(&ir0, true);
        assert_eq!(
            body,
            json!({
                "model":"mock-text",
                "instructions":"你是助手",
                "input":[
                    {"role":"user","content":[
                        {"type":"input_text","text":"看图"},
                        {"type":"input_image","image_url":"data:image/png;base64,AA"}
                    ]},
                    {"type":"function_call","call_id":"toolu_1","name":"f","arguments":"{\"a\":1}"},
                    {"type":"function_call_output","call_id":"toolu_1","output":"[tool_error] ok"}
                ],
                "tools":[{"type":"function","name":"f","description":"d","parameters":{"type":"object"}}],
                "tool_choice":{"type":"function","name":"f"},
                "max_output_tokens":77,
                "temperature":0.25,
                "top_p":0.9,
                "stream":true,
                "store":false
            })
        );
    }

    /// `stream:false` 时出站**仍然**是流式（票 26 决定 3：非流式只是把流缓到终态再组装）。
    #[test]
    fn outbound_responses_request_keeps_the_stream_flag_it_is_given() {
        let mut ir = anthropic_ir_with_tools();
        ir.messages.truncate(1);
        assert_eq!(encode_request(&ir, false)["stream"], false);
    }

    /// 票 57 的三层 `text.format` 与出站编码**拼在一起**逐字一条：`text` 是
    /// `{format:{type,name,schema,strict}}`（`name` 常量 `"response"`，`strict` 只透传显式值），
    /// 且出站编码自己**不写** `text`（免得把结构化输出字段吃掉）。
    #[test]
    fn text_format_keeps_the_three_layer_shape_verbatim() {
        use crate::egress::provider::{StructuredOutput, Wire};
        let mut ir = anthropic_ir_with_tools();
        ir.format = Some(ResponseFormat::JsonSchema {
            schema: json!({
                "type":"object",
                "properties":{"a":{"type":"string"}},
                "required":["a"]
            }),
            name: None,
            strict: Some(true),
        });
        let mut body = encode_request(&ir, true);
        assert!(body.get("text").is_none(), "编码器自己不该写 text：{body}");
        crate::egress::structured::apply(
            &mut body,
            ir.format.as_ref(),
            Wire::OpenAiResponses,
            StructuredOutput::Native,
            Some(Wire::OpenAiResponses),
        )
        .unwrap();
        assert_eq!(
            body["text"],
            json!({"format":{
                "type":"json_schema",
                "name":"response",
                "schema":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]},
                "strict":true
            }})
        );
    }

    /// 事件 → 一行紧凑文本（断言用；比一堆 `matches!` 好读，也能看清**顺序**）。
    fn render(ev: &IrEvent) -> String {
        match ev {
            IrEvent::Start { id, input_tokens } => format!("start:{id:?}:{input_tokens:?}"),
            IrEvent::BlockStart {
                index,
                kind,
                id,
                name,
            } => format!("block_start:{index}:{kind:?}:{id:?}:{name:?}"),
            IrEvent::TextDelta { index, text } => format!("text:{index}:{text}"),
            IrEvent::ThinkingDelta { index, text } => format!("thinking:{index}:{text}"),
            IrEvent::SignatureDelta { index, signature } => {
                format!("signature:{index}:{signature}")
            }
            IrEvent::ToolInputDelta {
                index,
                partial_json,
            } => format!("tool_input:{index}:{partial_json}"),
            IrEvent::BlockStop { index } => format!("block_stop:{index}"),
            IrEvent::Ping => "ping".into(),
            IrEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => format!(
                "usage:{input_tokens:?}:{output_tokens:?}:{cache_read_input_tokens:?}:{cache_creation_input_tokens:?}"
            ),
            IrEvent::Stop { reason } => format!("stop:{reason}"),
            IrEvent::Error { kind, message } => format!("error:{kind}:{message}"),
            IrEvent::Eos => "eos".into(),
        }
    }

    /// 上游帧用 `sse::frame` 生成（与线上同一套分隔符），再逐帧过解码器。
    fn decode_all(events: &[(&str, Value)]) -> Vec<String> {
        let mut parser = sse::SseParser::default();
        let mut frames = Vec::new();
        for (name, data) in events {
            frames.extend(parser.push(sse::frame(name, &data.to_string()).as_bytes()));
        }
        frames.extend(parser.finish());
        let mut dec = ResponsesDecoder::default();
        let mut out = Vec::new();
        for f in &frames {
            out.extend(dec.decode(f).iter().map(render));
        }
        out
    }

    /// 上游 Responses 帧 → IR：`output_item.added` 开块、`output_text.delta` 逐片、
    /// `output_item.done` 收块、`response.completed` 给 usage 与终态。
    #[test]
    fn decodes_upstream_responses_text_frames_into_ir_events() {
        let got = decode_all(&[
            (
                "response.created",
                json!({"type":"response.created","response":{"id":"resp_up_1","status":"in_progress","output":[]}}),
            ),
            (
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":0,
                       "item":{"type":"message","id":"msg_up_1","role":"assistant","status":"in_progress","content":[]}}),
            ),
            (
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_up_1",
                       "content_index":0,"delta":"你好"}),
            ),
            (
                "response.output_text.delta",
                json!({"type":"response.output_text.delta","output_index":0,"item_id":"msg_up_1",
                       "content_index":0,"delta":"，世界"}),
            ),
            (
                "response.output_item.done",
                json!({"type":"response.output_item.done","output_index":0,
                       "item":{"type":"message","id":"msg_up_1","status":"completed",
                               "content":[{"type":"output_text","text":"你好，世界","annotations":[]}]}}),
            ),
            (
                "response.completed",
                json!({"type":"response.completed","response":{"id":"resp_up_1","status":"completed",
                       "usage":{"input_tokens":11,"input_tokens_details":{"cached_tokens":3},
                                "output_tokens":2,"output_tokens_details":{"reasoning_tokens":0},
                                "total_tokens":13}}}),
            ),
        ]);
        assert_eq!(
            got,
            vec![
                "start:Some(\"resp_up_1\"):None",
                "block_start:0:Text:Some(\"msg_up_1\"):None",
                "text:0:你好",
                "text:0:，世界",
                "block_stop:0",
                "usage:Some(11):Some(2):Some(3):None",
                "stop:end_turn",
                "eos",
            ]
        );
    }

    /// 推理项 + `function_call`：块序号由本层顺序分配（上游的 `output_index` 只是它自己的），
    /// 配对键取 `call_id`，终态 stop_reason 报 `tool_use`。
    ///
    /// 推理块**懒开**：`output_item.added` 那一下不开块（真上游的 reasoning 项 summary 常是空的），
    /// 等 `response.reasoning_summary_text.delta` 来正文时才开 —— 所以这里块 id 是 `None`。
    #[test]
    fn decodes_upstream_reasoning_and_tool_call_items() {
        let got = decode_all(&[
            (
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":0,
                       "item":{"type":"reasoning","id":"rs_up_1","summary":[]}}),
            ),
            (
                "response.reasoning_summary_text.delta",
                json!({"type":"response.reasoning_summary_text.delta","output_index":0,
                       "item_id":"rs_up_1","summary_index":0,"delta":"想一下"}),
            ),
            (
                "response.output_item.done",
                json!({"type":"response.output_item.done","output_index":0,
                       "item":{"type":"reasoning","id":"rs_up_1","summary":[]}}),
            ),
            (
                "response.output_item.added",
                json!({"type":"response.output_item.added","output_index":1,
                       "item":{"type":"function_call","id":"fc_up_1","call_id":"call_up_1",
                               "name":"get_weather","arguments":"","status":"in_progress"}}),
            ),
            (
                "response.function_call_arguments.delta",
                json!({"type":"response.function_call_arguments.delta","output_index":1,
                       "item_id":"fc_up_1","delta":"{\"city\""}),
            ),
            (
                "response.function_call_arguments.delta",
                json!({"type":"response.function_call_arguments.delta","output_index":1,
                       "item_id":"fc_up_1","delta":":\"上海\"}"}),
            ),
            (
                "response.output_item.done",
                json!({"type":"response.output_item.done","output_index":1,
                       "item":{"type":"function_call","id":"fc_up_1","call_id":"call_up_1",
                               "name":"get_weather","arguments":"{\"city\":\"上海\"}","status":"completed"}}),
            ),
            (
                "response.completed",
                json!({"type":"response.completed","response":{"id":"resp_up_2","status":"completed",
                       "usage":{"input_tokens":5,"output_tokens":9}}}),
            ),
        ]);
        assert_eq!(
            got,
            vec![
                "start:None:None",
                "block_start:0:Thinking:None:None",
                "thinking:0:想一下",
                "block_stop:0",
                "block_start:1:ToolUse:Some(\"call_up_1\"):Some(\"get_weather\")",
                "tool_input:1:{\"city\"",
                "tool_input:1::\"上海\"}",
                "block_stop:1",
                "usage:Some(5):Some(9):None:None",
                "stop:tool_use",
                "eos",
            ]
        );
    }

    /// 上游在流里报错（`response.failed`）与 `response.incomplete`（撞上限）各自的终态。
    #[test]
    fn upstream_error_and_incomplete_frames_map_to_ir() {
        let failed = decode_all(&[(
            "response.failed",
            json!({"type":"response.failed","response":{"id":"resp_1","status":"failed",
                   "error":{"code":"server_error","message":"上游炸了"}}}),
        )]);
        assert_eq!(failed, vec!["error:server_error:上游炸了"]);

        let incomplete = decode_all(&[(
            "response.incomplete",
            json!({"type":"response.incomplete","response":{"id":"resp_1","status":"incomplete",
                   "incomplete_details":{"reason":"max_output_tokens"},
                   "usage":{"input_tokens":4,"output_tokens":16}}}),
        )]);
        assert_eq!(
            incomplete,
            vec![
                "start:Some(\"resp_1\"):None",
                "usage:Some(4):Some(16):None:None",
                "stop:max_tokens",
                "eos"
            ]
        );
    }
}
