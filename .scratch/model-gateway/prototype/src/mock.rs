//! mock 上游：按「场景 × wire」回放录制下来的 SSE 字节，末尾绝不补终态。
//! 用途是让编解码与转发在被外部网络和上游怪癖干扰之前先立住。
//!
//! 票 29 加了两件事：① 按 `meta.json` 的字节偏移切块发（半帧 / 单字节 / CRLF 靠它造出来）；
//! ② 把收到的请求（路径、头、body）回显进 `/api/echo` —— 头类 quirk 靠它才断言得到。
//!
//! 票 30 再加三件：① 错误信封负例（按 model 名直接回 401/403/429/500，不走 fixture）；
//! ② `hang`（一个字节都不发，验 ping 保活与探针档超时）；③ `hardcut`（发到中途硬切连接，
//! 验 `upstream.rs` 的传输错误分支——这不是「流正常结束」，是 TCP 级断）。

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::replay::Fixture;
use crate::App;

/// `hang` 保持沉默多久再收尾：够 `GATEWAY_PING_SECS`（测试拨到 1 秒）插进至少一帧 ping。
const HANG_SILENCE: Duration = Duration::from_millis(2500);

/// count_tokens 的假上游（票 26）：固定值刻意与本地估算不同，用来分辨数字到底来自哪里。
/// 票 30 第 9 条的两个降级负例也挂在这里：非 2xx 与「响应缺 input_tokens」都必须降级到本地估算。
pub async fn count(Json(body): Json<Value>) -> Response {
    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("");
    match model {
        "mock-count404" => (
            StatusCode::NOT_FOUND,
            Json(
                json!({"error":{"type":"not_found_error","message":"mock 没有 count_tokens 路由"}}),
            ),
        )
            .into_response(),
        "mock-countnoinput" => Json(json!({"ok": true})).into_response(),
        _ => Json(json!({"input_tokens": 4242})).into_response(),
    }
}

/// 上游错误信封负例（票 30 第 4 条）：状态码 → 上游侧的 Anthropic 错误类型。
/// 网关必须**保持状态码**并就地转成 Anthropic 信封（README §2 第 7 条）。
fn error_status(scenario: &str) -> Option<(StatusCode, &'static str)> {
    match scenario {
        "err401" => Some((StatusCode::UNAUTHORIZED, "authentication_error")),
        "err403" => Some((StatusCode::FORBIDDEN, "permission_error")),
        "err429" => Some((StatusCode::TOO_MANY_REQUESTS, "rate_limit_error")),
        "err500" => Some((StatusCode::INTERNAL_SERVER_ERROR, "api_error")),
        _ => None,
    }
}

/// 回放的切法由测试用 `POST /__chunks` 拨（票 29）：它是 mock 侧的控制面，
/// 不占真实 wire 的任何形状，也就不用假装某个头是「协议的一部分」。
fn scheme(app: &App) -> String {
    app.chunk_scheme
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "frame".into())
}

pub async fn replay(
    app: State<Arc<App>>,
    uri: axum::http::Uri,
    wire: &'static str,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let model = body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("mock-text")
        .to_string();
    let scenario = model
        .strip_prefix("mock-")
        .unwrap_or(model.as_str())
        .to_string();

    let echo = |extra: Value| {
        let mut base = json!({
            "path": uri.path().to_string(),
            "wire": wire,
            "scheme": scheme(&app),
            "model": model,
            "headers": headers
                .iter()
                .map(|(k, v)| (k.as_str().to_lowercase(), Value::String(v.to_str().unwrap_or("<binary>").to_string())))
                .collect::<serde_json::Map<String, Value>>(),
            "body": body,
        });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    };

    // 错误信封：上游侧的 JSON 错误体，不是 SSE。
    if let Some((status, kind)) = error_status(&scenario) {
        crate::echo_push(&app, echo(json!({"error_status": status.as_u16() })));
        return (
            status,
            Json(json!({
                "type": "error",
                "error": {"type": kind, "message": format!("mock 上游拒绝：{status}")}
            })),
        )
            .into_response();
    }

    // `hang`：先回响应头再静默到 `HANG_SILENCE`（验网关向前发 ping）。
    // `hang-nohdr`：连响应头都不回，直到超时才说话（验探针档上限 → 504，票 26 实测的那个形状）。
    if scenario == "hang" || scenario == "hang-nohdr" {
        crate::echo_push(&app, echo(json!({"hang": scenario})));
        if scenario == "hang-nohdr" {
            tokio::time::sleep(HANG_SILENCE).await;
        }
        let delay_body = scenario == "hang";
        let stream = futures_util::stream::once(async move {
            if delay_body {
                tokio::time::sleep(HANG_SILENCE).await;
            }
            Ok::<Vec<u8>, std::io::Error>(Vec::new())
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let fixture = match Fixture::load(&app.fixtures, &scenario, wire) {
        Ok(f) => f,
        Err(_) => {
            let path = app.fixtures.join(&scenario).join(format!("{wire}.sse"));
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error":{"type":"invalid_request_error","message":format!("没有 fixture {}", path.display())}})),
            )
                .into_response();
        }
    };

    // 回显：测试能力只放在 mock 里，不进网关的生产路径。
    let chunks = fixture.chunks(&scheme(&app));
    let hard_cut = scenario == "hardcut";
    crate::echo_push(
        &app,
        echo(json!({
            "fixture": fixture.path.display().to_string(),
            "chunks": chunks.len(),
            "hard_cut": hard_cut,
        })),
    );

    // 逐块之间的停顿只是模仿真实上游的节奏；单字节流那种几千块的切法直接连发，
    // 否则一条 fixture 要跑几十秒（切块要验的是「跨 chunk 半帧」，不是吞吐）。
    let per_chunk = if chunks.len() > 64 {
        Duration::ZERO
    } else {
        Duration::from_millis(25)
    };
    // `hardcut` 在字节发完之后**产一个 Err**：连接被硬切，不是「流正常结束」。
    let mut items: Vec<Result<Vec<u8>, std::io::Error>> = chunks.into_iter().map(Ok).collect();
    if hard_cut {
        items.push(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "mock 硬切连接",
        )));
    }
    let stream = futures_util::stream::iter(items).then(move |c| async move {
        tokio::time::sleep(per_chunk).await;
        c
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
