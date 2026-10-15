//! 入站适配层（spec §3 / §4）。
//!
//! 两个面分开挂：管理 API 在 `/api/*`（`api`），转发面在 `/v1/*`（`proxy`）。
//! 管理面给前端与外壳读，转发面按 wire 协议解码成 IR。
//! 本票只落管理面的骨架，转发面留给票 40 / 41 / 51。

pub mod api;
pub mod proxy;

use axum::Router;

/// 组装整个入站路由。两个 `Router` 各自带自己的状态（转发面不需要状态），
/// 合并后仍不重叠：`/api/status` 存在，`/v1/status` 不存在。
pub fn router(api_state: api::ApiState) -> Router {
    api::router(api_state).merge(proxy::router())
}
