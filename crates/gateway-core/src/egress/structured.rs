//! 结构化输出：IR 的 `format` → 三条出站 wire 的**逐字**字段，以及「上游 × 出站协议」能力位
//! 的自检探针（spec §5 / §6；票 37 的十条定案，票 57 落地）。
//!
//! # 三条映射（层数是数着键说的）
//!
//! | 出站 wire | 字段路径 | 层数 | `Json` 怎么走 |
//! | --- | --- | --- | --- |
//! | Anthropic | `output_config.format` → `{type:"json_schema", schema}` | 3 | **400** |
//! | OpenAI Chat | `response_format` → `json_schema` → `{name, schema, strict}` | 4 | `{"type":"json_object"}` |
//! | OpenAI Responses | `text.format` → `{name, schema, strict}` | 3 | `{"type":"json_object"}` |
//!
//! - Anthropic 面**没有** `name` / `strict` / `json_schema` 外层包裹，且官方硬要求每个 object 层
//!   的 `additionalProperties` 为 `false`（否则 400）→ 出站时**递归补**（[`with_additional_properties_false`]）。
//! - OpenAI 两条的 `name` 一律填常量 [`RESPONSE_NAME`]（Anthropic / Ollama 的入站都没有名字，
//!   虚拟模型名又常含 `.` `/` 且随绑定变）；`strict` **只透传客户端显式值**，没给就吃上游缺省 `false`。
//! - 表达不了一律**请求期 400**（[`apply`]）：不静默丢弃，也不放行加警告。
//!
//! # 能力位与「状态码骗人」
//!
//! 上游可能在**状态码 200** 的同时逐字静默忽略这些字段（实测 commandcode 的 Anthropic 兼容端点
//! 对 6 个变体全 200 且输出与不带字段时逐字相同），所以「认不认」只能靠带**敌意提示词**的探针
//! （[`probe`]）判，判出来的结论挂在「上游 × 出站协议」上（[`crate::egress::provider`] 的
//! `upstreams.structured_output_json`）。

use serde_json::{json, Map, Value};

use crate::egress::provider::{
    self, EarlyReject, StructuredOutput, StructuredSource, Upstream, Wire,
};
use crate::egress::upstream;
use crate::ir::ResponseFormat;
use crate::store::Store;

/// OpenAI 两条 wire 上 `name` 的常量值（票 37 第 6 条）：不提 `title`、不做消毒、不追虚拟模型名。
pub const RESPONSE_NAME: &str = "response";

/// 探针的敌意提示词，逐字（`research/structured-output-wires.md` §3.2 实测用的那一句）。
///
/// 没有它测不出「忽略」——模型本来就会吐 JSON；只有明确叫它**不要** JSON，才能区分
/// 「schema 在起作用」与「模型自己愿意」。
pub const HOSTILE_PROMPT: &str =
    "Reply with exactly the word hello and nothing else. Do not output JSON.";

/// 探针的 `max_tokens`：敌意提示词只要一个词，但推理型模型要先思考，给足空间（与
/// `research/structured-output-wires.md` §3.2 的实测探针同一个档位，便于逐字比对）。
pub const STRUCTURED_PROBE_MAX_TOKENS: u64 = 512;

/// 探针用的 schema：最小 object + 一个必填字符串字段。
pub fn probe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"a": {"type": "string"}},
        "required": ["a"],
        "additionalProperties": false
    })
}

// ───────────────────────── 出站映射 ─────────────────────────

/// 把 IR 的 `format` 落成目标 wire 的逐字字段；表达不了 / 能力位是 `ignore` → 早拒 400。
///
/// `origin` = 入站协议对应的出站 wire（[`ResponseFormat::Other`] 的「与入站同形状」判据）；
/// Ollama 面没有出站 wire，传 `None` —— 它的 `Other` 一律 400。
///
/// 调用点在**出站请求的唯一组装点**（[`crate::egress::upstream::forward`] 与
/// `forward_buffered`），四条出口（流式 / 非流式 / 计数 / 探针）共用同一套判据。
pub fn apply(
    body: &mut Value,
    format: Option<&ResponseFormat>,
    wire: Wire,
    cap: StructuredOutput,
    origin: Option<Wire>,
) -> Result<(), EarlyReject> {
    let Some(format) = format else {
        // 客户端没要结构化输出：能力位与映射都不参与（`undefined` 不等于「要一个默认的」）。
        return Ok(());
    };
    if cap == StructuredOutput::Ignore {
        return Err(EarlyReject {
            status: 400,
            error_type: "invalid_request_error",
            message: format!(
                "该上游的 {} 面会静默忽略结构化输出字段（结构化输出能力位 = ignore，状态码照样 200）。\
                 请把这个模型改绑到该上游别的协议面，或先用自检确认这一面真的支持。",
                wire.as_str()
            ),
        });
    }

    match format {
        ResponseFormat::Json => match wire {
            // `output_config.format` 只有 json_schema 一种类型、必须 object + `additionalProperties:false`，
            // 写不出「任意 JSON」；硬凑一个最宽松的 schema 只会反过来被上游 400（票 37 第 9 条）。
            Wire::Anthropic => Err(EarlyReject {
                status: 400,
                error_type: "invalid_request_error",
                message:
                    "Anthropic 的 output_config.format 只有 json_schema 一种类型（且必须 object + \
                          additionalProperties:false），写不出「任意 JSON」的 JSON mode。\
                         请改绑到该上游的 OpenAI 兼容面，或让客户端给出 schema。"
                        .to_string(),
            }),
            Wire::OpenAiChat => {
                set(body, "response_format", json!({"type": "json_object"}));
                Ok(())
            }
            Wire::OpenAiResponses => {
                set(body, "text", json!({"format": {"type": "json_object"}}));
                Ok(())
            }
        },
        ResponseFormat::JsonSchema { schema, strict, .. } => match wire {
            Wire::Anthropic => {
                let schema = with_additional_properties_false(schema);
                set_output_config_format(body, json!({"type": "json_schema", "schema": schema}));
                Ok(())
            }
            Wire::OpenAiChat => {
                let mut js = Map::new();
                js.insert("name".into(), json!(RESPONSE_NAME));
                js.insert("schema".into(), schema.clone());
                if let Some(s) = strict {
                    js.insert("strict".into(), json!(s));
                }
                set(
                    body,
                    "response_format",
                    json!({"type": "json_schema", "json_schema": Value::Object(js)}),
                );
                Ok(())
            }
            Wire::OpenAiResponses => {
                let mut f = Map::new();
                f.insert("type".into(), json!("json_schema"));
                // Responses 侧 `name` 必填，且与 Chat 同层（少一层 `json_schema` 包裹）。
                f.insert("name".into(), json!(RESPONSE_NAME));
                f.insert("schema".into(), schema.clone());
                if let Some(s) = strict {
                    f.insert("strict".into(), json!(s));
                }
                set(body, "text", json!({"format": Value::Object(f)}));
                Ok(())
            }
        },
        // 没见过的形状：只在**与入站同形状**的 wire 上原样带（放回同一个字段），其余 400。
        ResponseFormat::Other(raw) => {
            if origin != Some(wire) {
                return Err(EarlyReject {
                    status: 400,
                    error_type: "invalid_request_error",
                    message: format!(
                        "客户端给的结构化输出形状（{}）在 {} 面上表达不了：这个形状只在与入站协议\
                         同形状的 wire 上原样带，网关不做猜测性改写。",
                        truncate(&raw.to_string(), 200),
                        wire.as_str()
                    ),
                });
            }
            match wire {
                Wire::Anthropic => {
                    set_output_config(body, raw.clone());
                    Ok(())
                }
                Wire::OpenAiChat => {
                    set(body, "response_format", raw.clone());
                    Ok(())
                }
                Wire::OpenAiResponses => {
                    set(body, "text", raw.clone());
                    Ok(())
                }
            }
        }
    }
}

/// 顶层字段（`response_format` / `text`）整体替换：这两个字段在出站体里只由本模块产生。
fn set(body: &mut Value, key: &str, value: Value) {
    if let Some(o) = body.as_object_mut() {
        o.insert(key.to_string(), value);
    }
}

/// Anthropic 的 `output_config`：只动 `format`，**保留兄弟键**。
///
/// `effort` 这类兄弟键不在本票的契约里，走 `extra` 原样带过（`anthropic::decode_request`）；
/// 这里若整份替换就会把它们悄悄丢掉。
fn set_output_config(body: &mut Value, whole: Value) {
    if let Some(o) = body.as_object_mut() {
        o.insert("output_config".to_string(), whole);
    }
}

fn set_output_config_format(body: &mut Value, format: Value) {
    let Some(o) = body.as_object_mut() else {
        return;
    };
    let oc = o.entry("output_config").or_insert_with(|| json!({}));
    if !oc.is_object() {
        *oc = json!({});
    }
    oc["format"] = format;
}

/// Anthropic 官方硬要求：**每个** object 层的 `additionalProperties` 必须是 `false`，否则 400。
///
/// 只做「不补一定失败」的机械修补：**只在缺失时补**。客户端显式给了值就一个字都不动
/// （给了 `true` 是它的语义，我们不改；真不合法就让上游自己 400）。
///
/// shortcut: 递归只下到 `properties` / `patternProperties` / `$defs` / `definitions` /
/// `items` / `not` / `additionalProperties` / `anyOf` / `allOf` / `oneOf` —— Anthropic 支持的
/// 组合子就是这些；真碰到别的容器再加。
pub fn with_additional_properties_false(schema: &Value) -> Value {
    let mut out = schema.clone();
    patch_objects(&mut out);
    out
}

fn patch_objects(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    // 是不是一个 object schema：`type:"object"`，或者写了 `properties`（有的 schema 省 type）。
    if obj.get("type").and_then(Value::as_str) == Some("object") || obj.contains_key("properties") {
        obj.entry("additionalProperties")
            .or_insert(Value::Bool(false));
    }
    for key in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(Value::Object(map)) = obj.get_mut(key) {
            for (_, v) in map.iter_mut() {
                patch_objects(v);
            }
        }
    }
    for key in ["items", "not", "additionalProperties"] {
        if let Some(v) = obj.get_mut(key) {
            patch_objects(v);
        }
    }
    for key in ["anyOf", "allOf", "oneOf"] {
        if let Some(Value::Array(items)) = obj.get_mut(key) {
            for v in items.iter_mut() {
                patch_objects(v);
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "…"
}

// ───────────────────────── 自检探针 ─────────────────────────

/// 一次结构化输出能力位探针的结论。
///
/// `prompt` / `answer` 是**原文**：验收要贴它们——「状态码骗人」这条判据的证据就是
/// 「敌意提示词 + 上游照直说」这一对。
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredProbe {
    pub upstream_id: String,
    pub real_model: String,
    pub wire: Wire,
    /// HTTP 状态码；超时记 504、连不上记 502（与转发面同一套口径）。
    pub status: u16,
    /// 判出来的能力位；`None` = 这一发**没能判定**（非 2xx / 连不上），此时不落库。
    pub mode: Option<StructuredOutput>,
    pub elapsed_ms: u128,
    pub prompt: String,
    pub answer: String,
    /// 人话（界面直接显示）。**不含密钥与 baseUrl**。
    pub detail: String,
}

/// 带敌意提示词的最短请求探一次，判这一面认不认结构化输出，并把结论落库（`source = selfcheck`）。
///
/// 判据不是状态码：**回答被 schema 约束住**才算 `native`，否则 `ignore`。
pub async fn probe(
    client: &reqwest::Client,
    store: &Store,
    up: &Upstream,
    wire: Wire,
    real_model: &str,
    deadline: std::time::Duration,
) -> StructuredProbe {
    let t0 = std::time::Instant::now();
    let mut result = StructuredProbe {
        upstream_id: up.id.clone(),
        real_model: real_model.to_string(),
        wire,
        status: 0,
        mode: None,
        elapsed_ms: 0,
        prompt: HOSTILE_PROMPT.to_string(),
        answer: String::new(),
        detail: String::new(),
    };

    let Some(url) = up.url(wire) else {
        result.status = 502;
        result.detail = format!("上游 {} 没有 {} 面的路径模板", up.id, wire.as_str());
        return finish(result, t0, store).await;
    };
    let key = provider::wire_credential(store, &up.id, wire)
        .await
        .ok()
        .flatten();
    let Some(key) = key else {
        result.status = 502;
        result.detail = format!("上游 {} 没有可用的凭据", up.id);
        return finish(result, t0, store).await;
    };
    let Some(body) = probe_body(wire, real_model) else {
        result.status = 502;
        result.detail = format!("{} 面编不出探针请求", wire.as_str());
        return finish(result, t0, store).await;
    };

    // 探针档的协议头：Anthropic 面按官方契约带版本头（不代发 `anthropic-beta`，票 37 第 8 条）。
    let pass: Vec<(String, String)> = if wire == Wire::Anthropic {
        vec![("anthropic-version".to_string(), "2023-06-01".to_string())]
    } else {
        Vec::new()
    };
    let sent = upstream::request(
        client,
        up,
        wire,
        up.auth_for(wire),
        &key,
        &url,
        &body,
        HOSTILE_PROMPT,
        &pass,
    )
    .timeout(deadline)
    .send()
    .await;

    match sent {
        Ok(resp) => {
            let status = resp.status();
            result.status = status.as_u16();
            if status.is_success() {
                let v: Value = resp.json().await.unwrap_or(Value::Null);
                result.answer = answer_text(wire, &v);
                result.mode = Some(if conforms(&result.answer, &probe_schema()) {
                    StructuredOutput::Native
                } else {
                    StructuredOutput::Ignore
                });
                result.detail = format!(
                    "上游 {}；回答{}被 schema 约束住",
                    status.as_u16(),
                    if result.mode == Some(StructuredOutput::Native) {
                        ""
                    } else {
                        "没"
                    }
                );
            } else {
                let text = resp.text().await.unwrap_or_default();
                result.detail = format!(
                    "上游 {}：{}（判不出来，不动能力位）",
                    status.as_u16(),
                    truncate(&text, 200)
                );
            }
        }
        Err(e) if e.is_timeout() => {
            result.status = 504;
            result.detail = format!("上游超时（{} 秒内无响应）", deadline.as_secs());
        }
        Err(_) => {
            result.status = 502;
            result.detail = "连不上上游（连接失败）".to_string();
        }
    }
    finish(result, t0, store).await
}

/// 收尾：填耗时、按结论落库（`None` 不落——「没测出」不等于 `native`）。
async fn finish(
    mut result: StructuredProbe,
    t0: std::time::Instant,
    store: &Store,
) -> StructuredProbe {
    result.elapsed_ms = t0.elapsed().as_millis();
    if let Some(mode) = result.mode {
        if let Err(e) = provider::set_structured_output(
            store,
            &result.upstream_id,
            result.wire,
            mode,
            StructuredSource::Selfcheck,
        )
        .await
        {
            eprintln!(
                "[warn] 结构化输出能力位写不进去（{} / {}）：{e}",
                result.upstream_id,
                result.wire.as_str()
            );
        }
    }
    result
}

/// 探针请求体：最短 + 带 schema，字段路径由 [`apply`] 给（三条 wire 共用一套映射）。
fn probe_body(wire: Wire, real_model: &str) -> Option<Value> {
    let mut body = match wire {
        Wire::Anthropic | Wire::OpenAiChat => json!({
            "model": real_model,
            "max_tokens": STRUCTURED_PROBE_MAX_TOKENS,
            "messages": [{"role": "user", "content": HOSTILE_PROMPT}],
        }),
        Wire::OpenAiResponses => json!({
            "model": real_model,
            "max_output_tokens": STRUCTURED_PROBE_MAX_TOKENS,
            "input": [{"role": "user", "content": HOSTILE_PROMPT}],
        }),
    };
    let format = ResponseFormat::JsonSchema {
        schema: probe_schema(),
        name: None,
        // 探针显式要严格模式：这是「能不能强制」的最强问法（Anthropic 面本来也不发 strict）。
        strict: Some(true),
    };
    apply(
        &mut body,
        Some(&format),
        wire,
        StructuredOutput::Native,
        Some(wire),
    )
    .ok()?;
    Some(body)
}

/// 从各 wire 的成功响应里取出**回答文本**（探针只关心它）。
fn answer_text(wire: Wire, v: &Value) -> String {
    match wire {
        Wire::Anthropic => v["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        Wire::OpenAiChat => v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Wire::OpenAiResponses => v["output"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("output_text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
    }
}

/// 回答是不是被 schema 约束住了：能解析成 JSON，且满足 `required` 与逐字段类型。
///
/// shortcut: 只认 `type` / `required` / `properties` 三件事——够判探针这一发；探针要的是
/// 「模型有没有照 schema 吐 JSON」，不是通用校验器。
pub fn conforms(answer: &str, schema: &Value) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(answer.trim()) else {
        return false;
    };
    schema_ok(&v, schema)
}

fn schema_ok(v: &Value, schema: &Value) -> bool {
    let Some(s) = schema.as_object() else {
        return true;
    };
    match s.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Some(o) = v.as_object() else {
                return false;
            };
            if let Some(required) = s.get("required").and_then(Value::as_array) {
                if required
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|k| !o.contains_key(k))
                {
                    return false;
                }
            }
            if let Some(props) = s.get("properties").and_then(Value::as_object) {
                for (k, sub) in props {
                    if let Some(pv) = o.get(k) {
                        if !schema_ok(pv, sub) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        Some("string") => v.is_string(),
        Some("number") | Some("integer") => v.is_number(),
        Some("boolean") => v.is_boolean(),
        Some("array") => v.is_array(),
        Some("null") => v.is_null(),
        _ => true,
    }
}
