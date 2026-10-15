//! `/api/upstreams*` + `/api/providers`（上游页：列表六列 + 四步向导的后端）。
//!
//! 业务能力全在 [`crate::egress::provider`]（票 49）与 [`crate::egress::discovery`]（票 50）
//! 里，这里只做「HTTP ↔ 领域类型」的翻译与**密钥绝不出界**（响应里只有掩码）。
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 11–21 条。

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use axum::{extract::DefaultBodyLimit, Router};
use serde_json::{json, Value};

use crate::binding;
use crate::egress::discovery;
use crate::egress::provider::{self, AuthScheme, Quirk, Upstream, Wire, WireChoice};
use crate::store::Store;

use super::{admin, ok_json, unix_now, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/providers", get(list_providers))
        .route("/api/upstreams", get(list_upstreams).post(create_upstream))
        .route("/api/upstreams/{id}", put(update_upstream).delete(remove_upstream))
        .route("/api/upstreams/{id}/references", get(references))
        .route(
            "/api/upstreams/{id}/credentials",
            put(set_credential).delete(delete_credential),
        )
        .route("/api/upstreams/{id}/discovery", post(run_discovery))
        .route(
            "/api/upstreams/{id}/models",
            get(list_models).post(add_manual_model),
        )
        .route("/api/upstreams/{id}/selftest", post(selfcheck))
        .route(
            "/api/upstreams/{id}/models/{model}/prices",
            put(set_prices),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
}

/// 声明式供应商表（向导第①步的预填；**可跳过**，直接手填 baseUrl 也行）。
async fn list_providers(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let presets = provider::provider_presets();
    // models.dev 快照里的 provider 一并给：预填表只补了「models.dev 给不出 baseUrl」的那几家。
    let snapshot = admin
        .store
        .read(|c| {
            let mut st = c.prepare(
                "SELECT provider, COUNT(*) FROM model_catalog GROUP BY provider ORDER BY provider",
            )?;
            let rows = st.query_map([], |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "modelCount": r.get::<_, i64>(1)?}))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .map_err(|e| ApiFailure::from(e))?;
    Ok(ok_json(json!({
        "presets": presets,
        "snapshot": snapshot,
        "wires": Wire::ALL.iter().map(|w| w.as_str()).collect::<Vec<_>>(),
        "quirks": Quirk::all().iter().map(Quirk::name).collect::<Vec<_>>(),
    })))
}

/// 上游列表六列 + 被引用数 + 凭据掩码。
async fn list_upstreams(State(state): State<ApiState>) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let ups = provider::list_upstreams(&admin.store)
        .await
        .map_err(ApiFailure::from)?;
    let mut out = Vec::with_capacity(ups.len());
    for up in &ups {
        out.push(view(&admin.store, up).await?);
    }
    Ok(ok_json(json!({ "upstreams": out })))
}

async fn view(store: &Store, up: &Upstream) -> Result<Value, ApiFailure> {
    let refs = provider::upstream_references(store, &up.id)
        .await
        .map_err(ApiFailure::from)?;
    let models = provider::list_models(store, &up.id)
        .await
        .map_err(ApiFailure::from)?;
    let mut credentials = Vec::new();
    for wire in std::iter::once("default".to_string())
        .chain(Wire::ALL.iter().map(|w| w.as_str().to_string()))
    {
        let mask = store
            .credential_mask(&up.id, &wire)
            .await
            .map_err(ApiFailure::from)?;
        if let Some(mask) = mask {
            credentials.push(json!({ "wire": wire, "mask": mask }));
        }
    }
    Ok(json!({
        "id": up.id,
        "name": up.name,
        "baseUrl": up.base_url,
        "protocols": up.path_templates.keys().map(|w| w.as_str()).collect::<Vec<_>>(),
        "paths": up.path_templates.iter().map(|(w, p)| (w.as_str().to_string(), p.clone())).collect::<BTreeMap<_, _>>(),
        "auth": up.auth.iter().map(|(k, v)| (k.clone(), v.as_str())).collect::<BTreeMap<_, _>>(),
        "quirks": up.quirks.iter().map(Quirk::name).collect::<Vec<_>>(),
        "headerPassthrough": up.header_passthrough,
        "lastSelfcheck": up.last_selfcheck.map(|s| json!({"at": s.at, "ok": s.ok})),
        "lastPull": up.last_pull.as_ref().map(|p| json!({
            "at": p.at, "okAt": p.ok_at, "status": p.status.as_str(), "warning": p.warning,
        })),
        "references": refs.len(),
        "referenceList": refs.iter().map(|r| r.label()).collect::<Vec<_>>(),
        "credentials": credentials,
        "modelCount": models.len(),
        "modelsUrl": up.models_url(),
    }))
}

/// 建上游：`presetId` 给了就用预填表当起点；`credentials` 里的 key **只进库、不回显**。
async fn create_upstream(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiFailure> {
    let admin = admin(&state)?;
    let up = upstream_from_body(&body, None)?;
    if up.id.trim().is_empty() {
        return Err(bad_request("上游 id 不能为空", "向导第②步应当带一个 id（可用 baseUrl 生成）"));
    }
    if provider::load_upstream(&admin.store, &up.id)
        .await
        .map_err(ApiFailure::from)?
        .is_some()
    {
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            ApiError::new(
                StatusCode::CONFLICT,
                format!("上游 {} 已经存在", up.id),
                "换个 id，或去列表里编辑那一条",
            ),
        ));
    }
    provider::save_upstream(&admin.store, &up)
        .await
        .map_err(ApiFailure::from)?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    let saved = save_credentials(&admin.store, &up.id, &body).await?;
    let view = view(&admin.store, &up).await?;
    Ok((StatusCode::CREATED, ok_json(json!({"upstream": view, "credentialsSaved": saved}))))
}

/// 改上游（声明式字段；凭据走 `PUT /credentials`）。
async fn update_upstream(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let existing = provider::load_upstream(&admin.store, &id)
        .await
        .map_err(ApiFailure::from)?
        .ok_or_else(|| not_found(&id))?;
    let mut up = upstream_from_body(&body, Some(existing))?;
    up.id = id.clone();
    provider::save_upstream(&admin.store, &up)
        .await
        .map_err(ApiFailure::from)?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    let saved = save_credentials(&admin.store, &id, &body).await?;
    let view = view(&admin.store, &up).await?;
    Ok(ok_json(json!({"upstream": view, "credentialsSaved": saved})))
}

/// 删上游：仍被引用时 409 + 引用处；`?force=true`（用户在弹窗里确认过）才连绑定一起删。
async fn remove_upstream(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(q): Query<ForceQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    if q.force.unwrap_or(false) {
        let removed = provider::delete_upstream_forced(&admin.store, &id)
            .await
            .map_err(ApiFailure::from)?;
        admin.reload_forwarding().await.map_err(ApiFailure::from)?;
        return Ok(ok_json(json!({"deleted": true, "bindingsRemoved": removed})));
    }
    match provider::delete_upstream(&admin.store, &id)
        .await
        .map_err(ApiFailure::from)?
    {
        provider::DeleteOutcome::Deleted => Ok(ok_json(json!({"deleted": true, "bindingsRemoved": 0}))),
        // 409 那条带引用处：前端弹窗列出引用处（票 17 第 39 条）。
        provider::DeleteOutcome::Referenced(refs) => Err(ApiFailure::new(
            StatusCode::CONFLICT,
            ApiError::new(
                StatusCode::CONFLICT,
                format!("上游 {id} 还被 {} 处引用，没有删", refs.len()),
                "先在「模型与映射」里改绑那些槽位 / 虚拟模型，或确认「一并删除那些绑定」",
            )
            .with_action("去模型与映射改绑", "goto", Some("models")),
        )),
    }
}

async fn references(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let refs = provider::upstream_references(&admin.store, &id)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "references": refs.iter().map(|r| json!({
            "client": r.client_profile_id, "model": r.name, "label": r.label(),
        })).collect::<Vec<_>>(),
    })))
}

/// 写一份凭据（按「上游 × wire」）。**明文只进库（DPAPI 密文），回执里没有它。**
async fn set_credential(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let wire = body
        .get("wire")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let key = body
        .get("key")
        .or_else(|| body.get("credential"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if key.trim().is_empty() {
        return Err(bad_request("key 不能为空", "向导第②步的 key 输入框是空的"));
    }
    if !credential_wire_ok(&wire) {
        return Err(bad_request(
            format!("未知的 wire：{wire}"),
            "凭据要么挂 default，要么挂 anthropic / openai-chat / openai-responses",
        ));
    }
    store_credential(&admin.store, &id, &wire, key).await?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    let mask = admin
        .store
        .credential_mask(&id, &wire)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({"saved": true, "wire": wire, "mask": mask})))
}

async fn delete_credential(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Query(q): Query<WireQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    admin
        .store
        .delete_credential(&id, q.wire.as_deref().unwrap_or("default"))
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({"deleted": true})))
}

/// 拉模型：三种结局 + 两道凭据门，全部由票 50 的 `run_pull` 判，界面只渲染回执。
async fn run_discovery(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let mut up = provider::load_upstream(&admin.store, &id)
        .await
        .map_err(ApiFailure::from)?
        .ok_or_else(|| not_found(&id))?;
    // 界面下拉选的那份凭据：把清单查询绑到某个 wire 上（不改库，只这一次拉取）。
    if let Some(wire) = body.get("wire").and_then(Value::as_str) {
        if !wire.trim().is_empty() {
            up.set_auth(wire, up.auth_default());
        }
    }
    let report = discovery::run_pull(&admin.store, &admin.http, &up, unix_now())
        .await
        .map_err(|e| {
            ApiFailure::new(
                StatusCode::BAD_GATEWAY,
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "拉模型没能完成",
                    "凭据解不开（换机器 / 换用户后库里密文失效）或上游连不上",
                )
                .with_action("重试", "retry", None)
                .with_raw(e),
            )
        })?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    let applied = json!({
        "added": report.applied.added,
        "updated": report.applied.updated,
        "removed": report.applied.removed,
        "keptManual": report.applied.kept_manual,
        "undeclared": report.applied.undeclared,
    });
    Ok(ok_json(json!({
        "disposition": match report.disposition {
            discovery::PullDisposition::Fetched => "fetched",
            discovery::PullDisposition::NoDiscovery => "no-discovery",
            discovery::PullDisposition::Failed => "failed",
            discovery::PullDisposition::NoCredential => "no-credential",
            discovery::PullDisposition::CredentialChoice => "credential-choice",
        },
        "message": report.message,
        "credential": report.credential.note(),
        "applied": applied,
        "actions": report.actions.iter().map(|a| json!({"action": format!("{a:?}"), "label": a.label()})).collect::<Vec<_>>(),
        "reuseAt": report.reuse_at,
        "warnings": report.warnings,
        "chooseFrom": report.choose_from,
        "models": models_view(&admin.store, &id).await?,
    })))
}

/// 目录条目：`model_id` + 声明过的 wire + 来源 + 手工四项价与逐项来源 + 未声明标注。
async fn list_models(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    Ok(ok_json(json!({ "models": models_view(&admin.store, &id).await? })))
}

async fn models_view(store: &Store, id: &str) -> Result<Vec<Value>, ApiFailure> {
    let models = provider::list_models(store, id)
        .await
        .map_err(ApiFailure::from)?;
    let mut out = Vec::with_capacity(models.len());
    for m in models {
        let prices = provider::resolved_prices(store, id, &m.model_id)
            .await
            .map_err(ApiFailure::from)?;
        let price = |r: provider::ResolvedPrice| json!({"usdPerMtok": r.usd_per_mtok, "source": r.source.as_str()});
        out.push(json!({
            "modelId": m.model_id,
            "supportedWires": m.supported_wires.iter().map(|w| w.as_str()).collect::<Vec<_>>(),
            "source": m.source.as_str(),
            "note": match m.validate_wire(Wire::Anthropic) {
                Ok(WireChoice::Undeclared) => Some(provider::UNDECLARED_NOTE),
                _ => None,
            },
            "prices": {
                "input": price(prices.input),
                "output": price(prices.output),
                "cacheRead": price(prices.cache_read),
                "cacheWrite": price(prices.cache_write),
            },
        }));
    }
    Ok(out)
}

/// 手工新增一条目录条目（`manual` 锁定，重拉不覆盖）+ 可选四项价。
async fn add_manual_model(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let model_id = body
        .get("modelId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if model_id.is_empty() {
        return Err(bad_request("模型 ID 不能为空", "手填模型 ID 的输入框是空的"));
    }
    let wires = parse_wires(&body)?;
    provider::save_manual_model(&admin.store, &id, &model_id, &wires)
        .await
        .map_err(ApiFailure::from)?;
    if let Some(p) = body.get("prices") {
        provider::set_manual_prices(&admin.store, &id, &model_id, &parse_prices(p)?)
            .await
            .map_err(ApiFailure::from)?;
    }
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "saved": true,
        "models": models_view(&admin.store, &id).await?,
    })))
}

/// 手工四项价（可填可留空；填了就锁定，留空回退快照）。
async fn set_prices(
    State(state): State<ApiState>,
    Path((id, model)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let prices = parse_prices(&body)?;
    provider::set_manual_prices(&admin.store, &id, &model, &prices)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "saved": true,
        "models": models_view(&admin.store, &id).await?,
    })))
}

/// 强制自检（向导第④步）：发一条最短真实请求过认证与路径，**只看状态码**，结果落库。
async fn selfcheck(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let up = provider::load_upstream(&admin.store, &id)
        .await
        .map_err(ApiFailure::from)?
        .ok_or_else(|| not_found(&id))?;
    let (wire, real_model) = pick_probe_target(&admin.store, &up, &body).await?;
    let result = binding::selfcheck(
        &admin.http,
        &admin.store,
        &up,
        wire,
        &real_model,
        binding::PROBE_DEADLINE,
    )
    .await;
    Ok(ok_json(json!({
        "ok": result.ok,
        "status": result.status,
        "elapsedMs": result.elapsed_ms as u64,
        "detail": result.detail,
        "upstream": result.upstream_id,
        "realModel": result.real_model,
        "wire": result.wire.as_str(),
        "selfcheckAt": unix_now(),
    })))
}

/// 探针目标：body 指定 > 目录里第一条已声明 wire 的模型 > 默认 anthropic。
async fn pick_probe_target(
    store: &Store,
    up: &Upstream,
    body: &Value,
) -> Result<(Wire, String), ApiFailure> {
    if let (Some(w), Some(m)) = (
        body.get("wire").and_then(Value::as_str).and_then(Wire::parse),
        body.get("model").and_then(Value::as_str),
    ) {
        if !m.trim().is_empty() {
            return Ok((w, m.to_string()));
        }
    }
    let models = provider::list_models(store, &up.id)
        .await
        .map_err(ApiFailure::from)?;
    for m in &models {
        if let Some(w) = m.supported_wires.iter().next() {
            return Ok((*w, m.model_id.clone()));
        }
    }
    if let Some(m) = models.first() {
        let wire = Wire::ALL
            .iter()
            .copied()
            .find(|w| up.path(*w).is_some())
            .unwrap_or(Wire::Anthropic);
        return Ok((wire, m.model_id.clone()));
    }
    Err(ApiFailure::new(
        StatusCode::BAD_REQUEST,
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "上游模型目录是空的，自检没有目标",
            "先在向导第③步拉一次模型，或手填一条模型 ID",
        )
        .with_action("手工添加模型", "goto", Some("upstreams")),
    ))
}

// ───────────────────────── 小工具 ─────────────────────────

#[derive(serde::Deserialize)]
struct ForceQuery {
    force: Option<bool>,
}

#[derive(serde::Deserialize)]
struct WireQuery {
    wire: Option<String>,
}

fn not_found(id: &str) -> ApiFailure {
    ApiFailure::new(
        StatusCode::NOT_FOUND,
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("没有这个上游：{id}"),
            "它可能已经被删了；刷新列表看看",
        ),
    )
}

fn bad_request(what: impl Into<String>, cause: impl Into<String>) -> ApiFailure {
    ApiFailure::new(
        StatusCode::BAD_REQUEST,
        ApiError::new(StatusCode::BAD_REQUEST, what, cause),
    )
}

fn credential_wire_ok(wire: &str) -> bool {
    wire == "default" || Wire::parse(wire).is_some()
}

async fn store_credential(
    store: &Store,
    upstream_id: &str,
    wire: &str,
    key: &str,
) -> Result<(), ApiFailure> {
    store
        .put_credential(upstream_id, wire, key.trim())
        .await
        .map_err(ApiFailure::from)
}

/// `credentials: [{"wire":"default","key":"..."}]`（向导第②步的第二个 key 输入框也走它）。
async fn save_credentials(
    store: &Store,
    upstream_id: &str,
    body: &Value,
) -> Result<Vec<String>, ApiFailure> {
    let mut saved = Vec::new();
    if let Some(list) = body.get("credentials").and_then(Value::as_array) {
        for item in list {
            let wire = item.get("wire").and_then(Value::as_str).unwrap_or("default");
            let key = item.get("key").and_then(Value::as_str).unwrap_or_default();
            if key.trim().is_empty() {
                continue;
            }
            if !credential_wire_ok(wire) {
                return Err(bad_request(
                    format!("未知的 wire：{wire}"),
                    "凭据要么挂 default，要么挂 anthropic / openai-chat / openai-responses",
                ));
            }
            store_credential(store, upstream_id, wire, key).await?;
            saved.push(wire.to_string());
        }
    }
    Ok(saved)
}

/// 请求体 → 上游表。`base` 给了就是编辑（未提到的字段保持原值），否则从预填表或空表起步。
fn upstream_from_body(body: &Value, base: Option<Upstream>) -> Result<Upstream, ApiFailure> {
    let preset = body
        .get("presetId")
        .and_then(Value::as_str)
        .and_then(|id| {
            provider::provider_presets()
                .into_iter()
                .find(|p| p.id == id)
        });
    let mut up = match (base, preset) {
        (Some(mut up), _) => {
            up.last_pull = None;
            up.last_selfcheck = None;
            up
        }
        (None, Some(p)) => provider::preset_to_upstream(&p),
        (None, None) => Upstream::new(
            body.get("id").and_then(Value::as_str).unwrap_or_default(),
            body.get("name").and_then(Value::as_str).unwrap_or_default(),
            body.get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
    };
    if let Some(v) = body.get("id").and_then(Value::as_str) {
        if !v.trim().is_empty() {
            up.id = v.trim().to_string();
        }
    }
    if let Some(v) = body.get("name").and_then(Value::as_str) {
        if !v.trim().is_empty() {
            up.name = v.trim().to_string();
        }
    }
    if let Some(v) = body.get("baseUrl").and_then(Value::as_str) {
        if !v.trim().is_empty() {
            up.base_url = v.trim().trim_end_matches('/').to_string();
        }
    }
    if let Some(map) = body.get("paths").and_then(Value::as_object) {
        for (wire, path) in map {
            if let (Some(w), Some(p)) = (Wire::parse(wire), path.as_str()) {
                up.set_path(w, p.to_string());
            }
        }
    }
    if let Some(map) = body.get("auth").and_then(Value::as_object) {
        for (wire, scheme) in map {
            if let Some(s) = scheme.as_str() {
                up.set_auth(wire, AuthScheme::parse(s));
            }
        }
    }
    if let Some(list) = body.get("quirks").and_then(Value::as_array) {
        up.quirks = list
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|q| {
                let (name, arg) = q.split_once(':').unwrap_or((q, ""));
                Quirk::parse(name, arg)
            })
            .collect();
    }
    if let Some(list) = body.get("headerPassthrough").and_then(Value::as_array) {
        up.header_passthrough = list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    if up.base_url.trim().is_empty() {
        return Err(bad_request(
            "baseUrl 不能为空",
            "向导第②步要求填上游地址；选供应商可以预填，也可以直接手填",
        ));
    }
    Ok(up)
}

fn parse_wires(body: &Value) -> Result<Vec<Wire>, ApiFailure> {
    let Some(list) = body.get("wires").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(Value::as_str)
        .map(|w| {
            Wire::parse(w).ok_or_else(|| {
                bad_request(
                    format!("未知的 wire：{w}"),
                    "wire 只有 anthropic / openai-chat / openai-responses 三个取值",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?)
}

fn parse_prices(body: &Value) -> Result<provider::Prices, ApiFailure> {
    let num = |key: &str| -> Result<Option<f64>, ApiFailure> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().map(Some).ok_or_else(|| {
                bad_request(
                    format!("价格 {key} 不是数字"),
                    "四项价要么留空，要么填每百万 token 的美元数",
                )
            }),
        }
    };
    Ok(provider::Prices {
        input: num("input")?,
        output: num("output")?,
        cache_read: num("cacheRead")?,
        cache_write: num("cacheWrite")?,
    })
}
