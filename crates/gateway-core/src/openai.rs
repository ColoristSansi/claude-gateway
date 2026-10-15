//! OpenAI Chat Completions 编解码：IR -> 出站请求、上游 SSE -> IR 事件。
//! 只做 Anthropic 保真能力需要的那部分映射；映射不回来的按「有损」处理并记在 README。

use serde_json::{json, Value};

use crate::ir::{BlockKind, IrBlock, IrEvent, IrRequest};
use crate::sse;

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
                cache_creation_input_tokens: None,
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
