//! Anthropic Messages 编解码：入站请求、出站请求（直通）、出站 SSE 解码、入站 SSE 编码。

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ir::{BlockKind, IrBlock, IrEvent, IrMessage, IrRequest};
use crate::sse;

static SEQ: AtomicU64 = AtomicU64::new(1);

// ---------- 请求：Anthropic JSON -> IR ----------

pub fn decode_request(v: &Value) -> Result<IrRequest> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("请求体不是 JSON 对象"))?;
    let model = obj
        .get("model")
        .and_then(|m| m.as_str())
        .ok_or_else(|| anyhow!("缺少 model"))?
        .to_string();

    let system = match obj.get("system") {
        Some(Value::String(s)) => vec![json!({"type":"text","text":s})],
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };

    let mut messages = Vec::new();
    if let Some(Value::Array(msgs)) = obj.get("messages") {
        for m in msgs {
            let role = m
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user")
                .to_string();
            let blocks = match m.get("content") {
                Some(Value::String(s)) => vec![IrBlock::Text {
                    text: s.clone(),
                    cache_control: None,
                }],
                Some(Value::Array(bs)) => bs.iter().map(decode_block).collect(),
                _ => Vec::new(),
            };
            messages.push(IrMessage { role, blocks });
        }
    }
    if messages.is_empty() {
        return Err(anyhow!("messages 为空"));
    }

    let tools = match obj.get("tools") {
        Some(Value::Array(t)) => t.clone(),
        _ => Vec::new(),
    };

    let stop = match obj.get("stop_sequences") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };

    // 未建模的顶层字段原样带过：出站 Anthropic 直通时不丢字段。
    let known = [
        "model",
        "system",
        "messages",
        "tools",
        "tool_choice",
        "max_tokens",
        "temperature",
        "top_p",
        "stop_sequences",
        "thinking",
        "metadata",
        "stream",
    ];
    let mut extra = Map::new();
    for (k, val) in obj {
        if !known.contains(&k.as_str()) {
            extra.insert(k.clone(), val.clone());
        }
    }

    Ok(IrRequest {
        model,
        system,
        messages,
        tools,
        tool_choice: obj.get("tool_choice").cloned(),
        max_tokens: obj
            .get("max_tokens")
            .and_then(|m| m.as_u64())
            .unwrap_or(4096),
        temperature: obj.get("temperature").and_then(|t| t.as_f64()),
        top_p: obj.get("top_p").and_then(|t| t.as_f64()),
        stop,
        thinking: obj.get("thinking").cloned(),
        metadata: obj.get("metadata").cloned(),
        extra,
        // Anthropic 入站没有自由格式工具这个概念（票 32 只从 Responses 侧引入）。
        freeform_tools: Vec::new(),
    })
}

fn decode_block(b: &Value) -> IrBlock {
    let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let cc = b.get("cache_control").cloned();
    match ty {
        "text" => IrBlock::Text {
            text: b
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            cache_control: cc,
        },
        "image" => IrBlock::Image {
            source: b.get("source").cloned().unwrap_or(Value::Null),
            cache_control: cc,
        },
        "thinking" | "redacted_thinking" => IrBlock::Thinking {
            thinking: b
                .get("thinking")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            signature: b
                .get("signature")
                .and_then(|s| s.as_str())
                .map(str::to_string),
        },
        "tool_use" => IrBlock::ToolUse {
            id: b
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .to_string(),
            name: b
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
            input: b.get("input").cloned().unwrap_or(json!({})),
        },
        "tool_result" => IrBlock::ToolResult {
            tool_use_id: b
                .get("tool_use_id")
                .and_then(|i| i.as_str())
                .unwrap_or_default()
                .to_string(),
            content: b.get("content").cloned().unwrap_or(Value::Null),
            is_error: b.get("is_error").and_then(|e| e.as_bool()),
        },
        _ => IrBlock::Unknown(b.clone()),
    }
}

// ---------- 请求：IR -> Anthropic JSON ----------

pub fn encode_request(ir: &IrRequest, stream: bool) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(ir.model));
    if !ir.system.is_empty() {
        body.insert("system".into(), json!(ir.system));
    }
    let msgs: Vec<Value> = ir
        .messages
        .iter()
        .map(|m| json!({"role": m.role, "content": m.blocks.iter().map(encode_block).collect::<Vec<_>>()}))
        .collect();
    body.insert("messages".into(), json!(msgs));
    if !ir.tools.is_empty() {
        body.insert("tools".into(), json!(ir.tools));
    }
    for (k, v) in [
        ("tool_choice", ir.tool_choice.clone()),
        ("temperature", ir.temperature.map(|t| json!(t))),
        ("top_p", ir.top_p.map(|t| json!(t))),
        ("thinking", ir.thinking.clone()),
        ("metadata", ir.metadata.clone()),
    ] {
        if let Some(v) = v {
            body.insert(k.into(), v);
        }
    }
    if !ir.stop.is_empty() {
        body.insert("stop_sequences".into(), json!(ir.stop));
    }
    body.insert("max_tokens".into(), json!(ir.max_tokens));
    body.insert("stream".into(), json!(stream));
    for (k, v) in &ir.extra {
        body.insert(k.clone(), v.clone());
    }
    Value::Object(body)
}

/// count_tokens 的请求形状与 `/v1/messages` 同源，只去掉官方契约里没有的 `max_tokens` / `stream`。
pub fn encode_count_request(ir: &IrRequest) -> Value {
    let mut v = encode_request(ir, false);
    if let Some(o) = v.as_object_mut() {
        o.remove("max_tokens");
        o.remove("stream");
    }
    v
}

fn encode_block(b: &IrBlock) -> Value {
    match b {
        IrBlock::Text {
            text,
            cache_control,
        } => with_cc(json!({"type":"text","text":text}), cache_control),
        IrBlock::Image {
            source,
            cache_control,
        } => with_cc(json!({"type":"image","source":source}), cache_control),
        IrBlock::Thinking {
            thinking,
            signature,
        } => {
            let mut v = json!({"type":"thinking","thinking":thinking});
            if let Some(s) = signature {
                v["signature"] = json!(s);
            }
            v
        }
        IrBlock::ToolUse { id, name, input } => {
            json!({"type":"tool_use","id":id,"name":name,"input":input})
        }
        IrBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut v = json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content});
            if let Some(e) = is_error {
                v["is_error"] = json!(e);
            }
            v
        }
        IrBlock::Unknown(v) => v.clone(),
    }
}

fn with_cc(mut v: Value, cc: &Option<Value>) -> Value {
    if let Some(cc) = cc {
        v["cache_control"] = cc.clone();
    }
    v
}

// ---------- 流：上游 Anthropic 帧 -> IR 事件 ----------

#[derive(Default)]
pub struct AnthropicDecoder;

impl AnthropicDecoder {
    pub fn decode(&mut self, f: &sse::Frame) -> Vec<IrEvent> {
        let Ok(v) = serde_json::from_str::<Value>(&f.data) else {
            // `event: error` 且非 JSON、或空 keepalive：按 ping 处理，不能中断转发。
            return vec![IrEvent::Ping];
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "message_start" => {
                let m = v.get("message").cloned().unwrap_or(json!({}));
                vec![IrEvent::Start {
                    id: m.get("id").and_then(|i| i.as_str()).map(str::to_string),
                    input_tokens: m.pointer("/usage/input_tokens").and_then(|t| t.as_u64()),
                }]
            }
            "content_block_start" => {
                let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                let cb = v.get("content_block").cloned().unwrap_or(json!({}));
                let kind = match cb.get("type").and_then(|t| t.as_str()).unwrap_or("text") {
                    "thinking" | "redacted_thinking" => BlockKind::Thinking,
                    "tool_use" => BlockKind::ToolUse,
                    "image" => BlockKind::Image,
                    _ => BlockKind::Text,
                };
                vec![IrEvent::BlockStart {
                    index: idx,
                    kind,
                    id: cb.get("id").and_then(|i| i.as_str()).map(str::to_string),
                    name: cb.get("name").and_then(|n| n.as_str()).map(str::to_string),
                }]
            }
            "content_block_delta" => {
                let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                let d = v.get("delta").cloned().unwrap_or(json!({}));
                match d.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => vec![IrEvent::TextDelta {
                        index: idx,
                        text: d
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    "thinking_delta" => vec![IrEvent::ThinkingDelta {
                        index: idx,
                        text: d
                            .get("thinking")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    "signature_delta" => vec![IrEvent::SignatureDelta {
                        index: idx,
                        signature: d
                            .get("signature")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    "input_json_delta" => vec![IrEvent::ToolInputDelta {
                        index: idx,
                        partial_json: d
                            .get("partial_json")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    }],
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => vec![IrEvent::BlockStop {
                index: v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32,
            }],
            "message_delta" => {
                let mut out = Vec::new();
                let u = v.get("usage").cloned().unwrap_or(json!({}));
                if u.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                    out.push(IrEvent::Usage {
                        input_tokens: u.get("input_tokens").and_then(|t| t.as_u64()),
                        output_tokens: u.get("output_tokens").and_then(|t| t.as_u64()),
                        cache_read_input_tokens: u
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64()),
                        cache_creation_input_tokens: u
                            .get("cache_creation_input_tokens")
                            .and_then(|t| t.as_u64()),
                    });
                }
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(|r| r.as_str()) {
                    out.push(IrEvent::Stop {
                        reason: r.to_string(),
                    });
                }
                out
            }
            "message_stop" => vec![IrEvent::Eos],
            "ping" => vec![IrEvent::Ping],
            "error" => vec![IrEvent::Error {
                kind: v
                    .pointer("/error/type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("api_error")
                    .to_string(),
                message: v
                    .pointer("/error/message")
                    .and_then(|t| t.as_str())
                    .unwrap_or("上游返回 error 事件")
                    .to_string(),
            }],
            _ => Vec::new(),
        }
    }
}

// ---------- 非流式：IR 事件 -> 完整 message ----------

/// 非流式响应的组装器（票 26 决定 3）：与 `AnthropicEncoder` 同一套块语义、同一套 usage 取值，
/// 区别只是不发 SSE —— 把 delta 攒到 `message()` 一次性给出。
#[derive(Default)]
pub struct AnthropicAssembler {
    id: Option<String>,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    /// 缓存写（Anthropic 的 `cache_creation_input_tokens`）：只进日志行与计价（票 58）。
    cache_write: Option<u64>,
    ended: bool,
    error: Option<(String, String)>,
    stop_reason: Option<String>,
    blocks: Vec<AsmBlock>,
}

#[derive(Default)]
struct AsmBlock {
    kind: Option<BlockKind>,
    text: String,
    thinking: String,
    signature: Option<String>,
    partial_json: String,
    id: Option<String>,
    name: Option<String>,
}

impl AnthropicAssembler {
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
            IrEvent::SignatureDelta { index, signature } => {
                self.slot(index).signature = Some(signature)
            }
            IrEvent::ToolInputDelta {
                index,
                partial_json,
            } => self.slot(index).partial_json.push_str(&partial_json),
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
            IrEvent::Ping | IrEvent::BlockStop { .. } => {}
        }
    }

    fn slot(&mut self, index: u32) -> &mut AsmBlock {
        while self.blocks.len() <= index as usize {
            self.blocks.push(AsmBlock::default());
        }
        &mut self.blocks[index as usize]
    }

    /// 上游给过终态（`message_stop` / `[DONE]`）才算完整应答。
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// 上游在流里报的错（原样转给客户端，状态码由调用方定）。
    pub fn error(&self) -> Option<(String, String)> {
        self.error.clone()
    }

    /// 四项 usage：`(输入, 输出, 缓存读, 缓存写)`。上游没报的项是 `None`（**不编 0**，票 58）。
    pub fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        (
            self.usage_in,
            self.usage_out,
            self.cache_read,
            self.cache_write,
        )
    }

    pub fn message(&self, model: &str) -> Value {
        let mut content = Vec::new();
        for b in &self.blocks {
            match b.kind {
                Some(BlockKind::Text) => content.push(json!({"type":"text","text":b.text})),
                // 没有 signature 的 thinking 块不能回给客户端：下一轮官方校验会拒。
                Some(BlockKind::Thinking) if b.signature.is_some() => content.push(json!({
                    "type":"thinking",
                    "thinking":b.thinking,
                    "signature":b.signature.clone().unwrap_or_default(),
                })),
                Some(BlockKind::ToolUse) => content.push(json!({
                    "type":"tool_use",
                    "id":b.id.clone().unwrap_or_default(),
                    "name":b.name.clone().unwrap_or_default(),
                    "input":serde_json::from_str::<Value>(&b.partial_json).unwrap_or(json!({})),
                })),
                _ => {}
            }
        }
        let mut usage = Map::new();
        usage.insert("input_tokens".into(), json!(self.usage_in.unwrap_or(0)));
        usage.insert("output_tokens".into(), json!(self.usage_out.unwrap_or(0)));
        if let Some(c) = self.cache_read {
            usage.insert("cache_read_input_tokens".into(), json!(c));
        }
        json!({
            "id": self.id.clone().unwrap_or_else(|| format!("msg_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed))),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": content,
            "stop_reason": self.stop_reason.clone().unwrap_or_else(|| "end_turn".into()),
            "stop_sequence": null,
            "usage": Value::Object(usage),
        })
    }
}

// ---------- 流：IR 事件 -> 发给客户端的 Anthropic SSE ----------

/// usage 与 stop_reason 都**缓冲到 Eos** 才发 message_delta：
/// OpenAI 侧的 usage 在 finish_reason 之后单独一帧，先发会丢 token 计数。
pub struct AnthropicEncoder {
    id: String,
    /// 对客户端暴露的模型名（族名/虚拟模型名），不回显上游真实 ID。
    model: String,
    usage_in: Option<u64>,
    usage_out: Option<u64>,
    cache_read: Option<u64>,
    cache_create: Option<u64>,
    stop_reason: Option<String>,
    started: bool,
    done: bool,
}

impl AnthropicEncoder {
    pub fn new(model: String) -> Self {
        Self {
            id: format!("msg_gw_{:x}", SEQ.fetch_add(1, Ordering::Relaxed)),
            model,
            usage_in: None,
            usage_out: None,
            cache_read: None,
            cache_create: None,
            stop_reason: None,
            started: false,
            done: false,
        }
    }

    /// OpenAI Chat 上游没有 message_start 的对应事件，但客户端协议要求它必须是第一帧，
    /// 所以任何事件先于 Start 到达时补发一个。
    pub fn push(&mut self, ev: IrEvent) -> String {
        let pre = if !self.started && !matches!(ev, IrEvent::Start { .. }) {
            self.write_start(None, None)
        } else {
            String::new()
        };
        pre + &self.push_inner(ev)
    }

    fn write_start(&mut self, id: Option<String>, input_tokens: Option<u64>) -> String {
        if let Some(i) = id {
            self.id = i;
        }
        self.usage_in = input_tokens.or(self.usage_in);
        self.started = true;
        sse::frame(
            "message_start",
            &json!({"type":"message_start","message":{"id":self.id,"type":"message","role":"assistant","model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":self.usage_in.unwrap_or(0),"output_tokens":0}}}).to_string(),
        )
    }

    fn push_inner(&mut self, ev: IrEvent) -> String {
        match ev {
            IrEvent::Start {
                id,
                input_tokens,
            } => self.write_start(id, input_tokens),
            IrEvent::BlockStart {
                index,
                kind,
                id,
                name,
            } => {
                let cb = match kind {
                    BlockKind::Thinking => json!({"type":"thinking","thinking":""}),
                    BlockKind::ToolUse => json!({"type":"tool_use","id":id.unwrap_or_else(|| format!("toolu_gw_{index}")),"name":name.unwrap_or_default(),"input":{}}),
                    _ => json!({"type":"text","text":""}),
                };
                sse::frame(
                    "content_block_start",
                    &json!({"type":"content_block_start","index":index,"content_block":cb}).to_string(),
                )
            }
            IrEvent::TextDelta { index, text } => sse::frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}).to_string(),
            ),
            IrEvent::ThinkingDelta { index, text } => sse::frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":text}}).to_string(),
            ),
            IrEvent::SignatureDelta { index, signature } => sse::frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}}).to_string(),
            ),
            IrEvent::ToolInputDelta { index, partial_json } => sse::frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":partial_json}}).to_string(),
            ),
            IrEvent::BlockStop { index } => sse::frame(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":index}).to_string(),
            ),
            IrEvent::Ping => sse::frame("ping", &json!({"type":"ping"}).to_string()),
            IrEvent::Usage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            } => {
                self.usage_in = input_tokens.or(self.usage_in);
                self.usage_out = output_tokens.or(self.usage_out);
                self.cache_read = cache_read_input_tokens.or(self.cache_read);
                self.cache_create = cache_creation_input_tokens.or(self.cache_create);
                String::new()
            }
            IrEvent::Stop { reason } => {
                self.stop_reason = Some(reason);
                String::new()
            }
            IrEvent::Error { kind, message } => {
                self.done = true;
                sse::frame(
                    "error",
                    &json!({"type":"error","error":{"type":kind,"message":message}}).to_string(),
                )
            }
            IrEvent::Eos => {
                if self.done {
                    return String::new();
                }
                self.done = true;
                let mut usage = Map::new();
                usage.insert("input_tokens".into(), json!(self.usage_in.unwrap_or(0)));
                usage.insert("output_tokens".into(), json!(self.usage_out.unwrap_or(0)));
                if let Some(c) = self.cache_read {
                    usage.insert("cache_read_input_tokens".into(), json!(c));
                }
                if let Some(c) = self.cache_create {
                    usage.insert("cache_creation_input_tokens".into(), json!(c));
                }
                let reason = self.stop_reason.clone().unwrap_or_else(|| "end_turn".into());
                let mut out = sse::frame(
                    "message_delta",
                    &json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":Value::Object(usage)}).to_string(),
                );
                out.push_str(&sse::frame("message_stop", &json!({"type":"message_stop"}).to_string()));
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(a: &mut AnthropicAssembler) {
        a.push(IrEvent::Usage {
            input_tokens: Some(7),
            output_tokens: Some(2),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        });
        a.push(IrEvent::Stop {
            reason: "max_tokens".into(),
        });
        a.push(IrEvent::Eos);
    }

    #[test]
    fn assembles_text_and_usage() {
        let mut a = AnthropicAssembler::default();
        a.push(IrEvent::Start {
            id: Some("msg_1".into()),
            input_tokens: None,
        });
        a.push(IrEvent::BlockStart {
            index: 0,
            kind: BlockKind::Text,
            id: None,
            name: None,
        });
        a.push(IrEvent::TextDelta {
            index: 0,
            text: "你".into(),
        });
        a.push(IrEvent::TextDelta {
            index: 0,
            text: "好".into(),
        });
        a.push(IrEvent::BlockStop { index: 0 });
        finish(&mut a);
        assert!(a.ended());
        let m = a.message("claude-haiku");
        assert_eq!(m["id"], "msg_1");
        assert_eq!(m["model"], "claude-haiku");
        assert_eq!(m["content"][0]["text"], "你好");
        assert_eq!(m["stop_reason"], "max_tokens");
        assert_eq!(m["usage"]["input_tokens"], 7);
        assert_eq!(m["usage"]["output_tokens"], 2);
        assert!(m["usage"].get("cache_read_input_tokens").is_none());
    }

    /// 票 58：Anthropic 侧的缓存写字段，**用真录帧**
    /// （`tests/fixtures/cache-write/anthropic.sse`，2026-10-15 真打 commandcode `/messages`
    /// + `claude-sonnet-5-5`，system 带 `cache_control: ephemeral`）。
    ///
    /// 实测（帧文件就是独立来源）：缓存写报在 `message_delta.usage.cache_creation_input_tokens`
    /// = **3819**，同帧还有嵌套的 `cache_creation.ephemeral_5m_input_tokens: 3819`——**不数嵌套那层**
    /// （顶层那个才是上游给的总数，嵌套是它的拆分）。缓存读同帧 0、输入 9、输出 13。
    #[test]
    fn anthropic_cache_creation_input_tokens_lands_in_cache_write() {
        let mut parser = sse::SseParser::default();
        let mut frames = parser.push(REAL_CACHE_WRITE_ANTHROPIC.as_bytes());
        frames.extend(parser.finish());
        assert!(frames.len() >= 3, "真录帧不该这么少：{}", frames.len());

        let mut asm = AnthropicAssembler::default();
        for f in &frames {
            for ev in AnthropicDecoder.decode(f) {
                asm.push(ev);
            }
        }
        assert!(asm.ended(), "真录帧到 message_stop，必须算完整应答");
        assert_eq!(
            asm.usage(),
            (Some(9), Some(13), Some(0), Some(3819)),
            "(输入, 输出, 缓存读, 缓存写)"
        );
    }

    /// 真录帧的字节（录制工具 `tests/record.ps1` 原样落盘；只读、不许改）。
    const REAL_CACHE_WRITE_ANTHROPIC: &str =
        include_str!("../tests/fixtures/cache-write/anthropic.sse");

    #[test]
    fn thinking_without_signature_is_dropped() {
        let mut a = AnthropicAssembler::default();
        for (index, sig) in [(0u32, None), (1, Some("sig-1"))] {
            a.push(IrEvent::BlockStart {
                index,
                kind: BlockKind::Thinking,
                id: None,
                name: None,
            });
            a.push(IrEvent::ThinkingDelta {
                index,
                text: format!("想{index}"),
            });
            if let Some(s) = sig {
                a.push(IrEvent::SignatureDelta {
                    index,
                    signature: s.into(),
                });
            }
        }
        finish(&mut a);
        let m = a.message("claude-haiku");
        assert_eq!(m["content"].as_array().unwrap().len(), 1);
        assert_eq!(m["content"][0]["signature"], "sig-1");
    }

    #[test]
    fn tool_input_is_parsed_from_partial_json() {
        let mut a = AnthropicAssembler::default();
        a.push(IrEvent::BlockStart {
            index: 0,
            kind: BlockKind::ToolUse,
            id: Some("toolu_1".into()),
            name: Some("read".into()),
        });
        a.push(IrEvent::ToolInputDelta {
            index: 0,
            partial_json: "{\"path\":".into(),
        });
        a.push(IrEvent::ToolInputDelta {
            index: 0,
            partial_json: "\"a.txt\"}".into(),
        });
        finish(&mut a);
        let m = a.message("claude-haiku");
        assert_eq!(m["content"][0]["input"]["path"], "a.txt");
    }

    #[test]
    fn upstream_error_and_missing_eos_are_visible() {
        let mut a = AnthropicAssembler::default();
        a.push(IrEvent::Error {
            kind: "upstream_abort".into(),
            message: "断了".into(),
        });
        assert!(!a.ended());
        assert_eq!(a.error().unwrap().0, "upstream_abort");
    }

    #[test]
    fn count_request_drops_max_tokens_and_stream() {
        let ir = decode_request(&json!({
            "model": "claude-haiku",
            "max_tokens": 1,
            "stream": false,
            "system": "sys",
            "messages": [{"role": "user", "content": "."}],
        }))
        .unwrap();
        let v = encode_count_request(&ir);
        assert!(v.get("max_tokens").is_none());
        assert!(v.get("stream").is_none());
        assert_eq!(v["system"][0]["text"], "sys");
        assert_eq!(v["messages"][0]["content"][0]["text"], ".");
    }
}
