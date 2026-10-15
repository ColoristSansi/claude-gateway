//! 管理 API：前端与外壳读的 `/api/*`（spec §3 / §10），与转发面 `/api` 之外的 `/v1/*` 分开挂。
//!
//! 只读快照 + 两个开关动作，不碰请求路径；转发路径上只有转换，没有业务。
//!
//! 票 53 在这里挂日志与用量基础（日志页与概览要的那半）：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/logs` | 元数据表格：时间倒序 50 条 + 四个筛选 + 模型名 / 请求 ID 搜索 + 翻页 |
//! | `GET /api/logs/filters` | 筛选下拉的候选值（库里真出现过的） |
//! | `GET/POST /api/logs/body-recording` | body 开关（默认关、开启需隐私确认、1 小时自动关） |
//! | `GET /api/usage/summary` | 概览要的今日 / 近 7 天 × 虚拟模型 / 上游 |
//!
//! 详情四段（`GET /api/logs/:requestId`）、五个时间范围与按天明细归票 59。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::obs::{self, BodyLogWindow, LogQuery, BODY_LOG_WINDOW};
use crate::store::Store;

/// 管理 API 的共享状态。`running` 由服务生命周期翻转（`service::Gateway`）。
///
/// `logs` 只在转发面挂上时才有（日志库与 body 窗口都在 [`crate::ingress::proxy::Gateway`] 里，
/// 票 53 用 [`ApiState::attach_logs`] 接过来，不另开第二个窗口实例）。
#[derive(Clone)]
pub struct ApiState {
    version: String,
    db_path: String,
    port: u16,
    started_at: SystemTime,
    running: Arc<AtomicBool>,
    logs: Option<LogsState>,
}

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
            logs: None,
        }
    }

    /// 把转发面的日志库与 body 窗口接过来（票 53）。同一个窗口实例，所以日志页上的开关
    /// 就是转发路径读的那个开关。
    pub fn attach_logs(mut self, forwarding: &crate::ingress::proxy::Gateway) -> Self {
        self.logs = Some(LogsState {
            store: forwarding.store.clone(),
            body_log: forwarding.body_log.clone(),
        });
        self
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
        // 票 53：日志页（表 / 筛选候选 / body 开关）与概览的用量汇总。
        .route("/api/logs", get(logs))
        .route("/api/logs/filters", get(log_filters))
        .route(
            "/api/logs/body-recording",
            get(body_recording).post(set_body_recording),
        )
        .route("/api/usage/summary", get(usage_summary))
        .with_state(state)
}

async fn status(State(state): State<ApiState>) -> Json<GatewayStatus> {
    Json(state.snapshot())
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
async fn usage_summary(
    State(state): State<ApiState>,
    Query(p): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(logs) = &state.logs else {
        return unavailable();
    };
    let range = p.get("range").map(String::as_str).unwrap_or("today");
    let dim = p.get("dim").map(String::as_str).unwrap_or("virtual_model");
    match obs::usage_summary(&logs.store, range, dim).await {
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
