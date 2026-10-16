//! 入站适配层：`/v1/messages`、`/v1/chat/completions`、`/v1/responses`、`/api/*`（spec §3 / §4）。
//! 职责是把客户端请求解码成 IR，出站帧编码回客户端 wire。
//!
//! 本模块是**门面 + 挂载点**：门面按「客户端面」把三个编解码文件的入站符号 re-export 出来
//! （各文件同时含入站与出站，不硬拆两半，ADR-0007 第 2 条；上游面在 [`crate::egress`]）；
//! 挂载点把管理面 `/api/*`（票 43）与转发面 `/v1/*`（票 41）分开挂，两者互不重叠；
//! Ollama 兼容面（票 56，`/api/chat` 等五个端点）挂在转发面那一侧，`/api/*` 与 `/v1/*`
//! 的兜底也归它 ——**具体路由优先**，所以管理面的 `/api/status` / `/api/logs*` 不会被那条兜底
//! 吃掉。票 64 起两张脸的兜底**合成一个**挂在合并点（[`assemble`]），管理面将来自己加
//! fallback 也不会让 `Router::merge` 在启动时 panic。
//!
//! re-export 清单：
//!
//! - `crate::anthropic::decode_request` → [`decode_anthropic_request`]
//! - `crate::anthropic::AnthropicEncoder` / `AnthropicAssembler`（IR 事件 → 客户端 wire）
//! - `crate::openai::decode_request` → [`decode_chat_request`]（票 64 的 Chat 入站）
//! - `crate::openai::ChatEncoder` / `ChatAssembler`
//! - `crate::responses::decode_request` → [`decode_responses_request`]
//! - `crate::responses::ResponsesEncoder` / `ResponsesAssembler`
//!
//! 三个 `decode_request` 同名，门面里按协议加后缀区分；原型名字在各编解码文件里没变。

pub mod api;
pub mod ollama;
pub mod proxy;

pub use crate::anthropic::{
    decode_request as decode_anthropic_request, AnthropicAssembler, AnthropicEncoder,
};
pub use crate::openai::{decode_request as decode_chat_request, ChatAssembler, ChatEncoder};
pub use crate::responses::{
    decode_request as decode_responses_request, ResponsesAssembler, ResponsesEncoder,
};

use std::sync::Arc;

use axum::Router;

/// 组装整个入站路由。两个 `Router` 各自带自己的状态，合并后仍不重叠：
/// `/api/status` 存在，`/v1/status` 不存在。
///
/// `forwarding` 是转发面（`/v1/*`）的状态：`None` = 此刻只挂管理面
/// （外壳还没接出站转发的阶段；票 49 / 51 起把它接上，路由表就来自那里）。
pub fn router(api_state: api::ApiState, forwarding: Option<Arc<proxy::Gateway>>) -> Router {
    // 票 53：日志面（`/api/logs*` 与 `/api/usage/summary`）要的是转发面那个库与**同一个**
    // body 窗口，所以在挂路由之前先接过来；转发面没挂时日志面回 503，不装成空表。
    let api_state = match &forwarding {
        Some(gateway) => api_state.attach_logs(gateway),
        None => api_state,
    };
    let app = api::router(api_state);
    match forwarding {
        // 票 64：转发面给的是**不带兜底**的路由表（`proxy::routes`），兜底在合并点统一挂一条。
        Some(state) => assemble(app, proxy::routes().with_state(state)),
        None => app,
    }
}

/// 两张脸的兜底**合成一个**（票 64；评审「中」第五条）：在 `merge` **之后**挂唯一那条 fallback。
///
/// 这样管理面以后自带 fallback 也不会让 `Router::merge` 在启动时 panic（axum 两边都设
/// fallback 即 panic）—— `Router::fallback` 是替换语义，最后挂的这条说了算。
/// 代价：管理面自己的 fallback 会被这一条覆盖，这是有意的（兜底本来就该由合并点统一拥有）。
fn assemble(api: Router, forwarding: Router) -> Router {
    api.merge(forwarding).fallback(ollama::api_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    /// 评审「中」第五条钉死在这里：管理面将来自带 fallback 时，合并点不能再 panic。
    /// 只构造、不请求——出问题的那一刻就是 `merge` / `fallback` 的构造期。
    #[test]
    fn two_faces_merge_into_one_fallback_without_panicking() {
        let api = Router::new()
            .route("/api/status", get(|| async { "ok" }))
            .fallback(|| async { "管理面自己的兜底" });
        let forwarding = Router::new().route("/v1/chat/completions", get(|| async { "chat" }));
        let app = assemble(api, forwarding);
        // 合成点之外再挂一次也不能 panic（`fallback` 是替换，不是合并）。
        let app = app.fallback(ollama::api_fallback);
        assert!(app.has_routes());
    }
}
