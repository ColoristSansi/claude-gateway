//! 管理 API：前端与外壳读的 `/api/*`（spec §3 / §10），与转发面 `/api` 之外的 `/v1/*` 分开挂。
//!
//! 只读快照，不碰请求路径；转发路径上只有转换，没有业务。
//!
//! 票 62 在这里补了分发收尾那一格（设置页的设置项与导出 / 导入）：`/api/settings/autostart`、
//! `/api/export`、`/api/import`、`/api/dist/info`。它们要动数据目录与库，所以由外壳在启动时
//! 递进来一个 [`Maintenance`] 句柄；**没递进来就 503**，不静默假装成功。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::dist::autostart::{self, HkcuRun};
use crate::dist::{bundle, DPAPI_MIGRATION_WARNING};
use crate::store::Store;

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
#[derive(Clone)]
pub struct ApiState {
    version: String,
    db_path: String,
    port: u16,
    started_at: SystemTime,
    running: Arc<AtomicBool>,
    /// 没递进来 = 这套构建 / 这个进程不提供维护面（`/api/export` 等一律 503）。
    maint: Option<Maintenance>,
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
            maint: None,
        }
    }

    /// 挂上维护面（外壳启动时调）。
    pub fn with_maintenance(mut self, maint: Maintenance) -> Self {
        self.maint = Some(maint);
        self
    }

    pub fn maintenance(&self) -> Option<&Maintenance> {
        self.maint.as_ref()
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// 网关监听端口（还原客户端配置时拼基址用）。
    pub fn port(&self) -> u16 {
        self.port
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
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/dist/info", get(dist_info))
        .route(
            "/api/settings/autostart",
            get(autostart_get).put(autostart_put),
        )
        .route("/api/export", post(export_config))
        .route("/api/import", post(import_config))
        .with_state(state)
}

async fn status(State(state): State<ApiState>) -> Json<GatewayStatus> {
    Json(state.snapshot())
}

// ── 票 62：分发收尾的后端接口（票 59 的设置页调这些） ──────────────────────────

/// 接口出错时的统一形状：`{ "error": "人话" }` + 合适的 HTTP 码。
type ApiError = (StatusCode, Json<serde_json::Map<String, serde_json::Value>>);

fn bad(msg: impl std::fmt::Display) -> ApiError {
    let mut map = serde_json::Map::new();
    map.insert(
        "error".to_string(),
        serde_json::Value::String(msg.to_string()),
    );
    (StatusCode::BAD_REQUEST, Json(map))
}

fn unavailable() -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json({
            let mut map = serde_json::Map::new();
            map.insert(
                "error".to_string(),
                serde_json::Value::String(
                    "这个进程没有挂维护面（外壳没递数据目录与库），导出 / 导入 / 自启开关不可用"
                        .to_string(),
                ),
            );
            map
        }),
    )
}

fn maintenance(state: &ApiState) -> Result<&Maintenance, ApiError> {
    state.maintenance().ok_or_else(unavailable)
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

/// `GET /api/settings/autostart`：只读注册表（注册表是这一项的唯一真相）。
async fn autostart_get(State(state): State<ApiState>) -> Result<Json<AutostartStatus>, ApiError> {
    let maint = maintenance(&state)?;
    let key = HkcuRun::named(maint.autostart_value());
    let enabled = autostart::is_enabled(&key).map_err(bad)?;
    let exe = std::env::current_exe().map_err(|e| bad(format!("取当前 exe 路径失败：{e}")))?;
    Ok(Json(AutostartStatus {
        enabled,
        value_name: maint.autostart_value().to_string(),
        run_key: autostart::RUN_KEY_PATH.to_string(),
        command_line: autostart::command_line(&exe),
    }))
}

/// `PUT /api/settings/autostart` body `{"enabled": true|false}`。
async fn autostart_put(
    State(state): State<ApiState>,
    Json(req): Json<AutostartRequest>,
) -> Result<Json<AutostartStatus>, ApiError> {
    let maint = maintenance(&state)?;
    let key = HkcuRun::named(maint.autostart_value());
    let exe = std::env::current_exe().map_err(|e| bad(format!("取当前 exe 路径失败：{e}")))?;
    let enabled = autostart::set_enabled(&key, req.enabled, &exe).map_err(bad)?;
    Ok(Json(AutostartStatus {
        enabled,
        value_name: maint.autostart_value().to_string(),
        run_key: autostart::RUN_KEY_PATH.to_string(),
        command_line: autostart::command_line(&exe),
    }))
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
async fn export_config(
    State(state): State<ApiState>,
    Json(req): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, ApiError> {
    let maint = maintenance(&state)?;
    let opts = bundle::ExportOptions {
        include_secrets: req.include_secrets,
        password: req.password.clone(),
    };
    let text = bundle::export(maint.store(), state.version(), &opts)
        .await
        .map_err(bad)?;
    std::fs::write(&req.path, text.as_bytes())
        .map_err(|e| bad(format!("写导出文件失败（{}）：{e}", req.path.display())))?;
    Ok(Json(ExportResponse {
        path: req.path,
        bytes: text.len(),
        secrets: opts.include_secrets,
        encrypted: opts.password.is_some(),
        dpapi_warning: DPAPI_MIGRATION_WARNING.to_string(),
    }))
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
async fn import_config(
    State(state): State<ApiState>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<bundle::ImportReport>, ApiError> {
    let maint = maintenance(&state)?;
    let text = std::fs::read_to_string(&req.path)
        .map_err(|e| bad(format!("读导入文件失败（{}）：{e}", req.path.display())))?;
    let mode = if req.overwrite {
        bundle::ImportMode::Replace
    } else {
        bundle::ImportMode::SkipExisting
    };
    let report = bundle::import(maint.store(), &text, req.password.as_deref(), mode)
        .await
        .map_err(bad)?;
    Ok(Json(report))
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

/// `SystemTime` → `YYYY-MM-DDTHH:MM:SSZ`（UTC）。
///
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
