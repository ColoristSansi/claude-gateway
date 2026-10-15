//! 转发面 `/v1/*`：协议转换与 SSE 转发（票 40 / 41 / 51 落在这里）。
//!
//! 与 `/api/*` 分开挂：管理路径不参与转发，转发路径不做业务（spec §3）。
//! 转发面此刻是空的，只留挂载点——票 43 不搬原型 `main.rs` 的路由与硬编码表（ADR-0007 第 2 条）。

use axum::Router;

pub fn router() -> Router {
    Router::new()
}
