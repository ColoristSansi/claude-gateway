//! 设置页的后端（spec §10 第 7 条，票 59）：设置项读写、打开数据目录、快照刷新、关于与版本。
//!
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 40–46 条：
//!
//! | 端点 | 用途 |
//! | --- | --- |
//! | `GET /api/settings` | 端口 / 自启 / 托盘行为 / 保留期 / body 开关 / 库位置 / 汇率 / 快照时间 / 版本 |
//! | `PUT /api/settings` | 写回上面几项（端口走 `/api/gateway/port` 那条**带确认与同步**的路） |
//! | `POST /api/settings/db/open` | 「打开目录」（数据目录 / 日志目录） |
//! | `POST /api/catalog/refresh` | models.dev 价格快照手动刷新（失败沿用旧快照，不自动拉） |
//! | `GET /api/about` | 版本 + 上一版回退指引 + 免安装 zip 版提示 |
//!
//! **凭据不放设置页**（票 17 第 66 条）：本模块一个密钥字段都不碰。

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dist::autostart::{self, HkcuRun};
use crate::egress::provider;
use crate::obs;
use crate::store::settings_keys;

use super::{admin, ok_json, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/settings", get(read_settings).put(write_settings))
        .route("/api/settings/db/open", post(open_db_dir))
        .route("/api/catalog/refresh", post(refresh_catalog))
        .route("/api/about", get(about))
}

/// 截断错误链（界面只要一句人话，原始错误在 `raw` 里）。
fn bad_request(what: &str, likely_cause: &str, raw: impl std::fmt::Display) -> ApiFailure {
    ApiFailure::new(
        StatusCode::BAD_REQUEST,
        ApiError::new(StatusCode::BAD_REQUEST, what, likely_cause).with_raw(raw),
    )
}

/// 当前设置（`GET` / `PUT` 同一个响应体）。
async fn settings_json(state: &ApiState) -> Result<Value, ApiFailure> {
    let admin = admin(state)?;
    let retention = obs::retention_days(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    let (body_on, body_remaining) = state
        .logs
        .as_ref()
        .map(|l| (l.body_log.on(), l.body_log.seconds_remaining()))
        .unwrap_or((false, None));
    let snapshot_at = provider::catalog_snapshot_at(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    // 自启要读注册表：维护面没挂上（无头 / 单测）就是 `null`，不假装「关着」。
    let autostart = match state.maintenance() {
        Some(maint) => {
            let key = HkcuRun::named(maint.autostart_value());
            autostart::is_enabled(&key)
                .ok()
                .map(|enabled| json!({"enabled": enabled, "valueName": maint.autostart_value()}))
        }
        None => None,
    };
    Ok(json!({
        "version": state.version(),
        "port": admin.port().await,
        "dbPath": admin.db_path,
        "dataDir": admin.data_dir.to_string_lossy(),
        "logRetentionDays": retention,
        "usdCnyRate": admin.exchange_rate().await,
        "catalogSnapshotAt": snapshot_at,
        "trayCloseAction": tray_close_action(&admin.store).await,
        "bodyRecording": {
            "on": body_on,
            "seconds_remaining": body_remaining,
            "window_seconds": obs::BODY_LOG_WINDOW.as_secs(),
            "retention_days": retention,
        },
        "autostart": autostart,
    }))
}

async fn read_settings(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    Ok(ok_json(settings_json(&state).await?))
}

/// 托盘行为：关窗是「隐藏到托盘」还是「直接退出」（`settings.tray_close_action`）。
/// 外壳的 `on_window_event` 读的就是这一条；没有这一项时按 `hide`（票 18 §2 的默认）。
pub const TRAY_HIDE: &str = "hide";
pub const TRAY_QUIT: &str = "quit";

/// 读托盘行为（外壳与设置页共用；认不出的值退成 `hide`）。
pub async fn tray_close_action(store: &crate::store::Store) -> String {
    let raw = super::setting_of(store, settings_keys::TRAY_CLOSE_ACTION).await;
    match raw.as_deref() {
        Some(TRAY_QUIT) => TRAY_QUIT.to_string(),
        _ => TRAY_HIDE.to_string(),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsPatch {
    /// 新端口：走 `/api/gateway/port` 同一段 `apply_port_change`（写库 + 同步已生效客户端）。
    port: Option<u16>,
    log_retention_days: Option<i64>,
    usd_cny_rate: Option<f64>,
    tray_close_action: Option<String>,
}

/// 写设置：只认这四项，其余一律不碰（凭据、body 开关各有自己的端点）。
async fn write_settings(
    State(state): State<ApiState>,
    Json(patch): Json<SettingsPatch>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    if let Some(days) = patch.log_retention_days {
        if !(0..=3650).contains(&days) {
            return Err(bad_request(
                "日志保留期超出可设范围",
                "保留期是 0–3650 天（0 = 不自动清理）",
                days,
            ));
        }
        admin
            .set_setting(settings_keys::LOG_RETENTION_DAYS, &days.to_string())
            .await
            .map_err(ApiFailure::from)?;
    }
    if let Some(rate) = patch.usd_cny_rate {
        if !(rate.is_finite() && rate > 0.0 && rate < 1000.0) {
            return Err(bad_request(
                "汇率不合法",
                "汇率是大于 0 的美元→人民币数字（比如 7.10）；不联网，只按这个数换算",
                rate,
            ));
        }
        admin
            .set_setting(settings_keys::USD_CNY_RATE, &format!("{rate:.4}"))
            .await
            .map_err(ApiFailure::from)?;
    }
    if let Some(action) = patch.tray_close_action.as_deref() {
        if action != TRAY_HIDE && action != TRAY_QUIT {
            return Err(bad_request(
                "托盘行为取值不认识",
                "只认 hide（关窗隐藏到托盘）与 quit（关窗退出应用）",
                action,
            ));
        }
        admin
            .set_setting(settings_keys::TRAY_CLOSE_ACTION, action)
            .await
            .map_err(ApiFailure::from)?;
    }
    if let Some(port) = patch.port {
        // 换端口不静默（spec §10 故障态）：设置页那颗按钮走的也是 `apply_port_change`，
        // 写库之外还会**自动同步已生效客户端的地址**。
        if port == 0 {
            return Err(bad_request("端口不合法", "端口是 1–65535 的整数", port));
        }
        let current = admin.port().await;
        if port != current {
            if !super::port_is_free(port).await {
                return Err(ApiFailure::new(
                    StatusCode::CONFLICT,
                    ApiError::new(
                        StatusCode::CONFLICT,
                        format!("端口 {port} 已经被占用"),
                        "换一个空闲端口再试（设置页里那行端口会先探测一个空闲的预填）",
                    ),
                ));
            }
            super::gateway::apply_port_change(
                &admin.store,
                &admin.data_dir,
                &admin.env,
                current,
                port,
            )
            .await
            .map_err(ApiFailure::from)?;
            admin.set_real_port(port);
        }
    }
    Ok(ok_json(settings_json(&state).await?))
}

/// 「打开目录」：数据目录 / 日志目录（**只显示路径 + 打开，不提供搬家**，票 18 第 87 条）。
#[derive(Debug, Default, Deserialize)]
struct OpenDirBody {
    /// `data`（默认）/ `logs`。
    which: Option<String>,
}

/// 目标目录（纯函数，好断言）：`which` 只认 `data` / `logs`。
fn db_target(data_dir: &std::path::Path, which: Option<&str>) -> Option<std::path::PathBuf> {
    match which.unwrap_or("data") {
        "data" => Some(data_dir.to_path_buf()),
        "logs" => Some(data_dir.join("logs")),
        _ => None,
    }
}

async fn open_db_dir(
    State(state): State<ApiState>,
    body: Option<Json<OpenDirBody>>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let which = body
        .and_then(|Json(b)| b.which)
        .unwrap_or_else(|| "data".to_string());
    let target = db_target(&admin.data_dir, Some(&which)).ok_or_else(|| {
        bad_request(
            "不认识的目录名",
            "只认 data（数据库所在目录）与 logs（运行日志目录）",
            which.clone(),
        )
    })?;
    // 目录可能还没建（新的数据目录）：先建再开，免得资源管理器报「找不到」。
    let _ = std::fs::create_dir_all(&target);
    open_in_explorer(&target).map_err(|e| {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "没能打开目录",
                "资源管理器没起来（或这一档没有桌面）",
            )
            .with_raw(format!("{}：{e}", target.display())),
        )
    })?;
    Ok(ok_json(json!({
        "opened": target.to_string_lossy(),
        "which": which,
    })))
}

/// 用资源管理器打开一个目录。
fn open_in_explorer(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "只有 Windows 上有资源管理器",
        ))
    }
}

/// `POST /api/catalog/refresh`：联网拉一次 models.dev 快照。
/// **失败沿用旧快照**（票 33 第 1 节：不自动重试、不启动时自动拉）——这里把失败如实报出去，
/// 由界面提示「沿用旧快照」，库里那一份一个字节没动。
async fn refresh_catalog(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let before = provider::catalog_snapshot_at(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    match provider::refresh_catalog(&admin.store, &admin.http, provider::MODELS_DEV_URL).await {
        Ok(r) => Ok(ok_json(json!({
            "refreshed": true,
            "imported": r.imported,
            "at": r.at,
            "reusedOldSnapshot": false,
            "message": format!("已刷新 {} 条模型价，快照时间已更新", r.imported),
        }))),
        Err(e) => Ok(ok_json(json!({
            "refreshed": false,
            "imported": 0,
            "at": before,
            "reusedOldSnapshot": true,
            "message": format!("刷新失败，沿用旧快照：{e}"),
        }))),
    }
}

/// 关于与版本（spec §10 第 7 条 / 票 18 第 45 条）：版本 + 上一版回退指引 + 免安装 zip 版提示。
///
/// 回退指引与本地留包从**票 61** 的 `dist::update` 来（升级留包的正是它）：指引里会指名道姓
/// 写出该双击哪个文件，`packages` 给的是绝对路径。**不做一键回退按钮**——那一步本身就会写坏东西。
async fn about(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let rollback = crate::dist::update::rollback(&admin.data_dir, state.version());
    let packages: Vec<String> = rollback.packages.iter().map(|p| p.path.clone()).collect();
    let portable = is_portable_exe(&std::env::current_exe().unwrap_or_default());
    Ok(ok_json(json!({
        "version": state.version(),
        "dbPath": admin.db_path,
        "dataDir": admin.data_dir.to_string_lossy(),
        "updatesDir": rollback.installers_dir,
        "packages": packages,
        "rollback": {
            "guidance": rollback.guide,
            "hasPrevious": rollback.previous.is_some(),
        },
        "portable": {
            "isPortable": portable,
            "note": if portable {
                "免安装 zip 版不参与自动更新：此版本请手动替换。"
            } else {
                "安装版由安装包升级。"
            },
        },
        "update": {
            // 票 61 起内置自动更新：启动后静默检查一次，设置页可手动检查；下载后**必须**过签名
            // （不可关），passive 静默安装 + 装完自动重启。升级不改端口、数据与客户端配置。
            "supported": true,
            "note": "已内置自动更新：启动后静默检查一次，设置页可手动检查。下载后会校验签名（不可关），\
    被动静默安装并自动重启；升级不改端口、数据与客户端配置。"
        },
    })))
}

/// 免安装 zip 版判据（票 18 第 26 条）：NSIS 装出来的 exe 在
/// `%LOCALAPPDATA%\<productName>\` 下；不在那儿就是解压出来的 zip 版。
fn is_portable_exe(exe: &std::path::Path) -> bool {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let installed = std::path::PathBuf::from(local).join(autostart::VALUE_NAME);
    !exe.starts_with(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_target_only_knows_data_and_logs() {
        let dir = std::path::Path::new("C:\\data");
        assert_eq!(db_target(dir, None).unwrap(), dir);
        assert_eq!(db_target(dir, Some("data")).unwrap(), dir);
        assert_eq!(db_target(dir, Some("logs")).unwrap(), dir.join("logs"));
        assert!(db_target(dir, Some("elsewhere")).is_none());
    }

    #[test]
    fn about_reports_an_empty_rollback_for_a_dir_that_was_never_used() {
        // 「关于」那一格的回退指引来自 dist::update：数据目录里还没有 updates\installers\ 时
        // 应当是空清单 + 一条可执行的指引，而不是报错（票 61）。
        let rb = crate::dist::update::rollback(
            std::path::Path::new("C:\\definitely\\not\\here"),
            "0.1.0",
        );
        assert!(rb.packages.is_empty());
        assert!(rb.previous.is_none());
        assert!(rb.guide.contains("退出"), "{}", rb.guide);
        assert!(rb.guide.contains("当初下载"), "{}", rb.guide);
    }
}
