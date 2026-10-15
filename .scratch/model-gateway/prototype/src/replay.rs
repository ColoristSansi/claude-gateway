//! 回放器（票 19 §2、票 29 第 3 条）：把 fixture 字节按**字节偏移**切成若干块发出去。
//! 切块不改字节，只改到达节奏——一个 fixture 因此能派生「整帧 / 跨 chunk 半帧 / 单字节流 / 行尾变体」，
//! 而字节只存真录到的那一份。
//! 同一份代码两处复用：单元层直接喂切块结果给 `sse` / codec，集成层由 mock 决定怎么发。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// 内建切法：`frame` 一段一帧（默认）、`half` 每帧从中间劈开、`byte` 单字节流、
/// `whole` 整包一次给、`crlf` 先转 CRLF 再按帧切（行尾变体）。
pub const SCHEMES: [&str; 5] = ["frame", "half", "byte", "whole", "crlf"];

pub struct Fixture {
    pub bytes: Vec<u8>,
    pub meta: Value,
    pub path: PathBuf,
}

impl Fixture {
    /// `<dir>/<scenario>/<wire>.sse` 加同名 `.meta.json`（没有 meta 也能跑，切法走内建规则）。
    pub fn load(dir: &Path, scenario: &str, wire: &str) -> std::io::Result<Fixture> {
        let path = dir.join(scenario).join(format!("{wire}.sse"));
        let bytes = std::fs::read(&path)?;
        let meta = std::fs::read_to_string(dir.join(scenario).join(format!("{wire}.meta.json")))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        Ok(Fixture { bytes, meta, path })
    }

    /// meta.json 可以给某个切法写死字节偏移（`{"chunks":[{"name":"half","cuts":[120,300]}]}`）；
    /// 写死的优先，没写就按内建规则现算。
    pub fn chunks(&self, scheme: &str) -> Vec<Vec<u8>> {
        if let Some(cuts) = self
            .meta
            .get("chunks")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|e| e.get("name").and_then(Value::as_str) == Some(scheme))
            })
            .and_then(|e| e.get("cuts"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as usize))
                    .collect::<Vec<_>>()
            })
        {
            return split_at(&self.bytes, &cuts);
        }
        chunk(&self.bytes, scheme)
    }
}

/// 未知名字退回 `frame`：宁可跑默认切法，也别因为拼错一个名字就整条测试消失。
pub fn chunk(bytes: &[u8], scheme: &str) -> Vec<Vec<u8>> {
    match scheme {
        "whole" => vec![bytes.to_vec()],
        "byte" => bytes.iter().map(|b| vec![*b]).collect(),
        "half" => split_at(bytes, &half_cuts(bytes)),
        "crlf" => {
            let crlf = to_crlf(bytes);
            split_at(&crlf, &frame_cuts(&crlf))
        }
        _ => split_at(bytes, &frame_cuts(bytes)),
    }
}

/// 每帧结束的字节位置（最后一帧没有终止空行时也算一帧）。
pub fn frame_cuts(bytes: &[u8]) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut start = 0usize;
    while let Some((idx, sep)) = crate::sse::find_sep(&bytes[start..]) {
        start += idx + sep;
        cuts.push(start);
    }
    if start < bytes.len() {
        cuts.push(bytes.len());
    }
    cuts
}

/// 每帧从中间劈一刀（跨 chunk 半帧路径的唯一制造方式）。
fn half_cuts(bytes: &[u8]) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut prev = 0usize;
    for end in frame_cuts(bytes) {
        let mid = prev + (end - prev) / 2;
        if mid > prev && mid < end {
            cuts.push(mid);
        }
        cuts.push(end);
        prev = end;
    }
    cuts
}

/// `cuts` 是每个后续块的起始偏移；末尾隐式收尾，所以 cut == len 与乱序 cut 都丢掉。
fn split_at(bytes: &[u8], cuts: &[usize]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut prev = 0usize;
    for &c in cuts {
        if c <= prev || c >= bytes.len() {
            continue;
        }
        out.push(bytes[prev..c].to_vec());
        prev = c;
    }
    out.push(bytes[prev..].to_vec());
    out
}

/// LF -> CRLF，已有的 CRLF 不重复加 CR。
pub fn to_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    let mut prev = 0u8;
    for &b in bytes {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::SseParser;

    const SAMPLE: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    fn frames_of(chunks: &[Vec<u8>]) -> Vec<(Option<String>, String)> {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for c in chunks {
            for f in p.push(c) {
                out.push((f.event, f.data));
            }
        }
        for f in p.finish() {
            out.push((f.event, f.data));
        }
        out
    }

    #[test]
    fn every_scheme_keeps_the_bytes() {
        for scheme in SCHEMES {
            let got: Vec<u8> = chunk(SAMPLE, scheme).concat();
            let want = if scheme == "crlf" {
                to_crlf(SAMPLE)
            } else {
                SAMPLE.to_vec()
            };
            assert_eq!(got, want, "切法 {scheme} 改了字节");
        }
    }

    #[test]
    fn half_frames_split_inside_frames() {
        let frames = chunk(SAMPLE, "frame");
        let half = chunk(SAMPLE, "half");
        assert_eq!(frames.len(), 3);
        assert!(half.len() > frames.len(), "半帧切法没真的劈开帧");
    }

    #[test]
    fn reparse_is_chunking_invariant() {
        let want = frames_of(&chunk(SAMPLE, "whole"));
        assert_eq!(want.len(), 3);
        for scheme in SCHEMES {
            assert_eq!(
                frames_of(&chunk(SAMPLE, scheme)),
                want,
                "切法 {scheme} 改变了帧序列"
            );
        }
    }

    #[test]
    fn crlf_bytes_reparse_to_the_same_frames() {
        let crlf = to_crlf(SAMPLE);
        assert!(crlf.windows(2).any(|w| w == b"\r\n"));
        assert_eq!(
            frames_of(&chunk(&crlf, "frame")),
            frames_of(&chunk(SAMPLE, "whole"))
        );
    }

    #[test]
    fn meta_can_pin_explicit_cuts() {
        let f = Fixture {
            bytes: SAMPLE.to_vec(),
            meta: serde_json::json!({"chunks":[{"name":"frame","cuts":[10, 20]}]}),
            path: PathBuf::new(),
        };
        assert_eq!(f.chunks("frame").len(), 3);
        assert_eq!(f.chunks("frame").concat(), SAMPLE.to_vec());
        // 没写死偏移的切法仍走内建规则。
        assert_eq!(f.chunks("whole").len(), 1);
        assert_eq!(f.chunks("byte").len(), SAMPLE.len());
    }
}
