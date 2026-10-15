# 供应商协议调研（模型网关入站/出站适配用）

调研时间：2026-10-14（Asia/Shanghai）。所有结论均给出主源 URL；无法从官方文档确认的一律标注「未证实」。
抓取方式：官方文档页 HTML（`Invoke-WebRequest` + 去标签）与 `https://models.dev/api.json`（原始 JSON，228 providers / 8804 models）。

术语：
- **入站协议** = 客户端以哪种协议打进来。
- **出站协议** = 网关该往上游发哪种协议；本文「支持哪些协议」同时指二者（对同一供应商，入站与出站通常同集合）。
- **Chat** = OpenAI Chat Completions（`/chat/completions`），**Resp** = OpenAI Responses（`/responses`），**Msgs** = Anthropic Messages（`/messages`）。

---

## 0. 汇总速查

| 供应商 | OpenAI Chat | OpenAI Resp | Anthropic Msgs | base_url 需要 `/v1`？ | 认证头 | 最大陷阱 |
|---|---|---|---|---|---|---|
| DeepSeek | 是 | 是（无状态） | 是 | OpenAI 侧文档写无 `/v1`，Anthropic 侧为 `/anthropic`（**无 `/v1`**） | `Authorization: Bearer`；Anthropic 侧 `x-api-key` | 思考模式 + `tools` 时**必须回传 `reasoning_content`**，否则 400；`claude-*` 模型名被强制映射 |
| commandcode | 是 | 是 | 是 | 路径自带 `/provider/v1`，base 填 `…/provider/v1` | `Authorization: Bearer`；`/messages` 也接受 `x-api-key` | Claude 模型**只能**走 `/messages`，走 chat/responses 直接 400；Go 计划无 API 权限（403） |
| opencode（Console inference） | 是 | 是 | 是（+ Gemini） | base 不含 `/v1`，路径含 `/v1` | `Authorization: Bearer`（免费模型可省） | 一个模型只属于一个 API family，发错端点报错；Go 端点要求 `x-opencode-session` + 专属 UA |
| 千问 Qwen（平台 + Token Plan） | 是 | 是 | 是（+ DashScope 原生） | **Anthropic base 必须不带 `/v1` 后缀**；OpenAI 侧带 `/compatible-mode/v1` | OpenAI 侧 `Authorization: Bearer`；Anthropic 侧 `x-api-key` 或 `Authorization: Bearer` | Anthropic 端点**无 `/v1/models`**（模型发现 404）；Token Plan 用 `sk-sp-` key，与平台 `sk-` key 不通用 |
| models.dev `api.json` | —（数据源，非供应商） | — | — | `api` 字段原样照抄 | `env` 字段给 key 名 | `npm` 只标 AI SDK 包，不能等价为协议；12 个 provider 无 `api` |

**支持 Anthropic 协议的（本批四家全部支持）**：DeepSeek、commandcode、opencode、千问 Qwen —— 四家都提供真正的 `/messages` 端点（opencode 另有 Gemini 端点）。
**只有 OpenAI 兼容的**：本批无。但契约差异很大：DeepSeek 是同一个 host 下的 `/anthropic` 子路径；Qwen 是 `/apps/anthropic` 且**明令不得带 `/v1` 后缀**；opencode 是 `/inference/anthropic/v1` 协议族前缀；commandcode 是 `/provider/v1/messages` 直连路径。

---

## 1. DeepSeek

来源：
- 首次调用 API <https://api-docs.deepseek.com/zh-cn/>
- 模型 & 价格 <https://api-docs.deepseek.com/zh-cn/quick_start/pricing>
- 思考模式 <https://api-docs.deepseek.com/zh-cn/guides/thinking_mode>
- 使用 Anthropic API <https://api-docs.deepseek.com/zh-cn/guides/anthropic_api>
- 使用 Responses API <https://api-docs.deepseek.com/zh-cn/guides/responses_api>
- 图像理解 <https://api-docs.deepseek.com/zh-cn/guides/vision>
- Chat Completions API <https://api-docs.deepseek.com/zh-cn/api/create-chat-completion>
- 限速与隔离 <https://api-docs.deepseek.com/zh-cn/quick_start/rate_limit>

### base_url

| 协议 | base_url | 是否需要 `/v1` |
|---|---|---|
| OpenAI Chat + Responses | `https://api.deepseek.com` | 文档中的样例是 `https://api.deepseek.com/chat/completions`、SDK `base_url="https://api.deepseek.com"`（**不带 `/v1`**） |
| Anthropic Messages | `https://api.deepseek.com/anthropic` | **不带 `/v1`**；实际端点是 `https://api.deepseek.com/anthropic/v1/messages` |

> 兼容性：实测 `GET https://api.deepseek.com/v1/models` 与 `GET https://api.deepseek.com/models` 均返回 401（未带 key），`POST /v1/chat/completions`、`POST /chat/completions`、`POST /v1/responses` 也都是 401；不存在的路径同样返回 401。**该探测无法区分路由存在性，仅能证明边缘层在鉴权前统一返回 401。** 结论以文档为准：不要自作主张给 base_url 追加 `/v1`。

### 认证

- OpenAI 侧：`Authorization: Bearer ${DEEPSEEK_API_KEY}`。
- Anthropic 侧：`x-api-key` **完全支持**；`anthropic-version` 被忽略；`anthropic-beta` 在 `/messages` 上被忽略，但 Files API 端点必须带 `anthropic-beta: files-api-2025-04-14`。

### 模型

| 模型 ID | 上下文 | 最大输出 | 视觉 | 思考 | 工具调用 | 并发上限 |
|---|---|---|---|---|---|---|
| `deepseek-flash` | 1M | 384K | 支持 | 支持（默认开启） | 支持 | 2500 |
| `deepseek-v4-pro` | 1M | 384K | **不支持** | 支持 | 支持 | 500 |

- 旧模型名 `deepseek-v4-flash`、`deepseek-v4-flash-vision-exp` 仍可调用，但对应模型已下线，请求由 DeepSeek-V4.1-Flash 承接并按 Flash 计费。网关**不要**把它们当成独立模型列给用户。
- 两者都支持 JSON Output、Tool Calls、Responses API、Anthropic API、对话前缀续写（Beta）；FIM 仅非思考模式。
- 并发按账号计（与 API Key 无关），超限返回 HTTP 429。扩容需提工单。

### 流式差异与推理字段

- **OpenAI Chat**：标准 SSE，以 `data: [DONE]` 结束。思考内容在 `choices[0].message.reasoning_content`，与 `content` **同级**。`stream_options.include_usage: true` 时所有块都带 `usage`（除最后一块外为 `null`）——注意**不会单独下发一个只含 usage 的块**，统计信息附在最后一个内容块上（该块 `choices` 只有 1 个元素、无新增内容、`finish_reason` 非 null）。这与 OpenAI 官方行为不同，是最容易踩的流式解析坑。
- **Responses**：语义化 SSE，每个事件带 `event` 类型与递增的 `sequence_number`，**没有 `data: [DONE]`**。事件名：`response.created`、`response.in_progress`、`response.output_item.added|done`、`response.content_part.added|done`、`response.reasoning_text.delta|done`、`response.output_text.delta|done`、`response.function_call_arguments.delta|done`、`response.custom_tool_call_input.delta|done`，并以 `response.completed` / `response.incomplete` / `response.failed` 结束。
- **Anthropic**：`/messages`，思考内容以 `type: "thinking"` 内容块承载（兼容性表中明确「`array, type="thinking"` 支持」，`redacted_thinking` 不支持）。
- **保活**：非流式请求在等待推理期间**持续返回空行**，流式请求持续返回 SSE 注释 `: keep-alive`。自己解析 HTTP 响应的网关必须容忍它们；10 分钟未开始推理则服务端断开。

### 非标准字段与特殊要求

- 请求体额外参数（OpenAI 侧，非标准）：`thinking: {type: enabled|disabled}`、`reasoning_effort: none|low|high|max`（`minimal→low`，`medium`/`xhigh→high`）、`user_id`（正则 `[a-zA-Z0-9\-_]+`，≤512）。用 OpenAI SDK 时 `thinking` 与 `user_id` 必须走 `extra_body`。
- 思考模式下**不支持** `temperature`、`presence_penalty`、`frequency_penalty`：传了不报错但**不生效**（静默忽略）。`top_p` 仅思考模式生效，下限 0.95，低于按 0.95 处理；非思考模式恒为 1.0。
- 思考模式下 `tool_choice` **不支持** `required` 与指定具体 tool，会返回 400。
- **`reasoning_content` 回传规则（关键）**：请求携带 `tools` 时，历史所有 `reasoning_content` 都应回传并会被拼进上下文，**不回传会 400**；不携带 `tools` 时无需回传，传了也被忽略。
- Anthropic 侧被忽略的字段：`container`、`mcp_servers`、`service_tier`、`top_k`、`cache_control`、`citations`、`is_error`；`thinking.budget_tokens` 被忽略；`output_config` 只支持 `effort`；`metadata` 只支持 `user_id`。不支持的 content 类型：`document`、`search_result`、`redacted_thinking`、`code_execution_tool_result`、`mcp_tool_use`、`mcp_tool_result`、`container_upload`。
- **模型名映射（网关必须处理）**：Anthropic 端点收到 `claude-opus*` → 映射到 `deepseek-v4-pro`（按 Pro 计费）；`claude-haiku*` / `claude-sonnet*` → `deepseek-flash`；**任何不认识的模型名都会落到 `deepseek-flash`**（不报错）。
- Responses 侧：`previous_response_id`、`conversation`、`store`、`background`、`metadata`、`include`、`prompt`、`truncation`、`service_tier`、`stream_options` 均**不支持**（无状态 API），不支持参数**静默忽略**。`truncation` 不支持意味着超上下文会直接 400。
- 图片限制：单请求 48 MiB 体积、单图 base64/URL 32 MiB、`file_id` 单图 64 MiB、单请求最多 600 张；图片只能出现在 `user`（Responses 侧还有 `developer` 与工具输出）消息中，出现在 `system`/`assistant` 会 400。`deepseek-v4-pro` 不支持视觉。

---

## 2. commandcode（Command Code Provider API）

来源：
- Provider API <https://commandcode.ai/docs/provider>（HTML 去标签后的正文 830 行）
- GOAT Plan <https://commandcode.ai/docs/plans/goat>

> 说明：`/docs/provider` 与 `/docs/plans/goat` 首次经 `web_fetch` 只返回导航骨架（内容截断），改用 `Invoke-WebRequest` + 去标签拿到全文。`/docs/byok`、`/docs/reference/cli/models` 同样只拿到骨架，**未取到正文**——本节的模型清单来自 `/docs/provider` 与 `/docs/plans/goat`。

### base_url

单前缀 `https://api.commandcode.ai/provider/v1`（路径自带 `v1`，不要再追加）。

| 端点 | 方法 | 格式 |
|---|---|---|
| `https://api.commandcode.ai/provider/v1/chat/completions` | POST | OpenAI Chat Completions |
| `https://api.commandcode.ai/provider/v1/responses` | POST | OpenAI Responses |
| `https://api.commandcode.ai/provider/v1/messages` | POST | Anthropic Messages |
| `https://api.commandcode.ai/provider/v1/models` | GET | 模型列表 |
| `https://api.commandcode.ai/provider/v1/systemone` | POST | 决策模型（typesafe/jev，非聊天，**不流式**） |

### 认证

- `Authorization: Bearer <CMD_API_KEY>`（任何路由）。
- Anthropic SDK 走 `/messages` 时也接受 `x-api-key: <token>`。
- 同一把 key 同时用于 CLI 与 API。

### 支持的入站/出站协议

Chat、Responses、Anthropic Messages 三种**都有**，但**按模型分派**：
- OpenAI 模型与开源模型答 `/chat/completions`，其中多数也答 `/responses`。
- **Claude 模型只在 `/messages`**；把 Claude 模型发到 `/chat/completions` 或 `/responses` 会返回 **400**，指向 `/messages`，反向亦然。
- 模型列表里每个模型带 `supported_endpoints` 字段，**网关应在选择端点前查这个字段**（`GET /provider/v1/models`）。
- Responses 侧工具数组原样透传：`function` / `custom` 由客户端执行；`web_search` 等内置声明被转发给上游（上游可拒）。**远程 `type: "mcp"` 工具被拒绝**（上游会用它们的凭据拨你的服务器）。
- 消息内容只支持 text 与 image；audio/file/document 会被 schema 拒绝。视觉能力不做前置门控，选到无视觉模型时由上游报错。

### 流式

- `stream: true` 三个端点都支持。
- **每个端点都强制在流末尾下发 token usage，无需 opt-in**：
  - Chat 客户端看到最后一个 `usage` chunk；
  - Responses 客户端在终止事件 `response.completed` 上看到 usage；
  - Anthropic 客户端在 `message_delta` 事件里看到 usage。
- `typesafe/jev` 决策模型**从不流式**。

### 非标准字段 / 自定义 header

- `x-cmd-zdr: 1`：强制零数据保留 + 不训练。命中时只路由到支持 ZDR 的上游；若模型没有 ZDR 上游则**返回 422 `cmd_zdr_no_providers`**，不会回退到非 ZDR 供应商。开启后可能改变上游计费。`typesafe/jev` 无 ZDR 上游，带该头请求即 422。Anthropic 模型在 `/messages` 上本来账号级 ZDR，该头被接受但不影响路由。
- 带 `x-cmd-zdr: 1` 时只接受客户端自执行的工具（`function`、`custom`、`local_shell`），其它一律拒绝。
- CLI 侧等价开关是 `CMD_ZDR=1`。

### 额度与计划限制

- **Go 计划没有 API 权限**——调用返回 **403 `upgrade_required`**。GOAT / Pro / Max / Team / Provider 有 API 访问。
- GOAT $10/月给 $70 credits（7×），叠加 deals 可超 $100。Provider 计划 $15/月 + $1.01 处理费，$15 起用，按量付费、无加价、credits 不过期。
- 编码计划（GOAT/Pro/Max/Team）的 API 用量从计划 credits 里扣；**Provider 计划无 usage window/limit**。
- 429 = 被上游限流，需退避重试；5xx 会带上游错误信息。

### 错误信封（两种）

OpenAI 侧：`{"error":{"message":…,"type":"invalid_request_error|authentication_error|permission_error|rate_limit_error|server_error","code":…,"param":…}}`
Anthropic 侧：`{"type":"error","error":{"type":"invalid_request_error|authentication_error|permission_error|rate_limit_error|api_error","message":…}}`

常见码：400 `unsupported_model`（模型不在目录，仅 OpenAI 信封有 `code`）、400 `invalid_request_error`（端点/模型不匹配）、401、403 `upgrade_required`、422 `cmd_zdr_no_providers`、429、5xx。

---

## 3. opencode（OpenCode Console / Go inference）

来源：
- Go <https://opencode.ai/v2/docs/console/go/>
- Console Inference API <https://opencode.ai/v2/docs/console/api/inference/>
- HTTP API <https://opencode.ai/v2/docs/api>（这是本地 OpenCode 实例的 142 个操作的 OpenAPI，**不是推理 API**——别混用）

### base_url

两个独立面：
- Console inference：`https://opencode.ai/inference`
- Go 订阅：`https://opencode.ai/inference/go`（`/models` 为 `https://opencode.ai/inference/go/v1/models`）

| API | Console 路径 | Go 路径 |
|---|---|---|
| OpenAI Chat Completions | `/inference/openai/v1/chat/completions` | `/inference/go/v1/chat/completions` |
| OpenAI Responses | `/inference/openai/v1/responses` | `/inference/go/v1/responses` |
| Anthropic Messages | `/inference/anthropic/v1/messages` | `/inference/go/v1/messages` |
| Gemini | `/inference/google/v1beta/models/<model>:generateContent` | `/inference/go/v1/...`（Go 表未列 Gemini）|

注意路径里带 **协议族前缀**（`/openai`、`/anthropic`、`/google`），与多数供应商只换后缀不同。

### 认证

- `Authorization: Bearer <token>`，token 是 Console 里的 service account key。
- **免费 chat 模型可以不带该头调用**；付费模型必须带。
- 模型列表：`GET https://opencode.ai/inference/v1/models`；Go 侧 `GET https://opencode.ai/inference/go/v1/models`。
- **[票 14 实测更正，2026-10-14]** 凭据**按端点面分家**，不是「一把 Bearer 通吃」：同一个 Go key 打 `/zen/go/v1/chat/completions` 正常 200，打 `/zen/go/v1/messages` 返回 **401 `Go requires a credential`**，打 Console 面 `/zen/v1/messages` 返回 **401 `Missing API key`**。即 Go 计划的 Anthropic 端点要的是另一种凭据，拿不到就只能走 Chat 端点。

### 模型与端点分派（Go 计划，节选自官方表）

`opencode-go/<model-id>` 是 OpenCode 内部引用格式。

| 模型 ID | 端点族 | AI SDK 包 |
|---|---|---|
| `grok-4.7`、`grok-4.6`、`gpt-6-luna`、`gpt-5.6-luna`、`muse-spark-1.3-contributor`、`muse-spark-1.2-contributor` | `/v1/responses` | `@ai-sdk/openai` |
| `claude-haiku-5-5`、`minimax-m3`、`minimax-m2.7`、`qwen3.8-max`、`qwen3.8-flash`、`qwen3.7-plus` | `/v1/messages` | `@ai-sdk/anthropic` |
| `glm-5.3-flash`、`glm-5.3`、`glm-5.2`、`kimi-k3`、`kimi-k2.7-code`、`kimi-k2.6`、`longcat-2.0`、`deepseek-v4.1-flash`、`deepseek-v4-pro`、`deepseek-v4-flash`、`deepseek-v4-flash-vision-exp`、`mimo-*`、`hy4-preview`、`hy3`、`space-bunny`、`longcat-2.5-preview-free`、`step-5-preview-free`、`cat-jam-free` | `/v1/chat/completions` | `@ai-sdk/openai-compatible` |

**关键约束**：每个模型只由一个 API family 提供服务。发错端点会失败——网关必须按模型 ID 路由到对应协议，不能只配一个默认端点。

### 特殊要求（Go 计划特有，非标准）

1. 客户端要发**自己的 UA**，形如 `my-coding-agent/1.0`，不能是通用 SDK / HTTP 库名。
2. 每段对话要发稳定的 **`x-opencode-session`** 会话 ID，用于路由与 prompt 缓存优化。
3. 流量被监控是否有"损害其他用户体验"的滥用。
4. 官方维护「已验证客户端」与「已知有问题客户端」表。**DeepSeek Harness 列在已知问题一侧**：部分模型路径会带上会话信息，其他路径缺失（[Discussion #5495](https://github.com/deepseek-ai/deepseek-harness/discussions/5495)）。其他缺会话头的：GitHub Copilot Chat、Kimi Code、MiMo Code。Claude Code / Codex / ZCode 会被识别其原生会话头。
5. Go/Go Plus **只允许单成员 workspace**（Personal），有 Go 订阅的 workspace 不能邀请成员。

### 额度与订阅限制

- Go $10/月，Go Plus $40/月。
- 用量以**月度美元额度**计，**按模型分别限量**（如 GLM-5.3-Flash $60、GLM-5.3 $15、Kimi K3 $15、Qwen3.8 Max $15、DeepSeek V4 Pro $15、DeepSeek V4.1 Flash $80…）。
- 每个模型：5 小时窗口 = 月额度 20%，周 = 50%，月 = 100%。
- 达到限额后：若 Console 余额开了 **Use balance**，则回退到 pay-as-you-go；否则被阻塞（免费模型仍可用）。
- Go 表里各模型 token 单价与 Console 直连价一致；部分模型有 cached write 价。

### 隐私/留存（影响能否做 ZDR 承诺）

Grok / GPT / Claude Haiku 5.5 留存 30 天；多数开源模型 0 天；Muse Spark 1.3/1.2 Contributor 与 Cat Jam Free 会被用于训练。DeepSeek 系 ZDR 协议按月续签，当前有效期至 2026-10-31。

---

## 4. 千问 Qwen（platform.qianwenai.com / 千问AI平台）

来源：
- Token Plan 个人版快速开始 <https://platform.qianwenai.com/docs/token-plan/personal/token-plan-personal-quickstart>
- Anthropic Messages API 参考 <https://platform.qianwenai.com/docs/api-reference/chat/anthropic>
- OpenAI Chat API 参考 <https://platform.qianwenai.com/docs/api-reference/chat/openai-chat>
- OpenAI Responses API 参考 <https://platform.qianwenai.com/docs/api-reference/chat/openai-responses>

> 这几页是 Docusaurus 风格的 SSR HTML（体量大、`web_fetch` 会在正文前截断），实际正文由 `Invoke-WebRequest` 去标签后取到。

### base_url（注意域名与两套路径体系）

| 场景 | 协议 | Base URL |
|---|---|---|
| 平台（按量） | OpenAI 兼容 | `https://maas.qianwenaiapi.com/compatible-mode/v1` |
| 平台（按量） | Anthropic 兼容 | `https://maas.qianwenaiapi.com/apps/anthropic` |
| Token Plan 个人版 | OpenAI 兼容 | `https://token-plan.maas.qianwenaiapi.com/compatible-mode/v1` |
| Token Plan 个人版 | Anthropic 兼容 | `https://token-plan.maas.qianwenaiapi.com/apps/anthropic` |

- 文档域名是 `platform.qianwenai.com`，**API 域名是 `maas.qianwenaiapi.com`**（Token Plan 加前缀 `token-plan.`）。
- Anthropic 端点实际路径：`POST /apps/anthropic/v1/messages`。
- OpenAI Chat 端点：`POST /compatible-mode/v1/chat/completions`。
- Responses 端点：`POST /compatible-mode/v1/responses`。**旧路径 `/api/v2/apps/protocols/compatible-mode/v1/responses` 已停止维护**，网关迁移时不要用旧路径。

**`/v1` 陷阱（官方 FAQ 明写）**：Claude Desktop / Claude Code 等客户端会自动在 base URL 后追加 `/v1/models`。所以：
- base URL **不要以 `/v1/` 结尾**，Anthropic 侧应以 `/apps/anthropic` 结尾；误填 `.../apps/anthropic/v1/` 会产生 `/v1/v1/models` 并 404。
- Anthropic 兼容端点**只提供 Messages API（`/v1/messages`），不提供 `/v1/models`**——模型发现请求必然 404，客户端必须手动添加模型名跳过自动发现。这点直接影响网关设计：网关若要给 Claude 系客户端做 Anthropic 入站，应自己实现 `/v1/models`。

### 认证

- OpenAI 侧：`Authorization: Bearer <API Key>`（必填）。
- Anthropic 侧：`x-api-key: <API Key>` **或** `Authorization: Bearer`，二选一。
- **两套 Key 不可混用**：Token Plan 订阅生成的 key 以 **`sk-sp-`** 开头，用于模型调用、按 Credits 抵扣；千问AI平台 API Key 以 **`sk-`** 开头，用于 Harness 权益工具（联网搜索增强版等）。Token Plan 的 key 仅在生成时完整显示一次。

### 模型

- Anthropic 端点 `model` 支持范围（官方逐项列出）：千问Max `qwen3.8-max`、`qwen3.8-max-0902`、`qwen3.7-max`(+快照)；千问Plus `qwen3.7-plus`(+快照)、`qwen3.6-plus`、`qwen3.5-plus`、`qwen-plus`、`qwen-plus-latest`；千问Flash `qwen3.8-flash`、`qwen3.7-flash`、`qwen3.6-flash`、`qwen3.5-flash`、`qwen-flash`；Turbo `qwen-turbo-latest`；Coder `qwen3-coder-flash`；VL `qwen3-vl-plus`；开源 `qwen3.6-27b`、`qwen3.5-397b-a17b`、`qwen3.5-122b-a10b`、`qwen3.5-27b`、`qwen3.5-35b-a3b`、`qwen3.8-2.4t-a95b`、`qwen3.8-27b`；第三方 `deepseek-v4.1-flash`、`deepseek-v4-pro-0813`、`deepseek-v4-pro`、`deepseek-v4-flash-0731`、`kimi-k3`、`kimi-k2.7-code`、`kimi-k2.6`、`kimi-k2.5`、`glm-5.3`、`glm-5.2`、`glm-5.1`、`glm-5`、`MiniMax-M2.5`。
- Responses 端点 `model` 列表更短（官方列举）：`qwen3.8-max`、`qwen3.8-max-0902`、`qwen3.8-flash`、`qwen3.8-2.4t-a95b`、`qwen3.8-27b`、`qwen3.8-omni-flash`、`qwen3.7-max`(+快照/`-preview`)、`qwen3.7-plus`、`qwen3.6-plus`、`qwen3.5-plus`、`qwen3.7-flash`、`qwen3.6-flash`、`qwen3.5-flash`、`qwen3.6-35b-a3b`、`qwen3.5-397b-a17b`、`qwen3.5-122b-a10b`、`qwen3.5-27b`、`qwen3.5-35b-a3b`、`deepseek-v4.1-flash`、`deepseek-v4-pro-0813`、`deepseek-v4-pro`、`deepseek-v4-flash-0731`、`glm-5.3`、`glm-5.2`、`kimi-k3`。
- OpenAI Chat 端点覆盖面最广（含 Qwen-VL / Qwen-Omni / Qwen-Math / Qwen-Coder 及三方直供 DeepSeek、Kimi、GLM、MiniMax）。
- **上下文长度：官方这几页未给逐模型数值**，需查「模型市场」/模型列表页。⚠️ 未证实。
- **视觉**：`qwen3-vl-plus`、`qwen3.8-omni-flash` 等 VL/Omni 模型支持；OpenAI Chat 侧有 `vl_high_resolution_images` 参数控制高分辨率图像。
- **工具调用**：三个端点都支持 function call（Anthropic 侧 `tools[].name` / `input_schema` / `description`，`tool_choice` 支持 `auto|any|none|tool`）。
- `Qwen-Audio` **不支持 OpenAI 兼容协议**，仅 DashScope 协议。

### 流式与推理字段差异（重点）

**OpenAI Chat 侧**
- 思考内容在 **`reasoning_content`** 字段返回。
- 开关参数 `enable_thinking`（布尔，**非标准 OpenAI 参数**，Python SDK 必须走 `extra_body`）。允许的模型：Qwen3.7/3.6/3.5/3、Qwen3-Omni-Flash、Qwen3-VL，以及 deepseek-v4-pro/flash（阿里云直供）、Kimi-K2.6/K2.5、GLM 系列。`qwen3.8-max` / `qwen3.8-max-0902` **默认开启**思考；DeepSeek-V4 系列默认开启，用 `reasoning_effort` 调力度。
- `preserve_thinking`（布尔，默认 `false`，**非标准**）：是否把历史 assistant 消息的 `reasoning_content` 拼进模型输入。**`qwen3.8-max` 默认为 `true`，且必须完整回传历史 `reasoning_content`**；官方明确「不支持将 `reasoning_content` 拼接到 `content` 字段中回传」。开启后历史 `reasoning_content` 会计入输入 token 并计费。
- `thinking_budget`（整数，**非标准**）：思考最大 token 数；`kimi-k3` 不支持。
- `reasoning_effort`（字符串，默认 `"high"`）：**各模型取值域不同，且彼此不兼容**——
  - Qwen3.8 系列：`low|medium|xhigh`，默认 `xhigh`；**与 `thinking_budget` 互斥，同时设置会报错**；两者可互转（`low→4096`、`medium→16384`、`xhigh→262144`；反查 0-4096→low、4097-16384→medium、16385-262144→xhigh），都不设时用默认 `thinking_budget=131072` / `xhigh`。
  - DeepSeek-V4 与 GLM：`high|max`；`low`/`medium→high`，`xhigh→max`；`glm-5.3` 系列默认 `max`。
  - `deepseek-v4-flash-0731`、`deepseek-v4-pro-0813`：`max|high|low`，默认 `high`；`medium`/`xhigh→high`。
  - `deepseek-v4.1-flash`：`max|high|low`，默认 `high`；`minimal→low`，`medium`/`xhigh→high`，`ultra→max`。
  - `qwen3.8-omni-flash`：默认 `xhigh`；Chat Completions 接受 `none|minimal|low|medium|high|xhigh|max`，不能与 `thinking_budget` 同时设置。
  - `glm-5.3` 系列**始终开启思考**，`enable_thinking` 只接受 `true`，传 `false` **会导致请求失败**；`kimi-k3` 可以传 `false` 关闭。
- `clear_thinking`（**非标准**）：仅 GLM 系列（glm-5.3/5.2/5.1/5）支持，控制是否把历史 `reasoning_content` 作为上下文。
- `n`：1-4，仅 Qwen3 非思考模式；传 `tools` 时必须为 1。
- Usage 里带 `completion_tokens_details.reasoning_tokens`、`prompt_tokens_details.cached_tokens`（与 OpenAI 命名一致）。

**Responses 侧**
- 思考强度用 **`reasoning.effort`**；`enable_thinking` 仍可用但**官方标注「已不推荐」**。
- **扩展参数**：README 明说支持 OpenAI 规范之外的参数。
- **上下文限制**：为给内置工具与推理留空间，Responses API 最大输入上下文约为模型窗口的 **80%**（预留约 20%），超出**自动截断且不报错**——与 DeepSeek 的「超限 400」相反，网关做上下文预算时要注意。
- `previous_response_id` + header **`x-dashscope-session-cache: enable|disable`**（默认 `disable`）控制会话缓存：缓存创建按标准输入价 125% 计费、命中按 10%，有效期 5 分钟（命中后重置），**至少 1024 token 才创建缓存**；只做**精确匹配**，不支持前缀匹配与语义匹配。支持的模型为 qwen3.8-max/flash、qwen3.7-max/plus、qwen3.6-plus、qwen3.5-plus、qwen3.7/flash 等。SDK 传法：Python `default_headers`，Node.js `defaultHeaders`。
- 响应 usage 里出现过 **`x_billing_type: "response_api"`** 这种非标准字段。
- 不支持 `background`（仅同步调用）。

**Anthropic 侧**
- 内容块：`text`、`thinking`、`tool_use`。`thinking` 块带 `signature` 字段，**当前固定为空字符串**（不要把它当真实签名做校验）。
- `stop_reason` 只有 `end_turn` / `max_tokens` / `tool_use`；`stop_sequence` 字段**固定为 `null`**（即使命中 `stop_sequences`）。
- 流式：`message_start` 的 `usage` **只有** `input_tokens` 与 `output_tokens`；**完整的 4 个字段（含 `cache_creation_input_tokens`、`cache_read_input_tokens`）在 `message_delta` 事件中返回**。
- `temperature` 取值范围是 **[0, 2)**，**与 Anthropic 官方 [0.0, 1.0] 不同**——从 Anthropic 迁过来必须重新确认取值。`qwen3.8-max` 思考模式默认 0.6，传入小于 0.6 会被自动抬到 0.6。
- `max_tokens` 必填，且语义按模型不同：对 `deepseek-v4.1-flash`、`deepseek-v4-pro*`、`qwen3.8-max` 等，它是**回复 + 思维链的合计上限**（开启思考时须 `max_tokens > thinking.budget_tokens`）；对 `glm-5.3` 也是合计且**忽略 `thinking.budget_tokens`**；对 `glm-5.2` 传了 `budget_tokens` 时才只限回复；其他模型 `max_tokens` 不限思考长度。
- `thinking.budget_tokens` **即将废弃**，官方建议新接入改用 `output_config.effort`。`output_config.effort` 取值按模型不同（glm-5.3：`low|high|max` 默认 `max`；glm-5.2/deepseek-v4-pro/deepseek-v4-flash-0731：`high|max` 默认 `max`；qwen3.8-max：`low|medium|xhigh` 默认 `xhigh`，`max`/`high` 映射为 `xhigh`）。
- `output_config.format` 结构化输出：`qwen3.8`/`qwen3.7`/`deepseek`/`glm` 系列是**严格** schema 约束；其他模型降级为普通 JSON 模式，且**要求 `system` 或 `messages` 里出现关键词 "JSON"（不区分大小写）**，否则报 `'messages' must contain the word 'json' in some form`。
- `system` 只接受顶层参数，`messages` 数组里**不接受 `system` 角色**。

### Token Plan 额度/订阅限制

- 同一实名认证主体**限购一份**个人版；个人版与团队版可同时持有，各自独立计费。
- 套餐分档位与订阅周期；按 **Credits** 抵扣。
- Standard 与 Pro 套餐附赠 Harness 权益工具（联网搜索增强版、图像生成等），**不占 Credits**，通过千问AI平台 MCP 商店的 One Key MCP 接入。
- 多模态生成模型（`wan2.7-image`、`happyhorse-1.1-t2v` 等）走**专用端点**，不能走上面的对话 base URL。
- 具体限流阈值：**官方文档这几页未给出**。⚠️ 未证实。

---

## 5. models.dev 数据源

来源：
- 站点 <https://models.dev/providers/>、<https://models.dev/models/>
- 官方 README <https://raw.githubusercontent.com/anomalyco/models.dev/dev/README.md>
- 原始数据 <https://models.dev/api.json>（本次抓取 5,614,961 字节）

### 数据从哪来

- models.dev 是**社区贡献的开源数据库**，作者是 SST 维护者，同时被 **opencode 内部使用**。仓库：<https://github.com/anomalyco/models.dev>（`sst/models.dev` 会跳转到它）。
- 站点与 API 都是**生成物**，源头是仓库里的 **TOML 文件**：
  - 供应商级：`providers/<provider-id>/provider.toml`
  - 供应商侧模型：`providers/<provider-id>/models/<model-id>.toml`
  - **模型本体（与供应商无关）的元数据**：`models/<lab>/<model-id>.toml`（如 `models/openai/gpt-5.toml`）
  - Logo：`providers/<id>/logo.svg`（或 `curl https://models.dev/logos/{provider}.svg`）
- 供应商 TOML 可用 `base_model = "openai/gpt-5"` 继承模型元数据，**provider 字段覆盖 model 元数据**；被继承的模型会在产物里带 `canonical_model_id`（如 `anthropic/claude-haiku-4-5`）。
- 更新方式：向仓库提 PR，GitHub Action 校验 schema。

### 可用的 API 端点

```bash
curl https://models.dev/api.json                 # 供应商 + 供应商侧模型（默认省略 specialized 类型）
curl "https://models.dev/api.json?type=all"      # 全量；type=decision 只要决策模型
curl https://models.dev/models.json              # 仅与供应商无关的模型元数据
curl https://models.dev/catalog.json             # 两者合一
curl https://models.dev/model-schema.json
```
`type` 参数同时支持 `api.json` / `models.json` / `catalog.json` / `model-schema.json`。当前支持的 specialized 类型只有 `decision`。**模型 ID 即 AI SDK 的 identifier**，可用于跨供应商查同一模型。

### 实际抓取的顶层结构

- 顶层就是一个**以 provider id 为键的大对象**（不是数组），**没有** version / generatedAt 之类的元数据字段。
- 本次快照：**228 个 provider**，合计 **8804 个 model**。
- provider 字段全集（读遍 228 个 key 得到）：`id`、`env`、`npm`、`api`、`name`、`doc`、`models`。
- provider 侧**没有** `apiTypes` 之类的多协议数组字段：**一个 provider 只有一个可选的 `api` 字符串**。25 个 provider 没有 `api`；`doc` 全部齐全。
- model 字段全集（抽样统计）：`id`、`name`、`description`、`family`、`attachment`、`reasoning`、`reasoning_options`、`tool_call`、`interleaved`、`structured_output`、`temperature`、`knowledge`、`release_date`、`last_updated`、`modalities`、`open_weights`、`limit`、`status`、`cost`、`canonical_model_id`，另偶见 `experimental`、`provider`。
- `status` 取值仅 `alpha` / `beta` / `deprecated`。
- `interleaved` 取值：布尔 `true`，或对象 `{field:"reasoning_content"}` / `{field:"reasoning_details"}`——⚠️ 注意这里是 **camelCase `reasoning_details`**，与 OpenAI 系常见的 `reasoning_content` 是两种不同形态。
- `reasoning_options` 是数组，元素形如 `{type:"toggle"}`、`{type:"budget_tokens", min:1024}`、`{type:"effort", values:[...]}`；实测 `effort.values` 有 **60+ 种不同组合**（从 `["high"]` 到 `["none","default","low","medium","high"]` 等），**没有统一枚举**，不能拿来做硬编码映射表。

### 精简示例 JSON（结构真实、字段已裁剪）

顶层（provider 键 → 对象）：

```json
{
  "deepseek": {
    "id": "deepseek",
    "name": "DeepSeek",
    "env": ["DEEPSEEK_API_KEY"],
    "npm": "@ai-sdk/openai-compatible",
    "api": "https://api.deepseek.com",
    "doc": "https://api-docs.deepseek.com/quick_start/pricing",
    "models": { "…": "…" }
  },
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "env": ["ANTHROPIC_API_KEY"],
    "npm": "@ai-sdk/anthropic",
    "doc": "https://docs.anthropic.com/en/docs/about-claude/models",
    "models": { "…": "…" }
  }
}
```

注意 `anthropic` 条目里**没有 `api` 键**（第一方 SDK provider 不需要 base URL）。`env` 是**字符串数组**（如 `google` 为 `["GOOGLE_API_KEY","GOOGLE_GENERATIVE_AI_API_KEY","GEMINI_API_KEY"]`）。

单个 model（取真实条目裁剪）：

```json
{
  "id": "claude-haiku-4-5",
  "name": "Claude Haiku 4.5 (latest)",
  "description": "Fast Claude lane for lightweight agents, office tasks, and responsive chat",
  "family": "claude-haiku",
  "attachment": true,
  "reasoning": true,
  "reasoning_options": [{ "type": "budget_tokens", "min": 1024 }],
  "tool_call": true,
  "structured_output": true,
  "temperature": true,
  "knowledge": "2025-02-28",
  "release_date": "2025-10-15",
  "last_updated": "2025-10-15",
  "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
  "open_weights": false,
  "limit": { "context": 200000, "output": 64000 },
  "cost": { "input": 1, "output": 5, "cache_read": 0.1, "cache_write": 1.25 },
  "canonical_model_id": "anthropic/claude-haiku-4-5"
}
```

对比 DeepSeek 系带 `interleaved` 的例子：

```json
{
  "id": "deepseek-v4-flash-vision-exp",
  "family": "deepseek-flash",
  "reasoning": true,
  "reasoning_options": [{ "type": "toggle" }, { "type": "effort", "values": ["low", "high", "max"] }],
  "interleaved": { "field": "reasoning_content" },
  "modalities": { "input": ["text", "image"], "output": ["text"] },
  "limit": { "context": 1000000, "output": 393216 },
  "status": "deprecated",
  "cost": { "input": 0.15, "output": 0.6, "reasoning": 0.6, "cache_read": 0.003 },
  "canonical_model_id": "deepseek/deepseek-v4.1-flash"
}
```

单位：`cost.*` 是**每百万 token 的美元价**；`limit.context` / `limit.output` 是 token 数。

### 「API 类型」标注能否作为网关预设来源

`npm` 字段就是「API 类型」标注，实测 228 个 provider 的分布：

| npm（AI SDK 包） | provider 数 | 含义 |
|---|---|---|
| `@ai-sdk/openai-compatible` | 188 | OpenAI 兼容（**这类必须有 `api`**） |
| `@ai-sdk/anthropic` | 8 | Anthropic Messages |
| `@ai-sdk/openai` | 6 | OpenAI 第一方 |
| `@ai-sdk/azure` | 2 | Azure OpenAI |
| `@openrouter/ai-sdk-provider` | 2 | OpenRouter |
| `@ai-sdk/google` / `@ai-sdk/google-vertex` / `@ai-sdk/google-vertex/anthropic` | 各 1 | Google / Vertex |
| `@ai-sdk/amazon-bedrock`、`@ai-sdk/mistral`、`@ai-sdk/groq`、`@ai-sdk/xai`、`@ai-sdk/cohere`、`@ai-sdk/deepinfra`、`@ai-sdk/cerebras`、`@ai-sdk/perplexity`、`@ai-sdk/togetherai`、`@ai-sdk/gateway` | 各 1 | 各家第一方 |
| 其它第三方 provider 包 | 各 1 | 长尾 |

**结论：可以当"预设候选"，但不能当唯一真相。**

能用的部分：
1. `npm` + `api` + `env` + `doc` + 模型能力字段（`reasoning`/`tool_call`/`modalities`/`limit`/`cost`/`reasoning_options`/`interleaved`）足以**自动生成绝大多数网关预设**：188/228 是纯 OpenAI 兼容，直接据此拼 base_url 即可。
2. `interleaved.field` 直接告诉你该供应商的思考内容字段是 `reasoning_content` 还是 `reasoning_details`，正好是网关归一化的关键信息。
3. `reasoning_options` 给出该模型的力度枚举，`cost` 用于计费，`env` 用于提示 key 环境变量名。

不能用的部分（必须在网关侧补正）：
1. **`npm` 是 AI SDK 包名，不是协议声明。** 例如 `@ai-sdk/openai-compatible` 只说明「该 provider 有一个 OpenAI 兼容 base URL」，**不排除它还支持 Anthropic 或 Gemini 端点**。实测反例：
   - `opencode-go` 标 `@ai-sdk/openai-compatible` / `api = https://opencode.ai/inference/go/v1`，但官方文档明确同一 base 同时提供 `/chat/completions`、`/responses`、`/messages`，且**多数模型只属于其中一个 family**。
   - `deepseek` 标 `@ai-sdk/openai-compatible` / `api = https://api.deepseek.com`，其 `provider.toml` 注释里却写明 Anthropic Messages 在**另一个路径** `/anthropic/v1/messages`。**单 `api` 字段无法表达"一个供应商两个 base URL/两种协议"**。
   - `minimax` 标 `@ai-sdk/anthropic` 且 `api = https://api.minimax.io/anthropic/v1`（注意：Anthropic 类 provider **也带 `api`**，所以「有 `api` = OpenAI 兼容」这条经验规则不成立）。
2. **25 个 provider 完全没有 `api`**（`anthropic`、`openai`、`google`、`google-vertex`、`google-vertex-anthropic`、`amazon-bedrock`、`azure`、`azure-cognitive-services`、`mistral`、`xai`、`groq`、`cerebras`、`cohere`、`deepinfra`、`perplexity`、`togetherai`、`watsonx`、`gitlab`、`vercel`、`cloudflare-ai-gateway`、`aihubmix`、`venice`、`salad-cloud`、`sap-ai-core`、`qvac`），这些必须由网关自己对 base URL 打表。
3. **端点级的支持矩阵拿不到。** models.dev 没有 `supported_endpoints` 字段（对比 commandcode 的 `/provider/v1/models` 有该字段）。「哪些模型能走哪个协议」只能额外查各家 `/models` 接口。
4. **认证头形态拿不到**（`x-api-key` vs `Bearer` 混用、付费/免费是否要 key），只有 `env` 变量名。
5. `reasoning_options` 枚举无统一标准（60+ 种组合），**不能**做机械映射；且它是「被记录的力度值」而非供应商文档的一等事实。
6. 数据是社区维护的，会滞后（例：`deepseek` 条目里 `deepseek-v4-flash-vision-exp` 标记为 `deprecated`，与实际一致；但 `cost`/`limit` 的准确性没有 SLA）。**不要把 models.dev 当定价或容量的权威来源**——定价以各供应商官方价格页为准。

**建议的网关用法**：把 models.dev 当「provider + model 元数据的种子」与「协议类别的先验」，用 `npm` 决定默认协议适配器；同时维护一张**本地 overrides 表**修正 base URL（尤其 Anthropic 路径）、认证头、端点级支持矩阵与限流/ZDR 策略。

---

## 6. 未证实 / 待补

1. 千问 Qwen 各模型的**上下文长度与最大输出**（官方 API 参考页未给，需查模型市场页）。
2. 千问 Qwen 平台的**限流阈值**（文档未给具体数字）。
3. `commandcode` 的 `/docs/byok` 与 `/docs/reference/cli/models` 正文（仅取到导航骨架）；完整模型目录需登录或换抓取方式。
4. `commandcode` 各模型是否支持视觉/推理的**逐模型矩阵**——官方 FAQ 说「不做前置门控，由上游返回自己的错误」，故只能靠 `GET /provider/v1/models` 的 `supported_endpoints` 等字段动态判断。
5. `opencode` Go 表的 Gemini 路径在 Go 端点下是否可用（Go 端点表未列 Gemini，Console 端点表有）。
6. DeepSeek 的 `/v1` 前缀是否被接受——实测边缘层对所有路径（含不存在的路径）统一返 401，**无法通过探测区分**，本文件按官方文档「不带 `/v1`」记录。
