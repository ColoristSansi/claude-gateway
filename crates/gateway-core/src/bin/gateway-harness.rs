//! 回放 harness 的**网关进程**（票 41 的测试专用 bin）：把 `ingress::proxy::router()` 挂起来，
//! 路由表与供应商表用票 41 之前的原型脚手架填好（mock 场景 × wire + 家族槽位 + 两个真实上游）。
//!
//! **独立进程 + 测试专用**：挂 `required-features = ["mock"]`，产品构建里没有它。
//! 回放测试同时拉起两个进程（本 bin + `mock-upstream`），走**真实 HTTP**：
//! 「mock 与网关分进程」这条硬要求由它保证。
//!
//! 幂等点：路由表从哪来（种库 / 界面 / 客户端档案）归票 49 与票 51；它们接手后本 bin 改成
//! 「按测试给的绑定种库」，`/v1/*` 那一条路由链**不动**。
//!
//! 旋钮（都是环境变量）：`GATEWAY_ENV`（凭据 env 文件）/ `GATEWAY_DATA_DIR`（库目录）/
//! `GATEWAY_PORT` / `GATEWAY_MOCK_PORT` / `GATEWAY_PING_SECS` / `GATEWAY_PROBE_SECS` /
//! `GATEWAY_LOG_BODY`。

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use serde_json::{json, Value};

use gateway_core::binding::Route;
use gateway_core::egress::provider::{Auth, Env, Quirk, Upstream};
use gateway_core::ingress::proxy::{router, Gateway, BODY_LOG_WINDOW};
use gateway_core::store::Store;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::env::var("GATEWAY_ROOT").unwrap_or_else(|_| ".".into());
    let env_path = std::env::var("GATEWAY_ENV").unwrap_or_else(|_| format!("{root}/upstreams.env"));
    let data_dir = std::env::var("GATEWAY_DATA_DIR").unwrap_or_else(|_| format!("{root}/data"));
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

    let mut upstreams: HashMap<String, Upstream> = HashMap::new();

    // mock 上游：本机假上游，回放 fixtures/。由 mock-upstream 那个**独立进程**提供。
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

    // 真实上游 1：opencode。UA 与会话头由 quirk 声明，网关侧自动处理。
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
    let mut push = |name: String, upstream_id: &str, real_model: String, wire: &str, family| {
        routes.push(Route {
            name,
            upstream_id: upstream_id.into(),
            real_model,
            wire: wire.into(),
            is_family: family,
        });
    };

    // 家族槽位：客户端只能填这四个名字（原型脚手架；真实绑定归票 51）。
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
        push(name.into(), up, model.into(), "openai", true);
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
        push(name.into(), up, model.into(), wire, false);
    }

    // mock 场景：场景 × wire，两个编解码都过一遍。
    for scenario in SCENARIOS {
        for wire in ["anthropic", "openai"] {
            push(
                format!("mock-{wire}-{scenario}"),
                "mock",
                format!("mock-{scenario}"),
                wire,
                false,
            );
        }
    }
    for (scenario, wire) in ONE_WIRE_SCENARIOS {
        push(
            format!("mock-{wire}-{scenario}"),
            "mock",
            format!("mock-{scenario}"),
            wire,
            false,
        );
    }
    for scenario in SPECIAL_SCENARIOS {
        for wire in ["anthropic", "openai"] {
            push(
                format!("mock-{wire}-{scenario}"),
                "mock",
                format!("mock-{scenario}"),
                wire,
                false,
            );
        }
    }
    for code in ERROR_CODES {
        for wire in ["anthropic", "openai"] {
            push(
                format!("mock-{wire}-err{code}"),
                "mock",
                format!("mock-err{code}"),
                wire,
                false,
            );
        }
    }
    // count_tokens 只有 Anthropic wire 会转发上游，所以两条降级负例也只在那个 wire 上。
    for (name, real) in [
        ("mock-anthropic-count404", "mock-count404"),
        ("mock-anthropic-countnoinput", "mock-countnoinput"),
    ] {
        push(name.into(), "mock", real.into(), "anthropic", false);
    }

    let store = Store::open(std::path::Path::new(&data_dir))?;
    let mut gw = Gateway::new(store, upstreams, routes);
    gw.ping = Duration::from_secs(env_u64("GATEWAY_PING_SECS", 15));
    gw.probe = Duration::from_secs(env_u64("GATEWAY_PROBE_SECS", 20));
    if std::env::var("GATEWAY_LOG_BODY")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        gw.set_body_log(BODY_LOG_WINDOW.as_secs());
    }
    let state = Arc::new(gw);

    let app = router()
        .route("/healthz", get(|| async { "ok" }))
        // body 日志开关的测试控制面（票 30 第 7 条）：真产品里它是设置页那个开关 + 1 小时自动关。
        .route("/__bodylog", post(set_body_log))
        .with_state(state.clone());

    println!(
        "网关   http://127.0.0.1:{port}   (inbound: POST /v1/messages, POST /v1/messages/count_tokens, POST /v1/responses, GET /v1/models)"
    );
    println!("mock   http://127.0.0.1:{mock_port}  (独立进程 mock-upstream)");
    println!("路由表（{} 条）:", state.routes.len());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// body 日志开关的控制面（票 30 第 7 条）：`{"seconds": N}` 拨窗口，`0` 立即关。
async fn set_body_log(State(app): State<Arc<Gateway>>, Json(body): Json<Value>) -> Response {
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
