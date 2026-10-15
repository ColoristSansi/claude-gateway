//! 出站转发：IR -> 上游 wire 的请求，再把上游的 SSE 帧解回 IR、重编成 Anthropic SSE 发给客户端。
//! 原型刻意「解码再编码」而不是字节直通——直通更快，但那样 IR 就没被验证过。
//!
//! 票 41 从原型 `src/upstream.rs` 搬入，只换了三处落点：
//! ① 网关状态从原型的 `App` 换成 [`Gateway`](crate::ingress::proxy::Gateway)；
//! ② 日志落点从原型「每行单开连接直接写 sqlite」换成票 42 的 [`Store`] + [`obs`]
//!    （ADR-0007 第 6 条那笔债不搬）；
//! ③ 转发链、错误信封、ping 保活、探针 deadline 一字未改。

use std::{sync::Arc, time::Duration, time::Instant};

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::anthropic::{self, AnthropicAssembler, AnthropicDecoder, AnthropicEncoder};
use crate::egress::provider::{AuthScheme, EarlyReject, Quirk, Upstream, UpstreamModel, Wire};
use crate::egress::structured;
use crate::ingress::proxy::Gateway;
use crate::ir::{IrEvent, IrRequest};
use crate::obs::{self, RequestLog};
use crate::openai::{self, OpenAiDecoder};
use crate::responses::{ResponsesAssembler, ResponsesEncoder};
use crate::sse::{self, SseParser};
use crate::store::Store;

type ByteStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// 入站协议（客户端打进来的那一种）。出站 wire 由路由表给，两者解耦。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbound {
    Anthropic,
    Responses,
    /// 票 56：Ollama 兼容面（`/api/chat`）——NDJSON 传输、单层错误信封。
    Ollama,
}

impl Inbound {
    /// 日志与人话里用的名字。
    pub fn as_str(self) -> &'static str {
        match self {
            Inbound::Anthropic => "anthropic",
            Inbound::Responses => "responses",
            Inbound::Ollama => "ollama",
        }
    }

    /// 流式响应的 Content-Type：Ollama 面是 NDJSON，另两张脸是 SSE（spec §4.4）。
    fn stream_content_type(self) -> &'static str {
        match self {
            Inbound::Anthropic | Inbound::Responses => "text/event-stream",
            Inbound::Ollama => "application/x-ndjson",
        }
    }
}

impl Inbound {
    /// 入站协议对应的出站 wire：结构化输出的 `ResponseFormat::Other` 只在「与入站同形状」的
    /// wire 上原样带，这个映射就是那条判据（Ollama 面没有出站 wire，将来传 `None`）。
    pub fn format_origin(self) -> Option<Wire> {
        match self {
            Inbound::Anthropic => Some(Wire::Anthropic),
            Inbound::Responses => Some(Wire::OpenAiResponses),
            // Ollama 面只有入站、没有出站 wire（票 56）：`Other` 到它这里也没有「同形状」可言。
            Inbound::Ollama => None,
        }
    }
}

enum Decoder {
    A(AnthropicDecoder),
    O(OpenAiDecoder),
}

impl Decoder {
    fn decode(&mut self, f: &sse::Frame) -> Vec<IrEvent> {
        match self {
            Decoder::A(d) => d.decode(f),
            Decoder::O(d) => d.decode(f),
        }
    }
}

/// 入站编码器的形态（票 32 / 56）：同一批 IR 事件，编成 Anthropic SSE、Responses SSE 或
/// Ollama NDJSON。
enum ClientEncoder {
    A(AnthropicEncoder),
    R(ResponsesEncoder),
    L(crate::ingress::ollama::OllamaEncoder),
}

impl ClientEncoder {
    fn new(inbound: Inbound, model: String, freeform: Vec<String>) -> Self {
        match inbound {
            Inbound::Anthropic => ClientEncoder::A(AnthropicEncoder::new(model)),
            Inbound::Responses => ClientEncoder::R(ResponsesEncoder::new(model, freeform)),
            Inbound::Ollama => ClientEncoder::L(crate::ingress::ollama::OllamaEncoder::new(model)),
        }
    }

    fn push(&mut self, ev: IrEvent) -> String {
        match self {
            ClientEncoder::A(e) => e.push(ev),
            ClientEncoder::R(e) => e.push(ev),
            ClientEncoder::L(e) => e.push(ev),
        }
    }

    /// 上游静默到点时向前发的那一帧：SSE 两张脸发 ping，Ollama 面**没有保活帧型**（空串）。
    fn keepalive(&mut self) -> String {
        match self {
            ClientEncoder::A(_) | ClientEncoder::R(_) => {
                sse::frame("ping", &json!({"type":"ping"}).to_string())
            }
            ClientEncoder::L(_) => String::new(),
        }
    }
}

/// 非流式路径的组装器（同一批 IR 事件攒成完整应答体）。
enum ClientAssembler {
    A(AnthropicAssembler),
    R(ResponsesAssembler),
    L(crate::ingress::ollama::OllamaAssembler),
}

impl ClientAssembler {
    fn new(inbound: Inbound, freeform: Vec<String>) -> Self {
        match inbound {
            Inbound::Anthropic => ClientAssembler::A(AnthropicAssembler::default()),
            Inbound::Responses => ClientAssembler::R(ResponsesAssembler::new(freeform)),
            Inbound::Ollama => {
                ClientAssembler::L(crate::ingress::ollama::OllamaAssembler::default())
            }
        }
    }

    fn push(&mut self, ev: IrEvent) {
        match self {
            ClientAssembler::A(a) => a.push(ev),
            ClientAssembler::R(a) => a.push(ev),
            ClientAssembler::L(a) => a.push(ev),
        }
    }

    fn ended(&self) -> bool {
        match self {
            ClientAssembler::A(a) => a.ended(),
            ClientAssembler::R(a) => a.ended(),
            ClientAssembler::L(a) => a.ended(),
        }
    }

    fn error(&self) -> Option<(String, String)> {
        match self {
            ClientAssembler::A(a) => a.error(),
            ClientAssembler::R(a) => a.error(),
            ClientAssembler::L(a) => a.error(),
        }
    }

    /// 四项 usage：`(输入, 输出, 缓存读, 缓存写)`（票 58）。
    fn usage(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        match self {
            ClientAssembler::A(a) => a.usage(),
            ClientAssembler::R(a) => a.usage(),
            ClientAssembler::L(a) => a.usage(),
        }
    }

    fn body(&self, model: &str) -> Value {
        match self {
            ClientAssembler::A(a) => a.message(model),
            ClientAssembler::R(a) => a.response(model),
            ClientAssembler::L(a) => a.body(model),
        }
    }
}

/// 请求期早拒（ADR-0004 第 4 条的第二层）：目录条目声明过这个模型的 wire 集合时，wire 必须
/// 在集合里；未声明（空集合）是正常态，放行交给自检；目录里没有这条（回放 harness 的脚手架
/// 场景）也不拦——那是票 51 的绑定层该保证的事（绑定必须先有目录条目）。
pub fn guard(entry: Option<&UpstreamModel>, wire: Wire) -> Result<(), EarlyReject> {
    match entry {
        Some(e) => e.validate_wire(wire).map(|_| ()),
        None => Ok(()),
    }
}

/// 出站请求的唯一组装点：认证、协议头透传、quirk 注入。流式 / 非流式 / 计数 / 自检探针
/// 四个出口共用，免得「某条出口忘了带 anthropic-beta」这种事故。
///
/// 凭据按「上游 × wire」由调用方取好再传进来（`provider::wire_credential`），
/// 声明式表本身**不存密钥**。只收 `reqwest::Client`（不收整个 `Gateway`）：绑定层的自检探针
/// 也要用同一个组装点，而绑定层不能反过来依赖转发面的状态。
#[allow(clippy::too_many_arguments)]
pub(crate) fn request(
    client: &reqwest::Client,
    up: &Upstream,
    wire: Wire,
    auth: AuthScheme,
    key: &str,
    url: &str,
    body: &Value,
    seed: &str,
    pass_headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    let mut rb = client.post(url).json(body);
    rb = match auth {
        AuthScheme::Bearer => rb.header("authorization", format!("Bearer {key}")),
        AuthScheme::XApiKey => rb.header("x-api-key", key.to_string()),
    };
    // 协议头只在 Anthropic 直通时透传；换成 OpenAI wire 时它们没有意义。
    if wire == Wire::Anthropic {
        for (k, v) in pass_headers {
            rb = rb.header(k, v);
        }
    }
    for q in &up.quirks {
        match q {
            Quirk::ClientUserAgent(ua) => rb = rb.header("user-agent", ua.clone()),
            Quirk::SessionIdHeader(h) => rb = rb.header(h.clone(), up.session_id(seed)),
            _ => {}
        }
    }
    rb
}

/// 上游 Responses 面（`/responses`）的出站编解码还没做（M3）：早报清楚，别静默当 Chat 打。
fn wire_not_implemented(wire: Wire, inbound: Inbound, up_id: &str) -> Response {
    error_response_for(
        inbound,
        StatusCode::BAD_GATEWAY,
        "api_error",
        &format!(
            "上游 {up_id} 的 {} 面尚未实现（M3 起接）；请先把该模型绑到已实现的 wire 上。",
            wire.as_str()
        ),
    )
}

/// 探针档 `max_tokens`：这一档基本都是健康探针（Desktop 的推理探针是 `max_tokens:1`），
/// 实测有的 (上游, wire, 模型) 组合会在这一档**挂住不返回**（commandcode 的 Chat wire +
/// deepseek 直连 curl 也复现），所以只给这一档加短上限，普通生成长度不受影响。
/// 票 26 决定 5：统一超时不加，只封这一档。上限值由 `GATEWAY_PROBE_SECS` 给（默认 20 秒，
/// 票 30 第 9 条给测试留的旋钮——不拨短的话那条断言要真等 20 秒）。
pub const PROBE_MAX_TOKENS: u64 = 16;

/// 探针档的上游上限（秒）：超时 504（spec §4.5 / 票 26 决定 5）。
/// 绑定层的自检探针（`binding::selfcheck`）与转发面共用这一个值。
pub const PROBE_DEADLINE: Duration = Duration::from_secs(20);

fn with_deadline(
    rb: reqwest::RequestBuilder,
    max_tokens: u64,
    probe: Duration,
) -> reqwest::RequestBuilder {
    if max_tokens <= PROBE_MAX_TOKENS {
        rb.timeout(probe)
    } else {
        rb
    }
}

/// 上游失败的两个出口：超时给 504、连不上给 502，别把「等太久」说成「连不上」。
fn upstream_error(e: &reqwest::Error, url: &str, probe: Duration) -> (StatusCode, String) {
    if e.is_timeout() {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("上游超时（{} 秒内无响应）：{url}", probe.as_secs()),
        )
    } else {
        (StatusCode::BAD_GATEWAY, format!("连不上上游 {url}: {e}"))
    }
}

/// 出站永远是流式（票 26 决定 3）：非流式只是把这条流缓到终态再组装完整 message。
/// 好处是编解码面只有一套，Chat wire 迟到的 usage 也正好在收敛后拿全。
#[allow(clippy::too_many_arguments)]
pub async fn forward_buffered(
    app: Arc<Gateway>,
    up: &Upstream,
    wire: Wire,
    auth: AuthScheme,
    key: &str,
    req: IrRequest,
    client_model: String,
    // 客户端档案 id（票 51）：只进日志行，转发逻辑不看它。
    client: String,
    pass_headers: Vec<(String, String)>,
    inbound: Inbound,
) -> Response {
    let Some(url) = up.url(wire) else {
        return wire_not_implemented(wire, inbound, &up.id);
    };
    let mut body = match wire {
        Wire::Anthropic => anthropic::encode_request(&req, true),
        Wire::OpenAiChat => openai::encode_request(&req, true),
        Wire::OpenAiResponses => return wire_not_implemented(wire, inbound, &up.id),
    };
    // 票 57：结构化输出的两条请求期 400（表达不了 / 能力位 ignore）在这里落地：body 就在这一步
    // 组装，所以判据也只能在这里——关键是它排在**发出任何字节之前**。
    if let Err(rej) = structured::apply(
        &mut body,
        req.format.as_ref(),
        wire,
        up.structured_output(wire),
        inbound.format_origin(),
    ) {
        return error_response_for(
            inbound,
            StatusCode::from_u16(rej.status).unwrap_or(StatusCode::BAD_REQUEST),
            rej.error_type,
            &rej.message,
        );
    }
    let t0 = Instant::now();
    let sent = with_deadline(
        request(
            &app.client,
            up,
            wire,
            auth,
            key,
            &url,
            &body,
            &first_user_text(&req),
            &pass_headers,
        ),
        req.max_tokens,
        app.probe,
    )
    .send()
    .await;

    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            let (status, msg) = upstream_error(&e, &url, app.probe);
            log_plain(
                &app.store,
                &up.id,
                &req.model,
                wire,
                status.as_u16() as i64,
                &client_model,
                &client,
                Some(msg.clone()),
            )
            .await;
            return error_response_for(inbound, status, "api_error", &msg);
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = format!("上游 {status}：{}", truncate(&text, 600));
        log_plain(
            &app.store,
            &up.id,
            &req.model,
            wire,
            status.as_u16() as i64,
            &client_model,
            &client,
            Some(msg.clone()),
        )
        .await;
        let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return error_response_for(inbound, code, error_type_for(status.as_u16()), &msg);
    }

    let mut dec = match wire {
        Wire::Anthropic => Decoder::A(AnthropicDecoder),
        Wire::OpenAiChat => Decoder::O(OpenAiDecoder::default()),
        Wire::OpenAiResponses => return wire_not_implemented(wire, inbound, &up.id),
    };
    let mut parser = SseParser::default();
    let mut asm = ClientAssembler::new(inbound, req.freeform_tools.clone());
    let mut transport = None;
    // 响应侧记录的决定在**开流时**定一次（票 59）：窗口中途到点也不该把这一发记成半截。
    let recording = app.body_log_on();
    let mut sse_raw = String::new();
    let mut frames: Vec<obs::FrameLog> = Vec::new();
    {
        let mut bytes = resp.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            match chunk {
                Ok(c) => {
                    if recording && sse_raw.len() < MAX_SSE_BYTES {
                        sse_raw.push_str(&String::from_utf8_lossy(&c));
                    }
                    for f in parser.push(&c) {
                        if recording && frames.len() < MAX_FRAMES {
                            let seq = frames.len() as i64;
                            frames.push(frame_log(seq, t0.elapsed().as_millis() as i64, &f));
                        }
                        for ev in dec.decode(&f) {
                            asm.push(ev);
                        }
                    }
                }
                Err(e) => {
                    transport = Some(format!("上游传输中断: {e}"));
                    break;
                }
            }
        }
    }
    if transport.is_none() {
        for f in parser.finish() {
            for ev in dec.decode(&f) {
                asm.push(ev);
            }
        }
    }

    let total_ms = t0.elapsed().as_millis() as i64;
    let (usage_in, usage_out, cache_read, cache_write) = asm.usage();
    let failure = match (asm.error(), transport) {
        (Some((kind, message)), _) => Some((StatusCode::BAD_GATEWAY, kind, message)),
        (None, Some(message)) => Some((StatusCode::BAD_GATEWAY, "api_error".into(), message)),
        (None, None) if !asm.ended() => Some((
            StatusCode::BAD_GATEWAY,
            "api_error".into(),
            "上游在终态事件前断流（无 message_stop / 无 [DONE]）".into(),
        )),
        (None, None) => None,
    };
    let row = RequestLog {
        ts: 0,
        request_id: None,
        virtual_model: client_model.clone(),
        client: Some(client.clone()),
        upstream_id: up.id.clone(),
        real_model: req.model.clone(),
        wire: wire.as_str().into(),
        status: failure
            .as_ref()
            .map(|(s, _, _)| s.as_u16() as i64)
            .unwrap_or(200),
        first_byte_ms: total_ms,
        total_ms,
        input_tokens: usage_in,
        output_tokens: usage_out,
        cache_read,
        cache_write,
        aborted: failure.is_some(),
        error: failure.as_ref().map(|(_, _, m)| m.clone()),
        body: recording.then(|| body.to_string()),
        response_body: recording.then_some(sse_raw),
        frames,
    };
    let _ = obs::log(&app.store, &row).await;

    match failure {
        Some((status, kind, message)) => error_response_for(inbound, status, &kind, &message),
        None => axum::Json(asm.body(&client_model)).into_response(),
    }
}

/// `count_tokens` 的转发（票 26 决定 1）：只有 Anthropic wire 有原生端点，路径是
/// `/messages` 的兄弟 `/messages/count_tokens`。任何失败都返回 `None`，由调用方降级到本地估算。
#[allow(clippy::too_many_arguments)]
pub async fn count_tokens_upstream(
    app: &Gateway,
    up: &Upstream,
    auth: AuthScheme,
    key: &str,
    req: &IrRequest,
    pass_headers: &[(String, String)],
) -> Option<u64> {
    // count_tokens 是 `/messages` 的兄弟：按 Anthropic 路径模板推，模板缺失就没有上游计数。
    let url = format!("{}/count_tokens", up.url(Wire::Anthropic)?);
    let body = anthropic::encode_count_request(req);
    // shortcut: 计数是辅助端点，10 秒封顶，不让它挂住客户端；要更久就改这里。
    let resp = request(
        &app.client,
        up,
        Wire::Anthropic,
        auth,
        key,
        &url,
        &body,
        &first_user_text(req),
        pass_headers,
    )
    .timeout(std::time::Duration::from_secs(10))
    .send()
    .await
    .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    v.get("input_tokens").and_then(Value::as_u64)
}

#[allow(clippy::too_many_arguments)]
pub async fn forward(
    app: Arc<Gateway>,
    up: &Upstream,
    wire: Wire,
    auth: AuthScheme,
    key: &str,
    req: IrRequest,
    client_model: String,
    // 客户端档案 id（票 51）：只进日志行，转发逻辑不看它。
    client: String,
    pass_headers: Vec<(String, String)>,
    inbound: Inbound,
) -> Response {
    let Some(url) = up.url(wire) else {
        return wire_not_implemented(wire, inbound, &up.id);
    };
    let mut body = match wire {
        Wire::Anthropic => anthropic::encode_request(&req, true),
        Wire::OpenAiChat => openai::encode_request(&req, true),
        Wire::OpenAiResponses => return wire_not_implemented(wire, inbound, &up.id),
    };
    // 票 57：与 `forward_buffered` 同一套判据（两条出口共用，免得某条忘了拦）。
    if let Err(rej) = structured::apply(
        &mut body,
        req.format.as_ref(),
        wire,
        up.structured_output(wire),
        inbound.format_origin(),
    ) {
        return error_response_for(
            inbound,
            StatusCode::from_u16(rej.status).unwrap_or(StatusCode::BAD_REQUEST),
            rej.error_type,
            &rej.message,
        );
    }

    let seed = first_user_text(&req);
    let rb = with_deadline(
        request(
            &app.client,
            up,
            wire,
            auth,
            key,
            &url,
            &body,
            &seed,
            &pass_headers,
        ),
        req.max_tokens,
        app.probe,
    );

    let t0 = Instant::now();
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            let (status, msg) = upstream_error(&e, &url, app.probe);
            log_plain(
                &app.store,
                &up.id,
                &req.model,
                wire,
                status.as_u16() as i64,
                &client_model,
                &client,
                Some(msg.clone()),
            )
            .await;
            return error_response_for(inbound, status, "api_error", &msg);
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = format!("上游 {status}：{}", truncate(&text, 600));
        log_plain(
            &app.store,
            &up.id,
            &req.model,
            wire,
            status.as_u16() as i64,
            &client_model,
            &client,
            Some(msg.clone()),
        )
        .await;
        let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return error_response_for(inbound, code, error_type_for(status.as_u16()), &msg);
    }

    let dec = match wire {
        Wire::Anthropic => Decoder::A(AnthropicDecoder),
        Wire::OpenAiChat => Decoder::O(OpenAiDecoder::default()),
        Wire::OpenAiResponses => return wire_not_implemented(wire, inbound, &up.id),
    };
    let recording = app.body_log_on();
    let pump = Pump {
        inner: Box::pin(resp.bytes_stream()),
        parser: SseParser::default(),
        dec,
        enc: ClientEncoder::new(inbound, client_model.clone(), req.freeform_tools.clone()),
        done: false,
        logged: false,
        terminated: false,
        t0,
        first_byte_ms: None,
        ping: app.ping,
        probe: app.probe,
        store: app.store.clone(),
        recording,
        sse_raw: String::new(),
        row: RequestLog {
            ts: 0,
            virtual_model: client_model,
            client: Some(client),
            upstream_id: up.id.clone(),
            real_model: req.model.clone(),
            wire: wire.as_str().into(),
            status: 200,
            body: recording.then(|| body.to_string()),
            ..Default::default()
        },
    };

    let stream = futures_util::stream::unfold(pump, |mut p| async move {
        loop {
            if p.done {
                return None;
            }
            // 上游静默到点就向前发一帧 ping（票 30 第 6 条）：不给上游加超时（票 26 决定 5），
            // 只让客户端看到连接还活着，而不是干等。
            let next = match tokio::time::timeout(p.ping, p.inner.next()).await {
                Ok(next) => next,
                Err(_) => {
                    let ping = p.enc.keepalive();
                    // Ollama 面没有保活帧型：静默就继续等，不向前发空帧。
                    if ping.is_empty() {
                        continue;
                    }
                    return Some((Ok::<String, std::io::Error>(ping), p));
                }
            };
            match next {
                Some(Ok(chunk)) => {
                    let out = p.pump(&chunk);
                    if !out.is_empty() {
                        return Some((Ok::<String, std::io::Error>(out), p));
                    }
                }
                Some(Err(e)) => {
                    p.done = true;
                    let out = p.fail(&if e.is_timeout() {
                        format!("上游超时（探针档 {} 秒内没走完）：{e}", p.probe.as_secs())
                    } else {
                        format!("上游传输中断: {e}")
                    });
                    return Some((Ok(out), p));
                }
                None => {
                    p.done = true;
                    let out = p.finish();
                    return Some((Ok(out), p));
                }
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, inbound.stream_content_type())
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            error_response_for(
                inbound,
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "构建响应失败",
            )
        })
}

struct Pump {
    inner: ByteStream,
    parser: SseParser,
    dec: Decoder,
    enc: ClientEncoder,
    done: bool,
    logged: bool,
    /// 是否已经给客户端发过终态（正常 message_stop 或 error）。
    terminated: bool,
    t0: Instant,
    first_byte_ms: Option<i64>,
    /// 上游静默超过它就发一帧 ping（票 30 第 6 条）。
    ping: Duration,
    /// 探针档上限，只用来在错误文案里报秒数（票 30 第 9 条）。
    probe: Duration,
    store: Store,
    row: RequestLog,
    /// body 开关开着时才记的响应侧（票 59）：原始 SSE 文本 + 逐帧元数据。
    recording: bool,
    /// 上游回来的原始字节（按文本累积）：详情第三段「原始 SSE 文本」就是它。
    sse_raw: String,
}

/// 帧与原文的上限（票 59）：一条长回复可以几千帧、几兆字节，全存会把库撑爆。
/// shortcut: 超出就丢后面的帧 / 原文尾部（够看清形状与错误现场）；要全量另开票。
const MAX_FRAMES: usize = 500;
const MAX_SSE_BYTES: usize = 256 * 1024;

/// 一帧 → 详情三段里的那行（时间戳相对本次请求开始）。
fn frame_log(seq: i64, at_ms: i64, f: &sse::Frame) -> obs::FrameLog {
    let event = f.event.clone();
    obs::FrameLog {
        seq,
        at_ms,
        summary: truncate(&f.data, 200),
        raw: sse::frame(event.as_deref().unwrap_or("message"), &f.data),
        event,
    }
}

impl Pump {
    fn handle(&mut self, f: &sse::Frame, out: &mut String) {
        self.record(f);
        for ev in self.dec.decode(f) {
            match &ev {
                // Anthropic 的输入 token 在 `message_start` 里报（非流式那条路早就取了它，
                // 流式这条路原先漏了）：只在 `message_delta` 重复报一次时才落库，
                // 上游不重复报就整条输入 token 变 NULL、用量页少算一半。
                IrEvent::Start { input_tokens, .. } => {
                    self.row.input_tokens = input_tokens.or(self.row.input_tokens);
                }
                IrEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                } => {
                    self.row.input_tokens = input_tokens.or(self.row.input_tokens);
                    self.row.output_tokens = output_tokens.or(self.row.output_tokens);
                    self.row.cache_read = cache_read_input_tokens.or(self.row.cache_read);
                    self.row.cache_write = cache_creation_input_tokens.or(self.row.cache_write);
                }
                IrEvent::Eos | IrEvent::Error { .. } => self.terminated = true,
                _ => {}
            }
            out.push_str(&self.enc.push(ev));
        }
    }

    fn pump(&mut self, chunk: &[u8]) -> String {
        // 帧边界由解析器切，但「原始 SSE 文本」要的是**上游原样字节**（含注释帧、CRLF），
        // 所以另攒一份：解析后的帧重建不出原样的分隔符。
        if self.recording && self.sse_raw.len() < MAX_SSE_BYTES {
            self.sse_raw.push_str(&String::from_utf8_lossy(chunk));
        }
        let frames = self.parser.push(chunk);
        let mut out = String::new();
        for f in &frames {
            self.handle(f, &mut out);
        }
        if !out.is_empty() && self.first_byte_ms.is_none() {
            self.first_byte_ms = Some(self.t0.elapsed().as_millis() as i64);
        }
        out
    }

    /// 记一帧（只在该次请求开着 body 开关时；上限见 [`MAX_FRAMES`]）。
    fn record(&mut self, f: &sse::Frame) {
        if !self.recording || self.row.frames.len() >= MAX_FRAMES {
            return;
        }
        let seq = self.row.frames.len() as i64;
        let at_ms = self.t0.elapsed().as_millis() as i64;
        self.row.frames.push(frame_log(seq, at_ms, f));
    }

    /// 上游断在终态之前：必须给客户端一个 error 帧，不能静默收尾。
    fn finish(&mut self) -> String {
        let mut out = String::new();
        for f in &self.parser.finish() {
            self.handle(f, &mut out);
        }
        if !self.terminated {
            out.push_str(&self.fail("上游在终态事件前断流（无 message_stop / 无 [DONE]）"));
        } else {
            self.write_log(false);
        }
        out
    }

    fn fail(&mut self, msg: &str) -> String {
        self.terminated = true;
        self.row.aborted = true;
        self.row.error = Some(msg.to_string());
        let out = self.enc.push(IrEvent::Error {
            kind: "upstream_abort".into(),
            message: msg.to_string(),
        });
        self.write_log(true);
        out
    }

    fn write_log(&mut self, aborted: bool) {
        if self.logged {
            return;
        }
        self.logged = true;
        self.row.aborted |= aborted;
        self.row.first_byte_ms = self.first_byte_ms.unwrap_or(-1);
        self.row.total_ms = self.t0.elapsed().as_millis() as i64;
        if self.recording {
            self.row.response_body = Some(std::mem::take(&mut self.sse_raw));
        }
        write_log(&self.store, self.row.clone());
    }
}

/// 日志落点（票 42 的 `Store` 是异步写）：写线程收下就返回，不阻塞转发。
/// `Drop` 里也要能落（客户端提前断开），所以走 `spawn`；没有 runtime 就放弃这一行日志。
fn write_log(store: &Store, row: RequestLog) {
    let store = store.clone();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            // 写失败要留痕：静默丢掉一行日志，界面会以为「这次请求没发生」。
            if let Err(e) = obs::log(&store, &row).await {
                eprintln!("[warn] 请求日志写不进去：{e}");
            }
        });
    }
}

impl Drop for Pump {
    /// 客户端提前断开时 unfold 被丢弃，这里补一次日志，否则这次请求查不到。
    fn drop(&mut self) {
        if !self.logged {
            self.write_log(true);
        }
    }
}

/// 转发还没进流就失败时的日志行（连接失败 / 上游非 2xx）：这一发没有终态可等，一个字段一个字段地拼。
#[allow(clippy::too_many_arguments)]
async fn log_plain(
    store: &Store,
    upstream_id: &str,
    real_model: &str,
    wire: Wire,
    status: i64,
    client_model: &str,
    client: &str,
    error: Option<String>,
) {
    let _ = obs::log(
        store,
        &RequestLog {
            virtual_model: client_model.to_string(),
            client: Some(client.to_string()),
            upstream_id: upstream_id.to_string(),
            real_model: real_model.to_string(),
            wire: wire.as_str().into(),
            status,
            error,
            aborted: status != 200,
            ..Default::default()
        },
    )
    .await;
}

fn first_user_text(req: &IrRequest) -> String {
    for m in &req.messages {
        if m.role != "user" {
            continue;
        }
        for b in &m.blocks {
            if let crate::ir::IrBlock::Text { text, .. } = b {
                return truncate(text, 200);
            }
        }
    }
    req.model.clone()
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn error_type_for(status: u16) -> &'static str {
    match status {
        400 | 404 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        429 => "rate_limit_error",
        _ => "api_error",
    }
}

pub fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({"type":"error","error":{"type":kind,"message":message}})),
    )
        .into_response()
}

/// 错误信封按**入站**协议给形状：Anthropic 是 `{"type":"error","error":{…}}`，
/// OpenAI 系是 `{"error":{"message","type","code"}}`。Codex 只认后者，回错了它就只能
/// 抛一句无语境的「stream disconnected」（票 32）。
///
/// Ollama 面（票 56）是**第三种**：单层 `{"error":"<字符串>"}`（spec §4.4）——
/// 与 OpenAI 的嵌套形状不能混用。
pub fn error_response_for(
    inbound: Inbound,
    status: StatusCode,
    kind: &str,
    message: &str,
) -> Response {
    match inbound {
        Inbound::Anthropic => error_response(status, kind, message),
        Inbound::Ollama => (status, axum::Json(json!({"error": message}))).into_response(),
        Inbound::Responses => (
            status,
            axum::Json(json!({"error":{"message":message,"type":kind,"code":Value::Null}})),
        )
            .into_response(),
    }
}
