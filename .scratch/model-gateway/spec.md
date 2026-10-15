# 本地模型网关（Claude Gateway）：技术选型与架构设计

状态：草案 v0.6（wayfinder 第一期，2026-10-14 移出订阅渠道账号接入；同日并入票 15 的 Claude Desktop 3P 实测定案（§8.1）、票 17 的前端信息架构定案（§10）、票 24 的登录态调研（§8.3）与票 23 的供应商表粒度定案（§6/§7）、票 25 的 Codex 实测结案（§8.3））
日期：2026-10-14
关联：[wayfinder map](map.md) ｜ [术语表](../../GLOSSARY.md) ｜ [调研报告](research/)

---

## 1. 目标与非目标

一句话：在本机跑一个网关，把第三方模型聚合成本地模型目录，对上游说各家方言，对客户端说标准协议，从而让 **Claude Desktop（3P 模式）、Claude Code、Codex** 用上第三方模型。

**目标**

1. 三个客户端都能通过本网关使用第三方模型，配置由网关自动写入与还原。
2. 上游覆盖：普通 `base_url + api key` 供应商。
3. 入站协议：Anthropic Messages、OpenAI Chat Completions、OpenAI Responses、Ollama 兼容；出站协议：前三者。
4. 本机单用户、桌面应用形态、可长期常驻托盘。

**非目标**

- 订阅制服务的账号接入（登录态、凭据池、浏览器自动化）：整体不做。相关调研已归档在 [research/archive/](research/archive/)。
- 多用户 / 多租户 / 计费与配额；云端部署；Docker 交付。
- Gemini 原生协议的编解码（只在中间表示上预留扩展点）。

---

## 2. 技术选型

| 维度 | 选型 | 理由 |
| --- | --- | --- |
| 桌面外壳 | Tauri 2（Windows）+ 系统托盘 | 与 cc-switch / CodexPlusPlus 同构；安装包小、常驻内存低 |
| 后端语言 | Rust（tokio + axum + reqwest 流式） | 单进程常驻、SSE 转发稳定、无运行时依赖，便于打包 |
| 前端 | React + TypeScript + Vite | 只做外观与交互，经本地 HTTP/SSE 与后端通信 |
| 存储 | SQLite（配置 / 日志 / 用量同库）+ JSON 导出导入 | 便于查询、备份与版本化 |
| 密钥 | Windows DPAPI（`CryptProtectData`）+ 界面掩码 | 本机资产，不需要主密码 |
| 协议转换 | 自研薄转换层 + 中间表示（IR） | 五个候选封装方案全部出局，见 [build-vs-wrap](research/build-vs-wrap.md) |
| 配置写入 | 自研写入引擎，借鉴 cc-switch(MIT) 的「只改关键字段」做法 | 幂等、可备份、可还原，不破坏用户其他设置 |
| 测试 | 离线 fixture 回归（录制的上游 SSE 帧）+ 端到端冒烟 | 上游方言差异只能靠录制帧锁住 |

**不选现成网关的理由**（证据见 [build-vs-wrap](research/build-vs-wrap.md)）

- LiteLLM：协议能力最全，但 Python + 39 项代理依赖 + Prisma，安装包 300–600MB；且 `proxy` extra 钉了 proprietary 的 `litellm-enterprise`。形态与单机桌面错配。
- new-api：技术形态最合（单 Go 二进制 + 内嵌前端 + 托盘 + 路径映射），但 AGPL-3.0 且带附加署名条款，闭源分发不通。
- one-api：能力是 new-api 的真子集，且停滞 20 个月，还硬覆盖 `anthropic-beta`。
- claude-code-router（[musistudio/claude-code-router](https://github.com/musistudio/claude-code-router)，MIT）：客户端半边最好（配置写入 / 备份 / 还原 / Codex catalog / 四个家族名环境变量都是现成答案）；它的**仓库本身不含协议转换主体**——转换由 ccr 启动的外部包 `@the-next-ai/ai-gateway` 承担，当初的「黑盒」指的就是这一点。该外部包**有公开 MIT 源码**（[The-NeXT-AI/ai-gateway](https://github.com/The-NeXT-AI/ai-gateway)，与 npm 1.0.21 版本对齐），所以「黑盒」不成立，真正的出局理由是形态（Node/Fastify 服务器 + Redis/Docker/计费/MCP/插件，11.8 万行）与保真缺口（无 `count_tokens`、几乎没有 prompt caching）。它同时升级为**协议编解码的一手移植参照**，见 [research/ccr-gateway-package.md](research/ccr-gateway-package.md)。
- AIClient-2-API：GPL-3.0，一票否决。

---

## 3. 总体架构

```
客户端  Claude Desktop(3P) │ Claude Code │ Codex │ 其他 OpenAI/Anthropic 客户端 │ Ollama 客户端
                                   │  HTTP / SSE（127.0.0.1:8787）
                        ┌──────────▼──────────┐
                        │  入站适配层 ingress  │  /v1/messages、/v1/chat/completions、
                        │                     │  /v1/responses、/api/chat、/api/tags、
                        │                     │  /v1/models、/v1/messages/count_tokens
                        └──────────┬──────────┘
                                   │ 解码
                        ┌──────────▼──────────┐
                        │  中间表示 IR         │  Request / StreamEvent（唯一内部形状）
                        └──────────┬──────────┘
                                   │ 绑定与路由
                        ┌──────────▼──────────┐
                        │  绑定层 binding      │  家族槽位 / 虚拟模型 → (上游, 真实模型, wire)
                        │                     │  客户端档案覆盖
                        └──────────┬──────────┘
                                   │ 编码 + 取凭据
                        ┌──────────▼──────────┐
                        │  出站适配层 egress   │  AnthropicCodec / OpenAiChatCodec /
                        │                     │  OpenAiResponsesCodec + 声明式供应商表
                        └──────────┬──────────┘
                                   ▼
                        上游 HTTP(S) ｜ 可观测 obs
                        （日志 · 用量 · 成本）

旁路（不在请求路径上）：配置写入引擎 clients::writer → 三个客户端的配置文件
前端 React → 后端管理 API（/api/*），只读客户端状态、不参与转发
```

设计要点：

- **请求路径上只有转换，没有业务**；配置写入是旁路模块，转发路径不依赖它（否则客户端一改配置就把网关搞挂）。
- **所有协议转换经 IR**，绝不做「A 协议直转 B 协议」的两两实现（N 个协议从 N² 降到 2N，参考的几个程序都是这么做的）。
- 后端监听 `127.0.0.1`，端口默认 8787、可配置；启动时检测占用并给出明确报错（不静默换端口，因为地址已被写进客户端配置）。

---

## 4. 入站协议契约（必须做到的行为）

### 4.1 `POST /v1/messages`（Anthropic Messages）— 最高优先级

- 必须支持流式与非流式；流式事件顺序必须完整到 `message_delta` + `message_stop`，**不得缓冲**。
- 非流式（`stream:false` / 缺省）：**出站仍是 `stream:true`**，网关把同一条 IR 流缓冲到终态再组装完整 `message`（`content` 恒为数组、`stop_reason` 归一化、usage 取终态值）；**没有 signature 的 thinking 块丢弃**（带回去下一轮会被官方校验拒）；上游断流 / 无终态 → 502 JSON 错误信封。行为定案见[票 26](issues/26-grilling-count-tokens-and-non-streaming.md)。
- **必须原样转发 `anthropic-version` 与 `anthropic-beta`**，不做白名单化（Claude Code 依赖这一点）。
- 长静默必须转发 `ping`，否则客户端判空闲超时/连接中断。
- 认证：同时接受 `Authorization: Bearer`（Claude Code 的 `ANTHROPIC_AUTH_TOKEN`）与 `x-api-key`（`ANTHROPIC_API_KEY`）。
- 工具调用要完整支持 `tool_use` / `tool_result` 往返；**工具名未到不开块**，整轮无名则不宣布该块。

### 4.2 `POST /v1/responses`（OpenAI Responses）— Codex 的唯一合法出站

- Codex 的 `wire_api` 官方唯一合法值是 `responses`（填 `chat` 会让整份配置失效回退默认模型）。
- 增量 SSE，必须以 `response.completed` 结束；支持回放续轮（`previous_response_id` 或等价输入拼接）。
- `function_call` 与 `function_call_output` 必须成对；客户端固定发 `store:false`、`stream:true`、`include:["reasoning.encrypted_content"]`。
- **系统提示不在 `instructions` 里**：Codex 把它当 `input` 的 `role:"developer"` 消息发（实测，票 32），`developer` / `system` 必须归一化进 IR 的 system。
- **`reasoning` 项按 summary 回、不带 `encrypted_content`**（上游不是 OpenAI，伪造加密推理只会骗自己）；入站 `reasoning` 项一律丢弃。Codex 接受这种项（票 32 实测）。
- **续轮只做回放 `input`，不实现 `previous_response_id`**：Codex 固定 `store:false`，收到该字段当没看见（票 32 定案）。
- **不按 `x-openai-internal-codex-responses-lite` 分流**：一律回全量 Responses SSE，CLI 与桌面应用都过（票 32 实测）。
- **错误信封按入站协议给形状**：Responses 要 OpenAI 的 `{"error":{"message","type","code"}}`，Anthropic 那份 Codex 读不懂。
- 出站结构化输出走 `text.format`（**3 层，比 Chat 少一层 `json_schema` 包裹**；`name` 必填、`strict` 缺省 false），字段名互斥——Responses 没有 `response_format`、Chat 没有 `text`（[票 37](issues/37-grilling-structured-output-slot.md)）。

### 4.3 `POST /v1/chat/completions`（OpenAI Chat）

- 标准实现：流式 `data:` 帧 + `[DONE]`；注意 DeepSeek 的 usage 附在最后一个内容块、不单独发 chunk。
- 出站结构化输出走 `response_format`（**4 层**：`response_format` → `json_schema` → `{name, schema, strict}`）；`Json` 映射成 `{"type":"json_object"}`，此时上游仍会因「提示词里没出现 JSON」而 400——那条预检**不搬到网关**，原样透传上游错误（[票 37](issues/37-grilling-structured-output-slot.md)）。

### 4.4 Ollama 兼容（`/api/chat`、`/api/tags`）

给本机其他工具留的入口，形状按 Ollama 官方 API 走（`/api/chat`、`/api/tags` 的逐字段映射见 [research/ollama-inbound.md](research/ollama-inbound.md)，[票 34](issues/34-research-ollama-inbound.md)；三个辅助端点的逐字段契约见 [research/ollama-aux-endpoints.md](research/ollama-aux-endpoints.md)，[票 36](issues/36-grilling-ollama-endpoint-scope.md)），映射到 IR 后走同一条出站链路：

- **传输是 NDJSON，不是 SSE**：`application/x-ndjson`、一行一帧、无 `data:` 前缀、无 `[DONE]`。
- **终态是末帧 `done:true`**；末帧 `message.content` 是空串；`prompt_eval_count` / `eval_count` / `prompt_eval_cached_count` / `done_reason` 都在**帧顶层**，且 `done_reason` 带 omitempty、**可能整键缺失**。
- **`stream` 缺省为 true**（与 OpenAI Chat 面相反）；`stream:false` 回单个 JSON（`application/json`），实现上是把同一批流式帧缓冲到终态再组装（与 §4.1 的非流式同范式）。
- `message.content` 是**字符串**（不是 parts 数组）；`message.tool_calls[].function.arguments` 是**对象**（与 OpenAI Chat 的字符串相反），所以 Ollama 面**无法转发半个 JSON**——工具调用必须**缓冲到 `BlockStop` 一次性给出**。
- 工具结果回传方向：Ollama 原生客户端只发 `tool_name`，而 IR 的 `ToolResult` 只有 id → 入站必须**按名字反查前面 assistant 轮里的 `ToolUse.id`** 补上，否则出站到 Anthropic 会 400。
- 字段映射：`options.num_predict → max_tokens`、`options.stop` 接受 string|array、`prompt_eval_count → input_tokens`、`eval_count → output_tokens`、`prompt_eval_cached_count → cache_read_input_tokens`（`cache_creation` 无对应则略）。
- **错误信封是 `{"error":"<字符串>"}`**（不是 OpenAI 的嵌套形状）；流中出错：尚未写出任何帧 → 真实状态码 + `application/json`，已写出帧 → 追加一行 `{"error":…}`、状态码不变。
- **`GET /api/tags` 与 `GET /v1/models` 暴露同一集合**（四个家族槽位名 + 普通虚拟模型）；`size` 填 0、`digest` 取虚拟模型名的确定性 sha256、`details.*` 按「没有权重文件」的事实填空值（**不谎报 `format:"gguf"`**）。
- **`/v1/chat/completions` 不是第二个 Ollama 面**：它是 §4.3 的 OpenAI Chat 入站，与 `/api/chat` 共用同一条 IR 与出站链路，只有解码器与错误信封不同。
- **`/api/tags` 的 `name` 与 `model` 填同一个值**：都用**裸虚拟模型名**（不带 `:latest`），与 `GET /v1/models` 逐字一致；入站解析额外接受并剥掉 `:latest` 后缀（官方文档：tag 缺省即 latest）。`modified_at` 填该绑定的最后修改时间、无则网关启动时间（RFC3339 UTC）；上一行那些空值里，家族槽位的 `details.family` / `families` 填槽位名；`capabilities` 填 `["completion","tools"]`（四条出站链路都支持工具调用）。
- **装载 / 卸载本地回假终态，不打上游**：`messages: []` = 装载、`messages: []` + `keep_alive: 0` = 卸载 → 200 + `{model, created_at, message:{role:"assistant",content:""}, done_reason:"load"|"unload", done:true}`，统计字段一律不填（官方也因 `omitempty` 不填）；`stream:false` 时是同一个对象的单个 JSON。`messages` 非空时 `keep_alive` 一律忽略（网关不持有模型，它只影响驻留）。
- **同族辅助端点**（票 36 定案）：`GET /api/version` → 200 `{"version":"0.13.3"}`（唯一键；另有未文档化的 `HEAD` 一并实现。这个串等于声明「实现了到 0.13.3 为止的 API 契约」，semver 合法，且不影响任何上游调用）。`POST /api/show` → **只填能诚实填的键**：必给三件套 `details`（六个子键恒在：`parent_model` / `format` / `family` / `families` / `parameter_size` / `quantization_level`）、`model_info`（`{}`——源码点名 nil 会让集成崩溃）、`modified_at`（取该绑定的时间，不填零值以免界面显示 0001 年）；其余（`modelfile` / `parameters` / `template` / `license` / `system` / `renderer` / `parser` / `thinking` / `tensors` / `projector_info` / `requires` / `manifests`）整键省略；请求体用 `model`（`name` 只是已废弃的兜底），`verbose` 接受并忽略（网关永远没有 `tensors`）。`GET /api/ps` → 200 `{"models":[]}`（空数组、不是 `null`；官方在什么都没装载时就是这个形状）。三者的模型名都走**同一条绑定层解析**，解析不到统一 **404 `model '<name>' not found`**（逐字复用官方文案，工具的 show-then-pull 流程依赖它）；`/api/show` 另保留 400 `missing request body` / `model is required`。
- **其余 `/api/*` 一律不做**（`pull` / `create` / `copy` / `delete` / `blobs/*` / `generate` / `embed` / `embeddings`）→ 404。它们是「管理本地权重文件」与「非对话推理」两个面，网关不持有权重、也没有嵌入链路（已写进地图 Out of scope）。
- **`format`（JSON mode / JSON schema）**（[票 37](issues/37-grilling-structured-output-slot.md)）：入站解成 IR 的 `ResponseFormat`（裸 schema → `JsonSchema`；`"json"` → `Json`），随后按目标出站 wire 映射——Anthropic 是 `output_config.format`（3 层，无 `name`/`strict`/包裹，且递归补 `additionalProperties:false`）、Chat 是 `response_format.json_schema.{name,schema,strict}`（4 层）、Responses 是 `text.format.{name,schema,strict}`（3 层），OpenAI 两条的 `name` 一律填常量 `"response"`、`strict` 只透传客户端显式值。**表达不了就请求期 400**（`Json` 打到 Anthropic wire 就是这类：Anthropic 写不出「任意 JSON」），错误体用本节的 `{"error":"<字符串>"}` 信封；出站侧有没有真的生效由 §6 的能力位把关。**待实测（实现阶段）**：`done_reason` 的完整取值域（`length` 属推断）、流式帧里 `thinking` 的位置、结构化输出的**流式**行为（票 37 只实测了非流式）、`additionalProperties:false` 是否要求每个 object 层。

### 4.5 辅助端点

- `GET /v1/models`：**必须自己实现**，不能依赖上游（千问的 Anthropic 端点根本没有 `/v1/models`）。Claude 系客户端会把结果按「id 含 claude 或 anthropic」过滤，所以对 Claude 系客户端要暴露四个家族名；对 Codex 返回标准 `models` 数组。注意 Claude Desktop 在 `inferenceModels` 全填完整 ID 时**根本不会调它**（已实测，见 §8.1），它是给不填列表的客户端与 Codex 用的。
- `POST /v1/messages/count_tokens`：`wire=anthropic` 时转发上游 `<path_anthropic>/count_tokens`（形状 = `/v1/messages` 去掉 `max_tokens`/`stream`），**非 2xx / 连不上 / 10 秒超时 / 响应缺 `input_tokens` 全部降级本地估算**（端点永不失败）；本地估算 = system + tools + messages 全算、ASCII 4 字符/token、非 ASCII 每字符 1 token、base64 不计、图片常量 1500。定案见[票 26](issues/26-grilling-count-tokens-and-non-streaming.md)。
- 上游侧超时：**不做统一超时**（只有全局 300 秒）；`max_tokens <= 16` 的探针档加 **20 秒**上限，超时 504（实测 commandcode 的 Chat wire + deepseek 在 `max_tokens:1` 直连 curl 也挂住）。

---

## 5. 中间表示（IR）

```rust
struct IrRequest {
    system: Vec<Block>,          // 归一化后的系统提示（Anthropic 的多块 vs OpenAI 的单串）
    messages: Vec<IrMessage>,    // role + blocks
    tools: Vec<ToolDef>,
    tool_choice: ToolChoice,
    max_tokens: Option<u32>,
    temperature: Option<f32>,    // 注意：各家取值域不同，出站时按目标模型夹取
    thinking: Option<ThinkingReq>,
    format: Option<ResponseFormat>,   // 结构化输出（票 37）
    stream: bool,
}

enum Block { Text, Image, ToolUse{ id, name, input }, ToolResult{ id, content },
             Thinking{ text, signature }, CacheHint }

// 结构化输出：归一化，不原样透传（票 37）
enum ResponseFormat {
    Json,                                              // 无 schema 的 JSON mode
    JsonSchema { schema: Value, name: Option<String>, strict: Option<bool> },
    Other(Value),                                      // 没见过的形状：只在同形状的 wire 上原样带
}

enum StreamEvent { Start, BlockStart(Kind), TextDelta, ThinkingDelta,
                   ToolInputDelta, BlockStop, Usage(..), StopReason(..), Ping, End }
```

`format` 的出站映射（逐字形状，[票 37](issues/37-grilling-structured-output-slot.md)）；表达不了 → **请求期 400**，`Other` 只在**与入站同形状**的 wire 上原样带、其余 400：

| 出站 wire | 字段路径 | 层数 | `Json` 怎么走 |
| --- | --- | --- | --- |
| Anthropic | `output_config.format` → `{type:"json_schema", schema}` | 3（无 `name` / `strict` / `json_schema` 包裹，递归补 `additionalProperties:false`） | **400**（写不出「任意 JSON」） |
| OpenAI Chat | `response_format` → `json_schema` → `{name, schema, strict}` | 4（`name` 填常量 `"response"`，`strict` 只透传显式值） | `{"type":"json_object"}` |
| OpenAI Responses | `text.format` → `{name, schema, strict}` | 3（少一层 `json_schema` 包裹） | `{"type":"json_object"}` |

字段名互斥：Chat 没有 `text`、Responses 没有 `response_format`。出站到底有没有真生效由 §6 的能力位把关。

三条硬规则：

1. **`undefined`（没传）与 `null`（传了但不适用）必须分开**，混用会被下游的 `!== undefined` 判定吃掉修正（这条来自 deepseek-harness-codearts 的教训）。
2. **状态型信息（如 thinking 签名）跨协议时必须带版本号前缀**，还原侧按前缀白名单永久保留（cc-switch 的做法）。
3. 上游缺 `finish_reason` 时必须自己补一个终态，否则最后一个块不关、内容丢失（cc-switch 的线上 issue）。

---

## 6. 声明式供应商表与上游模型目录（不做 if-else 特例）

照 new-api `advancedcustom` 通道的形状，把「入站路径 → 上游路径」与协议差异全部数据化（粒度由[票 23](issues/23-design-provider-table-granularity.md) 定案）：

```jsonc
{
  "id": "qwen",
  "baseUrl": "https://platform.qianwenai.com/apps/anthropic",   // 注意：不能带 /v1
  "auth": {                                                      // 按 wire 各一份，缺省继承 default
    "default":   { "scheme": "bearer" },
    "anthropic": { "scheme": "x-api-key" }
  },
  "pathTemplates": {
    "anthropic":       "/v1/messages",
    "openaiChat":      "/v1/chat/completions",
    "openaiResponses": "/v1/responses"
  },
  "headerPassthrough": ["anthropic-version", "anthropic-beta"],
  "quirks": ["noModelsEndpoint", "temperatureMax2", "emptyThinkingSignature", "silentTruncate"]
}
```

上游模型目录（表 `upstream_models`）是第二张表：`upstream_id` + `model_id` + `supported_wires` + `source`。`supported_wires` 取值域**三个**（`anthropic` / `openai-chat` / `openai-responses`），**空集合表示未声明**（正常态，不等于不支持）；`source` 取值 `manual` / `upstream` / `doc`。上游 `/v1/models` 报的 `supported_endpoints` 是路径，按 `pathTemplates` **反查**成 wire，查不到的取值忽略，并在拉取结果里内联记一条警告（上游报的路径 + 我们期望的路径）；警告存进上游记录的「上次拉取警告」，上游卡片与列表显示角标，**下次成功拉取时自动清除**，不阻断保存、**不进概览「最近错误」**（那是请求错误的地盘）。拉取时机：上游向导「拉模型」抓一次存库，可手动重拉，请求期不实时查；**拉取用上游 `default` 凭据**（没有 `default` 时唯一一份即用它，多份则在拉取那一步下拉选），**不为每个 wire 面重复拉**（`/v1/models` 不属于任何一个 wire）。来源优先级 `manual`（锁定，重拉不覆盖）> `upstream` > `doc` > 未声明；models.dev 不参与 wire 判定。

**拉取的失败与空结果**（[票 35](issues/35-grilling-model-discovery-fallback.md)）：**404/405 = 该上游不提供模型发现**——软提示「请手填模型 ID」+ 手填入口，不算错误、卡片不报红（`noModelsEndpoint` quirk 的含义收窄为「不必尝试」，通用 404/405 规则兜住没声明过的上游）；**其他非 2xx（401/403/429/5xx）/ 超时 / 断网 = 拉取失败**，失败提示里直接给「手工添加模型」按钮，在向导同一页内联新增（条目按 `manual` 锁定），**上游仍可保存**（拉模型可跳过、只强制自检）。拉取**20 秒超时**（超时算失败）、**不自动重试**——401/403/429/404 立即返回，5xx/超时也只报一次，失败提示里给「重试」按钮由用户点（与票 26「不给上游转发加统一超时」不冲突：这是网关自己去拉一次清单）。**200 但零条**与「不提供模型发现」同一支处理。**重拉失败保留旧条目**，只提示「沿用上次结果（时间）」，只有成功拉取才覆盖 `upstream` 来源的条目。这两类提示都**不计入**侧栏「未通过自检的上游数」与概览三步就绪——判据只看自检。opencode 只拉带凭据的那份目录（19/85 的差异是权限过滤，未授权模型调用一律 403），不做匿名拉取。

`quirks` 只允许枚举值，新增一个 quirk 必须同时新增一条回归 fixture；不允许在转换代码里写供应商名判断。**「哪些协议面服务哪些模型」不进 quirks**——那是上游模型目录的职责（原 `ClaudeModelsOnAnthropicOnly` 已删除，见票 23）；`noModelsEndpoint` 是例外，它说的是**上游根本不提供模型发现端点**这个上游级事实，与「哪个协议面服务哪些模型」无关，只表示「不必尝试拉取、向导直接给手填入口」。

**结构化输出能力位**（[票 37](issues/37-grilling-structured-output-slot.md)）：粒度 = **上游 × 出站协议**，两态 `native`（缺省）/ `ignore`，另记 `source`（`manual` / `selfcheck`）。它存在的唯一理由是**状态码骗人**：实测 commandcode 的 Anthropic 兼容端点对 `output_config.format`（含旧名 ± beta 头、连伪造的 `response_format`）**6 个变体全 200 且逐字静默忽略**，而同一上游的 Chat wire 反而是真强制。判定靠自检，不靠状态码：**带 schema 的最短请求 + 敌意提示词**（「只说 hello，不要 JSON」），回答没被 schema 约束住就标 `ignore`。被标 `ignore` 时，客户端要结构化输出 → **请求期 400** 并说明原因。wire 级事实进表、不进 quirks（与上一段同一条口径）。

已录得的关键差异（详见 [providers.md](research/providers.md)）：DeepSeek 带 `tools` 时必须回传历史 `reasoning_content`；DeepSeek 流式 usage 不单独发 chunk；opencode 路径带协议族前缀且需要自定义 UA 与 `x-opencode-session`。commandcode 那种「逐模型分派端点」改由上游模型目录表达（实测它 89/89 带 `supported_endpoints`；opencode 两面都不报，逐模型 wire 靠随包内置文档表 + 手工）。

---

## 7. 模型暴露：家族槽位与虚拟模型

- **家族槽位四个**：`claude-fable`、`claude-opus`、`claude-sonnet`、`claude-haiku`。这是 Claude Desktop / Claude Code 能安全填写的名字，也是用户在客户端里唯一要记的东西。
- 每个槽位绑定**一条**上游映射：`(上游, 真实模型 ID, 出站协议)`。**不做多上游自动切换**（用户明确不要）。不同槽位可以绑不同上游。
- **绑定从上游模型目录里选**：先选目录条目（目录里没有的模型允许手工新增一条），再在它的 `supported_wires` 里选出站协议；未声明的模型可自由选，但界面标「未声明，靠自检」。模型 ID 打错字不会变成「未声明」被静默放行，因为那需要先建目录条目（票 23）。
- 普通虚拟模型（给 Codex 与其他客户端用）由用户自己取名，同样绑定一组三元组。
- **每客户端可覆盖**：Claude Code 想把 sonnet 槽位指向便宜模型时，不影响 Claude Desktop。
- Codex 侧**必须**写模型目录：为网关要暴露的模型生成目录文件并把 `model_catalog_json` 指向它——**不写目录，Codex CLI 与桌面应用的模型选择器都看不到网关模型**；写了目录之后 Codex **只认目录里的模型**（内置列表作废）。目录只装网关暴露的虚拟模型（四个家族名 + 普通虚拟模型，含 Codex 的覆盖名），**不抄官方条目**：网关模式下 `model_provider` 指向网关，官方条目留在菜单里也会被发到网关；摘键还原后官方模型自然回来。文件 = `<CODEX_HOME>/claudegateway-model-catalog.json`，指针写**相对文件名**（Codex 按 `CODEX_HOME` 解析）。行模板**按入站协议分**：Codex 走的是 Responses 入站，一律用中性模板（`shell_type="shell_command"`、无 freeform `apply_patch`）——判据是「网关在 Responses 上说不说得清自由格式工具」，跟出站是 `anthropic` 还是 `openai-chat` 无关（票 32 实测修正票 31 原先「按出站协议分」的措辞；依据是 cc-switch 给 native-`/responses` 网关用的那份中性模板）。其余字段沿用模板默认（`visibility="list"`、窗口 262144），不做用户可配。写入前自检（JSON 可解析、`models` 非空、至少一条 `visibility=list`、`slug` 非空唯一、解析必需字段齐），不过就不写；`model_catalog_json` 是接管键，磁盘值非网关所写即按票 28 判冲突放弃（冲突弹窗另给显式「认领并覆盖」）；写完提示**完全退出重启 Codex**。顺序固定：四家族名在前，普通虚拟模型按名排序。形状与依据见[票 31](issues/31-grilling-codex-model-catalog.md)。

---

## 8. 客户端接入与配置写入引擎

三条独立流程，共用同一个写入引擎（备份 → 只改关键字段 → 校验 → 可还原）。

| 客户端 | 落地位置 | 关键字段 | 备注 |
| --- | --- | --- | --- |
| Claude Code | `%USERPROFILE%\.claude\settings.json` 的 `env` 块 | `ANTHROPIC_BASE_URL`、**只写 `ANTHROPIC_AUTH_TOKEN`**（不写 `ANTHROPIC_API_KEY`）、四个 `ANTHROPIC_DEFAULT_*_MODEL` | 环境变量优先于 shell 导出；`apiKeyHelper` 会同时发两个头；**`ANTHROPIC_API_KEY` 需要一次 `/config` 交互批准、曾拒绝过就静默失灵**（见 §8.3）；凭据必须落在「首次运行向导之前可读」的位置，项目级 `.claude/settings*.json` 不合格 |
| Codex | `%USERPROFILE%\.codex\config.toml` | `model_providers.*`（`base_url` / `env_key` / `wire_api="responses"` / `http_headers`）、`model_catalog_json` | 必须保序改写；`wire_api` 只能是 `responses`；**不写 `requires_openai_auth = true`**（默认即免登录）；`env_key` 声明的环境变量必须真存在；不写 `auth.json`（见 §8.3） |
| Claude Desktop | `%LOCALAPPDATA%\Claude-3p\configLibrary\*.json` + `_meta.json` | `inferenceProvider="gateway"`、`inferenceGatewayBaseUrl`、`inferenceGatewayApiKey`、`inferenceGatewayAuthScheme`、`inferenceModels` | baseUrl **不带** `/v1`；切回 1P 只需摘 `inferenceProvider`（见 §8.1）；3P **不需要官方登录态**、key 允许占位（见 §8.3）。另有注册表托管路径 `HKLM\|HKCU\SOFTWARE\Policies\Claude`，本期不做 |

写入引擎的规则（照 cc-switch 的 `live/` 引擎；事务性与还原的细节由[票 28](issues/28-grilling-config-writer-transactionality.md) 定案，见 §8.4）：

1. **只改关键字段**，绝不整文件重建：每个「客户端 × 文件」有一份显式**接管键清单**，清单外的键（用户自加键、别的工具写的键、Codex 的 `[desktop]` / `[marketplaces.*]` / `[plugins.*]` 段）原样保留；按格式保序改写；**解析失败即停**，绝不退回空文档重写。
2. **首写快照常驻 + 滚动备份**：首次写入前整份备份一次，之后永不覆盖；此后每次写入前另留一份滚动备份（保留最近 5 份、超 90 天清理）。
3. **幂等重写**：解析后接管键值与目标一致就**完全不写**（连 mtime 都不变）；**接管键以网关为准**（用户手改接管键会被覆盖），非接管键的变化既不算冲突也不被抹掉。
4. **默认还原是「摘键」不是「删文件」**：删掉网关接管键，并清掉指针性条目（Desktop 还要从 `_meta.json` 的 `entries` 摘掉网关条目、`appliedId` 回退），**不动别的 profile**；只有「网关当初新建、且现在只剩网关内容」的文件才整个删掉。整份按字节回滚是显式的第二选项（票 28）。
5. **切回官方（1P）= 摘掉激活动词字段**（Claude Desktop 是 `inferenceProvider`，Claude Code 是那几个 `env` 键），**不需要还原备份**：3P 与 1P 跑两套 userData 目录，登录态互不覆盖，还原备份反而会把用户当时那份 3P 配置一起带回来。备份还原是「按字节回到原状」的大退路，不是切回 1P 的常规路径。

### 8.1 Claude Desktop 3P：已实测的接入事实（票 15，2026-10-14 端到端验收通过）

1. **`inferenceGatewayBaseUrl` 不要带 `/v1`**：Desktop 自己追加，实际打的是 `<baseUrl>` + `/v1/messages`。官方示例里的 `https://gateway.example.com/v1` 会得到 `/v1/v1/messages`。写入引擎按此生成，前端「客户端」页也要把这一点讲清楚。
2. **`inferenceModels` 全填完整模型 ID 时，Desktop 会跳过 `GET /v1/models`**：实测整个启动 + 多轮问答期间网关零调用。
3. **3P 与 1P 的 userData 目录与登录态完全隔离**：3P = `%LOCALAPPDATA%\Claude-3p\`，1P = MSIX 重定向下的 `…\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\`（`oauth:tokenCache`、`lastKnownAccountUuid` 在后者）。两侧 accountId 实测不同值 → 切回 1P 只需摘字段（见上方规则 5）。
4. **真实负载保真**：单轮 `input_tokens=34221`（Desktop 整套 system prompt + 工具定义）过 OpenAI Chat wire 编解码无碍；四个家族槽位全部流式到 `message_stop`、`status=200`，全程无 4xx/5xx。
5. `%LOCALAPPDATA%\Claude-3p\configLibrary\` 未被 MSIX 重定向，直接写入即生效；激活顺序是 `_meta.json.appliedId` → 指向的 `<id>.json` 里的 `inferenceProvider`。

证据与原始记录：[票 15](issues/15-prototype-desktop-3p-e2e.md)、[prototype/README.md §5](prototype/README.md)。非 `claude-*` 的 ID 会不会让整组被拒：**已实测，结论见 §8.2**。

### 8.2 Claude Desktop 3P：模型 ID 校验规则（票 16，2026-10-14 实测 + app.asar 双证）

**校验存在，但处置是「逐条剔除」，没有「整组 fail-all」。** 判据（装机版本 app.asar 内逐字可读，`gateway` 与 `anthropic` 共用）：

1. 先过**非 Anthropic 厂商黑名单**（`deepseek`/`gpt`/`gemini`/`qwen`/`openai`/`glm`/`kimi`/`llama`… 一大串）——命中即非法，含 `claude` 也救不回来（`claude-deepseek-*` 非法）。
2. 再过「**裸档位名**」（`sonnet`/`opus`/`haiku`/`fable`/`mythos`，可带 `-<数字>`）或「**子串**含 `claude`/`sonnet`/`opus`/`haiku`/`fable`/`mythos`/`anthropic`」。

实测（写 6 项 → `picker = 5`，用户看菜单确认）：`deepseek-v4.1-flash` 被单独剔除并留一条 warn，`mock-anthropic-text` 存活，整组未被拒收；全非法时不回写、留到会话层回落默认模型。

对实施的约束：

1. **四个家族名不是唯一可行解，但仍建议「安全 route id + `labelOverride` 显示名」**：自定义名必须避开黑名单厂商词。cc-switch 的 `claude-<角色>-*` 白名单是真实规则的**保守子集**，照它写安全但过窄。
2. **`/v1/models` 不需要按客户端过滤**：过滤在配置层完成，发现层用同一判据；全填完整 ID 时 app 根本不调该端点。四个家族名给 Claude 系、标准 `models` 给 Codex 即可。
3. **必须实现非流式 `/v1/messages` 与 `/v1/messages/count_tokens`**（本次实验挖出的硬约束）：Desktop 的推理探针是 `POST /v1/messages` + `{"model":…,"max_tokens":1,"messages":[…单条…]}`（**无 `stream`**），**400/404 会被判为 `model_rejected`**，界面常驻「Configured model not available」横幅并把 `ConfigHealth` 置为 `config_model_rejected`（探针模型按 `haiku → sonnet → opus` 取名字最短的一条）。原型「非流式一律 400」正好踩中过。**（2026-10-15 票 26 更正归属：`provider: gateway` 的探针链只有 `/v1/models`（Models 列表为空时）与这个非流式 `/v1/messages`，且**只看状态码**；`/v1/messages/count_tokens` 的唯一调用点是 Azure Foundry provider、SDK 的 `countTokens()` 零调用点。两个端点都已由票 26 实现。）**

完整证据、代码原文与实验记录：[票 16](issues/16-research-desktop-model-id-validation.md)。

### 8.3 三个客户端的登录态要求（票 24，2026-10-14 调研）

- **Claude Code、Codex CLI、Claude Desktop 3P 都不需要官方账号登录**：本机 Claude Code 2.1.299 在零凭据下走完 onboarding 并建立会话；Codex CLI 用隔离 `CODEX_HOME` 实测自定义 provider 免 ChatGPT 登录；Desktop 官方明说静态 API key「No per-device preparation is required」且允许占位 key（本机 3P 凭据即 `provider: gateway`，身份与 1P 隔离）。
- **~~唯一例外~~（2026-10-14 由票 25 实测定案推翻）**：Codex 桌面应用的选择器**不**按登录门控——它只显示 `model_catalog_json` 目录文件里的模型（零官方登录也能显示，不写目录就看不到网关模型）；桌面应用确实按 `config.toml` 的 provider 发 `POST /v1/responses`（**要 SSE**，回普通 JSON 会被判 `stream disconnected before completion`）。真正需要官方登录的只有**云线程**（`Sign in to ChatGPT to start a durable thread.`）。实测与证据见[票 25](issues/25-grilling-codex-desktop-card-split.md)、[codex-desktop-model-picker-gating.md](research/codex-desktop-model-picker-gating.md)。
- **两条写入硬约束**：Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`（`ANTHROPIC_API_KEY` 需一次交互批准、曾拒绝即静默失灵）；Codex 不写 `requires_openai_auth = true`、`env_key` 声明的变量必须真存在、不写 `auth.json`。
- **必须让用户手动做一次的步骤**：Desktop 开发者模式 + Configure Third-Party Inference；新机器首次运行向导；每个新工作目录一次 trust dialog；想让 Codex **桌面应用的云线程 / 远程功能**才需登录一次官方账号（Free 即可，模型选择器不需要）；三个客户端改配置后都要重启。**别用 `/logout` 清场**（官方说会重置首次运行状态）。
- **未验证（需一次隔离实测）**：Claude Code 只有 `settings.json` env 块时真实发请求（U1）；全新机器上 env 块能否抢在首次运行向导之前生效（U2，决定「全自动」承诺）；Codex CLI 真实发 `POST /v1/responses`（U4）；~~Codex 桌面应用在有 `model_providers` 时请求是否真走网关（U5）~~ —— **U5 已于 2026-10-14 实测结案：确实走网关**（mock 收到 `POST /v1/responses`，必须按 SSE 回；票 25）。OpenAI 官方 Codex 文档域在本机解析不到公网 IP，Codex 侧缺官方文档层证据。
- 证据与完整出处：[票 24](issues/24-research-client-login-state.md)、[client-login-state.md](research/client-login-state.md)。

### 8.4 写入引擎的事务性与还原（票 28，2026-10-15 定案）

1. **写入**：同目录 `create_new` 临时文件 → fsync 文件 → 原子替换（Windows `ReplaceFileW`，遇 `NotFound` / `ERROR_NOT_SUPPORTED` 退 `fs::rename`，最多 3 轮）；失败即删临时文件。不做父目录 fsync。
2. **写失败**：被占用 / 权限不足 / 只读 → 退避重试 3 次（~200ms），仍失败则报错并给出目标文件路径、备份路径与「可能有客户端正占用该文件」的提示。
3. **冲突检测只看接管键**：写前比对「接管键现值」与「`client_config_state` 里记录的上次写入值」；非接管键（含别的程序写的段）变化不算冲突。**冲突 = 放弃本次 + 提示**，不覆盖、不自动合并；状态记录缺失（首次运行 / 数据库被清 / 换机）按**首次接管**处理（备份后写 + 提示）。
4. **单实例**：启动用命名互斥量保证只跑一个实例，因而「同进程写入串行化」成立（每文件一把锁）；不做跨进程锁文件，冲突检测兜底。
5. **格式保真**：JSON 沿用目标文件原有的缩进、行尾（实测 CRLF）、BOM 有无；TOML 保序。真写入时接受解析后重序列化（用户注释会丢，无变化时不写所以平时不碰）。
6. **多文件顺序**：按依赖顺序逐个原子替换（Codex：目录文件 → `config.toml`；Desktop：profile → `_meta.json`），保证不出现悬空指针；中途失败报错并列出已改文件。**不做跨文件 pending 事务与启动补完**。
7. **备份**：`%APPDATA%\com.claudegateway.desktop\backups\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`；首写快照常驻不清理，滚动备份保留 5 份 / 90 天；首写时文件不存在只写 marker，不造空备份。
8. **还原**：默认摘键（含指针性条目）；可选整份回滚（仅当该文件自上次网关写入后未被外部改过）。卸载时的「还原客户端配置」= 摘键。
9. **历史**：每次写入/还原记 `client_config_state` + 元数据级日志（7 天），不新做历史页。

**为什么必须是「接管键」而不是「哨兵围出的区域」**（票 32 实测）：Codex 桌面应用启动时会改写 `config.toml`，实测它把 `[windows] sandbox = "elevated"` 插进了我们 provider 表的哨兵注释之间。按区域删就会把这张表一起带走；按接管键逐行认领则原样保住。原型 `codex-gateway.ps1` 已按此改并把这个场景做成 `-Mode selftest`。

---

## 9. 存储、安全与可观测

**SQLite 表（草案）**：`upstreams`（含「上次拉取时间 / 结果 / 警告」）、`upstream_models`（上游模型目录：真实模型 + 可用 wire 集合 + 来源 + **手工价四列**：输入/输出/缓存读/缓存写，可空 = 回退 models.dev）、`credentials`(密文，按上游 × wire)、`virtual_models`、`client_profiles`、`client_config_state`(每个「客户端 × 文件」一行：上次由网关写入的接管键值、时间、备份路径、首写时文件是否存在；§8.4)、`requests`(元数据 + `cost_usd`：请求时按当时价冻结)、`request_bodies`(可选)、`model_catalog`(随包内置的 models.dev 快照，设置页手动刷新)、`settings`(端口、日志保留、**汇率**、快照刷新时间)。

**密钥**：DPAPI 加密后入库；界面只显示掩码；导出 JSON 时默认不含明文密钥（可选带密码的加密导出）。

**日志与隐私（用户已确认）**：默认只记元数据（虚拟模型 / 上游 / 状态码 / 耗时 / 首字延迟 / token / 成本）；完整请求与响应 body 默认不写，可在界面临时开启；保留 7 天，自动清理。

**用量与成本**（[票 33](issues/33-grilling-cost-accounting.md) 定案）：按**虚拟模型 / 上游 / 客户端**三个维度聚合，**归集口径是虚拟模型**（四个家族槽位各自记账），上游只作另一维度。

- **价格优先级：手工覆盖 > models.dev 快照 > 未知**。手工四项价（输入 / 输出 / 缓存读 / 缓存写）各自可填可留空，填了的项锁定、刷新不覆盖，留空的回退 models.dev；手工价挂在**「上游 × 真实模型」的目录条目**上。models.dev 数据**随包内置一份快照**，设置页可**手动刷新**（失败静默沿用旧快照，不自动拉）。界面标出每一项价格的实际来源。
- **未知价显示「未知」，不计入总额**，另给「N 次无价格 / N 次 token 未知」计数；不显示 0、不留空。
- **缓存计价**：`cache_read` 按缓存读价，缺则退输入价；`cache_write` 按**缓存写价**（票 58 起：`requests.cache_write` 已采集；缺缓存写价按「未知价」处理，**不拿输入价顶替**——票 33 只给缓存读留了退路）。
- **冻结口径**：请求时按当时价算好 `cost_usd` 存库（当时无价则空）；改价与刷新快照**不重算历史**；上游没回 usage 时标「token 未知」、成本不计（不用本地估算反算）。
- **币种**：库内以**美元为锚**，界面**人民币为主**（汇率随包内置初值 + 设置页可手改，不联网），用量页可切美元；金额两位小数，不足 ¥0.01 显示 `<¥0.01`。
- **粒度**：概览 = 今日 + 近 7 天（按虚拟模型 / 按上游两维）；用量页 = 今日 / 近 7 天 / 近 30 天 / 全部 / 自定义区间（日边界按本机本地时区自然日）× 三维度 + 按天明细；日志每行带单次请求成本。

---

## 10. 前端（React）职责边界与信息架构

前端**只做外观与交互**，不参与转发、不直接读写客户端配置文件（写配置由后端 `clients::writer` 完成，前端只调 `/api/*`）。导航固定 **7 页**，不合并、不增设。本节是[票 17](issues/17-grilling-ui-information-architecture.md) 的 2026-10-14 定案。

**主路径**：由**概览页顶部的三步清单**串起「加上游 → 绑槽位 → 切客户端」；**首启不另做向导**，概览清单就是引导。就绪判据：上游 = 至少一条已保存且自检通过；槽位 = 四行都绑定且探针通过；客户端 = 三个客户端至少一个「已生效」。

1. **概览**：三步清单（未就绪高亮、点进对应页）；网关状态与端口、今日用量、最近错误（折叠）。动作：「一键把可用客户端都切到网关」（已生效的跳过，Desktop 提示重启）、三步全绿后的「跑一次端到端验收」（用 haiku 槽位发一条非流式最短请求，结果卡列上游 / 状态码 / 耗时 / 首字延迟，失败套错误模板并跳日志详情）。
2. **上游**：列表（名称 / baseUrl / 协议 / 上次自检结果 / 上次拉取时间与警告角标 / 被引用数）+ 四步向导（选供应商**可跳过** → 填 baseUrl 与 key（涉及第二种 wire 面时按需展开第二个 key 输入框）→ 拉 `/v1/models` 勾选**可跳过**、无该端点时手填模型 ID（**用上游 `default` 凭据**；勾选列表带可用 wire 徽章；手改过的条目锁定，重拉不覆盖；**404/405 与「200 但零条」= 该上游不提供模型发现**，给「请手填模型 ID」+ 手填入口，不报红；**其他失败**给内联报错 +「手工添加模型」按钮；**重拉失败保留旧条目**并提示「沿用上次结果（时间）」；wire 反查不到的路径内联一行警告 + 上游行角标，下次成功拉取清除）→ **强制自检并保存**）。删被引用的上游要列出引用处。凭据只在这里管。
3. **模型与映射**：顶部客户端标签（全局 / Claude Desktop / Claude Code / Codex，即「每客户端覆盖」的入口）；四行固定家族槽位表（槽位 | 上游 | 真实模型 | 协议 | 测试，测试 = 非流式 `max_tokens:1` 探针）；下方普通虚拟模型列表（名称 | 上游 | 真实模型 | 协议 | 用途），Codex 标签下即其模型目录。协议列**可手改，但候选取值只限该模型 `supported_wires` 里声明过的值**，未声明时可自由选并标注「未声明，靠自检」（[票 23](issues/23-design-provider-table-granularity.md) 已结案）。每个条目上一颗「价格」按钮（[票 33](issues/33-grilling-cost-accounting.md)）：输入 / 输出 / 缓存读 / 缓存写四项可填可留空，填了即锁定并标出实际来源。
4. **客户端**：三张卡片、四态徽章（未配置 / 已生效 / 值不匹配 / 已回官方），状态**每次进页面实读配置文件比对**，不缓存；可折叠的「当前值 vs 期望值」对照表；动作：一键切换 / 一键还原 /（Desktop）1P↔3P 开关；Desktop 卡片常驻「改完必须完全退出重开」提示。写文件与删数据的动作统一二次确认，弹窗列出文件绝对路径与备份路径。**卡片不把「未登录」当阻断性错误**（§8.3）；Codex 保持**一张卡**（CLI 与桌面应用不拆，[票 25](issues/25-grilling-codex-desktop-card-split.md) 已结案）。
5. **日志与调试**：默认元数据表格（时间倒序 50 条；按客户端 / 上游 / 虚拟模型 / 状态码筛选 + 模型名与请求 ID 搜索）；详情抽屉四段（概要 / 请求与响应体 / 帧时间轴可切原始 SSE 文本 / 错误信息）。「记录完整 body」开关默认关，开启需隐私确认，**1 小时后自动关**。
6. **用量与成本**：时间范围今日 / 近 7 天 / 近 30 天 / 全部 / 自定义区间；按虚拟模型 / 上游 / 客户端切维度；花费、请求数、token 的图表与按天明细表；**币种人民币为主、可切美元**；无价格与 token 未知的请求**单独计数、不计入总额**（[票 33](issues/33-grilling-cost-accounting.md)）。
7. **设置**：端口、开机自启、托盘行为、日志保留期与 body 开关（与日志页同一个）、数据库位置与打开目录、导出 / 导入（默认不含明文密钥）、**models.dev 价格快照刷新**、**汇率手改**（[票 33](issues/33-grilling-cost-accounting.md)）、关于与版本。

**故障态**：网关未运行时所有页面顶部常驻横幅 +「启动网关」按钮，数据页显示等待、**配置类页面仍可编辑**；端口被占时给「换端口」（探测空闲端口、用户确认后写入，不静默换端口），换端口后自动同步已生效客户端的地址。侧栏徽标显示未通过自检的上游数与未生效的客户端数。

**错误表达**：模板「发生了什么 + 可能原因 + 一个主按钮 + 复制诊断」，原始 JSON 可展开；出现在概览最近错误、操作当场的内联提示、日志详情三处。复制诊断含版本、端口与运行状态、上游配置的脱敏形式、错误原文、相关请求元数据，**不含密钥明文与 body**。

**托盘**：右键「显示主窗口 / Claude Desktop 1P↔3P 快切 / 网关状态（运行中 · 端口）/ 打开日志目录 / 退出」，左键单击开窗口，图标随状态变色。

后端 → 前端的接口统一在 `/api/*` 下，与转发用的 `/v1/*` 完全分离。

---

## 11. 原型（已达成）

用户定义的验收标准：**Claude Desktop 在 3P 模式下流式问答正常，四个家族名映射生效。**（2026-10-15 达成）

原型的范围（刻意做小）：

1. Rust 骨架 + 固定端口 + SQLite + 家族槽位表。
2. IR + Anthropic 编解码（入站 `/v1/messages` ↔ 出站 Anthropic 直通）+ SSE 逐帧转发。
3. **mock 上游**：本地假上游回放录制的 SSE 帧，覆盖工具调用、thinking、usage、`ping`、异常中断五类场景。
4. 真实上游各接一家：opencode 与 commandcode（用户已确认两家都接）。
5. 配置写入引擎最小实现：先 Claude Desktop 3P（`inferenceProvider=gateway` + 四个家族名），再 Claude Code 与 Codex。
6. 前端只做一个页面：网关状态 + 四个槽位的绑定。

原型自己的推进顺序是「mock 上游回归 → Anthropic 链路 → Claude Desktop 3P 端到端 → 再补 OpenAI Chat / Responses 与另外两个客户端」，**它是历史记录，不再是路线**：从原型到正牌应用的顺序见 §12。

**进度（2026-10-15）**：前四项已过——mock 回归与两条真实上游见[票 14](issues/14-prototype-conversion-core.md)，Claude Desktop 3P 端到端（四家族名 + 流式 + 切回 1P）见[票 15](issues/15-prototype-desktop-3p-e2e.md)；配置写入引擎最小版落在 `prototype/desktop-3p.ps1`（gateway / standard / restore 三模式）。**Codex 侧已过**（票 32）：`/v1/responses` 入站 + 模型目录 + `prototype/codex-gateway.ps1`，CLI 与桌面应用都实测跑通（详见 `prototype/README.md` §10）。**原型自 2026-10-15 起冻结**（[票 38](issues/38-grilling-implementation-order.md) / [ADR-0007](../docs/adr/0007-skeleton-and-implementation-order.md)）：不再演进，只当参照物。原型里没做、已排进 §12 的有：Claude Code 链路、前端页面（信息架构见 §10）、Ollama 兼容面、客户端档案覆盖、正牌配置写入引擎。

---

## 12. 实施顺序（里程碑）

骨架形状、四段划分与验收形态的定案见 [ADR-0007](../docs/adr/0007-skeleton-and-implementation-order.md)（[票 38](issues/38-grilling-implementation-order.md)，2026-10-15）。**「可以开始拿它日常用」= M2 收口**，不是功能全齐之后。

**骨架**：cargo workspace —— `crates/gateway-core`（lib：ingress / IR / binding / egress / 旁路 writer / obs，**不依赖 Tauri**，回放测试裸跑）+ `src-tauri`（Tauri 2 外壳 + 管理 API + 托盘，唯一的 bin）+ `app/`（React + Vite）。搬迁**一次性对齐 §3 的模块边界**：随行走 `ir.rs` / `sse.rs` / `anthropic.rs` / `openai.rs` / `responses.rs` / `provider.rs` / `upstream.rs` 与整套回归基建（`mock.rs` / `replay.rs` / `fixtures/` / `tests/` / `check.ps1` / `record.ps1`，ADR-0005 的门禁就是它，不重写）；不搬原型 `main.rs` 的路由与硬编码表，不搬 `desktop-3p.ps1` / `codex-gateway.ps1`（留作行为参照）；`prototype/` 原地冻结不删。三个编解码文件各自保持完整（各含入站与出站，不硬拆两半）。M1 顺手修三笔原型技术债：`store` 去掉「每行单开连接 + 同步写库阻塞 runtime」、mock / echo / `/__chunks` / `/__bodylog` 与网关分进程隔离、crate 拆成 lib + app。

**M1 地基**（规模 M；最大不确定性 = Tauri 2 外壳与 axum 同进程共存的形态：托盘、单实例、关窗隐藏、网关生命周期归属）

- 交付：workspace 立起来；核心搬迁完成；SQLite 按 §9 建表并能迁移；密钥 DPAPI 存取；管理 API 骨架（`/api/status`）；单实例互斥量；端口占用明确报错（不静默换端口）；GitHub Actions 门禁（推送跑 `check.ps1 -Quick -Replay`）。
- 收口验收：`check.ps1` 在新骨架里全绿且测试格数不减；应用能启动、托盘常驻、窗口能开关、关窗按 §10 隐藏到托盘；库与密钥能读写；端口被占时报错清楚。**此段结束仍不能日常用。**

**M2 日常可用**（规模 L，四段中唯一的大块；最大不确定性 = Claude Code 的写入与还原在真实客户端上的行为）

- 交付：配置写入引擎（[票 28](issues/28-grilling-config-writer-transactionality.md) 全 21 条，含[票 30](issues/30-task-build-regression-suite.md) 第 8 格那几条断言）；三个客户端的写入与还原（Desktop 含 1P↔3P 一键切换，[票 15](issues/15-prototype-desktop-3p-e2e.md) / §8.1；Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`，§8.3 / [票 24](issues/24-research-client-login-state.md)；Codex 含模型目录与逐行认领，§7 末 / [票 31](issues/31-grilling-codex-model-catalog.md) / [票 32](issues/32-prototype-codex-e2e.md)）；客户端档案覆盖（[票 10](issues/10-design-model-exposure-and-client-config.md)）；供应商表三层校验（[票 23](issues/23-design-provider-table-granularity.md)）；模型发现与三种结局的降级（[票 35](issues/35-grilling-model-discovery-fallback.md)）；models.dev 随包快照与手工价（[票 33](issues/33-grilling-cost-accounting.md)）；上游强制自检；前端四页（概览 / 上游 / 模型与映射 / 客户端，§10 第 1–4 节）；用量与日志基础；NSIS per-user 安装包（[票 18](issues/18-grilling-packaging-and-updates.md)）。
- 收口验收：三个真实客户端端到端各走一遍（Desktop 3P 四家族名 + 流式 + 切回 1P；Claude Code；Codex CLI 与桌面应用）；写入引擎与票 30 第 8 格全绿；NSIS 装得上、能跑；一键还原回官方态；用户当场三个客户端各问一句并签字。

**M3 保真长尾**（规模 M；最大不确定性 = 结构化输出的敌意探针判定）

- 交付：Ollama 兼容面四个端点（§4.4，[票 34](issues/34-research-ollama-inbound.md) / [票 36](issues/36-grilling-ollama-endpoint-scope.md)）；结构化输出实现（[票 37](issues/37-grilling-structured-output-slot.md)：IR 槽位 + 三条出站映射 + 能力位 + 探针）；`cache_write` 采集与计价（[票 33](issues/33-grilling-cost-accounting.md)）；用量与成本页完整粒度、日志页四段与诊断包、其余三页（§10 第 5–7 节）。
- 收口验收：`check.ps1` 全绿（含 Ollama 面的回放断言）；结构化输出探针在两条真实上游实测并给出能力位结论；用量数字与上游 usage 对得上。

**M4 分发收尾**（规模 S；最大不确定性 = updater 签名与 passive 静默安装在本机的行为）

- 交付：自动升级闭环（公开 GitHub Releases + `latest.json` + 签名校验）、开机自启开关（默认关）、卸载保留数据并询问是否还原、免安装 zip、导出 / 导入与版本回退指引（[票 18](issues/18-grilling-packaging-and-updates.md)）。
- 收口验收：干净机器上装 → 升一版 → 卸载（选还原）全流程走通。

**验收形态**：三层网（Rust 单测 / 黄金 fixture 回放 / 真实客户端半自动冒烟，[票 19](issues/19-grilling-test-strategy.md) / [ADR-0005](../docs/adr/0005-protocol-conversion-regression-strategy.md)）当**门禁**——日常改动必跑 `check.ps1 -Quick` + `-Replay`；`-Live` 与三个真实客户端端到端只在 **M2、M4 收口**时人工签字。

**欠账插位**（整条路线不提前）：写入引擎落在 M2 而不是 M1；Ollama 面与结构化输出落在 M3 而不是 M2，因为三个目标客户端都不用它们。地图 Out of scope 里「配置写入引擎重做」与「客户端档案覆盖回归断言」两笔欠账随 M2 一起销账。

### 12.1 实现期约定（2026-10-15，票 38 之后）

- **票据就是任务图**：`issues/39-*` 起 25 张 `Type: task` 票据覆盖这四段——M1 = 39–44、M2 = 45–55、M3 = 56–60、M4 = 61–63；每张的 `Blocked by:` 写依赖。它们是**实现期票据**，不是地图的决策票（`wayfinder:*` 那套随地图结清）。
- **一票一会话**：用 `tdd` 建，门禁 `check.ps1 -Quick -Replay` 必跑；收工把结果写进 `## Answer`、`Status:` 改 `resolved`（本地 markdown 追踪器的收工方式，见 [docs/agents/issue-tracker.md](../../docs/agents/issue-tracker.md)）。
- **集成方式**：一条 integration branch，每票一个 worktree / 分支（`/implement-spec` 编排）。**M1 天然串行**——[票 39](issues/39-task-repo-workspace-and-ci.md) 是所有票的前置，且此刻仓库根还没有 `.git`，它必须在主工作区先做完。
- **四张收口票（44 / 55 / 60 / 63）不甩给子代理**：它们是现场验收 + 用户签字，由主会话带着用户走。
- **`prototype/` 冻结**：只读参照物，改行为只发生在正牌骨架里。
- **实现期不改判据**：§4–§10 与 ADR-0001~0007 已定死的行为照做；要改先回到票据层面重新决策，不顺手改。

---

## 13. 风险与未决

| 风险 | 影响 | 处理 |
| --- | --- | --- |
| Claude Desktop 的模型 ID 校验规则**已定案**（票 16 实测 + app.asar）：校验存在，但非法名是**逐条剔除**、不是整组拒收；判据是「先过厂商黑名单，再认裸档位名或含 claude/档位名/anthropic 的子串」，比 cc-switch 的白名单宽（见 §8.2） | 决定四个家族名之外还能不能挂自定义名 | 家族名方案安全落地（§8.1）；自定义名按 §8.2 第 1 条生成；写入引擎可选做一次「本地预检」（只是省一次重启，不再承担 fail-all 风险） |
| Claude Desktop 的**非流式**推理探针 | 网关只做流式时，3P 界面常驻「Configured model not available」，`ConfigHealth=config_model_rejected`，即使聊天完全正常 | §8.2 第 3 条：实现非流式 `/v1/messages`（`max_tokens:1` 最小体）与 `/v1/messages/count_tokens` |
| 上游方言与客户端版本强耦合 | 转换层随时被上游改版打破 | 录制帧 fixture 回归；`quirks` 枚举化 |
| DPAPI 换机不可解密 | 备份不可迁移 | 导出时提供带密码的加密导出；界面明确提示 |
| 端口被占用 | 客户端配置全部失效 | 固定端口 + 启动时明确报错，不静默换端口；界面另给「换端口」（探测空闲端口、用户确认后写入并同步已生效客户端，见 §10） |
| Codex **桌面应用**的选择器只认模型目录文件~~按登录身份门控~~（2026-10-15 由[票 25](issues/25-grilling-codex-desktop-card-split.md) 实测推翻原判） | 不写 `model_catalog_json` 时桌面 GUI 选不到网关模型（CLI 不受影响） | 网关生成目录文件（§7、[票 31](issues/31-grilling-codex-model-catalog.md)）；「未登录」不告警，只有**云线程**需要官方登录（§8.3） |
