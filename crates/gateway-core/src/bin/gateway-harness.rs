//! 回放 harness 的**网关进程**（票 41 的测试专用 bin）：把 `ingress::proxy::router()` 挂起来，
//! 供应商表 / 上游模型目录 / 绑定表都**种进票 42 的库再从库里读出来**（票 51 起）——真实
//! 装配路径与外壳（票 43）走的是同一条：`provider::save_upstream` → `save_manual_model` →
//! `binding::save` → `binding::load` / `provider::catalog_map`。
//!
//! **独立进程 + 测试专用**：挂 `required-features = ["mock"]`，产品构建里没有它。
//! 回放测试同时拉起两个进程（本 bin + `mock-upstream`），走**真实 HTTP**：
//! 「mock 与网关分进程」这条硬要求由它保证。
//!
//! 种进库的那张表仍然是**回放脚手架**（mock 场景 × wire + 家族槽位 + 两个真实上游 +
//! 一条 codex 档案覆盖）；`/v1/*` 那条路由链本身不动。
//!
//! 旋钮（都是环境变量）：`GATEWAY_ENV`（凭据 env 文件）/ `GATEWAY_DATA_DIR`（库目录）/
//! `GATEWAY_PORT` / `GATEWAY_MOCK_PORT` / `GATEWAY_PING_SECS` / `GATEWAY_PROBE_SECS` /
//! `GATEWAY_LOG_BODY`。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json,
};
use serde_json::{json, Value};

use gateway_core::binding::{self, Binding, Profile};
use gateway_core::egress::provider::{self, Env, Quirk, Upstream, Wire};
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

/// 回放脚手架：攒「绑定 + 它需要的目录条目」，最后整批种进库。
struct Scaffold {
    upstreams: HashMap<String, Upstream>,
    bindings: Vec<Binding>,
    catalog: BTreeMap<(String, String), BTreeSet<Wire>>,
}

impl Scaffold {
    /// 一条绑定：上游不存在（没配那条上游的 env）就整条跳过——绑定有外键，种不进去也不该种。
    /// 同一条 `(上游, 真实模型)` 的两条绑定可能各用不同的 wire，所以目录取并集。
    fn bind(
        &mut self,
        profile: Profile,
        name: &str,
        upstream_id: &str,
        real_model: &str,
        wire: Wire,
    ) {
        if !self.upstreams.contains_key(upstream_id) {
            return;
        }
        self.catalog
            .entry((upstream_id.to_string(), real_model.to_string()))
            .or_default()
            .insert(wire);
        self.bindings
            .push(Binding::new(profile, name, upstream_id, real_model, wire));
    }
}

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
    // 出站凭据（按「上游 × wire」存票 42 的库，DPAPI 密文）：种库时用一次，之后只从库里读。
    let mut keys: HashMap<String, String> = HashMap::new();

    // mock 上游：本机假上游，回放 fixtures/。由 mock-upstream 那个**独立进程**提供。
    // 两条头类 quirk 挂在它身上（票 19 §6.3）：quirk 的唯一执法点在 HTTP 组装路径上，
    // mock 是集成层唯一能真读到「出站请求头」的上游。
    let mut mock = Upstream::new("mock", "mock", format!("http://127.0.0.1:{mock_port}"));
    mock.set_path(Wire::Anthropic, "/anthropic/v1/messages");
    mock.set_path(Wire::OpenAiChat, "/openai/v1/chat/completions");
    mock.quirks = vec![
        Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1".into()),
        Quirk::SessionIdHeader("x-opencode-session".into()),
    ];
    keys.insert("mock".into(), "mock".into());
    upstreams.insert("mock".into(), mock);

    // 真实上游 1：opencode。UA 与会话头由 quirk 声明，网关侧自动处理。
    if let (Some(base), Some(key)) = (env.get("OPENCODE_BASE_URL"), env.get("OPENCODE_API_KEY")) {
        let mut up = Upstream::new("opencode", "opencode", base);
        up.set_path(Wire::Anthropic, "/messages");
        up.set_path(Wire::OpenAiChat, "/chat/completions");
        up.quirks = vec![
            Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1".into()),
            Quirk::SessionIdHeader("x-opencode-session".into()),
        ];
        keys.insert("opencode".into(), key);
        upstreams.insert("opencode".into(), up);
    } else {
        eprintln!("[warn] 缺 OPENCODE_* 配置，opencode 上游不可用");
    }

    // 真实上游 2：commandcode。Claude 模型的位置由上游模型目录的 `supported_wires` 表达。
    if let (Some(base), Some(key)) = (
        env.get("COMMANDCODE_BASE_URL"),
        env.get("COMMANDCODE_API_KEY"),
    ) {
        let mut up = Upstream::new("commandcode", "commandcode", base);
        up.set_path(Wire::Anthropic, "/messages");
        up.set_path(Wire::OpenAiChat, "/chat/completions");
        keys.insert("commandcode".into(), key);
        upstreams.insert("commandcode".into(), up);
    } else {
        eprintln!("[warn] 缺 COMMANDCODE_* 配置，commandcode 上游不可用");
    }

    // ── 绑定表与目录（票 51）：先攒成内存里那张脚手架，再种进库、从库里读回来 ──
    let mut s = Scaffold {
        upstreams,
        bindings: Vec::new(),
        catalog: BTreeMap::new(),
    };

    // 家族槽位：客户端只能填这四个名字（脚手架值；真实绑定由界面 / 票 59 写）。
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
        s.bind(Profile::Global, name, up, model, Wire::OpenAiChat);
    }

    // 真实上游验证条目：两个上游 × 两种 wire，另加三条「上游会拒」的负例。
    for (name, up, model, wire) in [
        (
            "oc-deepseek",
            "opencode",
            "deepseek-v4.1-flash",
            Wire::OpenAiChat,
        ),
        (
            "cc-sonnet",
            "commandcode",
            "claude-sonnet-5-5",
            Wire::Anthropic,
        ),
        // 负例 1：opencode 的 Anthropic 端点与 Chat 端点凭据不同 → 401。
        (
            "oc-anthropic-unauth",
            "opencode",
            "claude-haiku-5-5",
            Wire::Anthropic,
        ),
        // 负例 2：commandcode 的 opus 不在本计划内 → 403 MODEL_NOT_IN_PLAN。
        (
            "cc-opus-no-plan",
            "commandcode",
            "claude-opus-5",
            Wire::Anthropic,
        ),
        // 负例 3：故意错配 wire（commandcode 只把 opus 放在 Anthropic 面）。
        (
            "cc-opus-on-chat",
            "commandcode",
            "claude-opus-5",
            Wire::OpenAiChat,
        ),
    ] {
        s.bind(Profile::Global, name, up, model, wire);
    }

    // 票 51 的每客户端覆盖（回放用例拿它验「同一槽位按客户端走不同上游」）：
    // codex 档案把 `claude-haiku` 指到 mock 的 Anthropic 面；全局那份仍指 commandcode。
    s.bind(
        Profile::Codex,
        "claude-haiku",
        "mock",
        "mock-text",
        Wire::Anthropic,
    );

    // mock 场景：场景 × wire，两个编解码都过一遍。
    for scenario in SCENARIOS {
        for (wire, prefix) in [(Wire::Anthropic, "anthropic"), (Wire::OpenAiChat, "openai")] {
            s.bind(
                Profile::Global,
                &format!("mock-{prefix}-{scenario}"),
                "mock",
                &format!("mock-{scenario}"),
                wire,
            );
        }
    }
    for (scenario, prefix) in ONE_WIRE_SCENARIOS {
        s.bind(
            Profile::Global,
            &format!("mock-{prefix}-{scenario}"),
            "mock",
            &format!("mock-{scenario}"),
            Wire::parse(prefix).unwrap_or(Wire::Anthropic),
        );
    }
    for scenario in SPECIAL_SCENARIOS {
        for (wire, prefix) in [(Wire::Anthropic, "anthropic"), (Wire::OpenAiChat, "openai")] {
            s.bind(
                Profile::Global,
                &format!("mock-{prefix}-{scenario}"),
                "mock",
                &format!("mock-{scenario}"),
                wire,
            );
        }
    }
    for code in ERROR_CODES {
        for (wire, prefix) in [(Wire::Anthropic, "anthropic"), (Wire::OpenAiChat, "openai")] {
            s.bind(
                Profile::Global,
                &format!("mock-{prefix}-err{code}"),
                "mock",
                &format!("mock-err{code}"),
                wire,
            );
        }
    }
    // count_tokens 只有 Anthropic wire 会转发上游，所以两条降级负例也只在那个 wire 上。
    for (name, real) in [
        ("mock-anthropic-count404", "mock-count404"),
        ("mock-anthropic-countnoinput", "mock-countnoinput"),
    ] {
        s.bind(Profile::Global, name, "mock", real, Wire::Anthropic);
    }

    // ── 种库：上游 → 目录 → 绑定 → 凭据；再从库里读回请求期要用的两张表 ──
    let store = Store::open(std::path::Path::new(&data_dir))?;
    for up in s.upstreams.values() {
        provider::save_upstream(&store, up).await?;
    }
    for ((upstream_id, model_id), wires) in &s.catalog {
        let wires: Vec<Wire> = wires.iter().copied().collect();
        provider::save_manual_model(&store, upstream_id, model_id, &wires).await?;
    }
    for b in &s.bindings {
        binding::save(&store, b)
            .await
            .map_err(|e| anyhow::anyhow!("种绑定失败（{}）：{e}", b.name))?;
    }
    for (id, key) in &keys {
        store.put_credential(id, "default", key).await?;
    }
    let table = binding::load(&store).await?;
    let catalog = provider::catalog_map(&store).await?;

    let mut gw = Gateway::new(store, s.upstreams, table);
    gw.catalog = catalog;
    gw.ping = Duration::from_secs(env_u64("GATEWAY_PING_SECS", 15));
    gw.probe = Duration::from_secs(env_u64("GATEWAY_PROBE_SECS", 20));
    if std::env::var("GATEWAY_LOG_BODY")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        gw.set_body_log(BODY_LOG_WINDOW.as_secs());
    }
    let state = Arc::new(gw);

    // 票 53：harness 把**管理面也挂上**（`/api/logs*` / `/api/usage/summary`），日志页那几条
    // 验收才走得到真实 HTTP。产品侧由 `ingress::router` 干这件事（这里要额外挂两个测试控制面，
    // 所以自己 merge 一次，挂的仍然是同一个 `api::router`）。
    let api_state = gateway_core::ingress::api::ApiState::new(
        env!("CARGO_PKG_VERSION"),
        std::path::Path::new(&data_dir)
            .join(gateway_core::store::DB_FILE_NAME)
            .to_string_lossy()
            .into_owned(),
        port,
    )
    .attach_logs(&state);
    let app = gateway_core::ingress::api::router(api_state).merge(
        router()
            .route("/healthz", get(|| async { "ok" }))
            // body 日志开关的测试控制面（票 30 第 7 条）：它拨的是**同一个**窗口实例，
            // 正式入口是 `/api/logs/body-recording`（票 53）。
            .route("/__bodylog", post(set_body_log))
            .with_state(state.clone()),
    );

    println!(
        "网关   http://127.0.0.1:{port}   (inbound: POST /v1/messages, POST /v1/messages/count_tokens, POST /v1/responses, GET /v1/models)"
    );
    println!("mock   http://127.0.0.1:{mock_port}  (独立进程 mock-upstream)");
    println!(
        "绑定表（{} 条绑定 / {} 个暴露模型）:",
        state.bindings.len(),
        state.bindings.exposed().len()
    );

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
