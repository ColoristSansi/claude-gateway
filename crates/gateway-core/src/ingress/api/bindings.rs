//! `/api/bindings*` + `/api/virtual-models` + `/api/pricing/*`（模型与映射页）。
//!
//! 四个家族槽位固定四行、普通虚拟模型一张列表；**每客户端可覆盖**（`?client=` 就是档案）。
//! 业务全在 [`crate::binding`]（票 51）与 [`crate::egress::provider`]（票 49）；这里只做翻译。
//! 路径对齐 `.scratch/model-gateway/frontend.md` §③ 第 22–26 条。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::binding::{self, Binding, Profile, FAMILY_SLOTS};
use crate::egress::provider::{self, Wire};
use crate::store::Store;

use super::{admin, ok_json, ApiError, ApiFailure, ApiState};

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/bindings", get(list_bindings))
        .route("/api/bindings/{slot}", put(save_binding))
        .route("/api/bindings/{slot}/probe", post(probe_slot))
        .route(
            "/api/virtual-models",
            get(list_virtual_models).post(save_virtual_model),
        )
        .route("/api/virtual-models/{name}", delete(remove_virtual_model))
        .route(
            "/api/pricing/{upstream}/{model}",
            get(get_prices).put(set_prices),
        )
}

#[derive(serde::Deserialize)]
struct ClientQuery {
    client: Option<String>,
}

fn profile_of(q: &ClientQuery) -> Result<Profile, ApiFailure> {
    match q.client.as_deref() {
        None => Ok(Profile::Global),
        Some(id) => Profile::parse(id).ok_or_else(|| {
            ApiFailure::new(
                StatusCode::BAD_REQUEST,
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("未知的客户端标签：{id}"),
                    "标签只有 global / claude-desktop / claude-code / codex",
                ),
            )
        }),
    }
}

/// 四行槽位 + 普通虚拟模型列表（按档案解析；**每条都带上 `global` 那份以备对照**）。
async fn list_bindings(
    State(state): State<ApiState>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let table = admin.bindings().await.map_err(ApiFailure::from)?;

    let slots: Vec<Value> = FAMILY_SLOTS
        .iter()
        .map(|slot| {
            let own = table
                .rows()
                .iter()
                .find(|b| b.profile == profile && b.name == *slot);
            let global = table
                .rows()
                .iter()
                .find(|b| b.profile == Profile::Global && b.name == *slot);
            json!({
                "slot": slot,
                "binding": own.map(binding_json),
                "global": global.map(binding_json),
                "overridden": own.is_some() && profile != Profile::Global,
            })
        })
        .collect();

    let virtuals: Vec<Value> = table
        .rows()
        .iter()
        .filter(|b| !b.is_family())
        .map(binding_json)
        .collect();

    Ok(ok_json(json!({
        "profile": profile.id(),
        "profileLabel": profile.display(),
        "profiles": Profile::ALL.iter().map(|p| json!({"id": p.id(), "label": p.display()})).collect::<Vec<_>>(),
        "slots": slots,
        "virtualModels": virtuals,
        "wires": Wire::ALL.iter().map(|w| w.as_str()).collect::<Vec<_>>(),
    })))
}

fn binding_json(b: &Binding) -> Value {
    json!({
        "profile": b.profile.id(),
        "name": b.name,
        "upstreamId": b.upstream_id,
        "realModel": b.real_model,
        "wire": b.wire.as_str(),
        "isFamily": b.is_family(),
        "triple": b.triple(),
    })
}

/// 存一条绑定（槽位或普通虚拟模型；`?client=` 决定档案）。
async fn save_binding(
    State(state): State<ApiState>,
    Path(slot): Path<String>,
    Query(q): Query<ClientQuery>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let upstream_id = str_field(&body, "upstreamId")?;
    let real_model = str_field(&body, "realModel")?;
    let wire = Wire::parse(body.get("wire").and_then(Value::as_str).unwrap_or(""))
        .ok_or_else(|| bad("wire 只能是 anthropic / openai-chat / openai-responses"))?;
    let b = Binding::new(profile, &slot, &upstream_id, &real_model, wire);
    let choice = binding::save(&admin.store, &b)
        .await
        .map_err(|e| binding_error(&slot, e))?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "saved": true,
        "binding": binding_json(&b),
        "wireNote": choice.note(),
    })))
}

/// 探针：非流式 `max_tokens:1` 走真实链路（与 Claude Desktop 自身探针同形）。
async fn probe_slot(
    State(state): State<ApiState>,
    Path(slot): Path<String>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let table = admin.bindings().await.map_err(ApiFailure::from)?;
    let b = table
        .rows()
        .iter()
        .find(|b| b.profile == profile && b.name == slot)
        .or_else(|| {
            table
                .rows()
                .iter()
                .find(|b| b.profile == Profile::Global && b.name == slot)
        })
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::NOT_FOUND,
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    format!("槽位 {slot} 还没绑定"),
                    "先在表里选上游与真实模型（目录里没有就手工新增一条）",
                )
                .with_action("去上游页加模型", "goto", Some("upstreams")),
            )
        })?;
    let up = provider::load_upstream(&admin.store, &b.upstream_id)
        .await
        .map_err(ApiFailure::from)?
        .ok_or_else(|| {
            ApiFailure::new(
                StatusCode::NOT_FOUND,
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    format!("槽位 {slot} 指的上游不在了：{}", b.upstream_id),
                    "上游被删过；重新绑一条",
                ),
            )
        })?;
    // 探针只看状态码、超时 504、连不上 502；结果一并落库（与向导第④步同一个函数）。
    let result = binding::selfcheck(
        &admin.http,
        &admin.store,
        &up,
        b.wire,
        &b.real_model,
        binding::PROBE_DEADLINE,
    )
    .await;
    Ok(ok_json(json!({
        "ok": result.ok,
        "status": result.status,
        "elapsedMs": result.elapsed_ms as u64,
        "detail": result.detail,
        "slot": slot,
        "upstream": result.upstream_id,
        "realModel": result.real_model,
        "wire": result.wire.as_str(),
        "requestId": super::random_id("probe"),
    })))
}

/// 普通虚拟模型列表（Codex 标签下这张列表就是它的模型目录）。
async fn list_virtual_models(
    State(state): State<ApiState>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let table = admin.bindings().await.map_err(ApiFailure::from)?;
    let rows: Vec<Value> = table
        .rows()
        .iter()
        .filter(|b| !b.is_family() && b.profile == profile)
        .map(binding_json)
        .collect();
    Ok(ok_json(json!({ "virtualModels": rows })))
}

/// 新增 / 改一条普通虚拟模型（`name` 给了就是改）。
async fn save_virtual_model(
    State(state): State<ApiState>,
    Query(q): Query<ClientQuery>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let name = str_field(&body, "name")?;
    if binding::is_family(&name) {
        return Err(bad("家族槽位是固定四行：普通虚拟模型请换个名字"));
    }
    let upstream_id = str_field(&body, "upstreamId")?;
    let real_model = str_field(&body, "realModel")?;
    let wire = Wire::parse(body.get("wire").and_then(Value::as_str).unwrap_or(""))
        .ok_or_else(|| bad("wire 只能是 anthropic / openai-chat / openai-responses"))?;
    let b = Binding::new(profile, &name, &upstream_id, &real_model, wire);
    let choice = binding::save(&admin.store, &b)
        .await
        .map_err(|e| binding_error(&name, e))?;
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "saved": true,
        "binding": binding_json(&b),
        "wireNote": choice.note(),
    })))
}

async fn remove_virtual_model(
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Query(q): Query<ClientQuery>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let profile = profile_of(&q)?;
    let (profile_id, name) = (profile.id().to_string(), name.clone());
    let deleted_name = name.clone();
    let n = admin
        .store
        .write(move |c| {
            Ok(c.execute(
                "DELETE FROM virtual_models WHERE client_profile_id=?1 AND name=?2",
                rusqlite::params![profile_id, name],
            )?)
        })
        .await
        .map_err(ApiFailure::from)?;
    if n == 0 {
        return Err(ApiFailure::new(
            StatusCode::NOT_FOUND,
            ApiError::new(
                StatusCode::NOT_FOUND,
                format!("这个档案下没有 {deleted_name}"),
                "它可能绑在别的客户端标签下（切到「全局」看看）",
            ),
        ));
    }
    admin.reload_forwarding().await.map_err(ApiFailure::from)?;
    Ok(ok_json(json!({"deleted": true, "name": deleted_name})))
}

/// 一个「上游 × 真实模型」的四项价与逐项来源（「价格」按钮打开的就是它）。
async fn get_prices(
    State(state): State<ApiState>,
    Path((upstream, model)): Path<(String, String)>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    Ok(ok_json(json!({
        "upstreamId": upstream,
        "modelId": model,
        "prices": prices_view(&admin.store, &upstream, &model).await?,
    })))
}

/// 写手工四项价（可填可留空；填了就锁定、刷新快照不覆盖，留空回退 models.dev）。
async fn set_prices(
    State(state): State<ApiState>,
    Path((upstream, model)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiFailure> {
    let admin = admin(&state)?;
    let prices = parse_prices(&body)?;
    provider::set_manual_prices(&admin.store, &upstream, &model, &prices)
        .await
        .map_err(ApiFailure::from)?;
    Ok(ok_json(json!({
        "saved": true,
        "prices": prices_view(&admin.store, &upstream, &model).await?,
    })))
}

async fn prices_view(store: &Store, upstream: &str, model: &str) -> Result<Value, ApiFailure> {
    let r = provider::resolved_prices(store, upstream, model)
        .await
        .map_err(ApiFailure::from)?;
    let one = |p: provider::ResolvedPrice| {
        json!({"usdPerMtok": p.usd_per_mtok, "source": p.source.as_str()})
    };
    Ok(json!({
        "input": one(r.input),
        "output": one(r.output),
        "cacheRead": one(r.cache_read),
        "cacheWrite": one(r.cache_write),
    }))
}

// ───────────────────────── 小工具 ─────────────────────────

fn str_field(body: &Value, key: &str) -> Result<String, ApiFailure> {
    body.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(format!("{key} 不能为空")))
}

fn bad(what: impl Into<String>) -> ApiFailure {
    ApiFailure::new(
        StatusCode::BAD_REQUEST,
        ApiError::new(StatusCode::BAD_REQUEST, what, "检查这一行的三个取值再存一次"),
    )
}

/// 绑定落库失败：把三层校验的人话原样给界面（目录里没有 / wire 没声明）。
fn binding_error(name: &str, e: binding::SaveError) -> ApiFailure {
    match e {
        binding::SaveError::NoCatalogEntry {
            upstream_id,
            real_model,
        } => ApiFailure::new(
            StatusCode::CONFLICT,
            ApiError::new(
                StatusCode::CONFLICT,
                format!("上游模型目录里没有 {upstream_id} / {real_model}"),
                "绑定只能从目录里选；目录里没有就先去上游页手工新增一条（会按 manual 锁定）",
            )
            .with_action("去上游页加模型", "goto", Some("upstreams")),
        ),
        binding::SaveError::WireNotDeclared(rej) => ApiFailure::new(
            StatusCode::BAD_REQUEST,
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} 的协议不在这条模型的声明里"),
                rej.message,
            ),
        ),
        other => ApiFailure::new(
            StatusCode::BAD_REQUEST,
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} 没能存下去"),
                other.to_string(),
            ),
        ),
    }
}

fn parse_prices(body: &Value) -> Result<provider::Prices, ApiFailure> {
    let num = |key: &str| -> Result<Option<f64>, ApiFailure> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().map(Some).ok_or_else(|| {
                bad(format!(
                    "价格 {key} 不是数字（要么留空，要么填每百万 token 的美元数）"
                ))
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
