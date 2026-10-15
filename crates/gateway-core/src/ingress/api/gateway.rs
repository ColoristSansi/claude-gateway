//! `/api/gateway/*`：未运行横幅的「启动网关」、端口被占时的「换端口」。
//!
//! 换端口**不静默**（spec §10 故障态）：探测只预填，写入要用户确认；写入成功后
//! **自动重写所有「已生效」客户端的地址**并提示「已同步 N 个客户端」，未生效的不动。
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 3–5 条。

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::clients::{self, ClientId, ClientState};
use crate::store::settings_keys;

use super::{admin, ok_json, port_is_free, probe_free_port, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/gateway/start", post(start))
        .route("/api/gateway/port/probe", post(probe))
        .route("/api/gateway/port", post(change_port))
}

/// 未运行横幅的主按钮：转给外壳把网关拉起来。
async fn start(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    admin.control.start().map_err(|e| {
        ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "启动网关没能完成",
                "端口被别的程序占着，或者这一档没有外壳（无头 / 单测）",
            )
            .with_action("换端口", "change-port", None)
            .with_raw(e),
        )
    })?;
    Ok(ok_json(json!({"started": true, "port": admin.port().await})))
}

/// 探测一个空闲端口（**只预填，不写入**）：用户确认后才走 `POST /api/gateway/port`。
async fn probe(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let current = admin.port().await;
    let suggestion = probe_free_port().await.ok_or_else(|| {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "探不到空闲端口",
                "操作系统没能分配临时端口",
            ),
        )
    })?;
    Ok(ok_json(json!({
        "currentPort": current,
        "currentPortFree": port_is_free(current).await,
        "suggestedPort": suggestion,
        "note": "只是预填：点「换端口」并确认之后才会写入，不静默换端口",
    })))
}

/// 用户确认后写入新端口 + **自动同步已生效客户端的地址**，再让外壳按新端口重启。
async fn change_port(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let port = body
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::BAD_REQUEST,
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "端口不合法",
                    "端口是 1–65535 的整数；先用 /api/gateway/port/probe 探一个空闲的",
                ),
            )
        })?;
    let old = admin.port().await;
    if port == old {
        return Ok(ok_json(json!({
            "port": port, "changed": false, "synced": [],
            "message": "端口没变，什么都没做",
        })));
    }
    if !port_is_free(port).await {
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            ApiError::new(
                StatusCode::CONFLICT,
                format!("端口 {port} 已经被占用"),
                "换一个：/api/gateway/port/probe 会探一个空闲端口预填",
            )
            .with_action("重新探测", "probe-port", None),
        ));
    }

    // 先记下「哪些客户端现在已生效」——换端口只同步它们（未生效的不动，spec §10 故障态）。
    let values_before = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let mut effective = Vec::new();
    for id in ClientId::ALL {
        let card = clients::status(&admin.env, &admin.store, id, &values_before)
            .await
            .map_err(ApiFailure::from)?;
        if card.state == ClientState::Effective {
            effective.push(id);
        }
    }

    admin
        .set_setting(settings_keys::PORT, &port.to_string())
        .await
        .map_err(ApiFailure::from)?;
    // shortcut: 端口同时写一份 `config.json`——外壳（票 43）的 `load_port` 还读它；
    // 等那处改读库（本票顺手做了 DB 优先）后这行可以删。
    let config = admin.data_dir.join("config.json");
    let _ = std::fs::write(&config, json!({ "port": port }).to_string());

    // 用**新的**网关地址重写那些已生效客户端的地址（只动已生效的）。
    // 先把上下文里的真端口拨到新值：这一步之后网关自己也就以新地址自称了。
    admin.set_real_port(port);
    let values_after = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let mut synced = Vec::new();
    let mut failures = Vec::new();
    for id in &effective {
        match clients::apply(&admin.env, &admin.store, *id, &values_after).await {
            Ok(_) => synced.push(json!({"id": id.id(), "display": id.display()})),
            Err(e) => failures.push(json!({
                "id": id.id(),
                "display": id.display(),
                "error": e.to_string(),
            })),
        }
    }

    let restart = admin.control.restart(port);
    let restart_note = match &restart {
        Ok(()) => "网关已按新端口重启".to_string(),
        Err(e) => format!("端口已写入，但网关没能自动重启：{e}（请手动重开应用）"),
    };

    Ok(ok_json(json!({
        "port": port,
        "changed": true,
        "previousPort": old,
        "synced": synced,
        "failed": failures,
        "restart": restart_note,
        "message": format!("已换到端口 {port}，已同步 {} 个客户端{}", synced.len(), if effective.is_empty() { "（没有已生效的客户端要同步）" } else { "" }),
        "restartHint": "Desktop 与 Codex 要完全退出重开才读到新端口",
    })))
}
