//! 结构级黄金比对（票 19 §3）：两侧 SSE 都先解析成帧，再逐帧逐字段比。
//! 只读不写——这里**没有**任何写黄金文件的代码路径，「首次跑自动写 golden」在结构上不可能发生。
//!
//! 票 41 搬入产品侧：解析器直接用 lib 的 [`gateway_core::sse`]（内核里本来就有这一份），
//! 不再 `#[path]` 引一份原型源码。

use gateway_core::sse::SseParser;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub event: Option<String>,
    pub data: Value,
}

pub fn parse(bytes: &[u8]) -> Vec<Frame> {
    let mut parser = SseParser::default();
    let mut frames = parser.push(bytes);
    frames.extend(parser.finish());
    frames
        .into_iter()
        .map(|f| Frame {
            event: f.event,
            data: serde_json::from_str(&f.data).unwrap_or(Value::String(f.data)),
        })
        .collect()
}

/// 字段级掩码：`message.id`、`content.0.id`、`usage.*`（`*` 通配一层）。
fn mask_at(v: &mut Value, segs: &[&str]) {
    let Some((head, rest)) = segs.split_first() else {
        *v = Value::String("<masked>".into());
        return;
    };
    match v {
        Value::Object(o) => {
            if *head == "*" {
                for (_, val) in o.iter_mut() {
                    mask_at(val, rest);
                }
            } else if let Some(val) = o.get_mut(*head) {
                mask_at(val, rest);
            }
        }
        Value::Array(a) => {
            if *head == "*" {
                for val in a.iter_mut() {
                    mask_at(val, rest);
                }
            } else if let Ok(i) = head.parse::<usize>() {
                if let Some(val) = a.get_mut(i) {
                    mask_at(val, rest);
                }
            }
        }
        _ => {}
    }
}

pub fn masked(data: &Value, masks: &[String]) -> Value {
    let mut v = data.clone();
    for m in masks {
        let segs: Vec<&str> = m.split('.').collect();
        mask_at(&mut v, &segs);
    }
    v
}

/// 比对并返回人可读的差异清单（空 = 一致）。**禁止在这里回写黄金文件。**
pub fn compare(got: &[Frame], want: &[Frame], masks: &[String]) -> Vec<String> {
    let mut diffs = Vec::new();
    if got.len() != want.len() {
        diffs.push(format!(
            "帧数不同：实际 {} 帧，黄金 {} 帧",
            got.len(),
            want.len()
        ));
    }
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        if g.event != w.event {
            diffs.push(format!(
                "帧 {i}: 事件名 实际 {:?} 黄金 {:?}",
                g.event, w.event
            ));
        }
        let (gv, wv) = (masked(&g.data, masks), masked(&w.data, masks));
        if let Some(d) = first_diff(&gv, &wv, "data") {
            diffs.push(format!("帧 {i} ({:?}): {d}", g.event));
        }
    }
    diffs
}

/// 第一个不同的字段路径，比「整坨 JSON 不相等」可读得多。
fn first_diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, xv) in x {
                match y.get(k) {
                    Some(yv) => {
                        if let Some(d) = first_diff(xv, yv, &format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k} 黄金里没有")),
                }
            }
            y.keys()
                .find(|k| !x.contains_key(*k))
                .map(|k| format!("{path}.{k} 实际里没有"))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path} 长度不同：{} vs {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (xv, yv))| first_diff(xv, yv, &format!("{path}.{i}")))
        }
        _ if a != b => Some(format!("{path} 实际 {a} 黄金 {b}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_a\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    fn masks() -> Vec<String> {
        vec!["message.id".to_string()]
    }

    #[test]
    fn parse_reads_event_names_and_json_payloads() {
        let f = parse(TWO);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].event.as_deref(), Some("message_start"));
        assert_eq!(f[0].data["message"]["id"], "msg_a");
        assert_eq!(f[1].data["type"], "message_stop");
    }

    #[test]
    fn mask_makes_the_unstable_id_comparable() {
        let a = parse(TWO);
        let b = parse(
            &String::from_utf8_lossy(TWO)
                .replace("msg_a", "msg_b")
                .into_bytes(),
        );
        assert_ne!(a[0].data, b[0].data);
        assert!(compare(&a, &b, &masks()).is_empty(), "掩码后不应有差异");
        assert!(!compare(&a, &b, &[]).is_empty(), "没掩码就该报差异");
    }

    #[test]
    fn text_change_is_caught_with_a_field_path() {
        let a = parse(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"你\"}}\n\n"
                .as_bytes(),
        );
        let b = parse(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"我\"}}\n\n"
                .as_bytes(),
        );
        let d = compare(&a, &b, &[]);
        assert_eq!(d.len(), 1);
        assert!(d[0].contains("data.delta.text"), "差异没给字段路径：{d:?}");
    }

    #[test]
    fn event_order_and_missing_frames_are_caught() {
        let two = parse(TWO);
        let swapped = vec![two[1].clone(), two[0].clone()];
        assert!(!compare(&two, &swapped, &masks()).is_empty());
        assert!(!compare(&two[..1], &two, &masks()).is_empty());
    }
}
