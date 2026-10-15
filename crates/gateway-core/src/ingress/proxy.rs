//! 转发面 `/v1/*`：入站协议 → IR → 绑定查表 → 出站转发（spec §3 / §4）。
//!
//! 票 41 铺了转发链、票 49 铺了供应商表与目录；票 51 把**绑定表接进来**：查表不再是一张
//! 事先拼好的 `Vec<Route>`，而是 [`BindingTable`]（家族槽位 / 普通虚拟模型 / 每客户端档案覆盖），
//! 出站凭据也从 `Gateway.keys` 那张 M1 临时表换成票 42 的 `Store`（按「上游 × wire」，未填继承
//! `default`）。管理面 `/api/*` 归票 43 的 `ingress/api.rs`，本模块不碰。
//!
//! `router()` 是这一面的唯一入口：外壳（票 43 的 `service`）与回放 harness 都挂它，
//! 不各自拼一套 `/v1/*`。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
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
use crate::binding::{self, BindingTable, Profile};
use crate::egress::provider::{self, Upstream, UpstreamModel, Wire};
use crate::egress::upstream::{self, Inbound};
use crate::ir;
use crate::responses;
use crate::store::Store;

/// body 日志窗口：开启后 1 小时自动关（票 11 的日志定案 / 票 30 第 7 条）。
/// 窗口本体在 [`crate::obs::BodyLogWindow`]（票 53 起管理面 `/api/logs/body-recording` 与这里
/// 共用同一个实例），这个常量只是转发面测试与 harness 的默认拨法。
pub use crate::obs::BODY_LOG_WINDOW;

/// 网关运行态：出站客户端、供应商表、绑定表、日志落点与三个旋钮。
pub struct Gateway {
    pub client: reqwest::Client,
    /// 声明式供应商表：id → 上游（路径模板 / 认证 / quirks）。**不含密钥**（票 49）。
    ///
    /// 三张运行期表都可**热重载**（票 52）：界面里加上游 / 拉模型 / 绑槽位之后立刻生效，
    /// 不必重启网关（否则「绑完槽位再问客户端」会 404）。
    pub upstreams: RwLock<HashMap<String, Upstream>>,
    /// 上游模型目录：`(上游 id, 真实模型)` → 目录条目（票 49 的三层校验第二层用它早拒）。
    pub catalog: RwLock<HashMap<(String, String), UpstreamModel>>,
    /// 绑定表（票 51）：虚拟模型名 + 客户端档案 → 上游 / 真实模型 / wire。
    pub bindings: RwLock<BindingTable>,
    /// 日志落点与**出站凭据**的来源（票 42 的 `Store`；密钥 DPAPI 密文入库，明文只在内存）。
    pub store: Store,
    /// 上游静默超过它就向前发一帧 `ping`（票 30 第 6 条）。
    pub ping: Duration,
    /// 探针档（`max_tokens <= PROBE_MAX_TOKENS`）的上游上限（票 26 决定 5）。
    pub probe: Duration,
    /// body 日志开关的窗口：`None` = 关。与 `/api/logs/body-recording` **同一个**实例（票 53）。
    pub body_log: Arc<crate::obs::BodyLogWindow>,
}

impl std::fmt::Debug for Gateway {
    /// 手写而不是 `derive`：这里不该出现任何能拼出密钥的字段（原本的 `keys` 已随票 51 退场）。
    /// 只报条数与旋钮。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field(
                "upstreams",
                &self.upstreams.read().map(|t| t.len()).unwrap_or(0),
            )
            .field(
                "catalog",
                &self.catalog.read().map(|t| t.len()).unwrap_or(0),
            )
            .field(
                "bindings",
                &self.bindings.read().map(|t| t.len()).unwrap_or(0),
            )
            .field("ping", &self.ping)
            .field("probe", &self.probe)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// 绑定表（读锁）。**只在该次读里用**：别把 guard 带过 `.await`。
    pub fn read_bindings(&self) -> std::sync::RwLockReadGuard<'_, BindingTable> {
        self.bindings.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read_upstreams(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Upstream>> {
        self.upstreams.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read_catalog(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, HashMap<(String, String), UpstreamModel>> {
        self.catalog.read().unwrap_or_else(|e| e.into_inner())
    }

    /// 一条上游（克隆出来，guard 不跨 `.await`）。
    pub fn upstream(&self, id: &str) -> Option<Upstream> {
        self.read_upstreams().get(id).cloned()
    }

    /// 一条目录条目（三层校验第二层用它早拒）。
    pub fn catalog_entry(&self, upstream_id: &str, model: &str) -> Option<UpstreamModel> {
        self.read_catalog()
            .get(&(upstream_id.to_string(), model.to_string()))
            .cloned()
    }

    /// 换一份目录（启动装配与热重载用）。
    pub fn set_catalog(&self, catalog: HashMap<(String, String), UpstreamModel>) {
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog;
    }

    /// 往目录里补一条（测试与热重载的细粒度入口）。
    pub fn set_catalog_entry(&self, upstream_id: &str, model: &str, entry: UpstreamModel) {
        self.catalog
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert((upstream_id.to_string(), model.to_string()), entry);
    }

    /// **三张运行期表全部从库里重读**（票 52：界面改完配置立刻生效，不必重启网关）。
    pub async fn reload(&self) -> anyhow::Result<()> {
        let upstreams = crate::egress::provider::list_upstreams(&self.store).await?;
        let catalog = crate::egress::provider::catalog_map(&self.store).await?;
        let bindings = crate::binding::load(&self.store).await?;
        *self.upstreams.write().unwrap_or_else(|e| e.into_inner()) =
            upstreams.into_iter().map(|u| (u.id.clone(), u)).collect();
        self.set_catalog(catalog);
        *self.bindings.write().unwrap_or_else(|e| e.into_inner()) = bindings;
        Ok(())
    }

    /// 装配运行态。出站客户端 300 秒总超时（探针档另有更短的 deadline）。
    pub fn new(store: Store, upstreams: HashMap<String, Upstream>, bindings: BindingTable) -> Self {
        Gateway {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
            upstreams: RwLock::new(upstreams),
            catalog: RwLock::new(HashMap::new()),
            bindings: RwLock::new(bindings),
            store,
            ping: Duration::from_secs(15),
            probe: binding::PROBE_DEADLINE,
            body_log: Arc::new(crate::obs::BodyLogWindow::new()),
        }
    }

    pub fn body_log_on(&self) -> bool {
        self.body_log.on()
    }

    /// 拨 body 日志窗口：`0` = 立即关，其余 = 从现在起这么多秒（到点自关）。
    pub fn set_body_log(&self, seconds: u64) {
        self.body_log.set(seconds);
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
    // 票 51：客户端档案归属按入站 `user-agent` + 客户端标识头判（同一槽位按客户端走不同上游）。
    let identity = client_identity(&headers);
    let profile = app.read_bindings().profile_for(&identity);
    // 票 15：记下客户端真实打的路径与协议头，用来定 base URL 语义。
    println!(
        "[in] {} proto={} model={name} profile={} ua={:?} ver={:?} beta={:?}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or(""),
        match inbound {
            Inbound::Anthropic => "anthropic",
            Inbound::Responses => "responses",
        },
        profile.id(),
        headers.get("user-agent").and_then(|v| v.to_str().ok()),
        headers
            .get("anthropic-version")
            .and_then(|v| v.to_str().ok()),
        headers.get("anthropic-beta").and_then(|v| v.to_str().ok()),
    );
    // body 日志开着时把入站体落盘：Codex 的真实请求形状只有它说得清（票 32）。
    dump_inbound(&app, &body);

    let rt = match app.read_bindings().resolve(&name, &identity) {
        Ok(rt) => rt,
        Err(e) => {
            return err_in(
                inbound,
                StatusCode::from_u16(e.status()).unwrap_or(StatusCode::NOT_FOUND),
                e.error_type(),
                &e.message(),
            )
        }
    };
    let streaming = body
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let up = match app.upstream(&rt.upstream_id) {
        Some(u) => u,
        None => {
            return err_in(
                inbound,
                StatusCode::BAD_GATEWAY,
                "api_error",
                &format!("上游 {} 未配置", rt.upstream_id),
            )
        }
    };
    let wire = rt.wire;

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

    // 三层校验的第二层（ADR-0004）：目录声明过这个模型的 wire 就早拒 400；未声明放行给自检。
    // 排在取凭据之前：配置错了就不该去碰密钥。
    let entry = app.catalog_entry(&rt.upstream_id, &rt.real_model);
    if let Err(rej) = upstream::guard(entry.as_ref(), wire) {
        return err_in(
            inbound,
            StatusCode::from_u16(rej.status).unwrap_or(StatusCode::BAD_REQUEST),
            rej.error_type,
            &rej.message,
        );
    }

    // 出站凭据：按「上游 × wire」查票 42 的 `Store`，没有这一面就继承 `default`（ADR-0004）。
    let Some(key) = credential(&app, &rt.upstream_id, wire).await else {
        return err_in(
            inbound,
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("上游 {} 没有可用凭据", rt.upstream_id),
        );
    };
    let auth = up.auth_for(wire);

    // 入站的 Anthropic 协议头必须带过去：`anthropic-beta` 一丢，prompt caching / thinking 就静默失效。
    let pass = protocol_headers(&headers);

    if streaming {
        upstream::forward(
            app,
            &up,
            wire,
            auth,
            &key,
            req,
            name,
            profile.id().to_string(),
            pass,
            inbound,
        )
        .await
    } else {
        upstream::forward_buffered(
            app,
            &up,
            wire,
            auth,
            &key,
            req,
            name,
            profile.id().to_string(),
            pass,
            inbound,
        )
        .await
    }
}

/// 出站凭据（票 42 的 `Store`，DPAPI 密文）：读失败就当没有，别把库的抖动说成上游的问题。
async fn credential(app: &Gateway, upstream_id: &str, wire: Wire) -> Option<String> {
    match provider::wire_credential(&app.store, upstream_id, wire).await {
        Ok(secret) => secret,
        Err(e) => {
            eprintln!(
                "[warn] 读凭据失败（{upstream_id} / {}）：{e}",
                wire.as_str()
            );
            None
        }
    }
}

/// 入站客户端身份：`user-agent` + 几个客户端标识头（头名与值都算）。
///
/// 归属判据是库里的 `client_profiles.match_ua`（`;` 分隔的子串，大小写不敏感）；
/// 这里只负责把「客户端到底是谁」的线索拼成一串——UA 里带版本号，标识头里带安装 ID，
/// 两者都可能是判据（Codex 桌面应用就是靠 `x-codex-*` 认出来的，票 32 的真录入站体）。
fn client_identity(headers: &HeaderMap) -> String {
    const MARKERS: [&str; 6] = [
        "user-agent",
        "x-app",
        "originator",
        "x-codex-installation-id",
        "x-codex-turn-metadata",
        "x-codex-window-id",
    ];
    let mut out = String::new();
    for name in MARKERS {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            // 头名也进串：桌面应用可能只给标识头、不给可辨认的 UA。
            out.push_str(name);
            out.push(' ');
            out.push_str(v);
            out.push('\n');
        }
    }
    out
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
    let identity = client_identity(&headers);
    println!(
        "[in] {} count_tokens model={name} profile={}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or(""),
        app.read_bindings().profile_for(&identity).id(),
    );
    let rt = match app.read_bindings().resolve(&name, &identity) {
        Ok(rt) => rt,
        Err(e) => {
            return err(
                StatusCode::from_u16(e.status()).unwrap_or(StatusCode::NOT_FOUND),
                e.error_type(),
                &e.message(),
            )
        }
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
    let wire = rt.wire;

    let mut source = "local";
    let mut n = None;
    if wire == Wire::Anthropic {
        if let Some(up) = app.upstream(&rt.upstream_id) {
            let up = &up;
            if let Some(key) = credential(&app, &rt.upstream_id, wire).await {
                n = upstream::count_tokens_upstream(
                    &app,
                    up,
                    up.auth_for(wire),
                    &key,
                    &req,
                    &protocol_headers(&headers),
                )
                .await;
                source = if n.is_some() {
                    "upstream"
                } else {
                    "local(降级)"
                };
            }
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

/// `GET /v1/models`（spec §4.5 / §8.2）：**自己实现，不依赖上游**。
///
/// 暴露集合 = [`BindingTable::exposed`]（四个家族名 + 普通虚拟模型，跨档案去重），
/// **不按客户端过滤**——过滤发生在配置层，发现层用同一判据（spec §8.2 第 2 条）。
/// 只有**信封形状**按入站客户端家族给：Claude 系要 Anthropic 的 `data` 那份
/// （它们还会按「id 含 claude/anthropic」再筛一次），Codex 要标准 `models` 数组
/// （它是 OpenAI 系客户端，只认 `object: list` + `data[].object: model`）。
async fn list_models(State(app): State<Arc<Gateway>>, headers: HeaderMap) -> Response {
    let identity = client_identity(&headers);
    let profile = app.read_bindings().profile_for(&identity);
    let data: Vec<Value> = app
        .read_bindings()
        .exposed()
        .into_iter()
        .map(|m| {
            let (kind, owned) = if m.is_family {
                ("model", "claude-gateway/family")
            } else {
                ("model", "claude-gateway")
            };
            json!({
                "type": kind,
                "id": m.name,
                "object": kind,
                "display_name": m.name,
                "created": 1_767_225_600u64,
                "created_at": "2026-01-01T00:00:00Z",
                "owned_by": owned,
            })
        })
        .collect();

    if profile == Profile::Codex {
        return Json(json!({ "object": "list", "data": data })).into_response();
    }
    Json(json!({
        "data": data,
        "has_more": false,
        "first_id": data.first().and_then(|m| m["id"].as_str()),
        "last_id": data.last().and_then(|m| m["id"].as_str())
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
