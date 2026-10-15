//! 管理 API：前端与外壳读的 `/api/*`（spec §3 / §10），与转发面 `/api` 之外的 `/v1/*` 分开挂。
//!
//! 只读快照，不碰请求路径；转发路径上只有转换，没有业务。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// 管理 API 的共享状态。`running` 由服务生命周期翻转（`service::Gateway`）。
#[derive(Clone)]
pub struct ApiState {
    version: String,
    db_path: String,
    port: u16,
    started_at: SystemTime,
    running: Arc<AtomicBool>,
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
        }
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
        .with_state(state)
}

async fn status(State(state): State<ApiState>) -> Json<GatewayStatus> {
    Json(state.snapshot())
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
