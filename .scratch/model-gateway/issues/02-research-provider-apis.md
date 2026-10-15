# 供应商 API 契约与 models.dev 数据源

Type: research
Status: resolved
Blocked by: —

## Question

DeepSeek、commandcode、opencode、千问四家的 base_url、认证、支持的协议、模型与流式差异分别是什么？models.dev 能不能当作供应商预设的数据源？

## Answer

调研报告：[providers.md](../research/providers.md)（汇总速查表 + 逐家小节 + 未证实清单）

- 四家**都支持 Anthropic Messages**（不止 OpenAI 兼容），但路径契约各不相同：DeepSeek `/anthropic`；commandcode `/provider/v1/messages`；opencode `/inference/anthropic/v1/messages`；千问 `/apps/anthropic/v1/messages`。
- 千问硬约束：Anthropic base **不能带 `/v1`**（客户端会自己追加 `/v1/models`，误填就是 `/v1/v1/models` 404），且该端点**没有 `/v1/models`** → 网关必须自己实现模型列表。
- 千问其他差异：temperature 取值域 `[0,2)`（Anthropic 官方是 `[0,1]`）；`stop_sequence` 恒 `null`；thinking 块 `signature` 恒空串；Responses 输入只给模型窗口 80% 且**静默截断**；Token Plan 用 `sk-sp-`、平台用 `sk-`，两套不通用。
- DeepSeek 陷阱：带 `tools` 时必须回传历史 `reasoning_content`，否则 400；Chat 流式 usage **不单独发 chunk**（附在最后内容块）；keep-alive 空行／`: keep-alive` 注释必须容忍；Responses 无状态（不支持 `previous_response_id`/`store`）。
- commandcode：Claude 模型**只能走 `/messages`**（走 chat/responses 直接 400）；`/v1/models` 每个模型带 `supported_endpoints`，选端点前必查；Go 计划**无 API 权限**（403）。
- opencode：路径带协议族前缀（`/openai`、`/anthropic`），**每个模型只属一个 family**，不能配单一默认端点；Go 计划需要自定义 UA + `x-opencode-session` 会话头。
- **models.dev**：约 228 个 provider / 8804 个 model，源头是仓库里的 TOML；`api.json` 字段为 `id/env/npm/api/name/doc/models`，**没有 `apiTypes` 数组**。`npm` 只能当预设先验（188/228 是 openai-compatible，25 个 provider 没有 `api`，且单一 `api` 表达不了「一个供应商两个 base URL / 两种协议」，opencode-go 与 deepseek 就是反例）。`interleaved.field` 直接给出 `reasoning_content` 还是 `reasoning_details`，是归一化的关键线索。
