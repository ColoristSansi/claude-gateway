//! 中间表示（IR）：请求与流式事件在网关内部的唯一形状。
//! 只建模四个编解码都要用的部分；没建模的字段进 `extra` / `Unknown` 原样带过，
//! 因为「不认识就丢」正是保真度事故的来源。

use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct IrRequest {
    /// 出站真实模型 ID（路由已完成）。
    pub model: String,
    pub system: Vec<Value>,
    pub messages: Vec<IrMessage>,
    pub tools: Vec<Value>,
    pub tool_choice: Option<Value>,
    pub max_tokens: u64,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    pub thinking: Option<Value>,
    pub metadata: Option<Value>,
    pub extra: Map<String, Value>,
    /// 自由格式工具（Responses 的 `type:"custom"`）的名字：对上游声明成「一个字符串参数」，
    /// 回给客户端时要还原成 `custom_tool_call`（票 32）。
    pub freeform_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct IrMessage {
    pub role: String,
    pub blocks: Vec<IrBlock>,
}

#[derive(Debug, Clone)]
pub enum IrBlock {
    Text {
        text: String,
        cache_control: Option<Value>,
    },
    Image {
        source: Value,
        cache_control: Option<Value>,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
        is_error: Option<bool>,
    },
    Unknown(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Image,
    Thinking,
    ToolUse,
}

/// 一张图折算的 token：Anthropic 的粗算是 `宽×高/750`，但 `source` 里只有 base64，
/// 拿不到像素尺寸（而且 base64 长度与 token 数无关）。用一个常见截图档位的常量。
const IMAGE_TOKENS: u64 = 1500;

/// 本地 token 估算（票 26 决定 2）：system + tools + messages 全算，
/// ASCII 约 4 字符/token，非 ASCII 每字符 1 token（CJK 近似）。
/// 只喂 `count_tokens` 这一个辅助端点：客户端从不校验它的绝对值。
pub fn estimate_tokens(req: &IrRequest) -> u64 {
    let mut n = req.system.iter().map(count_value).sum::<u64>();
    n += req.tools.iter().map(count_value).sum::<u64>();
    for m in &req.messages {
        n += m.blocks.iter().map(count_block).sum::<u64>();
    }
    n.max(1)
}

fn count_block(b: &IrBlock) -> u64 {
    match b {
        IrBlock::Text { text, .. } => count_str(text),
        IrBlock::Thinking { thinking, .. } => count_str(thinking),
        IrBlock::ToolUse { input, .. } => count_value(input),
        IrBlock::ToolResult { content, .. } => count_value(content),
        IrBlock::Image { .. } => IMAGE_TOKENS,
        IrBlock::Unknown(v) => count_value(v),
    }
}

/// base64 载荷（任何名字叫 `data` 的字段）不计入：它跟 token 数没有关系。
fn count_value(v: &Value) -> u64 {
    match v {
        Value::String(s) => count_str(s),
        Value::Array(a) => a.iter().map(count_value).sum(),
        Value::Object(o) => {
            if o.get("type").and_then(|t| t.as_str()) == Some("image") {
                return IMAGE_TOKENS;
            }
            o.iter()
                .filter(|(k, _)| k.as_str() != "data")
                .map(|(_, v)| count_value(v))
                .sum()
        }
        _ => 0,
    }
}

fn count_str(s: &str) -> u64 {
    let (ascii, wide) = s.chars().fold((0u64, 0u64), |(a, w), c| {
        if c.is_ascii() {
            (a + 1, w)
        } else {
            (a, w + 1)
        }
    });
    ascii.div_ceil(4) + wide
}

#[derive(Debug, Clone)]
pub enum IrEvent {
    Start {
        id: Option<String>,
        input_tokens: Option<u64>,
    },
    BlockStart {
        index: u32,
        kind: BlockKind,
        id: Option<String>,
        name: Option<String>,
    },
    TextDelta {
        index: u32,
        text: String,
    },
    ThinkingDelta {
        index: u32,
        text: String,
    },
    SignatureDelta {
        index: u32,
        signature: String,
    },
    ToolInputDelta {
        index: u32,
        partial_json: String,
    },
    BlockStop {
        index: u32,
    },
    Ping,
    Usage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },
    Stop {
        /// 已归一化到 Anthropic 词表：end_turn / max_tokens / tool_use / stop_sequence ...
        reason: String,
    },
    Error {
        kind: String,
        message: String,
    },
    /// 上游明确的终态（Anthropic `message_stop` / OpenAI `[DONE]`）。
    Eos,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(system: Value, tools: Vec<Value>, blocks: Vec<IrBlock>) -> IrRequest {
        IrRequest {
            model: "claude-haiku".into(),
            system: match system {
                Value::Array(a) => a,
                _ => Vec::new(),
            },
            messages: vec![IrMessage {
                role: "user".into(),
                blocks,
            }],
            tools,
            tool_choice: None,
            max_tokens: 1,
            temperature: None,
            top_p: None,
            stop: Vec::new(),
            thinking: None,
            metadata: None,
            extra: Map::new(),
            freeform_tools: Vec::new(),
        }
    }

    #[test]
    fn ascii_is_four_chars_per_token() {
        let r = req(
            Value::Null,
            vec![],
            vec![IrBlock::Text {
                text: "a".repeat(8),
                cache_control: None,
            }],
        );
        assert_eq!(estimate_tokens(&r), 2);
    }

    #[test]
    fn cjk_is_one_token_per_char() {
        let r = req(
            Value::Null,
            vec![],
            vec![IrBlock::Text {
                text: "你好世界".into(),
                cache_control: None,
            }],
        );
        assert_eq!(estimate_tokens(&r), 4);
    }

    #[test]
    fn system_and_tools_count_but_base64_does_not() {
        let r = req(
            json!([{"type":"text","text":"a".repeat(400)}]),
            vec![json!({"name":"t","description":"a".repeat(40)})],
            vec![IrBlock::Image {
                source: json!({"type":"base64","media_type":"image/png","data":"A".repeat(400000)}),
                cache_control: None,
            }],
        );
        // system {"type":"text"} 1 + 正文 100 + tools 11（"t" 与 40 字符描述）+ 图片常量 1500；
        // 400000 字节的 base64 一个字都不算。
        assert_eq!(estimate_tokens(&r), 1612);
    }

    #[test]
    fn never_zero() {
        assert_eq!(estimate_tokens(&req(Value::Null, vec![], vec![])), 1);
    }
}
