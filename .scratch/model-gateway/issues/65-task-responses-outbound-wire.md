# 上游 OpenAI Responses 出站 wire

Type: task
Status: open
Blocked by: —

## Question

spec §1 目标 3 写明「入站协议：Anthropic Messages、OpenAI Chat Completions、OpenAI Responses、Ollama 兼容；**出站协议：前三者**」。前三者里 **`openai-responses` 出站至今未接**：`crates/gateway-core/src/egress/upstream.rs` 的四处 `wire_not_implemented` 恒回 502，`Wire::implemented()` 已把它标成「本版未实现」并在界面禁选（票 64 前一轮评审修补），但**协议本身没实现**。

评审（`cg-notes/code-review.md`）把它列为两条**阻断**之一。票 57 只交付了 `text.format` 映射、票 49 的目录里 `supported_wires` 已经能声明 `openai-responses`（commandcode 有 70 条模型同时声明 chat+responses），所以上游侧是有真实样本可打的。

依据：spec §1 目标 3、§3、§4.2（Responses 的字段形状）、§5、§6（`pathTemplates.openaiResponses`）、§9；`docs/adr/0001-self-built-thin-conversion-layer.md`、`docs/adr/0004-provider-table-granularity.md`；票 37（`text.format` 3 层）、票 57 的 Answer、票 49 的 Answer（预设与拉取已经把 `/responses` 路径纳入）。

## 交付

- `crates/gateway-core/src/egress/` 侧新增 **Responses 出站编码器**：IR → 上游 OpenAI Responses 请求体。注意与 `responses.rs` 里已有的那半边**方向相反**——`responses.rs` 现在同时含「Codex 入站解码」与「回给 Codex 的帧编码」（票 41 定案），本票要的是「发给上游的请求编码」+「读上游响应的 SSE 解码成 IR」。ADR-0007 第 2 条要求编解码文件各自保持完整，按那条先例决定落点并在 Answer 说明。
- 系统提示的落位：上游 Responses 用 `instructions`（与 Codex 把 system 放 `input` 的 `role:"developer"` 那套**不是**一回事，别照抄票 32 的结论）。
- `text.format` 走票 57 已定的 3 层形状（`{name, schema, strict}`，`name` 填常量 `"response"`，`strict` 只透传显式值）。
- 流式：上游 Responses SSE → IR 事件（`response.output_text.delta`、`response.completed`、function_call 项、reasoning 项按 §4.2 的口径处理）；非流式：缓冲到终态再组装。
- 把 `egress/upstream.rs` 的四处 `wire_not_implemented` 换成真编码/解码；`Wire::implemented()` 对 `openai-responses` 翻成已实现，**同时把界面/接口那侧的「本版未实现」标注与禁选一并去掉**（那是票 64 前一轮加的临时诚实措施）。
- `pathTemplates.openaiResponses` 的反查与拉取口径沿用票 49，不改。

## 收口验收

- 单测 / 回放：**流式与非流式各一条黄金用例**（上游 Responses 帧 → IR → 客户端面），**工具往返一条**（function_call 成对），**`text.format` 3 层逐字一条**，**错误信封一条**（上游非 2xx 时按**入站**协议给形状，不是 Responses 的形状）。
- **真实上游**：commandcode 或 opencode 里声明了 `openai-responses` 的模型真打一次（流式 + 非流式），把请求体与帧序列贴进 Answer。凭据在 `C:\Users\ZHANQUAN\AIGC\Project\Claude Gateway\.scratch\model-gateway\prototype-upstream.env`（**gitignore 的、你 worktree 里没有**；绝对路径读进变量、输出掩码、**不许用会回显整行的方式读**，绝不入库）。
- `pwsh -File ./check.ps1 -Quick -Replay` 全绿（含前端类型检查步）；票 41/51/56/64 的回放格一格不许红。
- 前端：把「本版未实现」的禁选去掉后，`pnpm --dir app build` 仍绿，且**故意选一次 `openai-responses` 存一条绑定应当成功**（贴证据）。

## 不做

- 不改 spec / ADR；票 37/49/57 已定的形状照做，不再改判据。
- 不动客户端配置写入（票 45–48）与发布链路（票 61/63）。
