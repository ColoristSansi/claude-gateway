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
    /// 结构化输出（spec §5；票 37 定案 1）。**归一化**槽位：四种入站形状解成
    /// [`ResponseFormat`] 的三个变体，出站再按目标 wire 编成逐字形状（票 57）。
    pub format: Option<ResponseFormat>,
    pub extra: Map<String, Value>,
    /// 自由格式工具（Responses 的 `type:"custom"`）的名字：对上游声明成「一个字符串参数」，
    /// 回给客户端时要还原成 `custom_tool_call`（票 32）。
    pub freeform_tools: Vec<String>,
}

/// 结构化输出在 IR 里的归一化形状（spec §5；票 37 定案 1）。
///
/// **不原样透传**：四条入站协议（Anthropic `output_config.format` / Chat `response_format` /
/// Responses `text.format` / Ollama `format`）各自解成下面三个变体，出站再按目标 wire 编回去
/// （[`crate::egress::structured::apply`]）。表达不了的一律**请求期 400**，不静默丢弃。
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseFormat {
    /// 无 schema 的 JSON mode：Chat / Responses 的 `{"type":"json_object"}`、Ollama 的 `"json"`。
    Json,
    /// 带 schema。`name` / `strict` **只有入站显式给了才填**（`None` = 没给，不是 `false`）——
    /// spec §5 硬规则 1：`undefined`（没传）与 `null`（传了但不适用）必须分开。
    JsonSchema {
        schema: Value,
        name: Option<String>,
        strict: Option<bool>,
    },
    /// 没见过的形状：原样兜底，**只在「与入站同形状」的出站 wire 上原样带**（spec §5）。
    ///
    /// 值 = 该入站协议自己那个字段的**整份**值（Anthropic 是 `output_config`、Chat 是
    /// `response_format`、Responses 是 `text`、Ollama 是 `format`），这样「原样带」就是把它
    /// 放回同一个字段，不做任何猜测性改写。
    Other(Value),
}

impl ResponseFormat {
    /// Anthropic 入站：顶层 `output_config.format`（3 层，无 `name` / `strict` / `json_schema` 包裹）。
    ///
    /// `output_config` 里除 `format` 之外的兄弟键（如 `effort`）**不在这里建模**：它们不在
    /// `known` 名单里，仍走 `extra` 原样带过（见 [`crate::anthropic::decode_request`]）。
    pub fn from_anthropic_request(obj: &Map<String, Value>) -> Option<ResponseFormat> {
        let output_config = obj.get("output_config")?;
        let format = output_config.get("format")?;
        if format.get("type").and_then(Value::as_str) != Some("json_schema") {
            return Some(ResponseFormat::Other(output_config.clone()));
        }
        match format.get("schema") {
            Some(schema) if schema.is_object() => Some(ResponseFormat::JsonSchema {
                schema: schema.clone(),
                name: None,
                strict: None,
            }),
            _ => Some(ResponseFormat::Other(output_config.clone())),
        }
    }

    /// OpenAI Chat 入站：顶层 `response_format`（4 层：`response_format` → `json_schema` → …）。
    pub fn from_chat_request(obj: &Map<String, Value>) -> Option<ResponseFormat> {
        let rf = obj.get("response_format")?;
        match rf.get("type").and_then(Value::as_str) {
            Some("json_object") => Some(ResponseFormat::Json),
            // `{"type":"text"}` 是官方缺省值（明确「不要结构化输出」）：与没传同一个下场。
            Some("text") => None,
            Some("json_schema") => match rf.get("json_schema") {
                Some(js) if js.get("schema").is_some() => Some(ResponseFormat::JsonSchema {
                    schema: js.get("schema").cloned().unwrap_or(Value::Null),
                    name: js.get("name").and_then(Value::as_str).map(str::to_string),
                    strict: js.get("strict").and_then(Value::as_bool),
                }),
                _ => Some(ResponseFormat::Other(rf.clone())),
            },
            _ => Some(ResponseFormat::Other(rf.clone())),
        }
    }

    /// OpenAI Responses 入站：顶层 `text.format`（3 层，比 Chat 少一层 `json_schema` 包裹；
    /// `name` 与 `schema` 都必填）。`Other` 兜底带的是整份 `text`（放回同一个字段）。
    pub fn from_responses_request(obj: &Map<String, Value>) -> Option<ResponseFormat> {
        let text = obj.get("text")?;
        let format = text.get("format")?;
        match format.get("type").and_then(Value::as_str) {
            Some("json_object") => Some(ResponseFormat::Json),
            Some("text") => None,
            Some("json_schema") => match format.get("schema") {
                Some(schema) => Some(ResponseFormat::JsonSchema {
                    schema: schema.clone(),
                    name: format
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    strict: format.get("strict").and_then(Value::as_bool),
                }),
                None => Some(ResponseFormat::Other(text.clone())),
            },
            _ => Some(ResponseFormat::Other(text.clone())),
        }
    }

    /// Ollama 入站：顶层 `format` —— 裸 schema 对象或字符串 `"json"`（spec §4.4）。
    ///
    /// **只解不编**：Ollama 面只有入站（`/api/chat` 归票 56），出站仍走上面三条 wire。
    /// 空串与 `null` 都是「没要结构化输出」（官方 `format` 缺省即无）。
    pub fn from_ollama_request(obj: &Map<String, Value>) -> Option<ResponseFormat> {
        match obj.get("format")? {
            Value::String(s) if s == "json" => Some(ResponseFormat::Json),
            Value::String(s) if s.trim().is_empty() => None,
            Value::Null => None,
            Value::Object(_) => Some(ResponseFormat::JsonSchema {
                schema: obj.get("format").cloned().unwrap_or(Value::Null),
                name: None,
                strict: None,
            }),
            other => Some(ResponseFormat::Other(other.clone())),
        }
    }
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
            format: None,
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
