//! 入站适配层：`/v1/messages`、`/v1/chat/completions`、`/v1/responses`、`/api/*`（spec §3 / §4）。
//! 职责是把客户端请求解码成 IR，出站帧编码回客户端 wire。
//!
//! 本模块是**门面 + 挂载点**：门面按「客户端面」把三个编解码文件的入站符号 re-export 出来
//! （各文件同时含入站与出站，不硬拆两半，ADR-0007 第 2 条；上游面在 [`crate::egress`]）；
//! 挂载点把管理面 `/api/*` 与转发面 `/v1/*` 分开挂，两者互不重叠。
//! 转发面的路由表与端点 M2/M3 才长进来（票 41 起接）。
//!
//! re-export 清单：
//!
//! - `crate::anthropic::decode_request` → [`decode_anthropic_request`]
//! - `crate::anthropic::AnthropicEncoder` / `AnthropicAssembler`（IR 事件 → 客户端 wire）
//! - `crate::responses::decode_request` → [`decode_responses_request`]
//! - `crate::responses::ResponsesEncoder` / `ResponsesAssembler`
//!
//! 两个 `decode_request` 同名，门面里按协议加后缀区分；原型名字在各编解码文件里没变。

pub mod api;
pub mod proxy;

pub use crate::anthropic::{
    decode_request as decode_anthropic_request, AnthropicAssembler, AnthropicEncoder,
};
pub use crate::responses::{
    decode_request as decode_responses_request, ResponsesAssembler, ResponsesEncoder,
};

use axum::Router;

/// 组装整个入站路由。两个 `Router` 各自带自己的状态（转发面不需要状态），
/// 合并后仍不重叠：`/api/status` 存在，`/v1/status` 不存在。
pub fn router(api_state: api::ApiState) -> Router {
    api::router(api_state).merge(proxy::router())
}
