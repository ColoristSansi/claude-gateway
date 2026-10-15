//! 网关内核：入站 → IR → 绑定 → 出站，外加旁路的 obs / store / writer（spec §3）。
//!
//! 硬约束：**本 crate 不依赖 Tauri**（ADR-0007 第 1 条）。内核与回放测试裸跑，
//! 依赖方向由外壳持有（`src-tauri` → `gateway-core`），反向不成立。
//!
//! 请求路径上只有转换，没有业务；`writer`（配置写入引擎）是旁路模块，
//! 转发路径不得依赖它。

pub mod binding;
pub mod egress;
pub mod ingress;
pub mod ir;
pub mod obs;
pub mod service;
pub mod store;
pub mod writer;
