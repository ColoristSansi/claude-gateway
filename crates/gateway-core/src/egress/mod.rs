//! 出站适配层：Anthropic / OpenAI Chat / OpenAI Responses 三个 codec + 声明式供应商表，
//! 负责编码与取凭据（spec §3 / §6）。三个编解码文件各含入站与出站，不硬拆两半。
//!
//! 本模块是**门面 + 出站实现**：按「上游面」把符号 re-export 出来；客户端面在 [`crate::ingress`]。
//! 票 41 起，出站转发在 [`upstream`]；声明式供应商表、上游模型目录与价格在 [`provider`]
//! （票 49 把原型那份两值 `Wire` 的表换成 spec §6 的三值形状）。
//!
//! re-export 清单：
//!
//! - `crate::anthropic::encode_request` → [`encode_anthropic_request`]（IR → 上游 Anthropic 请求体）
//! - `crate::anthropic::encode_count_request`（count_tokens 降级路径的上游请求体）
//! - `crate::anthropic::AnthropicDecoder`（上游 Anthropic SSE → IR 事件）
//! - `crate::openai::encode_request` → [`encode_openai_chat_request`]（IR → 上游 Chat 请求体）
//! - `crate::openai::OpenAiDecoder`（上游 Chat SSE → IR 事件）
//!
//! `responses.rs` 在此**没有**导出：它的「出站编码」是把帧写回 Codex（客户端面），
//! 归 `ingress`；上游 Responses wire 未做。

pub mod provider;
pub mod upstream;

pub use crate::anthropic::{
    encode_count_request, encode_request as encode_anthropic_request, AnthropicDecoder,
};
pub use crate::openai::{encode_request as encode_openai_chat_request, OpenAiDecoder};
