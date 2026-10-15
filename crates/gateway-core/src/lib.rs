//! 网关内核：入站 → IR → 绑定 → 出站，外加旁路的 obs / store / writer（spec §3）。
//!
//! 硬约束：**本 crate 不依赖 Tauri**（ADR-0007 第 1 条）。内核与回放测试裸跑，
//! 依赖方向由外壳持有（`src-tauri` → `gateway-core`），反向不成立。
//!
//! 请求路径上只有转换，没有业务；`writer`（配置写入引擎）是旁路模块，
//! 转发路径不得依赖它。
//!
//! # 模块划分（票 40 的搬迁落点；票 41 补转发面）
//!
//! - `ir` / `sse`：内部形状与 SSE 帧解析，入站出站**两侧共用**。
//! - `anthropic` / `openai` / `responses`：三个编解码文件**各自保持完整**——每个文件同时含
//!   入站与出站，不硬拆成两半（ADR-0007 第 2 条）；硬拆会让一侧反向依赖另一侧。
//! - `ingress` / `egress`：两侧的**门面** + 两件实现：`ingress::proxy` 是转发面 `/v1/*`
//!   的唯一入口（`router()`），`egress::upstream` 是出站转发，`egress::provider` 是
//!   声明式供应商表。
//! - `binding` / `obs` / `store` / `writer`：绑定层、旁路可观测、持久化、配置写入引擎。
//!
//! 测试专用件（mock 上游 + 回放 harness）不在产品路径上：`src/bin/*` 两个 bin 都挂
//! `required-features = ["mock"]`，`tests/` 与 `src/replay.rs` 只有开 `mock` 特性才编。
//!
//! 每个编解码文件的两侧归属（门面按同一张表 re-export）：
//!
//! | 文件 | 入站面（客户端，`ingress`） | 出站面（上游，`egress`） |
//! | --- | --- | --- |
//! | `anthropic.rs` | `decode_request`、`AnthropicEncoder`、`AnthropicAssembler` | `encode_request`、`encode_count_request`、`AnthropicDecoder` |
//! | `openai.rs` | —（OpenAI Chat 入站未做，M3） | `encode_request`、`OpenAiDecoder` |
//! | `responses.rs` | `decode_request`、`ResponsesEncoder`、`ResponsesAssembler` | —（上游 Responses wire 未做） |

pub mod anthropic;
pub mod binding;
pub mod clients;
pub mod egress;
pub mod ingress;
pub mod ir;
pub mod obs;
pub mod openai;
pub mod responses;
pub mod service;
pub mod sse;
pub mod store;
pub mod writer;
