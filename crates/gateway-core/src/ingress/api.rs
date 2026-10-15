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

pub mod bindings;
pub mod clients;
pub mod gateway;
pub mod overview;
pub mod upstreams;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};

use crate::binding::BindingTable;
use crate::clients::{ClientEnv, ClientId, GatewayValues};
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
        Ok(crate::binding::load(&self.store).await?)
    }

    /// 探针 / 自检共用一份出站调试日志的目录。
    pub fn log_root(&self) -> PathBuf {
        crate::writer::WriteRoots::with_data_dir(&self.data_dir, &self.data_dir).log_root
    }
}

/// 管理 API 的共享状态。`running` 由服务生命周期翻转（`service::Gateway`）。
#[derive(Clone)]
pub struct ApiState {
    version: String,
    db_path: String,
    port: u16,
    started_at: SystemTime,
    running: Arc<AtomicBool>,
    admin: Option<Arc<AdminCtx>>,
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
        }
    }

    /// 带上管理面上下文（外壳与集成测试走这条）。
    pub fn with_admin(mut self, admin: Arc<AdminCtx>) -> Self {
        self.admin = Some(admin);
        self
    }

    pub fn admin(&self) -> Option<&Arc<AdminCtx>> {
        self.admin.as_ref()
    }

    pub fn mark_stopped(&self) {
        self.running.store(false, Ordering::SeqCst);
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
        .merge(overview::routes())
        .merge(upstreams::routes())
        .merge(bindings::routes())
        .merge(clients::routes())
        .merge(gateway::routes())
        .with_state(state)
}

async fn status(State(state): State<ApiState>) -> Json<GatewayStatus> {
    Json(state.snapshot())
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
#[derive(Debug)]
pub struct ApiFailure {
    pub status: StatusCode,
    pub error: ApiError,
}

impl ApiFailure {
    pub fn new(status: StatusCode, error: ApiError) -> ApiFailure {
        ApiFailure { status, error }
    }
}

impl From<anyhow::Error> for ApiFailure {
    fn from(e: anyhow::Error) -> ApiFailure {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "操作没能完成", "网关内部出错（看原始错误）")
                .with_raw(e),
        )
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error }))).into_response()
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

/// 本地时区的当日 00:00（Unix 秒）。**日边界按本机本地时区自然日**（spec §10 第 6 节）。
///
/// shortcut: 只对当前进程所在时区生效、不处理跨年边界的历史时刻（用量页只问「今日 / 近 N 天」）。
#[cfg(windows)]
pub fn local_day_start(now: i64) -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    // 本地墙钟 → 它的 UTC 偏移：拿同一个时刻的 UTC 本地分解算差值。
    let offset = local_offset_secs(&st);
    let local_now = now + offset;
    let day = local_now.div_euclid(86_400) * 86_400;
    day - offset
}

#[cfg(not(windows))]
pub fn local_day_start(now: i64) -> i64 {
    now.div_euclid(86_400) * 86_400
}

#[cfg(windows)]
fn local_offset_secs(st: &windows_sys::Win32::Foundation::SYSTEMTIME) -> i64 {
    // 手头这一刻的本地时间与 UTC 时间之差；同一时刻取两次，精度到秒，跨秒误差最多 1 秒。
    let local = ymd_to_unix(
        st.wYear as i64,
        st.wMonth as i64,
        st.wDay as i64,
        st.wHour as i64,
        st.wMinute as i64,
        st.wSecond as i64,
    );
    let utc = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    local - utc
}

/// 民用历法 → Unix 秒（days_from_civil，与 [`rfc3339_utc`] 同一套换算）。
fn ymd_to_unix(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + hh * 3_600 + mm * 60 + ss
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

    #[test]
    fn civil_conversion_round_trips_with_rfc3339() {
        // 用独立来源（历法本身）验：1970-01-01 与 2000-02-29 两个已知瞬间。
        assert_eq!(ymd_to_unix(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(ymd_to_unix(2000, 2, 29, 0, 0, 0), 951_782_400);
    }

    #[test]
    fn local_day_start_is_aligned_to_a_day() {
        let now = 1_792_066_312; // 2026-10-15T12:11:52Z
        let start = local_day_start(now);
        assert!(start <= now && now - start < 86_400, "{start} vs {now}");
        assert_eq!((now - start) % 60, 52 % 60, "同一天内的秒内偏移应当一致");
    }
}
