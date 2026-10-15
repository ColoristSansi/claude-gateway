//! 绑定层：家族槽位 / 虚拟模型 → (上游, 真实模型, wire)，含客户端档案覆盖（spec §7）。
//!
//! 票 41 只落地**转发查表要用的最小形状**：一条路由（虚拟模型名 → 上游 + 真实模型 + wire）。
//! 表从哪来（种库 / 界面 / 客户端档案覆盖 / 家族槽位展开）归票 49 与票 51——它们在这条
//! 路由上加字段与来源，不另开一套查表。`is_family` 现在就带着，免得 51 再改一次形状。

/// 一条绑定：客户端能填的虚拟模型名 → 出站对应的上游与真实模型。
#[derive(Debug, Clone)]
pub struct Route {
    pub name: String,
    pub upstream_id: String,
    pub real_model: String,
    /// `anthropic` / `openai`（M1 的两种出站 wire；`openai-responses` 归后面的票）。
    pub wire: String,
    /// 是不是四个家族槽位之一（spec §5；界面按它分组）。
    pub is_family: bool,
}
