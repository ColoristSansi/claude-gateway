//! 概览页与跨页机制的后端：侧栏徽标、最近错误、用量汇总、复制诊断、端到端验收。
//!
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 2、6、7、9、10 条。
//! 「最近错误」与「用量」都只读 `requests`（票 53 的落库口径），本模块**不新开写路径**。

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::binding::FAMILY_SLOTS;
use crate::clients::{ClientId, ClientState};
use crate::egress::discovery;
use crate::store::Store;

use super::{admin, ok_json, AdminCtx, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/sidebar/badges", get(badges))
        .route("/api/errors/recent", get(recent_errors))
        .route("/api/diagnostics/copy", post(diagnostics))
        .route("/api/acceptance/run", post(run_acceptance))
}

/// 侧栏两个徽标：**未通过自检的上游数** + **未生效的客户端数**（spec §10 故障态）。
/// 拉取警告与发现失败**不计入**（票 35 第 7 条）。
///
/// 客户端这一格只数**还需要动作**的两态：`未配置`（可以切过来）与 `值不匹配`（文件被动过）。
/// `已生效`当然不算，`已回官方`（用户自己还原过）也不算——那是用户的选择，不是待办
/// （票 55：三个客户端都还原之后徽标要归 0）。
async fn badges(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let tally = discovery::upstream_tally(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let mut not_effective = Vec::new();
    for id in ClientId::ALL {
        let card = crate::clients::status(&admin.env, &admin.store, id, &values)
            .await
            .map_err(ApiFailure::from)?;
        if matches!(
            card.state,
            ClientState::NotConfigured | ClientState::Mismatch
        ) {
            not_effective.push(json!({"id": card.id, "state": card.state_label}));
        }
    }
    Ok(ok_json(json!({
        "upstreamsFailedSelfcheck": tally.selfcheck_failed(),
        "upstreamsTotal": tally.total,
        "upstreamsWithPullWarning": tally.with_pull_warning,
        "clientsNotEffective": not_effective.len(),
        "clientsNotEffectiveList": not_effective,
        "ready": tally.ready(),
    })))
}

#[derive(serde::Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

/// 最近错误（概览折叠区）：**只收请求错误**，拉取失败 / 自检失败不进这里（票 35 第 7 条）。
async fn recent_errors(
    State(state): State<ApiState>,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200) as i64;
    let rows = admin
        .store
        .read(move |c| {
            let mut st = c.prepare(
                "SELECT id,ts,request_id,client,virtual_model,upstream_id,real_model,wire,status,
                        first_byte_ms,total_ms,error
                   FROM requests
                  WHERE COALESCE(status,500) >= 400 OR error IS NOT NULL
                  ORDER BY id DESC LIMIT ?1",
            )?;
            let it = st.query_map(rusqlite::params![limit], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "ts": r.get::<_, i64>(1)?,
                    "requestId": r.get::<_, Option<String>>(2)?,
                    "client": r.get::<_, Option<String>>(3)?,
                    "virtualModel": r.get::<_, Option<String>>(4)?,
                    "upstream": r.get::<_, Option<String>>(5)?,
                    "realModel": r.get::<_, Option<String>>(6)?,
                    "wire": r.get::<_, Option<String>>(7)?,
                    "status": r.get::<_, Option<i64>>(8)?,
                    "firstByteMs": r.get::<_, Option<i64>>(9)?,
                    "totalMs": r.get::<_, Option<i64>>(10)?,
                    "error": r.get::<_, Option<String>>(11)?,
                }))
            })?;
            Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .map_err(ApiFailure::from)?;
    let errors: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let status = row["status"].as_i64().unwrap_or(0);
            let raw = row["error"].as_str().unwrap_or_default().to_string();
            json!({
                "at": row["ts"],
                "requestId": row["requestId"],
                "client": row["client"],
                "virtualModel": row["virtualModel"],
                "upstream": row["upstream"],
                "realModel": row["realModel"],
                "wire": row["wire"],
                "status": status,
                "firstByteMs": row["firstByteMs"],
                "totalMs": row["totalMs"],
                "template": template_for(status, &raw),
                "raw": raw,
            })
        })
        .collect();
    Ok(ok_json(json!({ "errors": errors })))
}

/// 错误模板（spec §10 错误表达）：**发生了什么 + 可能原因 + 一个主按钮**。
/// 日志详情的第四段也用这一份（票 59：三处错误表达共用一套模板，不各写一份）。
pub(crate) fn template_for(status: i64, raw: &str) -> Value {
    let lower = raw.to_lowercase();
    let (what, cause, label, kind, page) = match status {
        401 => (
            "上游拒绝了这份凭据",
            "这家上游可能对这个接口要求另一种密钥（同一个上游的不同 wire 面凭据可以不同）",
            "去上游页检查凭据",
            "goto",
            Some("upstreams"),
        ),
        403 => (
            "上游认出了凭据，但没给这个模型 / 计划的权限",
            "计划里不含这个模型，或这份目录来自未授权的全量列表",
            "去模型与映射换个模型",
            "goto",
            Some("models"),
        ),
        404 => (
            "上游说这个接口或模型不存在",
            "baseUrl 与 wire 的路径模板对不上（比如 baseUrl 已经带了 /v1），或模型 ID 打错了",
            "去上游页检查地址与模型",
            "goto",
            Some("upstreams"),
        ),
        429 => (
            "上游限流了",
            "短时间内请求太多；等一会儿再试，或换个上游",
            "重试",
            "retry",
            None,
        ),
        502..=504 => (
            "连不上上游或上游没及时回",
            "断网 / 上游在维护 / 上游太慢（超时）",
            "重试",
            "retry",
            None,
        ),
        _ if lower.contains("context") && lower.contains("length") => (
            "上下文超过上游窗口",
            "这次请求太大；换个窗口更大的模型或缩短历史",
            "去模型与映射换模型",
            "goto",
            Some("models"),
        ),
        _ => (
            "这次请求失败了",
            "看下面可展开的原始错误；把「复制诊断」贴给开发者更快",
            "重试",
            "retry",
            None,
        ),
    };
    json!({
        "what": what,
        "likelyCause": cause,
        "action": {"label": label, "kind": kind, "page": page},
    })
}

/// 「复制诊断」：脱敏后的诊断文本（**不含密钥明文，也不含请求 / 响应 body**，spec §10 错误表达）。
async fn diagnostics(
    State(state): State<ApiState>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let text = diagnostics_text(&admin, &body, state.running()).await?;
    // 逐条自查：诊断文本里不该出现任何一份凭据的明文（拿库里的密文解出来比）。
    let leaked = credential_leak_check(&admin.store, &text).await;
    if let Some(leak) = leaked {
        return Err(ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "诊断文本里出现了密钥明文，已拦下不返回",
                "这是网关的 bug：复制诊断必须脱敏",
            )
            .with_raw(leak),
        ));
    }
    Ok(ok_json(json!({ "text": text })))
}

/// 诊断文本边界（spec §10）：版本 / 端口 / 运行状态 / 上游配置脱敏形式 / 错误原文 / 请求元数据。
async fn diagnostics_text(
    admin: &AdminCtx,
    body: &Value,
    running: bool,
) -> Result<String, ApiFailure> {
    let status = json!({
        "version": admin.version,
        "dbPath": admin.db_path,
        "dataDir": admin.data_dir.to_string_lossy(),
        "port": admin.port().await,
        "running": running,
    });
    let ups = crate::egress::provider::list_upstreams(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    let mut upstreams = Vec::new();
    for up in &ups {
        let mut creds = Vec::new();
        for wire in ["default", "anthropic", "openai-chat", "openai-responses"] {
            if let Some(mask) = admin
                .store
                .credential_mask(&up.id, wire)
                .await
                .map_err(ApiFailure::from)?
            {
                creds.push(json!({"wire": wire, "key": mask}));
            }
        }
        upstreams.push(json!({
            "id": up.id,
            "baseUrl": up.base_url,
            "paths": up.path_templates.iter().map(|(w, p)| (w.as_str().to_string(), p.clone())).collect::<BTreeMap<_, _>>(),
            "auth": up.auth.iter().map(|(k, v)| (k.clone(), v.as_str())).collect::<BTreeMap<_, _>>(),
            "quirks": up.quirks.iter().map(|q| q.name()).collect::<Vec<_>>(),
            "credentials": creds,
            "lastSelfcheck": up.last_selfcheck.map(|s| json!({"at": s.at, "ok": s.ok})),
            "lastPull": up.last_pull.as_ref().map(|p| json!({
                "at": p.at, "okAt": p.ok_at, "status": p.status.as_str(), "warning": p.warning,
            })),
        }));
    }
    let request = match body.get("requestId").and_then(Value::as_str) {
        Some(id) => request_meta(admin, id).await?,
        None => Value::Null,
    };
    let doc = json!({
        "说明": "诊断包（脱敏）：不含密钥明文，也不含请求 / 响应 body",
        "生成时间": crate::ingress::api::rfc3339_utc(std::time::SystemTime::now()),
        "应用": status,
        "上游配置": upstreams,
        "错误原文": body.get("error").cloned().unwrap_or(Value::Null),
        "请求元数据": request,
    });
    Ok(serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string()))
}

/// 一条请求的元数据（不读 body：body 只在 body 开关开着时才有，且不进诊断包）。
async fn request_meta(admin: &AdminCtx, request_id: &str) -> Result<Value, ApiFailure> {
    let request_id = request_id.to_string();
    let row = admin
        .store
        .read(move |c| {
            use rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT id,ts,request_id,client,virtual_model,upstream_id,real_model,wire,status,
                        first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,error
                   FROM requests WHERE request_id=?1 ORDER BY id DESC LIMIT 1",
                rusqlite::params![request_id],
                |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "ts": r.get::<_, i64>(1)?,
                        "requestId": r.get::<_, Option<String>>(2)?,
                        "client": r.get::<_, Option<String>>(3)?,
                        "virtualModel": r.get::<_, Option<String>>(4)?,
                        "upstream": r.get::<_, Option<String>>(5)?,
                        "realModel": r.get::<_, Option<String>>(6)?,
                        "wire": r.get::<_, Option<String>>(7)?,
                        "status": r.get::<_, Option<i64>>(8)?,
                        "firstByteMs": r.get::<_, Option<i64>>(9)?,
                        "totalMs": r.get::<_, Option<i64>>(10)?,
                        "inputTokens": r.get::<_, Option<i64>>(11)?,
                        "outputTokens": r.get::<_, Option<i64>>(12)?,
                        "cacheRead": r.get::<_, Option<i64>>(13)?,
                        "error": r.get::<_, Option<String>>(14)?,
                    }))
                },
            )
            .optional()?)
        })
        .await
        .map_err(ApiFailure::from)?;
    Ok(row.unwrap_or(Value::Null))
}

/// 诊断文本里出现任何一份凭据明文就报出来（自查；只有诊断端点用它）。
async fn credential_leak_check(store: &Store, text: &str) -> Option<String> {
    let ids = store
        .read(|c| {
            let mut st = c.prepare("SELECT DISTINCT upstream_id,wire FROM credentials")?;
            let rows =
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .ok()?;
    for (id, wire) in ids {
        if let Ok(Some(secret)) = store.credential_secret(&id, &wire).await {
            if !secret.is_empty() && text.contains(&secret) {
                return Some(format!("{id}/{wire}"));
            }
        }
    }
    None
}

/// 端到端验收（三步全绿后的那颗按钮）：用 **haiku 槽位**发一条非流式最短请求。
///
/// 走**真实入站**（`POST /v1/messages` 打自己）：这条路径才有转换、绑定解析、请求日志与成本，
/// 结果卡要的上游 / 状态码 / 耗时 / 首字延迟都从日志里读回（spec §10 第 1 节）。
async fn run_acceptance(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let slot = FAMILY_SLOTS[3]; // claude-haiku
    let values = admin.gateway_values().await.map_err(ApiFailure::from)?;
    let haiku_bound = values
        .exposed
        .iter()
        .any(|m| m.name == slot && m.binding.is_some());
    if !haiku_bound {
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            ApiError::new(
                StatusCode::CONFLICT,
                format!("{slot} 还没绑定，没法跑验收"),
                "三步清单第②步没绿：先去「模型与映射」把这个槽位绑上",
            )
            .with_action("去模型与映射", "goto", Some("models")),
        ));
    }
    let port = admin.port().await;
    let url = format!("http://{}:{port}/v1/messages", crate::service::HOST);
    let body = json!({
        "model": slot,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "."}],
    });
    let started = std::time::Instant::now();
    let sent = admin.http.post(&url).json(&body).send().await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let (ok, status, raw) = match sent {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            (status == 200, status, text)
        }
        Err(e) => (false, 0, format!("连不上网关自己：{e}")),
    };
    // 请求日志里读回上游 / 首字延迟（网关在响应收尾时才落库，这里等一拍）。
    let mut meta = Value::Null;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Some(row) = latest_request(&admin.store).await {
            if row["virtualModel"].as_str() == Some(slot) {
                meta = row;
                break;
            }
        }
    }
    let (what, cause, label, kind) = if ok {
        (String::new(), String::new(), String::new(), String::new())
    } else {
        let t = template_for(status as i64, &raw);
        (
            t["what"].as_str().unwrap_or("验收没通过").to_string(),
            t["likelyCause"].as_str().unwrap_or("").to_string(),
            t["action"]["label"].as_str().unwrap_or("重试").to_string(),
            t["action"]["kind"].as_str().unwrap_or("retry").to_string(),
        )
    };
    Ok(ok_json(json!({
        "ok": ok,
        "slot": slot,
        "upstream": meta["upstream"],
        "realModel": meta["realModel"],
        "wire": meta["wire"],
        "status": if status == 0 { meta["status"].clone() } else { json!(status) },
        "elapsedMs": elapsed_ms,
        "firstByteMs": meta["firstByteMs"],
        "totalMs": meta["totalMs"],
        "requestId": meta["requestId"],
        "logId": meta["id"],
        "raw": if raw.chars().count() > 2000 { raw.chars().take(2000).collect::<String>() } else { raw },
        "template": if ok { Value::Null } else { json!({
            "what": what, "likelyCause": cause,
            "action": {"label": label, "kind": kind, "page": "logs"},
        })},
    })))
}

async fn latest_request(store: &Store) -> Option<Value> {
    store
        .read(|c| {
            use rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT id,ts,request_id,virtual_model,upstream_id,real_model,wire,status,
                        first_byte_ms,total_ms,error
                   FROM requests ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "ts": r.get::<_, i64>(1)?,
                        "requestId": r.get::<_, Option<String>>(2)?,
                        "virtualModel": r.get::<_, Option<String>>(3)?,
                        "upstream": r.get::<_, Option<String>>(4)?,
                        "realModel": r.get::<_, Option<String>>(5)?,
                        "wire": r.get::<_, Option<String>>(6)?,
                        "status": r.get::<_, Option<i64>>(7)?,
                        "firstByteMs": r.get::<_, Option<i64>>(8)?,
                        "totalMs": r.get::<_, Option<i64>>(9)?,
                        "error": r.get::<_, Option<String>>(10)?,
                    }))
                },
            )
            .optional()?)
        })
        .await
        .ok()
        .flatten()
}
