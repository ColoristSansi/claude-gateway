//! 转发面 `/v1/*`：入站协议 → IR → 绑定查表 → 出站转发（spec §3 / §4）。
//!
//! 票 41 的落点：这里只放**转发链**，出站细节在 [`crate::egress::upstream`]，
//! 供应商表在 [`crate::egress::provider`]。管理面 `/api/*` 归票 43 的 `ingress/api.rs`，本模块不碰；
//! 路由表从哪来归票 49 / 51，这里只拿一张现成的 [`Route`] 表查。
//!
//! `router()` 是这一面的唯一入口：外壳（票 43 的 `service`）与回放 harness 都挂它，
//! 不各自拼一套 `/v1/*`。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::anthropic;
use crate::binding::Route;
use crate::egress::provider::{Upstream, Wire};
use crate::egress::upstream::{self, Inbound};
use crate::ir;
use crate::responses;
use crate::store::Store;

/// body 日志窗口：开启后 1 小时自动关（票 11 的日志定案 / 票 30 第 7 条）。
pub const BODY_LOG_WINDOW: Duration = Duration::from_secs(3600);

/// 网关运行态：出站客户端、供应商表、绑定表、日志落点与三个旋钮。
pub struct Gateway {
    pub client: reqwest::Client,
    /// 声明式供应商表：id → 上游（路径模板 / 认证 / quirks）。
    pub upstreams: HashMap<String, Upstream>,
    /// 绑定表：虚拟模型名 → 上游 + 真实模型 + wire。
    pub routes: Vec<Route>,
    /// 日志落点（票 42 的 `Store`）。
    pub store: Store,
    /// 上游静默超过它就向前发一帧 `ping`（票 30 第 6 条）。
    pub ping: Duration,
    /// 探针档（`max_tokens <= PROBE_MAX_TOKENS`）的上游上限（票 26 决定 5）。
    pub probe: Duration,
    /// body 日志开关的到点时间：`None` = 关。
    pub body_log_until: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for Gateway {
    /// 手写而不是 `derive`：供应商表里带着 API key，`Debug` 打出来就等于把密钥写进日志。
    /// 只报条数与旋钮。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("upstreams", &self.upstreams.len())
            .field("routes", &self.routes.len())
            .field("ping", &self.ping)
            .field("probe", &self.probe)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// 装配运行态。出站客户端 300 秒总超时（探针档另有更短的 deadline）。
    pub fn new(store: Store, upstreams: HashMap<String, Upstream>, routes: Vec<Route>) -> Self {
        Gateway {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
            upstreams,
            routes,
            store,
            ping: Duration::from_secs(15),
            probe: Duration::from_secs(20),
            body_log_until: Mutex::new(None),
        }
    }

    pub fn body_log_on(&self) -> bool {
        self.body_log_until
            .lock()
            .map(|until| until.map(|d| Instant::now() < d).unwrap_or(false))
            .unwrap_or(false)
    }

    /// 拨 body 日志窗口：`0` = 立即关，其余 = 从现在起这么多秒（到点自关）。
    pub fn set_body_log(&self, seconds: u64) {
        if let Ok(mut until) = self.body_log_until.lock() {
            *until = if seconds == 0 {
                None
            } else {
                Some(Instant::now() + Duration::from_secs(seconds))
            };
        }
    }
}

/// 转发面路由（`/v1/*` + 不带 `/v1` 的别名）。
pub fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/v1/messages", post(messages))
        // 票 26：Desktop 的推理探针是非流式 max_tokens:1，连接测试另走 count_tokens。
        .route("/v1/messages/count_tokens", post(count_tokens))
        // 票 32：Codex 的唯一合法出站（`wire_api = "responses"`）。
        .route("/v1/responses", post(responses))
        .route("/v1/models", get(list_models))
        // 票 15：`inferenceGatewayBaseUrl` 到底带不带 `/v1` 两份资料互相矛盾，
        // 别名路由让两种写法都能通。
        .route("/messages", post(messages))
        .route("/messages/count_tokens", post(count_tokens))
        .route("/responses", post(responses))
        .route("/models", get(list_models))
}

/// 入站 Anthropic Messages：Claude Desktop 3P / Claude Code 打的那个端点。
async fn messages(
    State(app): State<Arc<Gateway>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    proxy(app, uri, headers, body, Inbound::Anthropic).await
}

/// 入站 OpenAI Responses：Codex 打的那个端点（票 32）。
async fn responses(
    State(app): State<Arc<Gateway>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    proxy(app, uri, headers, body, Inbound::Responses).await
}

/// 两个入站协议共用的转发链路：查路由 → 解 IR → 换真实模型 → 出站。
/// 入站协议只影响「怎么解请求」和「怎么编响应」，其余一律共用。
async fn proxy(
    app: Arc<Gateway>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Value,
    inbound: Inbound,
) -> Response {
    // 票 30 第 4 条定下的规则：**去首尾空白 + 大小写敏感**（模型 ID 按契约大小写为准）。
    let name = virtual_model(&body);
    // 票 15：记下客户端真实打的路径与协议头，用来定 base URL 语义。
    println!(
        "[in] {} proto={} model={name} ua={:?} ver={:?} beta={:?}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or(""),
        match inbound {
            Inbound::Anthropic => "anthropic",
            Inbound::Responses => "responses",
        },
        headers.get("user-agent").and_then(|v| v.to_str().ok()),
        headers
            .get("anthropic-version")
            .and_then(|v| v.to_str().ok()),
        headers.get("anthropic-beta").and_then(|v| v.to_str().ok()),
    );
    // body 日志开着时把入站体落盘：Codex 的真实请求形状只有它说得清（票 32）。
    dump_inbound(&app, &body);

    let Some(rt) = app.routes.iter().find(|r| r.name == name) else {
        return err_in(
            inbound,
            StatusCode::NOT_FOUND,
            "not_found_error",
            &format!("未知虚拟模型 {name:?}"),
        );
    };
    let streaming = body
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let up = match app.upstreams.get(&rt.upstream_id) {
        Some(u) => u.clone(),
        None => {
            return err_in(
                inbound,
                StatusCode::BAD_GATEWAY,
                "api_error",
                &format!("上游 {} 未配置（缺 key）", rt.upstream_id),
            )
        }
    };
    let wire = Wire::parse(&rt.wire).unwrap_or(Wire::Anthropic);

    let decoded = match inbound {
        Inbound::Anthropic => anthropic::decode_request(&body),
        Inbound::Responses => responses::decode_request(&body),
    };
    let mut req = match decoded {
        Ok(r) => r,
        Err(e) => {
            return err_in(
                inbound,
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &e.to_string(),
            )
        }
    };
    req.model = rt.real_model.clone();

    if let Err(e) = upstream::guard(&up, wire, &rt.real_model) {
        return err_in(
            inbound,
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            &e,
        );
    }

    // 入站的 Anthropic 协议头必须带过去：`anthropic-beta` 一丢，prompt caching / thinking 就静默失效。
    let pass = protocol_headers(&headers);

    if streaming {
        upstream::forward(app, &up, wire, req, name, pass, inbound).await
    } else {
        upstream::forward_buffered(app, &up, wire, req, name, pass, inbound).await
    }
}

/// 虚拟模型名：去首尾空白（大小写敏感由查表本身保证）。
fn virtual_model(body: &Value) -> String {
    body.get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 入站请求体落盘（只在 body 日志开关打开时）：落在数据目录的 `inbound.json`。
/// 覆盖式写一份就够看清「客户端到底发了什么」；历史页归票 53。
fn dump_inbound(app: &Gateway, body: &Value) {
    if !app.body_log_on() {
        return;
    }
    let path = PathBuf::from(app.store.root()).join("inbound.json");
    if let Err(e) = std::fs::write(&path, body.to_string()) {
        eprintln!("[warn] 入站请求体写不进去 {}: {e}", path.display());
    }
}

/// `count_tokens`（票 26）：数字优先向上游要（只有 Anthropic wire 有原生端点），
/// 任何失败降级到本地估算 —— 这个端点绝不因为上游不实现它而失败。
async fn count_tokens(
    State(app): State<Arc<Gateway>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let name = virtual_model(&body);
    println!(
        "[in] {} count_tokens model={name}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or(""),
    );
    let Some(rt) = app.routes.iter().find(|r| r.name == name) else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found_error",
            &format!("未知虚拟模型 {name:?}"),
        );
    };
    let mut req = match anthropic::decode_request(&body) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &e.to_string(),
            )
        }
    };
    req.model = rt.real_model.clone();
    let wire = Wire::parse(&rt.wire).unwrap_or(Wire::Anthropic);

    let mut source = "local";
    let mut n = None;
    if wire == Wire::Anthropic {
        if let Some(up) = app.upstreams.get(&rt.upstream_id) {
            n = upstream::count_tokens_upstream(&app, up, &req, &protocol_headers(&headers)).await;
            source = if n.is_some() {
                "upstream"
            } else {
                "local(降级)"
            };
        }
    }
    let n = n.unwrap_or_else(|| ir::estimate_tokens(&req));
    println!(
        "[count] {name} wire={} source={source} input_tokens={n}",
        wire.as_str()
    );
    Json(json!({"input_tokens": n})).into_response()
}

/// 入站 Anthropic 协议头原样透传（`anthropic-beta` 丢了会让 beta 能力静默失效）。
fn protocol_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    ["anthropic-version", "anthropic-beta"]
        .iter()
        .filter_map(|k| {
            headers
                .get(*k)
                .and_then(|v| v.to_str().ok())
                .map(|v| (k.to_string(), v.to_string()))
        })
        .collect()
}

async fn list_models(State(app): State<Arc<Gateway>>) -> Response {
    let data: Vec<Value> = app
        .routes
        .iter()
        .map(|r| {
            json!({
                "type": "model",
                "id": r.name,
                "display_name": r.name,
                "created_at": "2026-01-01T00:00:00Z"
            })
        })
        .collect();
    Json(json!({
        "data": data,
        "has_more": false,
        "first_id": app.routes.first().map(|r| r.name.clone()),
        "last_id": app.routes.last().map(|r| r.name.clone())
    }))
    .into_response()
}

fn err(status: StatusCode, kind: &str, message: &str) -> Response {
    upstream::error_response(status, kind, message)
}

/// 入站协议决定错误信封的形状（Responses 要 OpenAI 那一份，见 `upstream::error_response_for`）。
fn err_in(inbound: Inbound, status: StatusCode, kind: &str, message: &str) -> Response {
    upstream::error_response_for(inbound, status, kind, message)
}
