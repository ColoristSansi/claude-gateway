//! 最小可运行网关原型（票 14）。只回答一个问题：IR + 编解码 + SSE 转发能不能立起来。
//! 不是产品代码：没有前端、没有 DPAPI、没有配置写入引擎、没有管理 API 鉴权。

mod anthropic;
mod ir;
mod mock;
mod openai;
mod provider;
mod replay;
mod responses;
mod sse;
mod store;
mod upstream;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
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

use provider::{Auth, Env, Quirk, Upstream, Wire};
use store::Route;
use upstream::Inbound;

pub struct App {
    pub db: PathBuf,
    pub fixtures: PathBuf,
    pub client: reqwest::Client,
    pub upstreams: HashMap<String, Upstream>,
    /// mock 收到的请求回显（票 29）：头类 quirk 靠它断言，只放 mock 里，不进生产路径。
    pub echo: std::sync::Mutex<Vec<Value>>,
    /// mock 回放用的切法（票 29）：`POST <mock>/__chunks` 改它，默认一段一帧。
    pub chunk_scheme: std::sync::Mutex<String>,
    /// 上游静默超过这个间隔就向前发一帧 `ping`（票 30 第 6 条）：不给上游加超时（票 26 决定 5），
    /// 只让客户端看到连接还活着。
    pub ping: Duration,
    /// 探针档（`max_tokens <= PROBE_MAX_TOKENS`）的上游上限，默认 20 秒（票 26 决定 5）。
    pub probe: Duration,
    /// body 日志开关的到点时间（票 30 第 7 条）：None = 关；开启时 1 小时自动关。
    pub body_log_until: std::sync::Mutex<Option<Instant>>,
}

/// body 日志开关的窗口：开启后 1 小时自动关（票 11 的日志定案）。
const BODY_LOG_WINDOW: Duration = Duration::from_secs(3600);
/// 只记元数据是默认（票 11）：要写 body 必须显式开，且到点自关。
const BODY_LOG_ENV: &str = "GATEWAY_LOG_BODY";

impl App {
    pub fn body_log_on(&self) -> bool {
        self.body_log_until
            .lock()
            .map(|until| until.map(|d| Instant::now() < d).unwrap_or(false))
            .unwrap_or(false)
    }

    /// `POST /__bodylog {"seconds": N}`：测试用它把窗口拨短，好断言「到点自动关」。
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

/// 回显缓冲上限：只看最近几次请求，防止长跑把内存吃掉。
const ECHO_CAP: usize = 200;

pub fn echo_push(app: &App, entry: Value) {
    if let Ok(mut buf) = app.echo.lock() {
        buf.push(entry);
        if buf.len() > ECHO_CAP {
            let drop = buf.len() - ECHO_CAP;
            buf.drain(..drop);
        }
    }
}

/// 有 fixture 字节、两种 wire 都有的场景（票 30 加了 `toolresult`：多轮 `tool_result` 入站）。
const SCENARIOS: [&str; 6] = ["text", "tools", "thinking", "usage", "abort", "toolresult"];
/// 只挂单边 wire 的场景：`keepalive` 是 2026-10-15 从 commandcode 的 Anthropic 面真录到的
/// 那份带 `event: ping` 的字节（票 30 第 3 条），Chat 面没有对应录制。
const ONE_WIRE_SCENARIOS: [(&str, &str); 1] = [("keepalive", "anthropic")];
/// 不需要 fixture 字节的场景：`hang` 先回头再静默（验 ping 保活），`hang-nohdr` 连响应头都不回
/// （验探针档上限 504）；`hardcut` 有自己的小 fixture，但发送中途会硬切连接（票 30 第 3、6 条）。
const SPECIAL_SCENARIOS: [&str; 3] = ["hang", "hang-nohdr", "hardcut"];
/// 上游错误信封的负例（票 30 第 4 条）：mock 直接回状态码，不经过 fixture 回放。
const ERROR_CODES: [&str; 4] = ["401", "403", "429", "500"];
const FAMILY_SLOTS: [&str; 4] = [
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::env::var("GATEWAY_ROOT").unwrap_or_else(|_| ".".into());
    let env_path = std::env::var("GATEWAY_ENV")
        .unwrap_or_else(|_| format!("{root}/../prototype-upstream.env"));
    let db =
        PathBuf::from(std::env::var("GATEWAY_DB").unwrap_or_else(|_| format!("{root}/gateway.db")));
    let fixtures = PathBuf::from(
        std::env::var("GATEWAY_FIXTURES").unwrap_or_else(|_| format!("{root}/fixtures")),
    );
    let env = Env::load(&env_path);
    let port = env
        .port
        .or_else(|| {
            std::env::var("GATEWAY_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(8787);
    let mock_port: u16 = std::env::var("GATEWAY_MOCK_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8788);
    // 票 30 的两个旋钮：保活间隔（第 6 条）与探针档上限（第 9 条，测试拨短用）。
    let ping = Duration::from_secs(env_u64("GATEWAY_PING_SECS", 15));
    let probe = Duration::from_secs(env_u64("GATEWAY_PROBE_SECS", 20));
    let body_log = std::env::var(BODY_LOG_ENV)
        .map(|v| v == "1")
        .unwrap_or(false);

    let mut upstreams: HashMap<String, Upstream> = HashMap::new();

    // mock 上游：本机假上游，回放 fixtures/。始终启用。
    // 两条头类 quirk 挂在它身上（票 19 §6.3）：quirk 的唯一执法点在 HTTP 组装路径上，
    // mock 是集成层唯一能真读到「出站请求头」的上游。
    upstreams.insert(
        "mock".into(),
        Upstream {
            id: "mock".into(),
            base_url: format!("http://127.0.0.1:{mock_port}"),
            auth: Auth::Bearer,
            api_key: "mock".into(),
            path_anthropic: "/anthropic/v1/messages".into(),
            path_openai: "/openai/v1/chat/completions".into(),
            quirks: vec![
                Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1"),
                Quirk::SessionIdHeader("x-opencode-session"),
            ],
        },
    );

    // 真实上游 1：opencode（Go 计划）。UA 与会话头由 quirk 声明，网关侧自动处理。
    if let (Some(base), Some(key)) = (env.get("OPENCODE_BASE_URL"), env.get("OPENCODE_API_KEY")) {
        upstreams.insert(
            "opencode".into(),
            Upstream {
                id: "opencode".into(),
                base_url: base,
                auth: Auth::Bearer,
                api_key: key,
                path_anthropic: "/messages".into(),
                path_openai: "/chat/completions".into(),
                quirks: vec![
                    Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1"),
                    Quirk::SessionIdHeader("x-opencode-session"),
                ],
            },
        );
    } else {
        eprintln!("[warn] 缺 OPENCODE_* 配置，opencode 上游不可用");
    }

    // 真实上游 2：commandcode。Claude 模型只在 /messages，用 quirk 表达。
    if let (Some(base), Some(key)) = (
        env.get("COMMANDCODE_BASE_URL"),
        env.get("COMMANDCODE_API_KEY"),
    ) {
        upstreams.insert(
            "commandcode".into(),
            Upstream {
                id: "commandcode".into(),
                base_url: base,
                auth: Auth::Bearer,
                api_key: key,
                path_anthropic: "/messages".into(),
                path_openai: "/chat/completions".into(),
                quirks: vec![Quirk::ClaudeModelsOnAnthropicOnly],
            },
        );
    } else {
        eprintln!("[warn] 缺 COMMANDCODE_* 配置，commandcode 上游不可用");
    }

    let mut routes: Vec<Route> = Vec::new();

    // 家族槽位：客户端只能填这四个名字，槽位绑到哪个上游由这张表决定。
    // 绑定按用户选定：四个槽位都用 deepseek-v4.1-flash，两家上游各两个（见 README 结论记录）。
    // 两个上游上该模型都只支持 Chat 端点（commandcode 的 supported_endpoints 明确排除了 /messages）。
    for (name, up, model) in [
        ("claude-fable", "opencode", "deepseek-v4.1-flash"),
        ("claude-opus", "opencode", "deepseek-v4.1-flash"),
        (
            "claude-sonnet",
            "commandcode",
            "deepseek/deepseek-v4.1-flash",
        ),
        (
            "claude-haiku",
            "commandcode",
            "deepseek/deepseek-v4.1-flash",
        ),
    ] {
        routes.push(Route {
            name: name.into(),
            upstream_id: up.into(),
            real_model: model.into(),
            wire: "openai".into(),
            family: true,
        });
    }

    // 真实上游验证条目：两个上游 × 两种 wire，另加三条「上游会拒」的负例。
    for (name, up, model, wire) in [
        ("oc-deepseek", "opencode", "deepseek-v4.1-flash", "openai"),
        ("cc-sonnet", "commandcode", "claude-sonnet-5-5", "anthropic"),
        // 负例 1：opencode 的 Anthropic 端点与 Chat 端点凭据不同 → 401。
        (
            "oc-anthropic-unauth",
            "opencode",
            "claude-haiku-5-5",
            "anthropic",
        ),
        // 负例 2：commandcode 的 opus 不在本计划内 → 403 MODEL_NOT_IN_PLAN。
        (
            "cc-opus-no-plan",
            "commandcode",
            "claude-opus-5",
            "anthropic",
        ),
        // 负例 3：故意错配 wire，验证 ClaudeModelsOnAnthropicOnly 这条 quirk 真的会拦。
        ("cc-opus-on-chat", "commandcode", "claude-opus-5", "openai"),
    ] {
        routes.push(Route {
            name: name.into(),
            upstream_id: up.into(),
            real_model: model.into(),
            wire: wire.into(),
            family: false,
        });
    }

    // mock 场景：场景 × wire，两个编解码都过一遍。
    for scenario in SCENARIOS {
        for wire in ["anthropic", "openai"] {
            routes.push(Route {
                name: format!("mock-{wire}-{scenario}"),
                upstream_id: "mock".into(),
                real_model: format!("mock-{scenario}"),
                wire: wire.into(),
                family: false,
            });
        }
    }

    // 单边 wire 的场景、不发 fixture 的场景、上游错误信封的负例，以及 count_tokens 的两条降级负例。
    for (scenario, wire) in ONE_WIRE_SCENARIOS {
        routes.push(Route {
            name: format!("mock-{wire}-{scenario}"),
            upstream_id: "mock".into(),
            real_model: format!("mock-{scenario}"),
            wire: wire.into(),
            family: false,
        });
    }
    for scenario in SPECIAL_SCENARIOS {
        for wire in ["anthropic", "openai"] {
            routes.push(Route {
                name: format!("mock-{wire}-{scenario}"),
                upstream_id: "mock".into(),
                real_model: format!("mock-{scenario}"),
                wire: wire.into(),
                family: false,
            });
        }
    }
    for code in ERROR_CODES {
        for wire in ["anthropic", "openai"] {
            routes.push(Route {
                name: format!("mock-{wire}-err{code}"),
                upstream_id: "mock".into(),
                real_model: format!("mock-err{code}"),
                wire: wire.into(),
                family: false,
            });
        }
    }
    // count_tokens 只有 Anthropic wire 会转发上游，所以两条降级负例也只在那个 wire 上。
    for (name, real) in [
        ("mock-anthropic-count404", "mock-count404"),
        ("mock-anthropic-countnoinput", "mock-countnoinput"),
    ] {
        routes.push(Route {
            name: name.into(),
            upstream_id: "mock".into(),
            real_model: real.into(),
            wire: "anthropic".into(),
            family: false,
        });
    }

    let ups: Vec<Upstream> = upstreams.values().cloned().collect();
    store::init(&db.to_string_lossy(), &ups, &routes)?;

    let app = Arc::new(App {
        db: db.clone(),
        fixtures,
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?,
        upstreams,
        echo: std::sync::Mutex::new(Vec::new()),
        chunk_scheme: std::sync::Mutex::new("frame".into()),
        ping,
        probe,
        body_log_until: std::sync::Mutex::new(body_log.then(|| Instant::now() + BODY_LOG_WINDOW)),
    });

    let gateway = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/messages", post(messages))
        // 票 26：Desktop 的推理探针是非流式 max_tokens:1，连接测试另走 count_tokens。
        .route("/v1/messages/count_tokens", post(count_tokens))
        // 票 32：Codex 的唯一合法出站（`wire_api = "responses"`）。
        .route("/v1/responses", post(responses))
        .route("/v1/models", get(list_models))
        // 票 15：`inferenceGatewayBaseUrl` 到底带不带 `/v1` 两份资料互相矛盾，
        // 别名路由让两种写法都能通，同时由 messages 打印的真实路径把答案记下来。
        .route("/messages", post(messages))
        .route("/messages/count_tokens", post(count_tokens))
        .route("/responses", post(responses))
        .route("/models", get(list_models))
        .route("/api/routes", get(api_routes))
        .route("/api/logs", get(api_logs))
        .route("/api/echo", get(api_echo))
        // 票 30 第 7 条：body 日志开关的测试控制面（真产品里它是设置页那个开关 + 1 小时自动关）。
        .route("/__bodylog", post(set_body_log))
        .with_state(app.clone());

    let mock_upstream = Router::new()
        .route(
            "/anthropic/v1/messages",
            post(
                |s: State<Arc<App>>, u: axum::http::Uri, h: HeaderMap, j: Json<Value>| {
                    mock::replay(s, u, "anthropic", h, j)
                },
            ),
        )
        .route("/anthropic/v1/messages/count_tokens", post(mock::count))
        .route(
            "/openai/v1/chat/completions",
            post(
                |s: State<Arc<App>>, u: axum::http::Uri, h: HeaderMap, j: Json<Value>| {
                    mock::replay(s, u, "openai", h, j)
                },
            ),
        )
        // mock 侧控制面（票 29）：拨回放切法，供「半帧 / 单字节 / CRLF 不变性」测试用。
        .route("/__chunks", post(set_chunks))
        .with_state(app.clone());

    println!("网关   http://127.0.0.1:{port}   (inbound: POST /v1/messages, POST /v1/messages/count_tokens, POST /v1/responses, GET /v1/models, GET /api/routes, GET /api/logs)");
    println!(
        "mock   http://127.0.0.1:{mock_port}  (fixtures: {})",
        app.fixtures.display()
    );
    println!("路由表（{} 条）:", routes.len());
    for r in &routes {
        println!(
            "  {:<22} -> {:<12} {:<22} {}{}",
            r.name,
            r.upstream_id,
            r.real_model,
            r.wire,
            if r.family { "  [家族槽位]" } else { "" }
        );
    }

    let mock_listener = tokio::net::TcpListener::bind(("127.0.0.1", mock_port)).await?;
    let gw_listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let a = tokio::spawn(async move { axum::serve(mock_listener, mock_upstream).await });
    let b = tokio::spawn(async move { axum::serve(gw_listener, gateway).await });
    let _ = tokio::join!(a, b);
    Ok(())
}

/// 入站 Anthropic Messages：这就是 Claude Desktop 3P / Claude Code 打的那个端点。
async fn messages(
    State(app): State<Arc<App>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    proxy(app, uri, headers, body, Inbound::Anthropic).await
}

/// 入站 OpenAI Responses：Codex 打的那个端点（票 32）。
async fn responses(
    State(app): State<Arc<App>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    proxy(app, uri, headers, body, Inbound::Responses).await
}

/// 两个入站协议共用的转发链路：查路由 → 解 IR → 换真实模型 → 出站。
/// 入站协议只影响「怎么解请求」和「怎么编响应」，其余一律共用。
async fn proxy(
    app: Arc<App>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Value,
    inbound: Inbound,
) -> Response {
    // 票 30 第 4 条定下的规则：**去首尾空白 + 大小写敏感**（模型 ID 按契约大小写为准）。
    let name = body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
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

    let routes = match store::routes(&app.db.to_string_lossy()) {
        Ok(r) => r,
        Err(e) => {
            return err_in(
                inbound,
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                &e.to_string(),
            )
        }
    };
    let Some(rt) = routes.iter().find(|r| r.name == name) else {
        return err_in(
            inbound,
            StatusCode::NOT_FOUND,
            "not_found_error",
            &format!("未知虚拟模型 {name:?}；GET /api/routes 看可用名字"),
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

/// 入站请求体落盘（只在 body 日志开关打开时）：路径 = 数据库同目录的 `inbound.json`。
/// 原型不建历史页，覆盖式写一份就够看清「客户端到底发了什么」。
fn dump_inbound(app: &App, body: &Value) {
    if !app.body_log_on() {
        return;
    }
    let path = app.db.with_extension("inbound.json");
    if let Err(e) = std::fs::write(&path, body.to_string()) {
        eprintln!("[warn] 入站请求体写不进去 {}: {e}", path.display());
    }
}

/// `count_tokens`（票 26）：数字优先向上游要（只有 Anthropic wire 有原生端点），
/// 任何失败降级到本地估算 —— 这个端点绝不因为上游不实现它而失败。
async fn count_tokens(
    State(app): State<Arc<App>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // 票 30 第 4 条定下的规则：**去首尾空白 + 大小写敏感**（模型 ID 按契约大小写为准）。
    let name = body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    println!(
        "[in] {} count_tokens model={name}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or(""),
    );
    let routes = match store::routes(&app.db.to_string_lossy()) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                &e.to_string(),
            )
        }
    };
    let Some(rt) = routes.iter().find(|r| r.name == name) else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found_error",
            &format!("未知虚拟模型 {name:?}；GET /api/routes 看可用名字"),
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

async fn list_models(State(app): State<Arc<App>>) -> Response {
    let Ok(routes) = store::routes(&app.db.to_string_lossy()) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "读不到路由表",
        );
    };
    let data: Vec<Value> = routes
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
    Json(json!({"data": data, "has_more": false, "first_id": routes.first().map(|r| r.name.clone()), "last_id": routes.last().map(|r| r.name.clone())})).into_response()
}

async fn api_routes(State(app): State<Arc<App>>) -> Response {
    let Ok(routes) = store::routes(&app.db.to_string_lossy()) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "读不到路由表",
        );
    };
    let data: Vec<Value> = routes
        .iter()
        .map(|r| {
            let up = app.upstreams.get(&r.upstream_id);
            json!({
                "name": r.name,
                "family_slot": r.family,
                "family_slots": FAMILY_SLOTS,
                "upstream": r.upstream_id,
                "base_url": up.map(|u| u.base_url.clone()),
                "real_model": r.real_model,
                "wire": r.wire,
                "auth": up.map(|u| u.auth.as_str()),
                "quirks": up.map(|u| u.quirks.iter().map(quirk_name).collect::<Vec<_>>()),
            })
        })
        .collect();
    Json(json!({"routes": data})).into_response()
}

async fn api_logs(State(app): State<Arc<App>>) -> Response {
    let db = app.db.to_string_lossy().to_string();
    let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let c = store::open(&db)?;
        let mut st = c.prepare(
            "SELECT ts,virtual_model,upstream_id,real_model,wire,status,first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,aborted,error,body
             FROM request_log ORDER BY id DESC LIMIT 50",
        )?;
        let it = st.query_map([], |r| {
            Ok(json!({
                "ts": r.get::<_, String>(0)?,
                "virtual_model": r.get::<_, String>(1)?,
                "upstream": r.get::<_, String>(2)?,
                "real_model": r.get::<_, String>(3)?,
                "wire": r.get::<_, String>(4)?,
                "status": r.get::<_, i64>(5)?,
                "first_byte_ms": r.get::<_, i64>(6)?,
                "total_ms": r.get::<_, i64>(7)?,
                "input_tokens": r.get::<_, Option<i64>>(8)?,
                "output_tokens": r.get::<_, Option<i64>>(9)?,
                "cache_read": r.get::<_, Option<i64>>(10)?,
                "aborted": r.get::<_, i64>(11)? != 0,
                "error": r.get::<_, Option<String>>(12)?,
                "body": r.get::<_, Option<String>>(13)?,
            }))
        })?;
        Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .await;
    match rows {
        Ok(Ok(v)) => Json(json!({"logs": v})).into_response(),
        Ok(Err(e)) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &e.to_string(),
        ),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &e.to_string(),
        ),
    }
}

/// mock 收到的请求回显（票 29）：头类 quirk / 路径模板 / auth 头靠它断言。
async fn api_echo(State(app): State<Arc<App>>) -> Response {
    let echo = app.echo.lock().map(|b| b.clone()).unwrap_or_default();
    Json(json!({"echo": echo})).into_response()
}

/// 拨 mock 的回放切法（票 29）。名字非法就 400 —— 静默退回默认切法会让测试假装通过。
async fn set_chunks(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    let scheme = body
        .get("scheme")
        .and_then(|s| s.as_str())
        .unwrap_or("frame")
        .to_string();
    if !replay::SCHEMES.contains(&scheme.as_str()) {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            &format!("未知切法 {scheme:?}；可用：{}", replay::SCHEMES.join(", ")),
        );
    }
    if let Ok(mut cur) = app.chunk_scheme.lock() {
        *cur = scheme.clone();
    }
    Json(json!({"scheme": scheme})).into_response()
}

/// body 日志开关的控制面（票 30 第 7 条）：`{"seconds": N}` 拨窗口，`0` 立即关。
async fn set_body_log(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    let seconds = body.get("seconds").and_then(|s| s.as_u64()).unwrap_or(0);
    app.set_body_log(seconds);
    Json(json!({"body_log": app.body_log_on(), "seconds": seconds})).into_response()
}

/// 整数环境变量：解析不了就用默认值（坏配置别把网关拦在启动前）。
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn quirk_name(q: &Quirk) -> String {
    match q {
        Quirk::ClientUserAgent(a) => format!("ClientUserAgent({a})"),
        Quirk::SessionIdHeader(a) => format!("SessionIdHeader({a})"),
        Quirk::ClaudeModelsOnAnthropicOnly => "ClaudeModelsOnAnthropicOnly".into(),
    }
}

fn err(status: StatusCode, kind: &str, message: &str) -> Response {
    upstream::error_response(status, kind, message)
}

/// 入站协议决定错误信封的形状（Responses 要 OpenAI 那一份，见 upstream::error_response_for）。
fn err_in(inbound: Inbound, status: StatusCode, kind: &str, message: &str) -> Response {
    upstream::error_response_for(inbound, status, kind, message)
}
