//! 管理 API：前端与外壳读的 `/api/*`（spec §3 / §10），与转发面 `/v1/*` 分开挂。
//!
//! 票 43 只落了 `GET /api/status` 一格；票 52 把前端四页（概览 / 上游 / 模型与映射 / 客户端）
//! 需要的聚合与动作端点补齐。**路径与语义对齐 `.scratch/model-gateway/frontend.md` §③ 那张
//! 反推表**，不另起一套命名。
//!
//! - 只读聚合在 [`overview`] / [`upstreams`] / [`bindings`] / [`clients`]；
//! - 端口与启停（只有外壳做得了）在 [`gateway`]，经 [`GatewayControl`] 转给外壳；
//! - 业务能力**全部复用已有公开 API**：`egress::provider`（票 49）、`egress::discovery`（票 50）、
//!   `binding`（票 51）、`writer` 三客户端（票 45–48）、`clients`（本票的装配层）。这里不重写。
//! - 界面侧只看到掩码：`/api/*` 的响应里**永远没有密钥明文**（spec §9 / 票 17 第 82 条）。
//!
//! 票 53 在这里挂日志与用量基础（日志页与概览要的那半，合并时原样保留）：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/logs` | 元数据表格：时间倒序 50 条 + 四个筛选 + 模型名 / 请求 ID 搜索 + 翻页 |
//! | `GET /api/logs/filters` | 筛选下拉的候选值（库里真出现过的） |
//! | `GET/POST /api/logs/body-recording` | body 开关（默认关、开启需隐私确认、1 小时自动关） |
//! | `GET /api/usage/summary` | 概览要的今日 / 近 7 天 × 虚拟模型 / 上游（票 53 的统计层） |
//!
//! 详情四段（`GET /api/logs/{id}`）、五个时间范围与按天明细归票 59。
//!
//! 票 59 在这里补齐用量、日志详情与设置三页：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/usage`（与 `/api/usage/summary` 同一个 handler） | 五个时间范围 × 三维度 + 按天明细 |
//! | `GET /api/logs/{id}` | 详情四段：概要 / 请求与响应体 / 帧时间轴 / 错误信息 |
//! | `POST /api/logs/clear` | 清空日志（body 与帧随外键级联走） |
//! | `GET/PUT /api/settings`、`POST /api/settings/db/open`、`POST /api/catalog/refresh`、`GET /api/about` | 设置页（见 [`settings`]） |
//!
//! 帧时间轴的落点、`request_frames` 与 `request_bodies.response_body` 的写入都在
//! `egress::upstream`（body 开关开着时才记），查询在 [`crate::obs::log_detail`]。
//!
//! 票 62 在这里补分发收尾那一格（设置页要的设置项与导出 / 导入）：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET/PUT /api/settings/autostart` | 开机自启开关（读写 `HKCU\...\Run`，默认关） |
//! | `POST /api/export` | 导出配置到文件（默认不含明文密钥；给口令 = 加密导出） |
//! | `POST /api/import` | 导入配置（回执里带版本与重复上游） |
//! | `GET /api/dist/info` | 导出格式版本、DPAPI 换机提示、用法文案 |
//!
//! 这四个要动数据目录与库，所以由外壳在启动时递进来一个 [`Maintenance`] 句柄；
//! **没递进来就 503**，不静默假装成功。
//!
//! 票 61 在这里补自动升级那一格（更新横幅与设置页调它）：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/update/status` | 当前版本、端点、上次检查结果、下载进度、回退指引与本地留包 |
//! | `POST /api/update/check` | 去端点问一次有没有新版（失败给 502 + 人话） |
//! | `POST /api/update/install` | 下载 + 验签 + 静默安装并自动重启（**立刻应答**，进度走 status） |
//!
//! 检查 / 下载 / 安装是 Tauri updater 插件的事（要 AppHandle），所以外壳递进来一个
//! [`crate::dist::update::UpdateSource`]；**没递进来同样是 503**。

pub mod bindings;
pub mod clients;
pub mod gateway;
pub mod overview;
pub mod settings;
pub mod upstreams;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::binding::BindingTable;
use crate::clients::{ClientEnv, ClientId, GatewayValues};
use crate::dist::autostart::{self, HkcuRun};
use crate::dist::update::UpdateSource;
use crate::dist::{bundle, DPAPI_MIGRATION_WARNING};
use crate::obs::{self, BodyLogWindow, LogQuery, BODY_LOG_WINDOW};
use crate::store::{settings_keys, Store};
use crate::writer::claude_code::ModelSlots;

/// 客户端填的占位 key（网关不校验入站凭据；真实上游凭据在网关侧、只以掩码出界面）。
pub const PLACEHOLDER_API_KEY: &str = "claude-gateway-local";

/// 外壳要替网关做的两件事（管理 API 跑在网关进程里，重启自己得靠外壳）。
pub trait GatewayControl: Send + Sync {
    /// 网关没在跑时把它拉起来（未运行横幅的「启动网关」）。
    fn start(&self) -> Result<(), String>;
    /// 换端口：外壳停掉旧监听、按新端口再起（**用户确认过才调**，不静默换端口）。
    fn restart(&self, port: u16) -> Result<(), String>;
}

/// 什么都不做的实现（回放 harness / 单测：管理面照样能用，只是启停没有落点）。
#[derive(Debug, Default)]
pub struct NoopControl;

impl GatewayControl for NoopControl {
    fn start(&self) -> Result<(), String> {
        Err("这一档没有外壳（无头模式），请在应用里点「启动网关」".to_string())
    }

    fn restart(&self, _port: u16) -> Result<(), String> {
        Err("这一档没有外壳（无头模式），换端口后请手动重启".to_string())
    }
}

/// 管理面的业务上下文：库、客户端的三个配置根、外壳句柄、出站 HTTP 客户端。
///
/// `ApiState` 里它是 `Option`：M1 那条「只挂管理面」的路（`forwarding = None` 的单测）
/// 不带它，`/api/*` 的聚合端点会明确报「这一档没有管理面」而不是 500。
pub struct AdminCtx {
    pub store: Store,
    pub data_dir: PathBuf,
    pub env: ClientEnv,
    pub control: Arc<dyn GatewayControl>,
    pub http: reqwest::Client,
    pub version: String,
    pub db_path: String,
    /// 转发面（票 41 的 [`crate::ingress::proxy::Gateway`]）：改完配置热重载那三张表用它。
    proxy: Option<Arc<crate::ingress::proxy::Gateway>>,
    /// 真正绑上的端口（`service::start` 绑定后写入）；0 = 还不知道，回退 `settings.port`。
    real_port: Arc<AtomicU16>,
}

/// 手写 `Debug`：上下文里有库句柄与凭据入口，**绝不打印内容**。
impl std::fmt::Debug for AdminCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminCtx")
            .field("data_dir", &self.data_dir)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl AdminCtx {
    /// 装一个管理面上下文（出站 HTTP 客户端在这里建：外壳不必自己引 reqwest）。
    pub fn new(
        store: Store,
        data_dir: PathBuf,
        env: ClientEnv,
        control: Arc<dyn GatewayControl>,
        version: impl Into<String>,
        db_path: impl Into<String>,
    ) -> AdminCtx {
        AdminCtx {
            store,
            data_dir,
            env,
            control,
            // 单次请求的上限比探针档（20 秒）宽：拉 /models 与验收都要留余量，各自另有超时。
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
            version: version.into(),
            db_path: db_path.into(),
            proxy: None,
            real_port: Arc::new(AtomicU16::new(0)),
        }
    }

    /// 挂上转发面（外壳与集成测试）；改配置的端点会经 [`AdminCtx::reload_forwarding`] 让它
    /// **立刻看到新表**，不必重启网关。
    pub fn with_proxy(mut self, proxy: Arc<crate::ingress::proxy::Gateway>) -> AdminCtx {
        self.proxy = Some(proxy);
        self
    }

    /// 上游 / 目录 / 绑定表改过之后重载转发面（没有挂转发面就是空操作：M1 那一档）。
    pub async fn reload_forwarding(&self) -> anyhow::Result<()> {
        match &self.proxy {
            Some(gw) => gw.reload().await,
            None => Ok(()),
        }
    }

    /// 监听绑定后由 [`crate::service::start`] 写进来：**实际端口的唯一真相**。
    pub fn set_real_port(&self, port: u16) {
        self.real_port.store(port, Ordering::SeqCst);
    }

    /// 读一条 `settings`（没有就是 `None`）。
    pub async fn setting(&self, key: &str) -> Option<String> {
        let key = key.to_string();
        self.store
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT value FROM settings WHERE key=?1",
                    rusqlite::params![key],
                    |r| r.get::<_, String>(0),
                )
                .ok())
            })
            .await
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    }

    /// 写一条 `settings`。
    pub async fn set_setting(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let (key, value) = (key.to_string(), value.to_string());
        self.store
            .write(move |c| {
                c.execute(
                    "INSERT INTO settings(key,value) VALUES(?1,?2)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    rusqlite::params![key, value],
                )?;
                Ok(())
            })
            .await
    }

    /// 当前端口：**实际绑上的那个**优先（测试与「端口写进库但还没重启」两种情况都靠它），
    /// 其次 `settings.port`（票 42 的表；外壳启动时也读它）。
    pub async fn port(&self) -> u16 {
        let real = self.real_port.load(Ordering::SeqCst);
        if real != 0 {
            return real;
        }
        self.setting(settings_keys::PORT)
            .await
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p != 0)
            .unwrap_or(crate::service::DEFAULT_PORT)
    }

    pub async fn base_url(&self) -> String {
        format!("http://{}:{}", crate::service::HOST, self.port().await)
    }

    /// 汇率（人民币为主、可切美元；库内美元为锚）。
    pub async fn exchange_rate(&self) -> f64 {
        self.setting(settings_keys::USD_CNY_RATE)
            .await
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|r| *r > 0.0)
            .unwrap_or(7.10)
    }

    /// 写进客户端的那几个值：地址跟端口走，模型清单按档案解析（票 51 的覆盖）。
    pub async fn gateway_values(&self) -> anyhow::Result<GatewayValues> {
        let table = crate::binding::load(&self.store).await?;
        let exposed = table.exposed_for(crate::binding::Profile::Codex);
        let default_model = exposed
            .iter()
            .find(|m| m.is_family && m.binding.is_some())
            .map(|m| m.name.clone())
            .unwrap_or_else(|| crate::binding::FAMILY_SLOTS[0].to_string());
        Ok(GatewayValues {
            base_url: self.base_url().await,
            api_key: PLACEHOLDER_API_KEY.to_string(),
            models: ModelSlots::default(),
            exposed,
            default_model,
        })
    }

    /// 绑定表（界面与诊断都要）。
    pub async fn bindings(&self) -> anyhow::Result<BindingTable> {
        crate::binding::load(&self.store).await
    }

    /// 探针 / 自检共用一份出站调试日志的目录。
    pub fn log_root(&self) -> PathBuf {
        crate::writer::WriteRoots::with_data_dir(&self.data_dir, &self.data_dir).log_root
    }
}

/// 维护动作（开机自启 / 导出 / 导入）要的东西：库 + 数据目录。
///
/// `autostart_value` 可换，只为测试能用一个临时值名做真实注册表往返（真实的
/// `Claude Gateway` 值不许被测试改来改去）。
#[derive(Clone)]
pub struct Maintenance {
    store: Store,
    data_dir: PathBuf,
    autostart_value: String,
}

impl Maintenance {
    /// 真实用法：Run 值名 = `productName`。
    pub fn new(store: Store, data_dir: impl Into<PathBuf>) -> Maintenance {
        Maintenance::with_autostart_value(store, data_dir, autostart::VALUE_NAME)
    }

    pub fn with_autostart_value(
        store: Store,
        data_dir: impl Into<PathBuf>,
        autostart_value: impl Into<String>,
    ) -> Maintenance {
        Maintenance {
            store,
            data_dir: data_dir.into(),
            autostart_value: autostart_value.into(),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn autostart_value(&self) -> &str {
        &self.autostart_value
    }
}

/// 手写 `Debug`：`Store` 没有 `Debug`（它握着写线程句柄），而 `GatewayConfig` 需要。
/// 这里只暴露路径与值名，不碰库内部。
impl std::fmt::Debug for Maintenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Maintenance")
            .field("db_path", &self.store.db_path())
            .field("data_dir", &self.data_dir)
            .field("autostart_value", &self.autostart_value)
            .finish()
    }
}

/// 管理 API 的共享状态。`running` 由服务生命周期翻转（`service::Gateway`）。
///
/// `logs` 只在转发面挂上时才有（日志库与 body 窗口都在 [`crate::ingress::proxy::Gateway`] 里，
/// 票 53 用 [`ApiState::attach_logs`] 接过来，不另开第二个窗口实例）。
/// `maint` 同理：外壳递进来才有（票 62），没有就 503。
#[derive(Clone)]
pub struct ApiState {
    version: String,
    db_path: String,
    port: u16,
    started_at: SystemTime,
    running: Arc<AtomicBool>,
    admin: Option<Arc<AdminCtx>>,
    logs: Option<LogsState>,
    maint: Option<Maintenance>,
    /// 自动升级（票 61）：外壳把 Tauri updater 插件接进来才有，没有就 503。
    updater: Option<Arc<dyn UpdateSource>>,
}

/// 日志面的两个句柄（票 53；与转发路径共用同一个 body 窗口实例）。
#[derive(Clone)]
struct LogsState {
    store: Store,
    body_log: Arc<BodyLogWindow>,
}

impl ApiState {
    /// 绑定成功后构造：此刻起算「运行中」。
    pub fn new(version: impl Into<String>, db_path: impl Into<String>, port: u16) -> Self {
        Self {
            version: version.into(),
            db_path: db_path.into(),
            port,
            started_at: SystemTime::now(),
            running: Arc::new(AtomicBool::new(true)),
            admin: None,
            logs: None,
            maint: None,
            updater: None,
        }
    }

    /// 票 53：把转发面的日志库与 body 窗口接过来（日志页上的开关就是转发路径读的那个）。
    pub fn attach_logs(mut self, forwarding: &crate::ingress::proxy::Gateway) -> Self {
        self.logs = Some(LogsState {
            store: forwarding.store.clone(),
            body_log: forwarding.body_log.clone(),
        });
        self
    }

    /// 带上管理面上下文（外壳与集成测试走这条）。
    pub fn with_admin(mut self, admin: Arc<AdminCtx>) -> Self {
        self.admin = Some(admin);
        self
    }

    pub fn admin(&self) -> Option<&Arc<AdminCtx>> {
        self.admin.as_ref()
    }

    /// 挂上维护面（票 62，外壳启动时调）。
    pub fn with_maintenance(mut self, maint: Maintenance) -> Self {
        self.maint = Some(maint);
        self
    }

    pub fn maintenance(&self) -> Option<&Maintenance> {
        self.maint.as_ref()
    }

    /// 挂上升级面（票 61，外壳把 Tauri updater 插件接进来）。
    pub fn with_updater(mut self, updater: Arc<dyn UpdateSource>) -> Self {
        self.updater = Some(updater);
        self
    }

    pub fn updater(&self) -> Option<&Arc<dyn UpdateSource>> {
        self.updater.as_ref()
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// 网关监听端口（还原客户端配置 / 拼基址用）。
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn mark_stopped(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// 此刻在不在跑（诊断包要如实报这一格，不能写死 `true`）。
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn started_at(&self) -> SystemTime {
        self.started_at
    }

    pub fn snapshot(&self) -> GatewayStatus {
        GatewayStatus {
            version: self.version.clone(),
            port: self.port,
            running: self.running.load(Ordering::SeqCst),
            started_at: rfc3339_utc(self.started_at),
            uptime_seconds: SystemTime::now()
                .duration_since(self.started_at)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            db_path: self.db_path.clone(),
            data_dir: self
                .admin
                .as_ref()
                .map(|a| a.data_dir.to_string_lossy().to_string()),
        }
    }
}

/// `GET /api/status` 的响应体（camelCase 给前端直接用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub version: String,
    pub port: u16,
    pub running: bool,
    pub started_at: String,
    pub uptime_seconds: u64,
    pub db_path: String,
    /// 数据目录（设置页与诊断包要用；没挂管理面时为 `null`）。
    pub data_dir: Option<String>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        // 票 53：日志页（表 / 筛选候选 / body 开关）与概览的用量汇总（合并时原样保留）。
        .route("/api/logs", get(logs))
        .route("/api/logs/filters", get(log_filters))
        .route(
            "/api/logs/body-recording",
            get(body_recording).post(set_body_recording),
        )
        .route("/api/usage/summary", get(usage_summary))
        // 票 59：用量页的完整粒度（五个时间范围 × 三维度 + 按天明细）。
        // 与 `/api/usage/summary` **同一个 handler**：概览与用量页不会各算一套
        // （`frontend.md` §③ 第 10 / 39 条的附注）。
        .route("/api/usage", get(usage_summary))
        // 票 59：日志详情四段 + 清空日志。
        .route("/api/logs/clear", post(clear_logs))
        .route("/api/logs/{id}", get(log_detail))
        // 票 62：分发收尾（设置页的设置项 + 导出 / 导入）。
        .route("/api/dist/info", get(dist_info))
        .route(
            "/api/settings/autostart",
            get(autostart_get).put(autostart_put),
        )
        .route("/api/export", post(export_config))
        .route("/api/import", post(import_config))
        // 票 61：自动升级（更新横幅 / 设置页调这三个）。
        .route("/api/update/status", get(update_status))
        .route("/api/update/check", post(update_check))
        .route("/api/update/install", post(update_install))
        .merge(overview::routes())
        .merge(upstreams::routes())
        .merge(bindings::routes())
        .merge(clients::routes())
        .merge(gateway::routes())
        .merge(settings::routes())
        .layer(axum::middleware::from_fn(cors))
        .with_state(state)
}

/// 管理面的跨源白名单：开发态 Vite（1420）与 Tauri webview 的两个来源。
///
/// **不用 `*`**：`/api/*` 能改客户端配置文件，放开给任意网页等于给本机开了个后门。
/// 只回自己人；不在白名单的来源一个 CORS 头都不给。
fn allowed_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://localhost:1420"
            | "http://127.0.0.1:1420"
            | "tauri://localhost"
            | "http://tauri.localhost"
            | "https://tauri.localhost"
    )
}

async fn cors(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let origin = req
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let preflight = req.method() == axum::http::Method::OPTIONS;
    let mut resp = if preflight {
        Response::new(axum::body::Body::empty())
    } else {
        next.run(req).await
    };
    if let Some(origin) = origin.filter(|o| allowed_origin(o)) {
        let headers = resp.headers_mut();
        if let Ok(value) = origin.parse() {
            headers.insert("access-control-allow-origin", value);
        }
        headers.insert(
            "access-control-allow-methods",
            "GET,POST,PUT,DELETE,OPTIONS".parse().unwrap(),
        );
        headers.insert(
            "access-control-allow-headers",
            "content-type".parse().unwrap(),
        );
        headers.insert("vary", "origin".parse().unwrap());
    }
    resp
}

async fn status(State(state): State<ApiState>) -> Json<GatewayStatus> {
    Json(state.snapshot())
}

// ── 票 62：分发收尾的后端接口（票 59 的设置页调这些） ──────────────────────────

/// 维护面没递进来（外壳没给数据目录与库）时的统一答复：503，说清是「没挂上」而不是「没有数据」。
/// 错误体沿用日志面那份形状（`{"error":{"type","message"}}`），前端只认一种。
fn maintenance_unavailable() -> Response {
    failed_status(
        StatusCode::SERVICE_UNAVAILABLE,
        "维护面不可用：外壳没给数据目录与库（导出 / 导入 / 自启开关不可用）".to_string(),
    )
}

/// 维护动作出错的统一答复：400 + 人话（前端按 `error.message` 显示）。
fn maintenance_failed(msg: impl std::fmt::Display) -> Response {
    failed_status(StatusCode::BAD_REQUEST, msg.to_string())
}

/// 开机自启的当前状态（`GET` / `PUT` 同一个响应体）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartStatus {
    pub enabled: bool,
    /// 写 / 删的那个 Run 值名（= `productName`，卸载器按同名清理）。
    pub value_name: String,
    pub run_key: String,
    /// 打开时会写进去的命令行（界面可原样显示给用户看）。
    pub command_line: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartRequest {
    pub enabled: bool,
}

/// 自启状态响应（拼一次，读写两条路共用）。
fn autostart_status(enabled: bool, maint: &Maintenance, exe: &Path) -> Response {
    Json(AutostartStatus {
        enabled,
        value_name: maint.autostart_value().to_string(),
        run_key: autostart::RUN_KEY_PATH.to_string(),
        command_line: autostart::command_line(exe),
    })
    .into_response()
}

/// `GET /api/settings/autostart`：只读注册表（注册表是这一项的唯一真相）。
async fn autostart_get(State(state): State<ApiState>) -> Response {
    let Some(maint) = state.maintenance() else {
        return maintenance_unavailable();
    };
    let key = HkcuRun::named(maint.autostart_value());
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return maintenance_failed(format!("取当前 exe 路径失败：{e}")),
    };
    match autostart::is_enabled(&key) {
        Ok(enabled) => autostart_status(enabled, maint, &exe),
        Err(e) => maintenance_failed(e),
    }
}

/// `PUT /api/settings/autostart` body `{"enabled": true|false}`。
async fn autostart_put(
    State(state): State<ApiState>,
    Json(req): Json<AutostartRequest>,
) -> Response {
    let Some(maint) = state.maintenance() else {
        return maintenance_unavailable();
    };
    let key = HkcuRun::named(maint.autostart_value());
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return maintenance_failed(format!("取当前 exe 路径失败：{e}")),
    };
    match autostart::set_enabled(&key, req.enabled, &exe) {
        Ok(enabled) => autostart_status(enabled, maint, &exe),
        Err(e) => maintenance_failed(e),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// 导出文件路径（由界面的「另存为」给；后端不弹窗）。
    pub path: PathBuf,
    /// 给了口令就加密导出（能跨机搬密钥）。
    #[serde(default)]
    pub password: Option<String>,
    /// 是否把密钥明文放进文件。**默认 false**；为 true 时必须配口令。
    #[serde(default)]
    pub include_secrets: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResponse {
    pub path: PathBuf,
    pub bytes: usize,
    pub secrets: bool,
    pub encrypted: bool,
    /// 界面上必须显示这句：DPAPI 换机不可解密（spec §13）。
    pub dpapi_warning: String,
}

/// `POST /api/export`：写导出文件，回执里带上「这次带没带密钥 / 加没加密」。
async fn export_config(State(state): State<ApiState>, Json(req): Json<ExportRequest>) -> Response {
    let Some(maint) = state.maintenance() else {
        return maintenance_unavailable();
    };
    let opts = bundle::ExportOptions {
        include_secrets: req.include_secrets,
        password: req.password.clone(),
    };
    let text = match bundle::export(maint.store(), state.version(), &opts).await {
        Ok(text) => text,
        Err(e) => return maintenance_failed(e),
    };
    if let Err(e) = std::fs::write(&req.path, text.as_bytes()) {
        return maintenance_failed(format!("写导出文件失败（{}）：{e}", req.path.display()));
    }
    Json(ExportResponse {
        path: req.path,
        bytes: text.len(),
        secrets: opts.include_secrets,
        encrypted: opts.password.is_some(),
        dpapi_warning: DPAPI_MIGRATION_WARNING.to_string(),
    })
    .into_response()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub password: Option<String>,
    /// 重复上游是否覆盖（默认跳过，只把重复的列进回执）。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /api/import`：回执就是 [`bundle::ImportReport`]（版本 / 重复上游 / 各类计数）。
async fn import_config(State(state): State<ApiState>, Json(req): Json<ImportRequest>) -> Response {
    let Some(maint) = state.maintenance() else {
        return maintenance_unavailable();
    };
    let text = match std::fs::read_to_string(&req.path) {
        Ok(text) => text,
        Err(e) => {
            return maintenance_failed(format!("读导入文件失败（{}）：{e}", req.path.display()))
        }
    };
    let mode = if req.overwrite {
        bundle::ImportMode::Replace
    } else {
        bundle::ImportMode::SkipExisting
    };
    match bundle::import(maint.store(), &text, req.password.as_deref(), mode).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => maintenance_failed(e),
    }
}

// ── 票 61：自动升级（更新横幅 / 设置页调这三个） ────────────────────────────────

/// 升级面没挂上（外壳没把 updater 插件接进来）时的统一答复：503，说清是「没挂上」。
fn update_unavailable() -> Response {
    failed_status(
        StatusCode::SERVICE_UNAVAILABLE,
        "升级面不可用：外壳没有把 updater 接进来（检查更新 / 安装更新不可用）".to_string(),
    )
}

/// `GET /api/update/status`：当前版本、端点、上次检查结果、下载进度、回退指引与本地留包。
async fn update_status(State(state): State<ApiState>) -> Response {
    let Some(updater) = state.updater() else {
        return update_unavailable();
    };
    Json(updater.snapshot()).into_response()
}

/// `POST /api/update/check`：去端点问一次。查不动（网络 / 清单格式 / 端点不是 https）给 502 + 人话，
/// 界面按 `error.message` 原样显示——升级失败不许吞掉原因。
async fn update_check(State(state): State<ApiState>) -> Response {
    let Some(updater) = state.updater() else {
        return update_unavailable();
    };
    match updater.clone().check().await {
        Ok(status) => Json(status).into_response(),
        Err(message) => failed_status(StatusCode::BAD_GATEWAY, message),
    }
}

/// `POST /api/update/install`：下载 + 验签 + 静默安装。**立刻应答**，不等安装器：
/// 真装起来时插件会 `exit(0)` 让 NSIS 把新版拉起，这个进程根本回不了话；
/// 失败（验签不过、下载不到）落在 `status.error` 里，界面轮询 status 就能看到。
async fn update_install(State(state): State<ApiState>) -> Response {
    let Some(updater) = state.updater() else {
        return update_unavailable();
    };
    match updater.clone().start_install() {
        Ok(status) => Json(status).into_response(),
        Err(message) => failed_status(StatusCode::BAD_REQUEST, message),
    }
}

/// 给设置页（票 59）的一格常量：导出文件格式版本、DPAPI 换机提示、命令行用法。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistInfo {
    pub export_format: String,
    pub export_version: u32,
    pub dpapi_warning: String,
    pub usage: String,
    /// 开机自启的默认值（界面上「默认关闭」这句话的来源）。
    pub autostart_default: bool,
    pub restore_command: String,
}

async fn dist_info() -> Json<DistInfo> {
    Json(DistInfo {
        export_format: bundle::FORMAT_TAG.to_string(),
        export_version: bundle::FORMAT_VERSION,
        dpapi_warning: DPAPI_MIGRATION_WARNING.to_string(),
        usage: crate::dist::cli::USAGE.to_string(),
        autostart_default: false,
        restore_command: "Claude Gateway.exe --restore-clients --silent".to_string(),
    })
}

/// 读一条 `settings`（外壳 IPC 在没有 [`AdminCtx`] 时用它）。
pub async fn setting_of(store: &Store, key: &str) -> Option<String> {
    let key = key.to_string();
    store
        .read(move |c| {
            Ok(c.query_row(
                "SELECT value FROM settings WHERE key=?1",
                rusqlite::params![key],
                |r| r.get::<_, String>(0),
            )
            .ok())
        })
        .await
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
}

/// 写一条 `settings`。
pub async fn set_setting_of(store: &Store, key: &str, value: &str) -> anyhow::Result<()> {
    let (key, value) = (key.to_string(), value.to_string());
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
        .await
}

/// 管理面上下文；没挂（M1 单测那档）就明确报，不 500。
pub(crate) fn admin(state: &ApiState) -> Result<Arc<AdminCtx>, ApiFailure> {
    state.admin.clone().ok_or_else(|| {
        ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "这一档只挂了管理面骨架（没有库与客户端配置根）",
                "网关是以无头 / 单测形态起的，没有 `/api/*` 的聚合上下文",
            ),
        )
    })
}

/// 错误模板（spec §10 错误表达）：**发生了什么 + 可能原因 + 一个主按钮 + 复制诊断**。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// 发生了什么（一句话，人话）。
    pub what: String,
    /// 可能原因。
    pub likely_cause: String,
    /// 主按钮（前端拿它渲染一颗按钮；`null` = 只提示）。
    pub action: Option<ApiAction>,
    /// 原始错误原文（可展开）。
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAction {
    pub label: String,
    /// 动作类型（前端据此换页 / 重试 / 起网关 / 换端口）。
    pub kind: String,
    /// 换页类动作的目标页（`upstreams` / `models` / `clients` / `overview`）。
    pub page: Option<String>,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        what: impl Into<String>,
        likely_cause: impl Into<String>,
    ) -> ApiError {
        let _ = status;
        ApiError {
            what: what.into(),
            likely_cause: likely_cause.into(),
            action: None,
            raw: None,
        }
    }

    pub fn with_action(mut self, label: &str, kind: &str, page: Option<&str>) -> ApiError {
        self.action = Some(ApiAction {
            label: label.to_string(),
            kind: kind.to_string(),
            page: page.map(str::to_string),
        });
        self
    }

    pub fn with_raw(mut self, raw: impl std::fmt::Display) -> ApiError {
        self.raw = Some(raw.to_string());
        self
    }
}

/// 带上 HTTP 状态码的错误信封。
///
/// `error` 装箱：`ApiError` 里有几句话（String），不装箱会让每个 `Result<_, ApiFailure>`
/// 的 Err 变体撑到 150+ 字节（clippy 的 `result_large_err`），handler 遍地都是。
#[derive(Debug)]
pub struct ApiFailure {
    pub status: StatusCode,
    pub error: Box<ApiError>,
}

impl ApiFailure {
    pub fn new(status: StatusCode, error: ApiError) -> ApiFailure {
        ApiFailure {
            status,
            error: Box::new(error),
        }
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(e: anyhow::Error) -> ApiFailure {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "操作没能完成",
                "网关内部出错（看原始错误）",
            )
            .with_raw(e),
        )
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error.as_ref() }))).into_response()
    }
}

/// `Json<T>` 之外还要给一件「原始结果也带上」的：直接透传 serde 值。
pub(crate) fn ok_json(value: Value) -> Json<Value> {
    Json(value)
}

/// 128 位随机 id（本地用，不做密码学）：探针 / 验收请求的 requestId 前缀。
pub fn random_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{prefix}-{:x}{:x}", nanos, pid)
}

/// 现在（Unix 秒）。
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `GET /api/logs` 的查询串。空串当没传（界面清空输入框时就是空串）。
#[derive(Debug, Default, Deserialize)]
struct LogParams {
    limit: Option<usize>,
    cursor: Option<i64>,
    client: Option<String>,
    upstream: Option<String>,
    virtual_model: Option<String>,
    status: Option<String>,
    q: Option<String>,
}

/// 空串 → `None`：`?client=` 与不带这个参数是同一件事。
fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// 日志元数据表格：时间倒序，缺省 50 条；四个筛选维度 + 模型名 / 请求 ID 搜索 + 游标翻页。
///
/// `next_cursor` 只在**这一页满了**时给（等于最后一行的 `id`），界面上「加载更多」拿它再要一页。
/// `rate` 是库里的美元→人民币汇率（票 33 第 2 节：库内美元为锚，界面按当前汇率换算）。
async fn logs(State(state): State<ApiState>, Query(p): Query<LogParams>) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    let limit = p.limit.filter(|v| *v > 0).unwrap_or(obs::DEFAULT_PAGE);
    let query = LogQuery {
        limit: Some(limit),
        cursor: p.cursor,
        client: non_empty(p.client),
        upstream: non_empty(p.upstream),
        virtual_model: non_empty(p.virtual_model),
        status: non_empty(p.status),
        q: non_empty(p.q),
    };
    match obs::list(&logs.store, &query).await {
        Ok(rows) => {
            let next_cursor = (rows.len() >= limit.min(500))
                .then(|| rows.last().and_then(|r| r["id"].as_i64()))
                .flatten();
            Json(json!({
                "logs": rows,
                "limit": limit,
                "next_cursor": next_cursor,
                "rate": obs::usd_cny_rate(&logs.store).await.unwrap_or(None),
                "retention_days": obs::retention_days(&logs.store).await.unwrap_or(obs::DEFAULT_RETENTION_DAYS),
            }))
            .into_response()
        }
        Err(e) => failed(format!("读日志失败：{e}")),
    }
}

/// 筛选下拉的候选值（客户端 / 上游 / 虚拟模型来自库，状态码四档固定）。
async fn log_filters(State(state): State<ApiState>) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    match obs::filters(&logs.store).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failed(format!("读筛选候选失败：{e}")),
    }
}

/// `POST /api/logs/body-recording` 的请求体。
///
/// `seconds` 是窗口长度，缺省 = 1 小时；**上限就是 1 小时**（spec：开启后 1 小时自动关，
/// 界面没有「一直开着」这个选项）。测试用它把窗口拨短。
#[derive(Debug, Deserialize)]
struct BodyRecordingBody {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    consent: bool,
    seconds: Option<u64>,
}

/// body 开关的当前状态（含还剩几秒自动关）。
async fn body_recording(State(state): State<ApiState>) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    Json(state_json(&logs.body_log, &logs.store).await).into_response()
}

/// 开 / 关 body 开关。**开启必须带一次隐私确认**（`consent: true`），否则 400 不动开关。
async fn set_body_recording(
    State(state): State<ApiState>,
    Json(body): Json<BodyRecordingBody>,
) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    if body.enabled {
        if !body.consent {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {
                    "type": "consent_required",
                    "message": "开启「记录完整 body」前需要一次隐私确认：请求与响应体会明文存进本机 SQLite，随日志一起保留若干天后自动清理。",
                }})),
            )
                .into_response();
        }
        let window = body
            .seconds
            .filter(|s| *s > 0)
            .unwrap_or(BODY_LOG_WINDOW.as_secs())
            .min(BODY_LOG_WINDOW.as_secs());
        logs.body_log.set(window);
    } else {
        logs.body_log.set(0);
    }
    Json(state_json(&logs.body_log, &logs.store).await).into_response()
}

/// 开关状态 + 保留期（界面要说清「明文存本机 SQLite、保留 N 天」）。
async fn state_json(body_log: &BodyLogWindow, store: &Store) -> serde_json::Value {
    json!({
        "on": body_log.on(),
        "seconds_remaining": body_log.seconds_remaining(),
        "window_seconds": BODY_LOG_WINDOW.as_secs(),
        "retention_days": obs::retention_days(store).await.unwrap_or(obs::DEFAULT_RETENTION_DAYS),
    })
}

/// `GET /api/usage/summary?range=today|7d&dim=virtual_model|upstream|client`（概览要的那半）。
///
/// 票 59 起 `range` 也认 `30d` / `all` / `custom`（自定义要配 `from` / `to`，本地日期
/// `YYYY-MM-DD`，`to` 含当天），返回里多一份**按天明细** `daily`。
async fn usage_summary(
    State(state): State<ApiState>,
    Query(p): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    let range = p.get("range").map(String::as_str).unwrap_or("today");
    let dim = p.get("dim").map(String::as_str).unwrap_or("virtual_model");
    let from = p.get("from").map(String::as_str);
    let to = p.get("to").map(String::as_str);
    match obs::usage(&logs.store, range, dim, from, to).await {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert(
                    "rate".into(),
                    json!(obs::usd_cny_rate(&logs.store).await.unwrap_or(None)),
                );
            }
            Json(v).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"type": "invalid_request_error", "message": e.to_string()}})),
        )
            .into_response(),
    }
}

/// 详情四段（票 59）：概要 / 请求与响应体 / 帧时间轴 / 错误信息。
///
/// `:id` 认两种写法：纯数字 = 行 id（界面从列表跳过来用那把钥匙），其余 = `request_id`。
/// 找不到是 **404**（不是 200 + 空详情）。
async fn log_detail(
    State(state): State<ApiState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    let detail = match obs::log_detail(&logs.store, &id).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return failed_status(
                StatusCode::NOT_FOUND,
                format!("没有这条请求日志：{id}（按行 id 或 request_id 都查不到）"),
            )
        }
        Err(e) => return failed(format!("读日志详情失败：{e}")),
    };
    let recording_on = logs.body_log.on();
    let retention = obs::retention_days(&logs.store)
        .await
        .unwrap_or(obs::DEFAULT_RETENTION_DAYS);
    let summary = &detail["summary"];
    let bodies = &detail["bodies"];
    let error_raw = summary["error"].clone();
    let status = summary["status"].as_i64().unwrap_or(0);
    let aborted = summary["aborted"].as_bool().unwrap_or(false);
    // 第四段：模板与概览「最近错误」共用一份（`overview::template_for`）。
    let template = (aborted || status >= 400).then(|| {
        overview::template_for(
            if status == 0 { 500 } else { status },
            error_raw.as_str().unwrap_or_default(),
        )
    });
    let has_bodies = bodies["request"].is_string() || bodies["response"].is_string();
    Json(json!({
        "summary": summary,
        "bodies": {
            "request": bodies["request"],
            "response": bodies["response"],
            "recorded": has_bodies,
            "recording_on": recording_on,
            "retention_days": retention,
        },
        "frames": {
            "frames": detail["frames"],
            // 「原始 SSE 文本」= `request_bodies.response_body`（上游原样字节）。
            "sse": bodies["response"],
            "recorded": detail["frames"].as_array().map(|f| !f.is_empty()).unwrap_or(false),
        },
        "error": {
            "raw": error_raw,
            "template": template,
        },
    }))
    .into_response()
}

/// 清空日志（票 59）：删请求行，body 与帧随外键级联走。
/// **二次确认是界面的事**（票 17 第 86 条），这里只认「调用方已经确认过」。
async fn clear_logs(State(state): State<ApiState>) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    match obs::clear_logs(&logs.store).await {
        Ok(deleted) => Json(json!({"deleted": deleted})).into_response(),
        Err(e) => failed(format!("清空日志失败：{e}")),
    }
}

/// 转发面没挂上（库开不了 / 上游表读不出）时日志面也没有数据源：502 比空表诚实。
fn unavailable() -> Response {
    failed_status(
        StatusCode::SERVICE_UNAVAILABLE,
        "日志面不可用：转发面未挂载（库或上游表读不出来）".to_string(),
    )
}

fn failed(message: String) -> Response {
    failed_status(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn failed_status(code: StatusCode, message: String) -> Response {
    (
        code,
        Json(json!({"error": {"type": "api_error", "message": message}})),
    )
        .into_response()
}

/// 手写历法换算（Howard Hinnant 的 civil_from_days），不为一个时间戳引日期库。
/// 外壳写自己的运行日志时也用这一份，时间格式全局一致。
pub fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (rem / 3_600, rem % 3_600 / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// 一个端口此刻是不是空的（换端口探测用它）。
pub async fn port_is_free(port: u16) -> bool {
    tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .is_ok()
}

/// 探一个空闲端口（操作系统挑，**只预填、不写入**）。
pub async fn probe_free_port() -> Option<u16> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .ok()?;
    listener.local_addr().ok().map(|a| a.port())
}

/// 客户端 id → 档案 / 配置根的转发，省得每个 handler 都写一遍 match。
pub(crate) fn parse_client(id: &str) -> Result<ClientId, ApiFailure> {
    ClientId::parse(id).ok_or_else(|| {
        ApiFailure::new(
            StatusCode::NOT_FOUND,
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("没有这个客户端：{id}"),
                "客户端只有 claude-desktop / claude-code / codex 三个",
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc3339_matches_known_instants() {
        // 期望值取自历法本身（含闰日与闰年百年规则），不是用本函数反算出来的。
        let cases = [
            (0_u64, "1970-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_749_997_600, "2025-06-15T14:26:40Z"),
            (1_792_066_312, "2026-10-15T12:11:52Z"),
            (4_102_444_800, "2100-01-01T00:00:00Z"),
        ];
        for (secs, want) in cases {
            let t = UNIX_EPOCH + Duration::from_secs(secs);
            assert_eq!(rfc3339_utc(t), want, "epoch {secs}");
        }
    }
}
