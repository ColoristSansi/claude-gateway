# 三个客户端如何指向自建网关

Type: research
Status: resolved
Blocked by: —

## Question

Claude Desktop（3P 模式）、Claude Code、Codex 分别怎么配置指向自建网关？各自要求哪些端点、什么模型 ID 约束、什么流式语义？

## Answer

调研报告：[client-integration.md](../research/client-integration.md)（375 行，一手来源带 URL）

- **Claude Desktop 3P 是官方功能**，激活键 `inferenceProvider=gateway`；配置落在 `%LOCALAPPDATA%\Claude-3p\configLibrary\`（`_meta.json` + `<id>.json`），或注册表 `HKLM|HKCU\SOFTWARE\Policies\Claude`。字段：`inferenceGatewayBaseUrl`、`inferenceGatewayApiKey`、`inferenceGatewayAuthScheme`（bearer 默认 / x-api-key）、`inferenceModels`、`inferenceStreamIdleTimeoutSec`、`modelDiscoveryEnabled`、`toolSearchEnabled`。必需端点 `POST /v1/messages`（含流式与工具）；`GET /v1/models` 可选。
- **模型 ID 有冲突未解**：官方文档说自动发现只展示「像 Claude」的 ID，但显式 `inferenceModels` 可填任意完整 ID；cc-switch 反编译称 app.asar 里有 `sonnet/opus/haiku/fable/mythos` 白名单，非法名会让 Desktop 拒收整组。→ 票据 [16](16-research-desktop-model-id-validation.md)。本设计选的四个家族名在两种说法下都安全。
- **Claude Code**：`ANTHROPIC_BASE_URL` + 凭据（`ANTHROPIC_AUTH_TOKEN` → `Authorization: Bearer`，`ANTHROPIC_API_KEY` → `x-api-key`）；配置在 `%USERPROFILE%\.claude\settings.json` 的 `env` 块，优先于 shell 导出。必需 `POST /v1/messages`（实际带 `?beta=true`，按路径匹配）；`count_tokens` 可选（缺失退化为字符估算）；`/v1/models` 仅在 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` 时使用且只保留 id 含 `claude`/`anthropic` 的条目。SSE 必须保序到 `message_delta` + `message_stop`，必须转发 `ping`，`anthropic-version`/`anthropic-beta` 必须原样透传。
- **Codex**：`wire_api` 官方唯一合法值是 `responses`（填 `chat` 会让整份配置失效回退默认模型）；必须 `POST /v1/responses` + 增量 SSE + `response.completed` 收尾 + 回放续轮 + `function_call`/`function_call_output` 配对；自定义模型名必须配 `model_catalog_json`。配置在 `%USERPROFILE%\.codex\config.toml`。客户端固定发 `store:false`、`stream:true`、`include:["reasoning.encrypted_content"]`。
- 未证实项单列在报告末节（Desktop 的 count_tokens、官方「像 Claude」的具体判定字符串等）。
