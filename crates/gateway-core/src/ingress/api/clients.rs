//! `/api/clients*`（客户端页）：三张卡的四态实读、一键切换 / 一键还原 / 1P↔3P、
//! 一键切换全部可用客户端、备份清单。
//!
//! 实读与写入都在 [`crate::clients`]（本票的装配层，底下是票 45–48 的写入引擎）。
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 27–33 条。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::clients::{self, ClientId, ClientState};

use super::{admin, ok_json, parse_client, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/clients", get(list_clients))
        .route("/api/clients/switch-all", post(switch_all))
        .route("/api/clients/desktop/mode", post(desktop_mode))
        .route("/api/clients/{id}/apply", post(apply))
        .route("/api/clients/{id}/restore", post(restore))
        .route("/api/clients/{id}/keep-mine", post(keep_mine))
        .route("/api/clients/{id}/claim-overwrite", post(claim_overwrite))
        .route("/api/clients/{id}/backups", get(backups))
}

#[derive(serde::Deserialize)]
struct ApplyBody {
    /// 「认领并覆盖」：只有 Codex 的目录文件被外部改过时才用得上（二次确认之后）。
    #[serde(default)]
    claim: bool,
}

/// 三张卡：**每次进页面实读配置文件比对**（不缓存），四态徽章 + 绝对路径 + 备份路径。
async fn list_clients(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let mut cards = Vec::new();
    for id in ClientId::ALL {
        let status = clients::status(&admin.env, &admin.store, id, &values)
            .await
            .map_err(ApiFailure::from)?;
        cards.push(status);
    }
    Ok(ok_json(json!({
        "clients": cards,
        "gateway": {
            "baseUrl": admin.base_url().await,
            "port": admin.port().await,
            "apiKeyMask": "（占位 key，网关不校验入站凭据）",
        },
        "warnings": [
            "Desktop 的 inferenceGatewayBaseUrl 不带 /v1（填了会变成 /v1/v1/messages）",
            "三个客户端改完都要完全退出重开才生效",
        ],
    })))
}

/// 一键切换 / 一键还原 / 认领覆盖（`id` = 客户端 id）。
async fn apply(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    body: Option<Json<ApplyBody>>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let client = parse_client(&id)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let claim = body.map(|Json(b)| b.claim).unwrap_or(false);
    let report = if claim {
        clients::apply_claim(&admin.env, &admin.store, client, &values)
            .await
            .map_err(|e| action_error(client, "认领并覆盖", e))?
    } else {
        clients::apply(&admin.env, &admin.store, client, &values)
            .await
            .map_err(|e| action_error(client, "切换到网关", e))?
    };
    let card = clients::status(&admin.env, &admin.store, client, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "report": report,
        "client": card,
        "restartHint": restart_hint(client),
    })))
}

async fn restore(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let client = parse_client(&id)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let report = clients::restore(&admin.env, &admin.store, client, &values)
        .await
        .map_err(|e| action_error(client, "还原到安装时", e))?;
    let card = clients::status(&admin.env, &admin.store, client, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "report": report,
        "client": card,
        "restartHint": restart_hint(client),
    })))
}

/// 「值不匹配」→ 保留我的修改：把网关记录更新成磁盘现值（不写文件）。
///
/// 语义：用户说「我知道文件被改过，别拿网关的覆盖回去」。办法是**把库里的记录对齐磁盘**，
/// 于是下一次「一键切换」不再判冲突、按首次接管重新备份后写。
async fn keep_mine(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let client = parse_client(&id)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let specs = clients::specs(&admin.env, &admin.store, client, &values)
        .map_err(ApiFailure::from)?;
    let writer = admin.env.writer(&admin.store, client);
    let mut kept = Vec::new();
    for spec in &specs {
        let inspect = writer.inspect(spec).await;
        if let Some(err) = &inspect.error {
            return Err(ApiFailure::new(
                StatusCode::CONFLICT,
                ApiError::new(
                    StatusCode::CONFLICT,
                    format!("{} 解析不了，没法保留你的修改", inspect.path.display()),
                    "文件不是合法 JSON / TOML：先手动改回能解析的形状",
                )
                .with_raw(err),
            ));
        }
        writer
            .remember(spec, "write")
            .await
            .map_err(ApiFailure::from)?;
        kept.push(inspect.path.to_string_lossy().to_string());
    }
    let card = clients::status(&admin.env, &admin.store, client, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "kept": kept,
        "message": "已按磁盘现值更新记录：下次「一键切换」不会再判冲突，会先备份再覆盖接管键",
        "client": card,
    })))
}

/// 冲突弹窗的「认领并覆盖」（只有 Codex 的模型目录有这条）。
async fn claim_overwrite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let client = parse_client(&id)?;
    if client != ClientId::Codex {
        return Err(ApiFailure::new(
            StatusCode::BAD_REQUEST,
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "「认领并覆盖」只对 Codex 的模型目录有定义",
                "别的客户端没有「目录文件被外部改过」这一态",
            ),
        ));
    }
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let report = clients::apply_claim(&admin.env, &admin.store, client, &values)
        .await
        .map_err(|e| action_error(client, "认领并覆盖", e))?;
    let card = clients::status(&admin.env, &admin.store, client, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({"report": report, "client": card})))
}

/// Desktop 的 1P↔3P 快切（托盘那一项复用同一个动作）。
async fn desktop_mode(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or("");
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let report = clients::desktop_mode(&admin.env, &admin.store, &values, mode)
        .await
        .map_err(|e| {
            ApiFailure::new(
                StatusCode::BAD_REQUEST,
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("Desktop 模式快切失败：{e}"),
                    "mode 只有 1p / 3p",
                ),
            )
        })?;
    let card = clients::status(&admin.env, &admin.store, ClientId::Desktop, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({"report": report, "client": card})))
}

/// 一键把可用客户端都切到网关：**已生效的跳过**（票 17 第 28 条）。
async fn switch_all(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let mut results = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    for id in ClientId::ALL {
        let before = clients::status(&admin.env, &admin.store, id, &values)
            .await
            .map_err(ApiFailure::from)?;
        if before.state == ClientState::Effective {
            skipped.push(id.id().to_string());
            results.push(json!({
                "client": id.id(), "display": id.display(), "skipped": true,
                "message": "已经是「已生效」，跳过",
            }));
            continue;
        }
        match clients::apply(&admin.env, &admin.store, id, &values).await {
            Ok(report) => results.push(json!({
                "client": id.id(), "display": id.display(), "skipped": false,
                "message": report.message, "report": report,
                "restartHint": restart_hint(id),
            })),
            Err(e) => {
                failed.push(id.id().to_string());
                results.push(json!({
                    "client": id.id(), "display": id.display(), "skipped": false,
                    "message": format!("{e}"),
                    "error": ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("切 {} 没能完成", id.display()),
                        "看原始错误；被客户端占用时先完全退出它",
                    ).with_raw(&e),
                }));
            }
        }
    }
    Ok(ok_json(json!({
        "results": results,
        "skipped": skipped,
        "failed": failed,
        "message": format!("已同步 {} 个客户端，跳过 {} 个（已生效）", results.iter().filter(|r| r["skipped"] == json!(false) && r.get("error").is_none()).count(), skipped.len()),
    })))
}

/// 二次确认弹窗要列的历史备份（绝对路径）。
async fn backups(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let client = parse_client(&id)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let files = clients::backups(&admin.env, &admin.store, client, &values)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "client": client.id(),
        "backups": files,
        "backupRoot": admin.env.backup_root.to_string_lossy().to_string(),
        "restoreHint": "还原默认摘键（回到官方态）；「整份回滚」是显式的第二选项，被外部改过会被拒绝",
    })))
}

fn restart_hint(id: ClientId) -> &'static str {
    match id {
        ClientId::Desktop => "配置改完必须**完全退出并重开** Claude Desktop 才生效（不做热重载）",
        ClientId::ClaudeCode => "重开一个终端 / 会话即可；正在跑的会话读的是旧环境变量",
        ClientId::Codex => "Codex CLI 重开会话、桌面应用完全退出重开；模型目录只在启动时读",
    }
}

fn action_error(id: ClientId, what: &str, e: anyhow::Error) -> ApiFailure {
    ApiFailure::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{}「{what}」没能完成", id.display()),
            "文件被客户端占用 / 权限不足；先完全退出该客户端再试",
        )
        .with_action("重试", "retry", None)
        .with_raw(e),
    )
}
