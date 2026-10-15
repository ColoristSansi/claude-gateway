//! SSE 逐帧解析：跨 chunk 的半帧必须缓存，且**按到达顺序**吐帧（保序是客户端硬要求）。

#[derive(Debug, Clone)]
pub struct Frame {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some((idx, sep)) = find_sep(&self.buf) {
            let raw: Vec<u8> = self.buf.drain(..idx + sep).collect();
            let text = String::from_utf8_lossy(&raw[..idx]).to_string();
            if let Some(f) = parse_frame(&text) {
                out.push(f);
            }
        }
        out
    }

    /// 流结束时的残留字节：不完整的一帧也尽力解析（上游断流时常见）。
    pub fn finish(&mut self) -> Vec<Frame> {
        let text = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        match parse_frame(&text) {
            Some(f) => vec![f],
            None => Vec::new(),
        }
    }
}

/// 找最早的帧终止空行，返回 (正文结束位置, 含终止符要排掉的字节数)。
/// 公开给回放器：切块与解析必须用同一套「帧边界」定义，否则半帧 fixture 会自我矛盾。
///
/// 正文结束位置一律取**行尾之前**：`\r\n\r\n` 要结束在第一个 `\r` 上，
/// 否则最后一行的 `data:` 会拖一个 `\r` 尾巴，JSON 解析失败、整帧被当成 ping 丢掉。
pub fn find_sep(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        let at = |k: usize| buf.get(i + k).copied();
        match (at(0), at(1), at(2), at(3)) {
            (Some(b'\r'), Some(b'\n'), Some(b'\r'), Some(b'\n')) => return Some((i, 4)),
            (Some(b'\r'), Some(b'\n'), Some(b'\n'), _) => return Some((i, 3)),
            (Some(b'\n'), Some(b'\n'), _, _) => return Some((i, 2)),
            (Some(b'\n'), Some(b'\r'), Some(b'\n'), _) => return Some((i, 3)),
            _ => {}
        }
    }
    None
}

fn parse_frame(text: &str) -> Option<Frame> {
    let mut event = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    if event.is_none() && data.is_empty() {
        return None;
    }
    Some(Frame {
        event,
        data: data.join("\n"),
    })
}

pub fn frame(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// SSE 注释帧：没有任何事件名，任何客户端都忽略它。
/// Responses 入站没有 `ping` 这种事件，保活只能用它（票 32）。
pub fn comment(text: &str) -> String {
    format!(": {text}\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "event: a\ndata: {\"n\":1}\n\nevent: b\ndata: {\"n\":2}\n\n";

    fn events(frames: &[Frame]) -> Vec<Option<String>> {
        frames.iter().map(|f| f.event.clone()).collect()
    }

    #[test]
    fn splits_frames_and_keeps_order() {
        let mut p = SseParser::default();
        let f = p.push(TWO.as_bytes());
        assert_eq!(events(&f), vec![Some("a".into()), Some("b".into())]);
        assert_eq!(f[0].data, "{\"n\":1}");
    }

    /// 半帧缓存分支：每个字节单独喂一次，仍必须一帧不多一帧不少。
    #[test]
    fn single_byte_chunks_are_buffered_until_the_frame_ends() {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for b in TWO.as_bytes() {
            out.extend(p.push(&[*b]));
        }
        assert_eq!(events(&out), vec![Some("a".into()), Some("b".into())]);
        assert_eq!(out[1].data, "{\"n\":2}");
    }

    #[test]
    fn crlf_terminator_is_understood() {
        let mut p = SseParser::default();
        let f = p.push(TWO.replace('\n', "\r\n").as_bytes());
        assert_eq!(events(&f), vec![Some("a".into()), Some("b".into())]);
        assert_eq!(f[1].data, "{\"n\":2}");
    }

    #[test]
    fn comment_frames_do_not_swallow_the_next_frame() {
        let mut p = SseParser::default();
        let mut out = p.push(b": keep-alive\n\nevent: a\ndata: {}\n\n");
        // 注释帧解析成空帧（无 event 无 data）被丢弃，但不影响后面的帧。
        out.extend(p.finish());
        assert_eq!(events(&out), vec![Some("a".into())]);
    }

    #[test]
    fn unfinished_tail_is_still_parsed_on_finish() {
        let mut p = SseParser::default();
        assert!(p.push(b"event: a\ndata: {\"n\":1}\n").is_empty());
        assert_eq!(events(&p.finish()), vec![Some("a".into())]);
    }
}
