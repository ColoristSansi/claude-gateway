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
    Ok(ok_json(
        json!({"started": true, "port": admin.port().await}),
    ))
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
    // 具体动作在 [`apply_port_change`] 里：外壳的 IPC 换端口走的是同一份判据。
    let after = apply_port_change(&admin.store, &admin.data_dir, &admin.env, old, port)
        .await
        .map_err(ApiFailure::from)?;
    admin.set_real_port(port);
    let synced: Vec<Value> = after
        .synced
        .iter()
        .map(|id| json!({"id": id, "display": id}))
        .collect();
    let failures: Vec<Value> = after
        .failed
        .iter()
        .map(|(id, e)| json!({"id": id, "display": id, "error": e}))
        .collect();

    let restart = admin.control.restart(port);
    let restart_note = match &restart {
        Ok(()) => "网关已按新端口重启".to_string(),
        Err(e) => format!("端口已写入，但网关没能自动重启：{e}（请手动重开应用）"),
    };

    Ok(ok_json(json!({
        "port": port,
        "changed": true,
        "previousPort": after.previous_port,
        "synced": synced,
        "failed": failures,
        "restart": restart_note,
        "message": format!("已换到端口 {port}，已同步 {} 个客户端{}", synced.len(), if synced.is_empty() { "（换之前没有已生效的客户端）" } else { "" }),
        "restartHint": "Desktop 与 Codex 要完全退出重开才读到新端口",
    })))
}

/// 一次换端口的结果（HTTP 端点与外壳 IPC 共用同一份判据）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortChangeOutcome {
    pub port: u16,
    pub previous_port: u16,
    /// 已同步地址的客户端（只含**换之前已生效**的那些）。
    pub synced: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// 写端口 + 同步已生效客户端的地址。**不静默**：调用方必须先拿到用户确认。
///
/// `settings.port` 与数据目录下的 `config.json` 一起写（外壳启动时库优先、json 兜底）。
///
/// `current_port` 由调用方给：**实际绑着的那个端口**（`port = 0` 时与 `settings.port` 不同，
/// 拿库里的旧值去比对会让「已生效客户端」全部认不出来）。
pub async fn apply_port_change(
    store: &crate::store::Store,
    data_dir: &std::path::Path,
    env: &crate::clients::ClientEnv,
    current_port: u16,
    port: u16,
) -> anyhow::Result<PortChangeOutcome> {
    use crate::ingress::api::set_setting_of;

    let previous = current_port;

    // 换之前哪几个已生效（只有它们要跟着换地址）。
    let before = values_for(store, previous).await?;
    let mut effective = Vec::new();
    for id in ClientId::ALL {
        if clients::status(env, store, id, &before).await?.state == ClientState::Effective {
            effective.push(id);
        }
    }

    set_setting_of(store, settings_keys::PORT, &port.to_string()).await?;
    // shortcut: 端口同时写一份 `config.json`——外壳（票 43）的 `load_port` 还读它；
    // 库优先接上之后，这一行只兜「库还没建起来」的极端情况。
    let _ = std::fs::write(
        data_dir.join("config.json"),
        json!({ "port": port }).to_string(),
    );

    let after = values_for(store, port).await?;
    let mut synced = Vec::new();
    let mut failed = Vec::new();
    for id in effective {
        match clients::apply(env, store, id, &after).await {
            Ok(_) => synced.push(id.id().to_string()),
            Err(e) => failed.push((id.id().to_string(), e.to_string())),
        }
    }
    Ok(PortChangeOutcome {
        port,
        previous_port: previous,
        synced,
        failed,
    })
}

/// 按给定端口算出「要写进客户端的那几个值」（换端口前后各算一次）。
async fn values_for(
    store: &crate::store::Store,
    port: u16,
) -> anyhow::Result<crate::clients::GatewayValues> {
    let table = crate::binding::load(store).await?;
    let exposed = table.exposed_for(crate::binding::Profile::Codex);
    let default_model = exposed
        .iter()
        .find(|m| m.is_family && m.binding.is_some())
        .map(|m| m.name.clone())
        .unwrap_or_else(|| crate::binding::FAMILY_SLOTS[0].to_string());
    Ok(crate::clients::GatewayValues {
        base_url: format!("http://{}:{}", crate::service::HOST, port),
        api_key: super::PLACEHOLDER_API_KEY.to_string(),
        models: crate::writer::claude_code::ModelSlots::default(),
        exposed,
        default_model,
    })
}
