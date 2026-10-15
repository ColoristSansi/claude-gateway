//! mock 上游（票 29 / 票 30 / 票 41）：按「场景 × wire」回放录制下来的 SSE 字节，末尾绝不补终态。
//! 用途是让编解码与转发在被外部网络和上游怪癖干扰之前先立住。
//!
//! **独立进程 + 测试专用**（票 41 的硬要求）：
//!
//! - 它是**自己的 bin**，挂 `required-features = ["mock"]`，所以 `cargo build -p gateway-core`
//!   与产品构建（`claude-gateway`）的产物里没有 `/__chunks`、`/api/echo` 这些字符串，
//!   网关状态上也没有 `echo` / `chunk_scheme` 字段。
//! - 网关与 mock **分进程**：网关侧只按 `base_url` 打 HTTP，不知道回放的存在。
//! - 回放切法与请求回显是**mock 自己的控制面**（`POST /__chunks` / `GET /api/echo`），
//!   不占真实 wire 的任何形状，也就不用假装某个头是「协议的一部分」。
//!
//! 票 29 加了两件事：① 按 `meta.json` 的字节偏移切块发（半帧 / 单字节 / CRLF 靠它造出来）；
//! ② 把收到的请求（路径、头、body）回显进 `/api/echo` —— 头类 quirk 靠它才断言得到。
//!
//! 票 30 再加三件：① 错误信封负例（按 model 名直接回 401/403/429/500，不走 fixture）；
//! ② `hang`（一个字节都不发，验 ping 保活与探针档超时）；③ `hardcut`（发到中途硬切连接，
//! 验出站转发的传输错误分支——这不是「流正常结束」，是 TCP 级断）。

#[path = "../replay.rs"]
mod replay;

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde_json::{json, Value};

use replay::{Fixture, SCHEMES};

/// `hang` 保持沉默多久再收尾：够网关的 `GATEWAY_PING_SECS`（测试拨到 1 秒）插进至少一帧 ping。
const HANG_SILENCE: Duration = Duration::from_millis(2500);
/// 回显缓冲上限：只看最近几次请求，防止长跑把内存吃掉。
const ECHO_CAP: usize = 200;

/// mock 自己的状态：**只有** mock 需要的东西（回放目录 / 回显 / 切法），
/// 网关那份运行态里没有它们。
struct Mock {
    fixtures: PathBuf,
    echo: std::sync::Mutex<Vec<Value>>,
    chunk_scheme: std::sync::Mutex<String>,
}

impl Mock {
    fn scheme(&self) -> String {
        self.chunk_scheme
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "frame".into())
    }

    fn push_echo(&self, entry: Value) {
        if let Ok(mut buf) = self.echo.lock() {
            buf.push(entry);
            if buf.len() > ECHO_CAP {
                let drop = buf.len() - ECHO_CAP;
                buf.drain(..drop);
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let fixtures =
        PathBuf::from(std::env::var("GATEWAY_FIXTURES").unwrap_or_else(|_| "fixtures".into()));
    let port: u16 = std::env::var("GATEWAY_MOCK_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8788);

    let app = Arc::new(Mock {
        fixtures,
        echo: std::sync::Mutex::new(Vec::new()),
        chunk_scheme: std::sync::Mutex::new("frame".into()),
    });

    let router = Router::new()
        .route(
            "/anthropic/v1/messages",
            post(
                |s: State<Arc<Mock>>, u: axum::http::Uri, h: HeaderMap, j: Json<Value>| {
                    replay(s, u, "anthropic", h, j)
                },
            ),
        )
        .route("/anthropic/v1/messages/count_tokens", post(count))
        .route(
            "/openai/v1/chat/completions",
            post(
                |s: State<Arc<Mock>>, u: axum::http::Uri, h: HeaderMap, j: Json<Value>| {
                    replay(s, u, "openai", h, j)
                },
            ),
        )
        // mock 侧控制面（票 29）：拨回放切法，供「半帧 / 单字节 / CRLF 不变性」测试用。
        .route("/__chunks", post(set_chunks))
        .route("/api/echo", get(api_echo))
        .with_state(app.clone());

    println!(
        "mock   http://127.0.0.1:{port}  (fixtures: {})",
        app.fixtures.display()
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// count_tokens 的假上游（票 26）：固定值刻意与本地估算不同，用来分辨数字到底来自哪里。
/// 票 30 第 9 条的两个降级负例也挂在这里：非 2xx 与「响应缺 input_tokens」都必须降级到本地估算。
async fn count(Json(body): Json<Value>) -> Response {
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
/// 网关必须**保持状态码**并就地转成 Anthropic 信封（原型 README §2 第 7 条）。
fn error_status(scenario: &str) -> Option<(StatusCode, &'static str)> {
    match scenario {
        "err401" => Some((StatusCode::UNAUTHORIZED, "authentication_error")),
        "err403" => Some((StatusCode::FORBIDDEN, "permission_error")),
        "err429" => Some((StatusCode::TOO_MANY_REQUESTS, "rate_limit_error")),
        "err500" => Some((StatusCode::INTERNAL_SERVER_ERROR, "api_error")),
        _ => None,
    }
}

async fn replay(
    app: State<Arc<Mock>>,
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
            "scheme": app.scheme(),
            "model": model,
            "headers": headers
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_lowercase(),
                        Value::String(v.to_str().unwrap_or("<binary>").to_string()),
                    )
                })
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
        app.push_echo(echo(json!({"error_status": status.as_u16() })));
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
        app.push_echo(echo(json!({"hang": scenario})));
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
    let chunks = fixture.chunks(&app.scheme());
    let hard_cut = scenario == "hardcut";
    app.push_echo(echo(json!({
        "fixture": fixture.path.display().to_string(),
        "chunks": chunks.len(),
        "hard_cut": hard_cut,
    })));

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

/// 拨回放切法（票 29）。名字非法就 400 —— 静默退回默认切法会让测试假装通过。
async fn set_chunks(State(app): State<Arc<Mock>>, Json(body): Json<Value>) -> Response {
    let scheme = body
        .get("scheme")
        .and_then(|s| s.as_str())
        .unwrap_or("frame")
        .to_string();
    if !SCHEMES.contains(&scheme.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"type":"invalid_request_error","message":format!("未知切法 {scheme:?}；可用：{}", SCHEMES.join(", "))}})),
        )
            .into_response();
    }
    if let Ok(mut cur) = app.chunk_scheme.lock() {
        *cur = scheme.clone();
    }
    Json(json!({"scheme": scheme})).into_response()
}

/// mock 收到的请求回显（票 29）：头类 quirk / 路径模板 / auth 头靠它断言。
async fn api_echo(State(app): State<Arc<Mock>>) -> Response {
    let echo = app.echo.lock().map(|b| b.clone()).unwrap_or_default();
    Json(json!({"echo": echo})).into_response()
}
