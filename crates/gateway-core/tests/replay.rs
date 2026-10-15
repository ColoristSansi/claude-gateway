//! 回放集成测试（票 29 第 7 条铺开成票 30 的整套）：测试自己把**两个进程**
//! （`gateway-harness` 与 `mock-upstream`）拉起来，走**真实 HTTP** 把 fixture 回一遍。
//! 断言是结构级的：解析两侧 SSE 后逐帧逐字段比，外加 HTTP 状态码、错误信封、头注入、
//! 并发隔离、首帧延迟 —— 不再数 `event:` 名和子串。
//!
//! 每个 `#[tokio::test]` 各起一份网关与 mock（端口按进程 id + 序号现算），所以它们能并行跑。
//!
//! 票 41 搬入产品侧：`fixtures` 在 `tests/fixtures/`（原型那份已冻结），回放器由
//! `src/replay.rs` 一份实现、mock 与测试两处 `#[path]` 引入（产品构建不含它），
//! 台账读产品侧的 `tests/LEDGER.md`。

// 回放器与 mock 共用同一份实现（`src/replay.rs` 不写进 lib.rs，产品构建看不到它）。
// 测试用不到的那几个入口在这里允许闲置。
#[path = "../src/replay.rs"]
#[allow(dead_code)]
mod replay;

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{golden, Gateway, SECRET_COMMANDCODE, SECRET_OPENCODE};
use replay::Fixture;
use serde_json::{json, Value};

/// 台账矩阵（票 30 第 1、2、3 条）：6 场景 × 2 wire + `hardcut` × 2 + 真录的 `keepalive`。
/// 每一行都必须有黄金文件（`fixtures/expected/<场景>.<wire>.sse`）。
const ROWS: [(&str, &str); 15] = [
    ("text", "anthropic"),
    ("text", "openai"),
    ("tools", "anthropic"),
    ("tools", "openai"),
    ("thinking", "anthropic"),
    ("thinking", "openai"),
    ("usage", "anthropic"),
    ("usage", "openai"),
    ("abort", "anthropic"),
    ("abort", "openai"),
    ("toolresult", "anthropic"),
    ("toolresult", "openai"),
    ("hardcut", "anthropic"),
    ("hardcut", "openai"),
    ("keepalive", "anthropic"),
];

/// 每个 fixture 都拿这几种切法各回一遍：切块只该改到达节奏，不该改结果。
const SCHEMES: [&str; 5] = ["frame", "half", "byte", "whole", "crlf"];

fn check(fails: &mut Vec<String>, cond: bool, msg: impl Into<String>) {
    if !cond {
        fails.push(msg.into());
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect()
}

fn masked_frames(bytes: &[u8], masks: &[String]) -> Vec<golden::Frame> {
    golden::parse(bytes)
        .into_iter()
        .map(|f| golden::Frame {
            event: f.event,
            data: golden::masked(&f.data, masks),
        })
        .collect()
}

/// 某个事件名的全部 data。
fn frames_of<'a>(frames: &'a [golden::Frame], event: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|f| f.event.as_deref() == Some(event))
        .map(|f| &f.data)
        .collect()
}

fn event_names(frames: &[golden::Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|f| f.event.clone().unwrap_or_else(|| "<无事件名>".into()))
        .collect()
}

/// 一段文本 delta 拼起来的结果（`content_block_delta` 里的 `text_delta`）。
fn joined_text(frames: &[golden::Frame]) -> String {
    frames_of(frames, "content_block_delta")
        .iter()
        .filter(|d| d["delta"]["type"] == "text_delta")
        .filter_map(|d| d["delta"]["text"].as_str())
        .collect()
}

/// 一段 partial JSON 拼起来的结果（工具入参增量）。
fn joined_partial_json(frames: &[golden::Frame]) -> String {
    frames_of(frames, "content_block_delta")
        .iter()
        .filter(|d| d["delta"]["type"] == "input_json_delta")
        .filter_map(|d| d["delta"]["partial_json"].as_str())
        .collect()
}

fn fixture_request(gw: &Gateway, scenario: &str, wire: &str) -> Value {
    Fixture::load(&gw.fixtures, scenario, wire)
        .unwrap_or_else(|e| panic!("读不到 fixture {scenario}/{wire}: {e}"))
        .meta["request"]
        .clone()
}

async fn last_echo(gw: &Gateway) -> Value {
    let echo = gw.echo().await;
    echo["echo"]
        .as_array()
        .and_then(|a| a.last().cloned())
        .unwrap_or(Value::Null)
}

/// ① 台账矩阵：15 格的状态码、首帧、末帧，加上逐帧逐字段的黄金比对，
/// 以及几条**独立于黄金**的语义断言（黄金是离线核对过的，这里是第二把尺）。
#[tokio::test(flavor = "multi_thread")]
async fn fixtures_replay_through_the_gateway() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();

    for (scenario, wire) in ROWS {
        let fixture =
            Fixture::load(&gw.fixtures, scenario, wire).expect("读不到 fixture 字节或 meta");
        let tag = format!("{scenario}/{wire}");
        let request = fixture.meta["request"].clone();
        let expect_status = fixture.meta["expect"]["status"].as_u64().unwrap_or(200) as u16;
        let want_last = fixture.meta["expect"]["last_event"]
            .as_str()
            .unwrap_or("message_stop")
            .to_string();
        let masks = strings(&fixture.meta["masks"]);

        let (status, bytes) = gw.post_messages(&request).await;
        check(
            &mut fails,
            status == expect_status,
            format!("{tag}: HTTP {status}，期望 {expect_status}"),
        );
        let got = golden::parse(&bytes);
        let names = event_names(&got);
        check(
            &mut fails,
            names.first().map(String::as_str) == Some("message_start"),
            format!("{tag}: 首帧是 {:?}，期望 message_start", names.first()),
        );
        check(
            &mut fails,
            names.last().map(String::as_str) == Some(want_last.as_str()),
            format!("{tag}: 末帧是 {:?}，期望 {want_last}", names.last()),
        );

        // 黄金文件：每一格都必须有（票 30 第 1 条要求 15 格全换结构级断言）。
        let expected = gw
            .fixtures
            .join("expected")
            .join(format!("{scenario}.{wire}.sse"));
        if !expected.exists() {
            fails.push(format!("{tag}: 没有黄金文件 {}", expected.display()));
        } else {
            let want = golden::parse(&std::fs::read(&expected).expect("读不到黄金文件"));
            for d in golden::compare(&got, &want, &masks) {
                fails.push(format!("{tag}: 与黄金不一致：{d}"));
            }
        }

        // index 连续性：content_block_start 的 index 必须是 0,1,2…（结构级断言的一部分）。
        let starts: Vec<u64> = frames_of(&got, "content_block_start")
            .iter()
            .filter_map(|d| d["index"].as_u64())
            .collect();
        let want_starts: Vec<u64> = (0..starts.len() as u64).collect();
        check(
            &mut fails,
            starts == want_starts,
            format!("{tag}: content_block_start 的 index 不连续：{starts:?}"),
        );

        // 分场景的语义断言：不看黄金也能判对错。
        match scenario {
            "text" | "toolresult" => {
                let text = joined_text(&got);
                let want = if scenario == "text" {
                    "你好，这是 mock 上游的纯文本回放。"
                } else {
                    "上海现在晴，26 度。要不要带伞看你去哪。"
                };
                check(
                    &mut fails,
                    text == want,
                    format!("{tag}: 文本拼出来是 {text:?}，期望 {want:?}"),
                );
            }
            "tools" => {
                // 上游把 tool_use.input 分片发过来，拼完必须是完整合法的 JSON（票 30 第 2 条①）。
                let raw = joined_partial_json(&got);
                match serde_json::from_str::<Value>(&raw) {
                    Ok(v) => check(
                        &mut fails,
                        v == json!({"city": "上海"}),
                        format!("{tag}: 拼完的入参是 {v}，期望 {{\"city\":\"上海\"}}"),
                    ),
                    Err(e) => fails.push(format!("{tag}: 拼完的 partial_json 不是合法 JSON：{e}")),
                }
            }
            "usage" => {
                let deltas = frames_of(&got, "message_delta");
                let usage = deltas
                    .last()
                    .map(|d| d["usage"].clone())
                    .unwrap_or(Value::Null);
                check(
                    &mut fails,
                    usage["input_tokens"] == 100
                        && usage["output_tokens"] == 12
                        && usage["cache_read_input_tokens"] == 80,
                    format!("{tag}: message_delta.usage = {usage}"),
                );
            }
            "thinking" => {
                let sigs: Vec<String> = frames_of(&got, "content_block_delta")
                    .iter()
                    .filter(|d| d["delta"]["type"] == "signature_delta")
                    .filter_map(|d| d["delta"]["signature"].as_str().map(str::to_string))
                    .collect();
                if wire == "anthropic" {
                    check(
                        &mut fails,
                        sigs == vec!["bW9jay1zaWc=".to_string()],
                        format!("{tag}: Anthropic 直通必须保住签名，实际 {sigs:?}"),
                    );
                } else {
                    check(
                        &mut fails,
                        sigs.is_empty(),
                        format!("{tag}: Chat wire 换不回签名，不该有 signature_delta：{sigs:?}"),
                    );
                }
            }
            "abort" | "hardcut" => {
                let last = frames_of(&got, "error");
                check(
                    &mut fails,
                    last.len() == 1 && last[0]["error"]["type"] == "upstream_abort",
                    format!("{tag}: 断流必须给一个 upstream_abort 错误帧，实际 {last:?}"),
                );
                check(
                    &mut fails,
                    !names.iter().any(|n| n == "message_stop"),
                    format!("{tag}: 断流不该出现 message_stop"),
                );
            }
            "keepalive" => {
                // 真录字节里上游发过一个 event: ping，网关必须原样转给客户端而不是吞掉。
                let pings = frames_of(&got, "ping").len();
                check(
                    &mut fails,
                    pings == 1,
                    format!("{tag}: 期望恰好 1 个 ping 帧，实际 {pings}"),
                );
            }
            other => fails.push(format!("{tag}: 台账里出现了没写断言的场景 {other}")),
        }
    }

    assert!(
        fails.is_empty(),
        "台账矩阵 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ② 字节级怪癖：切块不变性（半帧 / 单字节 / CRLF）+ 头注入（quirk / 协议头 / 认证 / 路径模板）。
#[tokio::test(flavor = "multi_thread")]
async fn chunking_invariance_and_header_injection() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();

    let text_request = fixture_request(&gw, "text", "anthropic");
    let masks = vec!["message.id".to_string()];
    let mut baseline: Option<Vec<golden::Frame>> = None;
    for scheme in SCHEMES {
        gw.set_chunks(scheme).await;
        let (status, bytes) = gw.post_messages(&text_request).await;
        let frames = masked_frames(&bytes, &masks);
        check(
            &mut fails,
            status == 200,
            format!("切法 {scheme}: HTTP {status}"),
        );
        match &baseline {
            None => {
                check(
                    &mut fails,
                    frames.len() >= 6,
                    format!(
                        "切法 {scheme}: 只回了 {} 帧，基线不像一份完整应答",
                        frames.len()
                    ),
                );
                baseline = Some(frames);
            }
            Some(base) => {
                for d in golden::compare(&frames, base, &[]) {
                    fails.push(format!("切法 {scheme} 与 frame 基线不一致：{d}"));
                }
            }
        }
    }
    gw.set_chunks("frame").await;

    // ③ 头类 quirk（票 19 §6.3）：mock 回显的**出站请求头**必须真带上。
    let before = gw.echo().await["echo"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let body = |text: &str| {
        json!({"model":"mock-anthropic-text","max_tokens":256,"stream":true,
               "messages":[{"role":"user","content":text}]})
    };
    let protocol = [
        ("anthropic-version", "2023-06-01"),
        ("anthropic-beta", "prompt-caching-2024-07-31"),
    ];
    let _ = gw.post_messages_with(&body("同一段对话"), &protocol).await;
    let _ = gw.post_messages_with(&body("同一段对话"), &protocol).await;
    let _ = gw.post_messages_with(&body("另一段对话"), &protocol).await;
    // Chat wire 上协议头没有意义，必须**不**透传（README §2 第 6 条的边界）。
    let _ = gw
        .post_messages_with(
            &json!({"model":"mock-openai-text","max_tokens":256,"stream":true,
                    "messages":[{"role":"user","content":"跨 wire"}]}),
            &protocol,
        )
        .await;

    let echo = gw.echo().await;
    let entries: Vec<&Value> = echo["echo"]
        .as_array()
        .into_iter()
        .flatten()
        .skip(before)
        .collect();
    check(
        &mut fails,
        entries.len() == 4,
        format!("回显里应有 4 条本次请求，实际 {}", entries.len()),
    );
    let mut sessions: Vec<String> = Vec::new();
    for e in &entries {
        let h = &e["headers"];
        check(
            &mut fails,
            h["user-agent"] == "claude-gateway-prototype/0.0.1",
            format!("ClientUserAgent 没注入：user-agent = {}", h["user-agent"]),
        );
        let sid = h["x-opencode-session"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        check(
            &mut fails,
            sid.starts_with("gw-"),
            format!("SessionIdHeader 没注入：x-opencode-session = {sid:?}"),
        );
        check(
            &mut fails,
            h["authorization"] == "Bearer mock",
            format!("认证头不对：authorization = {}", h["authorization"]),
        );
        let anthropic_wire = e["wire"] == "anthropic";
        let want_path = if anthropic_wire {
            "/anthropic/v1/messages"
        } else {
            "/openai/v1/chat/completions"
        };
        check(
            &mut fails,
            e["path"] == want_path,
            format!("路径模板不对：{}，期望 {want_path}", e["path"]),
        );
        // 协议头透传：Anthropic 直通必须带，Chat wire 必须不带。
        for (k, v) in protocol {
            let got = h[k].as_str().unwrap_or("<缺>");
            if anthropic_wire {
                check(
                    &mut fails,
                    got == v,
                    format!("协议头 {k} 没透传：{got:?}，期望 {v:?}"),
                );
            } else {
                check(
                    &mut fails,
                    h.get(k).is_none(),
                    format!("协议头 {k} 不该在 Chat wire 上出现：{got:?}"),
                );
            }
        }
        if anthropic_wire {
            sessions.push(sid);
        }
    }
    if sessions.len() == 3 {
        check(
            &mut fails,
            sessions[0] == sessions[1],
            "同一段对话的会话头必须稳定".to_string(),
        );
        check(
            &mut fails,
            sessions[0] != sessions[2],
            "不同对话的会话头必须不同".to_string(),
        );
    }

    assert!(
        fails.is_empty(),
        "字节级怪癖与头注入 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ④ 工具往返（票 30 第 2 条）：多轮 `tool_result` 入站编到出站 wire 的位置，
/// 以及非流式路径下 partial `tool_use.input` 拼完是否完整合法。
#[tokio::test(flavor = "multi_thread")]
async fn tool_roundtrip_multiturn_and_partial_input() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();

    // ① Anthropic 直通：tool_result 仍挂在 user 轮的 content block 里，顺序不动。
    let _ = gw
        .post_messages(&fixture_request(&gw, "toolresult", "anthropic"))
        .await;
    let sent = last_echo(&gw).await["body"].clone();
    let msgs = sent["messages"].as_array().cloned().unwrap_or_default();
    check(
        &mut fails,
        msgs.len() == 3,
        format!("Anthropic 出站应有 3 轮，实际 {}", msgs.len()),
    );
    if msgs.len() == 3 {
        let blocks = msgs[2]["content"].as_array().cloned().unwrap_or_default();
        check(
            &mut fails,
            blocks.first().map(|b| b["type"].clone()) == Some(json!("tool_result")),
            format!("最后一轮首个块应是 tool_result：{blocks:?}"),
        );
        check(
            &mut fails,
            blocks.get(1).map(|b| b["type"].clone()) == Some(json!("text")),
            format!("同轮的文本块应排在 tool_result 之后：{blocks:?}"),
        );
        check(
            &mut fails,
            msgs[2]["role"] == "user",
            format!("tool_result 仍应留在 user 轮：{}", msgs[2]["role"]),
        );
    }

    // ② Chat wire：tool_result 必须提升成独立 role:"tool" 行，且排在同轮文本**之前**。
    let _ = gw
        .post_messages(&fixture_request(&gw, "toolresult", "openai"))
        .await;
    let sent = last_echo(&gw).await["body"].clone();
    let msgs = sent["messages"].as_array().cloned().unwrap_or_default();
    let roles: Vec<String> = msgs
        .iter()
        .filter_map(|m| m["role"].as_str().map(str::to_string))
        .collect();
    check(
        &mut fails,
        roles
            == vec![
                "system".to_string(),
                "user".into(),
                "assistant".into(),
                "tool".into(),
                "user".into(),
            ]
            || roles
                == vec![
                    "user".to_string(),
                    "assistant".into(),
                    "tool".into(),
                    "user".into(),
                ],
        format!("Chat 出站的轮次顺序不对：{roles:?}"),
    );
    let tool_row = msgs.iter().find(|m| m["role"] == "tool").cloned();
    match tool_row {
        Some(t) => {
            check(
                &mut fails,
                t["tool_call_id"] == "toolu_1" && t["content"] == "晴 26 度",
                format!("role:tool 行不对：{t}"),
            );
            let tool_at = msgs.iter().position(|m| m["role"] == "tool").unwrap_or(0);
            let last_user = msgs.iter().rposition(|m| m["role"] == "user").unwrap_or(0);
            check(
                &mut fails,
                tool_at < last_user,
                "role:tool 行必须排在同轮文本之前".to_string(),
            );
        }
        None => fails.push("Chat 出站里没有 role:tool 行".to_string()),
    }
    let assistant = msgs.iter().find(|m| m["role"] == "assistant").cloned();
    if let Some(a) = assistant {
        let call = &a["tool_calls"][0];
        check(
            &mut fails,
            call["function"]["name"] == "get_weather",
            format!("assistant 的 tool_calls 名字不对：{call}"),
        );
        check(
            &mut fails,
            call["function"]["arguments"] == "{\"city\":\"上海\"}",
            format!(
                "assistant 的 tool_calls 入参不对：{}",
                call["function"]["arguments"]
            ),
        );
    } else {
        fails.push("Chat 出站里没有 assistant 轮".to_string());
    }

    // ③ 非流式：网关把上游的 partial 分片拼完之后，`input` 必须是完整合法的 JSON。
    let body = json!({"model":"mock-anthropic-tools","max_tokens":256,"stream":false,
                      "messages":[{"role":"user","content":"你好"}]});
    let (status, v) = gw.post_json("/v1/messages", &body).await;
    check(
        &mut fails,
        status == 200,
        format!("非流式 tools: HTTP {status}"),
    );
    let tool_block = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|b| b["type"] == "tool_use")
        .cloned()
        .unwrap_or(Value::Null);
    check(
        &mut fails,
        tool_block["input"] == json!({"city": "上海"}),
        format!("非流式拼完的 tool_use.input 不对：{tool_block}"),
    );
    check(
        &mut fails,
        tool_block["id"] == "toolu_mock_1" && tool_block["name"] == "get_weather",
        format!("非流式 tool_use 的 id/name 不对：{tool_block}"),
    );

    assert!(
        fails.is_empty(),
        "工具往返 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑤ 错误信封与路由边界（票 30 第 4 条）：401 / 403 / 429 / 500 的状态码与错误类型、
/// 未知模型（含将来的名字）、`model` 大小写与首尾空白、入站鉴权缺失的显式声明。
#[tokio::test(flavor = "multi_thread")]
async fn error_envelopes_and_route_boundaries() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();

    let cases = [
        ("401", 401u16, "authentication_error"),
        ("403", 403, "permission_error"),
        ("429", 429, "rate_limit_error"),
        ("500", 500, "api_error"),
    ];
    for (code, want_status, want_kind) in cases {
        for wire in ["anthropic", "openai"] {
            let body = json!({
                "model": format!("mock-{wire}-err{code}"),
                "max_tokens": 256,
                "stream": true,
                "messages": [{"role":"user","content":"你好"}],
            });
            let (status, bytes) = gw.post_messages(&body).await;
            check(
                &mut fails,
                status == want_status,
                format!("err{code}/{wire}: HTTP {status}，期望 {want_status}"),
            );
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            check(
                &mut fails,
                v["type"] == "error" && v["error"]["type"] == want_kind,
                format!("err{code}/{wire}: 错误信封不对：{v}"),
            );
            check(
                &mut fails,
                !String::from_utf8_lossy(&bytes).contains("event:"),
                format!("err{code}/{wire}: 错误响应不该是 SSE"),
            );
        }
    }

    // 未知模型：包含 `claude-opus-6` 这类将来的名字 —— 都是 404 + not_found_error。
    for name in ["claude-opus-6", "根本没有这个模型", "mock-anthropic-txt"] {
        let body = json!({"model": name, "max_tokens": 256, "stream": true,
                          "messages":[{"role":"user","content":"你好"}]});
        let (status, bytes) = gw.post_messages(&body).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        check(
            &mut fails,
            status == 404 && v["error"]["type"] == "not_found_error",
            format!("未知模型 {name:?}: HTTP {status} / {v}"),
        );
    }

    // `model` 规则（票 30 第 4 条定案）：去首尾空白，大小写敏感。
    let ok = json!({"model":"  mock-anthropic-text  ","max_tokens":256,"stream":true,
                    "messages":[{"role":"user","content":"你好"}]});
    let (status, _) = gw.post_messages(&ok).await;
    check(
        &mut fails,
        status == 200,
        format!("首尾空白应被去掉后命中，实际 HTTP {status}"),
    );
    for name in ["MOCK-ANTHROPIC-TEXT", "Mock-Anthropic-Text"] {
        let body = json!({"model": name, "max_tokens": 256, "stream": true,
                          "messages":[{"role":"user","content":"你好"}]});
        let (status, _) = gw.post_messages(&body).await;
        check(
            &mut fails,
            status == 404,
            format!("大小写不同必须 404（ID 大小写敏感），{name} 实际 HTTP {status}"),
        );
    }

    // 入站鉴权缺失：本机单用户，**明确不鉴权**，所以不带任何凭据也必须通（这是声明，不是遗漏）。
    let anon = json!({"model":"mock-anthropic-text","max_tokens":256,"stream":true,
                      "messages":[{"role":"user","content":"无凭据"}]});
    let (status, _) = gw.post_messages(&anon).await;
    check(
        &mut fails,
        status == 200,
        format!("原型明确不鉴权，无凭据请求应 200，实际 {status}"),
    );

    assert!(
        fails.is_empty(),
        "错误信封与路由边界 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑥ 并发与会话隔离（票 19 §10）：两段不同对话并发不串会话头；同一段对话会话头一致；
/// 两个「客户端」同打一个槽位互不影响。
#[tokio::test(flavor = "multi_thread")]
async fn concurrency_and_session_isolation() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();
    let conv = |text: &str| {
        json!({"model":"mock-anthropic-text","max_tokens":256,"stream":true,
               "messages":[{"role":"user","content":text}]})
    };

    let jia = conv("并发甲");
    let yi = conv("并发乙");
    let (a, b) = tokio::join!(gw.post_messages(&jia), gw.post_messages(&yi));
    check(&mut fails, a.0 == 200, format!("并发甲 HTTP {}", a.0));
    check(&mut fails, b.0 == 200, format!("并发乙 HTTP {}", b.0));

    // 同一段对话连发两次：会话头必须一致。
    let _ = gw.post_messages(&jia).await;
    let _ = gw.post_messages(&jia).await;

    // 两个「客户端」同打一个槽位（用入站 user-agent 区分，路由不受它影响）。
    let ke_jia = conv("客户端甲");
    let ke_yi = conv("客户端乙");
    let (c, d) = tokio::join!(
        gw.post_messages_with(&ke_jia, &[("user-agent", "client-a/1.0")]),
        gw.post_messages_with(&ke_yi, &[("user-agent", "client-b/1.0")])
    );
    check(&mut fails, c.0 == 200, format!("客户端甲 HTTP {}", c.0));
    check(&mut fails, d.0 == 200, format!("客户端乙 HTTP {}", d.0));

    let echo = gw.echo().await;
    let entries: Vec<Value> = echo["echo"].as_array().cloned().unwrap_or_default();
    let session_of = |text: &str| -> Vec<String> {
        entries
            .iter()
            // 出站是 Anthropic wire：content 是块数组，文本在第一个块的 text 里。
            .filter(|e| e["body"]["messages"][0]["content"][0]["text"] == text)
            .filter_map(|e| {
                e["headers"]["x-opencode-session"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect()
    };
    for text in ["并发甲", "并发乙", "客户端甲", "客户端乙"] {
        let sids = session_of(text);
        check(
            &mut fails,
            !sids.is_empty(),
            format!("回显里找不到 {text} 的请求"),
        );
        check(
            &mut fails,
            sids.iter().all(|s| s == &sids[0]),
            format!("{text} 的会话头不稳定：{sids:?}"),
        );
    }
    let jia = session_of("并发甲");
    let yi = session_of("并发乙");
    if let (Some(x), Some(y)) = (jia.first(), yi.first()) {
        check(
            &mut fails,
            x != y,
            format!("两段不同对话的会话头串了：都是 {x}"),
        );
    }
    check(
        &mut fails,
        session_of("并发甲").len() == 3,
        "并发甲应有 3 条回显（并发一次 + 连发两次）".to_string(),
    );

    assert!(
        fails.is_empty(),
        "并发与会话隔离 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑦ 性能不变式（票 19 §10）：首帧延迟（证明无整包缓冲）+ 上游静默时的声明行为（向前发 ping）。
/// 不做吞吐基线。
#[tokio::test(flavor = "multi_thread")]
async fn first_frame_latency_and_silence_ping() {
    let gw = Gateway::start_with(&[("GATEWAY_PING_SECS", "1")]).await;
    let mut fails: Vec<String> = Vec::new();

    // ① 首帧延迟：text fixture 是多帧 + 每帧 25ms 的节奏，首块远早于读完就说明没有整包缓冲。
    let request = fixture_request(&gw, "text", "anthropic");
    let (status, bytes, first_ms, total_ms) = gw.post_messages_timed(&request).await;
    check(
        &mut fails,
        status == 200,
        format!("首帧延迟: HTTP {status}"),
    );
    check(
        &mut fails,
        first_ms * 2 < total_ms.max(1),
        format!("首帧延迟不变式不成立：首块 {first_ms}ms，读完 {total_ms}ms（像是整包缓冲）"),
    );
    check(
        &mut fails,
        !golden::parse(&bytes).is_empty(),
        "首帧延迟那一发没有帧".to_string(),
    );

    // ② 上游静默：mock 先回头再闭嘴 2.5 秒，网关必须向前发 ping，而不是让客户端干等。
    let hang = json!({"model":"mock-anthropic-hang","max_tokens":256,"stream":true,
                      "messages":[{"role":"user","content":"卡住"}]});
    let (status, bytes) = gw.post_messages(&hang).await;
    check(&mut fails, status == 200, format!("静默场景 HTTP {status}"));
    let got = golden::parse(&bytes);
    let names = event_names(&got);
    check(
        &mut fails,
        names.iter().filter(|n| *n == "ping").count() >= 1,
        format!("上游静默超过保活间隔时必须发 ping，实际帧序列 {names:?}"),
    );
    check(
        &mut fails,
        names.last().map(String::as_str) == Some("error"),
        format!(
            "静默后上游收尾，网关必须给错误帧，实际末帧 {:?}",
            names.last()
        ),
    );

    assert!(
        fails.is_empty(),
        "性能不变式 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑧ 探针档上限（票 26 决定 5 / 票 30 第 9 条）：`max_tokens <= 16` 的上游上限一到就 504，
/// 流式与非流式都算。旋钮是 `GATEWAY_PROBE_SECS`，不拨短就得真等 20 秒。
#[tokio::test(flavor = "multi_thread")]
async fn probe_tier_timeout_is_504() {
    let gw = Gateway::start_with(&[("GATEWAY_PROBE_SECS", "1"), ("GATEWAY_PING_SECS", "20")]).await;
    let mut fails: Vec<String> = Vec::new();

    for wire in ["anthropic", "openai"] {
        let probe = json!({
            "model": format!("mock-{wire}-hang-nohdr"),
            "max_tokens": 1,
            "stream": false,
            "messages": [{"role":"user","content":"."}],
        });
        let (status, v) = gw.post_json("/v1/messages", &probe).await;
        check(
            &mut fails,
            status == 504,
            format!("{wire} 非流式探针档: HTTP {status}，期望 504"),
        );
        check(
            &mut fails,
            v["error"]["type"] == "api_error"
                && v["error"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("超时"),
            format!("{wire} 非流式探针档: 错误信封不对：{v}"),
        );

        let streaming = json!({
            "model": format!("mock-{wire}-hang-nohdr"),
            "max_tokens": 1,
            "stream": true,
            "messages": [{"role":"user","content":"."}],
        });
        let (status, bytes) = gw.post_messages(&streaming).await;
        check(
            &mut fails,
            status == 504,
            format!("{wire} 流式探针档: HTTP {status}，期望 504"),
        );
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        check(
            &mut fails,
            v["error"]["type"] == "api_error",
            format!("{wire} 流式探针档: 错误信封不对：{v}"),
        );
    }

    assert!(
        fails.is_empty(),
        "探针档上限 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑨ `count_tokens` 与非流式（票 26 的行为，票 30 第 9 条补断言）。
#[tokio::test(flavor = "multi_thread")]
async fn count_tokens_and_non_streaming() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();
    let count_body =
        |model: &str| json!({"model": model, "messages":[{"role":"user","content":"你好 world"}]});

    // ① Anthropic wire 转上游：mock 固定回 4242（刻意与本地估算不同，用来分辨数字来源）。
    let (status, v) = gw
        .post_json(
            "/v1/messages/count_tokens",
            &count_body("mock-anthropic-text"),
        )
        .await;
    check(
        &mut fails,
        status == 200 && v["input_tokens"] == 4242,
        format!("转发路径: HTTP {status} / {v}，期望上游的 4242"),
    );

    // ② Chat wire 没有原生端点 → 本地估算（绝不该是 4242）。
    let (status, v) = gw
        .post_json("/v1/messages/count_tokens", &count_body("mock-openai-text"))
        .await;
    let n = v["input_tokens"].as_u64().unwrap_or(0);
    check(
        &mut fails,
        status == 200 && n > 0 && n != 4242,
        format!("本地估算路径: HTTP {status} / {v}（不该等于 4242）"),
    );

    // ③④ 上游 404 与「响应缺 input_tokens」都必须降级到本地估算，端点永不失败。
    for model in ["mock-anthropic-count404", "mock-anthropic-countnoinput"] {
        let (status, v) = gw
            .post_json("/v1/messages/count_tokens", &count_body(model))
            .await;
        let n = v["input_tokens"].as_u64().unwrap_or(0);
        check(
            &mut fails,
            status == 200 && n > 0 && n != 4242,
            format!("{model} 应降级本地估算: HTTP {status} / {v}"),
        );
    }

    // ⑤ 未知模型在 count_tokens 上同样是 404 信封。
    let (status, v) = gw
        .post_json("/v1/messages/count_tokens", &count_body("claude-opus-6"))
        .await;
    check(
        &mut fails,
        status == 404 && v["error"]["type"] == "not_found_error",
        format!("count_tokens 未知模型: HTTP {status} / {v}"),
    );

    // ⑥ 非流式：出站仍走 stream:true，网关缓冲到终态再组装完整 message。
    let (status, v) = gw
        .post_json(
            "/v1/messages",
            &json!({"model":"mock-anthropic-text","max_tokens":1,"stream":false,
                    "messages":[{"role":"user","content":"."}]}),
        )
        .await;
    check(&mut fails, status == 200, format!("非流式: HTTP {status}"));
    check(
        &mut fails,
        v["type"] == "message" && v["content"].is_array() && v["model"] == "mock-anthropic-text",
        format!("非流式必须回完整 message：{v}"),
    );
    check(
        &mut fails,
        v["content"][0]["text"] == "你好，这是 mock 上游的纯文本回放。",
        format!("非流式拼出来的文本不对：{}", v["content"][0]["text"]),
    );
    check(
        &mut fails,
        v["usage"]["output_tokens"].as_u64().unwrap_or(0) > 0,
        format!("非流式 usage 不该是空的：{v}"),
    );

    // ⑦ 非流式 + 上游断流（无终态）→ 502 + api_error 信封。
    let (status, v) = gw
        .post_json(
            "/v1/messages",
            &json!({"model":"mock-anthropic-abort","max_tokens":256,"stream":false,
                    "messages":[{"role":"user","content":"."}]}),
        )
        .await;
    check(
        &mut fails,
        status == 502 && v["error"]["type"] == "api_error",
        format!("非流式断流应 502 信封：HTTP {status} / {v}"),
    );

    assert!(
        fails.is_empty(),
        "count_tokens 与非流式 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑩ 日志与脱敏（票 19 §10）：默认不写 body、开关开了能捞到、到点自动关、库里与 API 里查不到密钥。
/// 票 41：日志从**库**里读（票 42 的 `Store` + `obs::recent`），不再经 `GET /api/logs`（归票 43 / 53）。
#[tokio::test(flavor = "multi_thread")]
async fn logs_are_metadata_only_and_redacted() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();
    let request = fixture_request(&gw, "text", "anthropic");

    // ① 默认只记元数据：body 列必须是空的。
    let _ = gw.post_messages(&request).await;
    let logs = gw.logs(1).await;
    let row = logs.first().cloned().unwrap_or(Value::Null);
    check(
        &mut fails,
        row["body"].is_null(),
        format!("默认不该写 body，实际 {}", row["body"]),
    );
    check(
        &mut fails,
        row["virtual_model"] == "mock-anthropic-text" && row["wire"] == "anthropic",
        format!("元数据不对：{row}"),
    );

    // ② 已知密钥反查：转发面响应与库文件里都不该出现（票 19 §10 ③）。
    let routes = gw.get_json("/v1/models").await.to_string();
    let logs_txt = Value::Array(logs.clone()).to_string();
    let db = gw.db_bytes();
    for secret in [SECRET_OPENCODE, SECRET_COMMANDCODE] {
        check(
            &mut fails,
            !routes.contains(secret),
            format!("密钥 {secret} 出现在 /v1/models 里"),
        );
        check(
            &mut fails,
            !logs_txt.contains(secret),
            format!("密钥 {secret} 出现在日志行里"),
        );
        check(
            &mut fails,
            !db.windows(secret.len()).any(|w| w == secret.as_bytes()),
            format!("密钥 {secret} 出现在库文件里"),
        );
    }

    // ③ 开关打开就能捞到 body，且是**发给上游的那一份**（出站请求体）。
    gw.set_body_log(2).await;
    let _ = gw.post_messages(&request).await;
    let row = gw.logs(2).await.first().cloned().unwrap_or(Value::Null);
    let body = row["body"].as_str().unwrap_or_default().to_string();
    check(
        &mut fails,
        body.contains("你好") && body.contains("mock-text"),
        format!("开关打开后 body 应该能捞到，实际 {body:?}"),
    );

    // ④ 到点自动关（真实窗口 1 小时，测试拨到 2 秒）。
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let _ = gw.post_messages(&request).await;
    let row = gw.logs(3).await.first().cloned().unwrap_or(Value::Null);
    check(
        &mut fails,
        row["body"].is_null(),
        format!("到点后不该再写 body，实际 {}", row["body"]),
    );

    assert!(
        fails.is_empty(),
        "日志与脱敏 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑫ Responses 入站（票 32）：Codex 走的那条路。开场必须是 `response.created`、收尾必须是
/// `response.completed`（少了它 Codex 判 `stream disconnected before completion`）；
/// 文本 / 推理 / 工具三类 item 各有各的 `.done`；非流式回完整 response 对象；
/// 错误回 OpenAI 信封；**绝不**伪造 `reasoning.encrypted_content`。
#[tokio::test(flavor = "multi_thread")]
async fn responses_inbound_contract() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();
    let body = |model: &str, stream: bool| {
        json!({
            "model": model, "stream": stream, "store": false,
            "include": ["reasoning.encrypted_content"],
            "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"."}]}],
            "tools": [{"type":"function","name":"get_weather","description":"d",
                       "parameters":{"type":"object","properties":{"city":{"type":"string"}}}}],
            "tool_choice": "auto", "parallel_tool_calls": false
        })
    };
    let items_of = |frames: &[golden::Frame]| -> Vec<Value> {
        frames_of(frames, "response.output_item.done")
            .iter()
            .map(|d| d["item"].clone())
            .collect()
    };
    let completed = |frames: &[golden::Frame]| -> Value {
        frames_of(frames, "response.completed")
            .last()
            .map(|d| d["response"].clone())
            .unwrap_or(Value::Null)
    };

    // ① 纯文本：首尾事件、增量拼接、终态对象里的 output 与 usage。
    let (status, bytes) = gw.post_responses(&body("mock-openai-text", true)).await;
    check(&mut fails, status == 200, format!("文本流: HTTP {status}"));
    let frames = golden::parse(&bytes);
    let names = event_names(&frames);
    check(
        &mut fails,
        names.first().map(String::as_str) == Some("response.created"),
        format!("首帧必须是 response.created，实际 {:?}", names.first()),
    );
    check(
        &mut fails,
        names.last().map(String::as_str) == Some("response.completed"),
        format!("末帧必须是 response.completed，实际 {:?}", names.last()),
    );
    let text: String = frames_of(&frames, "response.output_text.delta")
        .iter()
        .filter_map(|d| d["delta"].as_str())
        .collect();
    check(
        &mut fails,
        text == "你好，这是 mock 上游的纯文本回放。",
        format!("增量拼出来的文本不对：{text:?}"),
    );
    let resp = completed(&frames);
    check(
        &mut fails,
        resp["status"] == "completed" && resp["model"] == "mock-openai-text",
        format!("终态对象不对（只该暴露虚拟模型名）：{resp}"),
    );
    check(
        &mut fails,
        resp["output"][0]["content"][0]["text"] == "你好，这是 mock 上游的纯文本回放。",
        format!("终态 output 没带上完整消息：{}", resp["output"]),
    );
    check(
        &mut fails,
        resp["usage"]["input_tokens"].as_u64().unwrap_or(0) > 0
            && resp["usage"]["total_tokens"].as_u64().unwrap_or(0) > 0,
        format!("终态 usage 不对：{}", resp["usage"]),
    );

    // ② 工具调用：`function_call` 必须有 name / call_id / 可解析的 arguments，
    //    否则客户端配不上后面回来的 `function_call_output`。
    let (status, bytes) = gw.post_responses(&body("mock-openai-tools", true)).await;
    check(&mut fails, status == 200, format!("工具流: HTTP {status}"));
    let frames = golden::parse(&bytes);
    let items = items_of(&frames);
    match items.iter().find(|i| i["type"] == "function_call") {
        Some(c) => {
            check(
                &mut fails,
                c["name"] == "get_weather",
                format!("工具名不对：{c}"),
            );
            check(
                &mut fails,
                c["call_id"]
                    .as_str()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false),
                format!("function_call 缺 call_id：{c}"),
            );
            let args: Value = c["arguments"]
                .as_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            check(
                &mut fails,
                args["city"] == "上海",
                format!("arguments 不是合法 JSON 或缺 city：{c}"),
            );
        }
        None => fails.push(format!("没有 function_call item：{items:?}")),
    }
    check(
        &mut fails,
        completed(&frames)["output"]
            .as_array()
            .map(|o| o.iter().any(|i| i["type"] == "function_call"))
            .unwrap_or(false),
        "终态 output 里没有 function_call",
    );
    check(
        &mut fails,
        !frames_of(&frames, "response.function_call_arguments.delta").is_empty(),
        "工具调用的增量事件一帧都没发",
    );

    // ③ 推理项：只给 summary 文本，不给 encrypted_content。
    let (status, bytes) = gw.post_responses(&body("mock-openai-thinking", true)).await;
    check(&mut fails, status == 200, format!("推理流: HTTP {status}"));
    let items = items_of(&golden::parse(&bytes));
    match items.iter().find(|i| i["type"] == "reasoning") {
        Some(r) => {
            check(
                &mut fails,
                r["summary"][0]["text"].as_str().map(|s| !s.is_empty()) == Some(true),
                format!("reasoning item 的 summary 不该是空的：{r}"),
            );
            check(
                &mut fails,
                r.get("encrypted_content").is_none(),
                format!("上游不是 OpenAI，伪造 encrypted_content 只会骗自己：{r}"),
            );
        }
        None => fails.push(format!("没有 reasoning item：{items:?}")),
    }

    // ④ 非流式：出站照样 stream:true，网关缓冲到终态再组装完整 response。
    let (status, v) = gw
        .post_json("/v1/responses", &body("mock-openai-text", false))
        .await;
    check(
        &mut fails,
        status == 200 && v["object"] == "response" && v["status"] == "completed",
        format!("非流式: HTTP {status} / {v}"),
    );
    check(
        &mut fails,
        v["output"][0]["type"] == "message"
            && v["output"][0]["content"][0]["text"] == "你好，这是 mock 上游的纯文本回放。",
        format!("非流式 output 不对：{}", v["output"]),
    );
    check(
        &mut fails,
        v["usage"]["total_tokens"].as_u64().unwrap_or(0) > 0,
        format!("非流式 usage 不对：{}", v["usage"]),
    );

    // ⑤ 错误信封是 OpenAI 形状（Anthropic 那份 Codex 读不懂）。
    let (status, v) = gw
        .post_json("/v1/responses", &body("mock-openai-abort", false))
        .await;
    check(
        &mut fails,
        status == 502 && v["error"]["type"] == "api_error" && v["error"]["message"].is_string(),
        format!("非流式断流应 502 OpenAI 信封：HTTP {status} / {v}"),
    );
    let (status, v) = gw
        .post_json("/v1/responses", &body("claude-opus-does-not-exist", true))
        .await;
    check(
        &mut fails,
        status == 404 && v["error"]["type"] == "not_found_error" && v["type"].is_null(),
        format!("未知模型该回 OpenAI 信封：HTTP {status} / {v}"),
    );

    // ⑥ 续轮：`function_call` + `function_call_output` 成对回放，且必须真的成对送到上游。
    let (status, bytes) = gw
        .post_responses(&json!({
            "model":"mock-openai-text","stream":true,"store":false,
            "input":[
                {"type":"message","role":"user","content":[{"type":"input_text","text":"再查一次"}]},
                {"type":"function_call","name":"get_weather","arguments":"{\"city\":\"上海\"}","call_id":"call_1"},
                {"type":"function_call_output","call_id":"call_1","output":"晴 22 度"},
                // 推理项回放：没有可验证的加密内容，必须容忍（丢掉）而不是报错。
                {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"gAAAAA"}
            ],
            "tools":[{"type":"function","name":"get_weather","description":"d","parameters":{"type":"object"}}]
        }))
        .await;
    check(&mut fails, status == 200, format!("续轮: HTTP {status}"));
    let names = event_names(&golden::parse(&bytes));
    check(
        &mut fails,
        names.last().map(String::as_str) == Some("response.completed"),
        format!("续轮没走到 response.completed：{names:?}"),
    );
    let echo = last_echo(&gw).await;
    let outbound = echo["body"]["messages"].clone();
    let has_call = outbound
        .as_array()
        .map(|m| {
            m.iter()
                .any(|x| x["tool_calls"][0]["id"].as_str() == Some("call_1"))
        })
        .unwrap_or(false);
    let has_result = outbound
        .as_array()
        .map(|m| {
            m.iter()
                .any(|x| x["role"] == "tool" && x["tool_call_id"] == "call_1")
        })
        .unwrap_or(false);
    check(
        &mut fails,
        has_call && has_result,
        format!("function_call / function_call_output 没成对送到上游：{outbound}"),
    );

    assert!(
        fails.is_empty(),
        "Responses 入站 {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}

/// ⑪ 台账无缺口（票 19 §9 / 票 30 第 10 条 / 票 41 第 4 条）：原型 README §2 那 7 条
/// 「必须写进代码的差异」逐条对到一个真实存在的 fixture / 测试函数；空目标或「未覆盖」都算红。
/// 票 41：台账搬到了产品侧 `tests/LEDGER.md`（原型那份已冻结，只读）。
#[test]
fn ledger_has_no_gaps() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ledger = std::fs::read_to_string(manifest.join("tests").join("LEDGER.md"))
        .expect("读不到 crates/gateway-core/tests/LEDGER.md");

    // 「必须进代码的差异」清单本身必须是 7 条。
    let diff = section_of(&ledger, "## 差异清单", "## 台账");
    let items = diff
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.len() > 2
                && t.chars().next().map(|c| c.is_ascii_digit()) == Some(true)
                && t.contains(". ")
        })
        .count();
    assert_eq!(items, 7, "台账的差异清单应该有 7 条，实际 {items}");

    let table = section_of(&ledger, "## 台账", "## 明确不覆盖");
    let mut rows = 0usize;
    for line in table.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 3 || cells[0].starts_with('-') || cells[0] == "#" {
            continue;
        }
        rows += 1;
        let targets = cells[2];
        assert!(
            !targets.is_empty() && !targets.contains("未覆盖"),
            "台账第 {} 行还挂着未覆盖：{targets}",
            cells[0]
        );
        for target in targets.split('+').map(|s| s.trim().trim_matches('`')) {
            assert!(!target.is_empty(), "台账第 {} 行有空目标", cells[0]);
            let (file, symbol) = match target.split_once("::") {
                Some((f, s)) => (f, Some(s)),
                None => (target, None),
            };
            let path = manifest.join(file);
            assert!(path.exists(), "台账第 {} 行的目标不存在：{file}", cells[0]);
            if let Some(symbol) = symbol {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                assert!(
                    text.contains(&format!("fn {symbol}")),
                    "台账第 {} 行：{file} 里找不到 fn {symbol}",
                    cells[0]
                );
            }
        }
    }
    assert_eq!(rows, 7, "台账应有 7 行，实际 {rows}");
}

/// 取两个标题之间的那一段（标题行本身不含在内）。
fn section_of(text: &str, start: &str, end: &str) -> String {
    let Some(from) = text.find(start) else {
        return String::new();
    };
    let rest = &text[from..];
    let upto = rest.find(end).unwrap_or(rest.len());
    rest[..upto].to_string()
}

/// ⑬ 客户端档案覆盖与 `/v1/models` 的暴露集合（票 51；销掉票 30 台账里那条「明确不覆盖」）。
///
/// ① 同一个模型名 `claude-haiku`：codex 档案走 mock 的 Anthropic 面（200 + 真回放字节），
///    没有客户端标识的请求落回 `global`（commandcode，测试里 base 打不通 → 502）。
/// ② `/v1/models` **不按客户端过滤**（spec §8.2 第 2 条）：两个客户端拿到同一个模型集合，
///    只有信封形状按家族给（Claude 系是 `data` 那份，Codex 是标准 `models` 数组）。
#[tokio::test(flavor = "multi_thread")]
async fn client_profile_override_and_models_exposure() {
    let gw = Gateway::start().await;
    let mut fails: Vec<String> = Vec::new();

    let codex_ua = "codex_exec/0.162.1 (Windows 10.0.26200; x86_64) unknown (codex_exec; 0.162.1)";
    let code_ua = "claude-cli/2.1.299 (external, sdk-cli)";
    let slot = json!({"model":"claude-haiku","max_tokens":256,"stream":true,
                      "messages":[{"role":"user","content":"覆盖"}]});

    // ① codex 档案那份覆盖指向 mock → 走完一整条回放（文本就是 text fixture 的那句）。
    let (status, bytes) = gw
        .post_messages_with(&slot, &[("user-agent", codex_ua)])
        .await;
    check(
        &mut fails,
        status == 200,
        format!("codex 档案的 claude-haiku: HTTP {status}，期望 200"),
    );
    let text = joined_text(&golden::parse(&bytes));
    check(
        &mut fails,
        text == "你好，这是 mock 上游的纯文本回放。",
        format!("codex 档案没走到 mock 的 text 回放：{text:?}"),
    );

    // ② 同一个模型名、换成 Claude Code 的 UA：落回 global（commandcode，测试里打不通）→ 502。
    let (status, bytes) = gw
        .post_messages_with(&slot, &[("user-agent", code_ua)])
        .await;
    check(
        &mut fails,
        status == 502,
        format!("全局那份 claude-haiku 该打 commandcode（测试里不可达）→ 502，实际 {status}"),
    );
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    check(
        &mut fails,
        v["error"]["type"] == "api_error",
        format!("502 的错误信封不对：{v}"),
    );

    // ③ 不带任何客户端标识 = global，同上。
    let (status, _) = gw.post_messages(&slot).await;
    check(
        &mut fails,
        status == 502,
        format!("没有客户端标识该落回 global → 502，实际 {status}"),
    );

    // ④ `/v1/models`：Claude 系那份是 Anthropic 信封；Codex 那份是标准 models 数组。
    let anthropic = gw.get_json("/v1/models").await;
    let codex = gw
        .get_json_with("/v1/models", &[("user-agent", codex_ua)])
        .await;
    let ids = |v: &Value| -> Vec<String> {
        v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_string))
            .collect()
    };
    check(
        &mut fails,
        anthropic.get("has_more").is_some() && anthropic["first_id"] == json!("claude-fable"),
        format!("Claude 系的 /v1/models 信封不对：{anthropic}"),
    );
    check(
        &mut fails,
        codex["object"] == json!("list")
            && codex["data"][0]["object"] == json!("model")
            && codex["data"][0]["id"] == json!("claude-fable"),
        format!("Codex 的 /v1/models 该是标准 models 数组：{codex}"),
    );
    check(
        &mut fails,
        ids(&anthropic) == ids(&codex),
        format!(
            "两个客户端的暴露集合必须一致（不按客户端过滤）：{:?} vs {:?}",
            ids(&anthropic),
            ids(&codex)
        ),
    );
    let exposed = ids(&anthropic);
    check(
        &mut fails,
        exposed.starts_with(&[
            "claude-fable".to_string(),
            "claude-opus".into(),
            "claude-sonnet".into(),
            "claude-haiku".into(),
        ]),
        format!("四个家族名必须恒在且在最前：{exposed:?}"),
    );
    check(
        &mut fails,
        exposed.len() > 4 && exposed.contains(&"mock-anthropic-text".to_string()),
        format!("普通虚拟模型也该在清单里：{exposed:?}"),
    );

    assert!(
        fails.is_empty(),
        "客户端档案覆盖与 /v1/models {} 处不合格：\n{}",
        fails.len(),
        fails.join("\n")
    );
}
