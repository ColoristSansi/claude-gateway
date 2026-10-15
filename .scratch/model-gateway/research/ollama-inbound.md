# Ollama 入站（`/api/chat`、`/api/tags`）的形状与 IR 映射

调研时间：2026-10-15（Asia/Shanghai）。票：[34-research-ollama-inbound](../issues/34-research-ollama-inbound.md)。
抓取方式：Ollama 官方仓库原始文件（`raw.githubusercontent.com`）+ 官方文档站（`docs.ollama.com`）。**未使用任何二手博客**。

**证据分级**（每条结论都标了其中一种）：

- **【官方文档】** = `docs/api.md`、`docs/openapi.yaml`、`docs/api/errors.mdx`、`docs/api/openai-compatibility.mdx` 或 `docs.ollama.com` 站点原文。
- **【官方源码】** = Ollama 仓库的一手实现（`api/types.go`、`server/routes.go`、`middleware/openai.go`、`openai/openai.go`）。docs 与源码冲突时**以源码为准**并单独标注。
- **【推断】** = 本文档基于上述事实做的判断，官方未明说；落地时按可验证假设对待。
- **【未证实】** = 明确没查到的点。

## 0. 来源清单

| 代号 | URL | 用途 |
| --- | --- | --- |
| D1 | <https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md> | 官方 API 文档（含 `/api/chat` 与 `/api/tags` 的完整示例 JSON） |
| D2 | <https://raw.githubusercontent.com/ollama/ollama/main/docs/openapi.yaml> | 官方 OpenAPI 3.1 规范（字段级 schema，含 200 的两种 Content-Type） |
| D3 | <https://raw.githubusercontent.com/ollama/ollama/main/docs/api/errors.mdx> | 官方错误文档（状态码、错误体、流中错误） |
| D4 | <https://raw.githubusercontent.com/ollama/ollama/main/docs/api/openai-compatibility.mdx> | 官方 OpenAI 兼容面文档（支持/不支持字段表、`/v1/responses`） |
| D4b | <https://docs.ollama.com/api/openai-compatibility> | 同上内容的官方站点版（D4 是它的源文件） |
| S1 | <https://raw.githubusercontent.com/ollama/ollama/main/api/types.go> | 请求/响应 Go 结构体（字段的 `omitempty` 事实来源） |
| S2 | <https://raw.githubusercontent.com/ollama/ollama/main/server/routes.go> | 路由注册表、NDJSON 写出、非流式缓冲、错误体构造 |
| S3 | <https://raw.githubusercontent.com/ollama/ollama/main/middleware/openai.go> | OpenAI 兼容面的 SSE 写出（`data: …`、`[DONE]`）、兼容面错误体 |
| S4 | <https://raw.githubusercontent.com/ollama/ollama/main/openai/openai.go> | OpenAI 兼容面结构体与 `NewError` 错误信封 |

注：`https://docs.ollama.com/api/chat` 本次抓取失败（连接错误），但 D1 是官方仓库内的同一份文档源文件，且 D2 提供了机器可读的等价 schema，故不构成证据缺口。

---

## 1. `POST /api/chat`

### 1.1 请求形状

**【官方源码】** `api.ChatRequest`（S1）+ **【官方文档】** D2 的 `ChatRequest` schema：

| Ollama 字段 | 类型 | 必填 | 本网关 IR 落点 | 证据 |
| --- | --- | --- | --- | --- |
| `model` | string | 是 | `IrRequest.model`（先经绑定层解析虚拟模型/家族槽位 → 真实模型 ID） | 【官方文档】D2 `required: [model, messages]`；D1「`model`: (required)」 |
| `messages` | array | 是 | `IrRequest.messages` + `IrRequest.system`（拆见下表） | 同上 |
| `messages[].role` | `system`\|`user`\|`assistant`\|`tool` | 是 | `IrMessage.role`；`tool` → IR 的 `user` 轮 | 【官方文档】D2 `enum: [system, user, assistant, tool]`；D1 message 字段说明 |
| `messages[].content` | **string**（不是 parts 数组） | 是 | `IrBlock::Text{ text }` | 【官方源码】S1 `Content string`；【官方文档】D2 `content` 为 `type: string` |
| `messages[].images` | string[]（**裸 base64，无 `data:` 前缀、无 media_type**） | 否 | `IrBlock::Image{ source: {"type":"base64","media_type":<推断值>,"data":<原串>} }` | 【官方文档】D1「a list of images to include in the message」+ 示例为裸 base64；【官方源码】S4 注释「match `/api/chat`'s behavior of taking just unadorned base64」 |
| `messages[].thinking` | string | 否 | `IrBlock::Thinking{ thinking, signature: None }` | 【官方源码】S1 `Thinking string` |
| `messages[].tool_calls` | array | 否 | `IrBlock::ToolUse{…}`（助手轮） | 【官方文档】D1「With history, with tools」示例 |
| `messages[].tool_name` | string | 否 | ToolResult 的**名字**来源（IR 只存 id，见 §4.4） | 【官方文档】D1「`tool_name` (optional): add the name of the tool that was executed」 |
| `messages[].tool_call_id` | string | 否 | `IrBlock::ToolResult.tool_use_id` | 【官方源码】S1 `ToolCallID string \`json:"tool_call_id,omitempty"\``（**D1 未记载此字段**，见 §4.4） |
| `tools` | array | 否 | `IrRequest.tools`（IR 内统一存 Anthropic 形状 `input_schema`） | 【官方文档】D1/D2；形状见 §4.1 |
| `options` | object | 否 | 见下表逐项 | 【官方文档】D1「additional model parameters…such as `temperature`」 |
| `stream` | bool，**缺省 true** | 否 | 决定入站响应编码方式（不是 IR 字段） | 【官方文档】D2 `stream: {type: boolean, default: true}`；【官方源码】S1 `Stream *bool`（nil 即流式） |
| `format` | `"json"` 或 JSON schema | 否 | **IR 无对应字段**（⚠️ 见 §6 缺口 1） | 【官方文档】D2 `format` oneOf；D1「Structured outputs」 |
| `keep_alive` | string 或 number | 否 | 无 IR 字段；本网关无模型驻留概念 → 忽略 | 【官方文档】D2 `keep_alive` oneOf string/number |
| `think` | bool 或模型自定义字符串 | 否 | `IrRequest.thinking` | 【官方文档】D2 `ThinkValue`；D1「`think`: (for thinking models)…」 |
| `truncate` / `shift` | bool | 否 | 无 IR 字段 → 忽略 | 【官方源码】S1 两个 `*bool` 字段 |
| `logprobs` / `top_logprobs` | bool / int | 否 | 无 IR 字段 → 忽略 | 【官方源码】S1；【官方文档】D1 未展开 |

`options` 逐项（**【官方文档】** D2 `ModelOptions` + D1 D2 的 options 示例；**【官方源码】** S1 `api.Options`）：

| `options.*` | IR 落点 | 说明 |
| --- | --- | --- |
| `temperature` | `IrRequest.temperature` | 直接对应 |
| `top_p` | `IrRequest.top_p` | 直接对应 |
| `stop`（string 或 string[]） | `IrRequest.stop: Vec<String>` | **注意是 string 或 array 两种形状**（D2 `oneOf`），入站要归一成数组 |
| `num_predict` | `IrRequest.max_tokens` | 语义等价（最大生成 token 数）。`num_predict: -1`（默认）= 无上限 → 需给 IR 一个缺省值（票 26/32 的先例：Responses 无上限时用 4096） |
| `seed` | `IrRequest.extra["seed"]` | IR 无 seed 槽位，走 `extra` |
| `top_k` / `min_p` / `repeat_last_n` / `repeat_penalty` / `presence_penalty` / `frequency_penalty` | `IrRequest.extra[...]` | 出站是否传由各 wire 决定；OpenAI Chat 出站只认 `frequency_penalty`/`presence_penalty`（见 `prototype/src/openai.rs`） |
| `num_ctx` / `num_batch` / `num_gpu` / `main_gpu` / `use_mmap` / `num_thread` / `draft_num_predict` | 丢弃（或 `extra`） | 本机模型装载参数，对上游无意义 |
| `typical_p` | — | **已废弃**（D2 `deprecatedOptions` 注释：可作为请求参数带过但会 warn）；本网关忽略 |

### 1.2 响应：非流式（`"stream": false`）

**【官方文档】** D1 示例（逐字）：

```json
{
  "model": "llama3.2",
  "created_at": "2023-12-12T14:13:43.416799Z",
  "message": {
    "role": "assistant",
    "content": "Hello! How are you today?"
  },
  "done": true,
  "total_duration": 5191566416,
  "load_duration": 2154458,
  "prompt_eval_count": 26,
  "prompt_eval_duration": 383809000,
  "eval_count": 298,
  "eval_duration": 4799921000
}
```

同一形状、带工具调用时（**【官方文档】** D1「Chat request (No streaming, with tools)」逐字）：

```json
{
  "model": "llama3.2",
  "created_at": "2025-07-07T20:32:53.844124Z",
  "message": {
    "role": "assistant",
    "content": "",
    "tool_calls": [
      { "function": { "name": "get_weather", "arguments": { "city": "Tokyo" } } }
    ]
  },
  "done_reason": "stop",
  "done": true,
  "total_duration": 3244883583,
  "load_duration": 2969184542,
  "prompt_eval_count": 169,
  "prompt_eval_duration": 141656333,
  "eval_count": 18,
  "eval_duration": 133293625
}
```

字段位置结论：

- `message.content`、`message.tool_calls`、`message.thinking`、`message.images` **都在 `message` 对象内**。
- `prompt_eval_count` / `eval_count` / `total_duration` / `load_duration` / `prompt_eval_duration` / `eval_duration` / `prompt_eval_cached_count` 是**顶层字段**（不是 `message` 内），来自 `api.Metrics` 的**内嵌**（**【官方源码】** S1 `ChatResponse` 末尾 `Metrics`，无 json tag → 内嵌提升到顶层）。
- `done_reason` 也是**顶层**，且 **`omitempty`**（**【官方源码】** S1 `DoneReason string \`json:"done_reason,omitempty"\``）→ **值可能整键缺失**。D1 的普通聊天示例（无工具）就没有 `done_reason`，而工具示例有。客户端必须容忍缺失。
- 同理 `Metrics` 所有字段都有 `omitempty` → 值为 0（如 `load_duration: 0`、`prompt_eval_count: 0`）时**整键不出现**。D1「Load a model」示例只回 `{"model","created_at","message","done_reason":"load","done":true}` 即此因。
- **`Content-Type`：`application/json`**（**【官方源码】** S2 `writeChatResponse` 的非流式分支走 `c.JSON(http.StatusOK, resp)`）。

**【官方源码】** 非流式不是「上游非流式」，而是**把同一批流式帧在服务端缓冲后组装**：S2 `writeChatResponse` 遍历 `ch`，把 `t.Message.Thinking`/`t.Message.Content` 累加、`tool_calls` 全量 append，最后一次性 `c.JSON`。这与本网关票 26 的范式（出站仍 `stream:true`，网关缓冲到终态再组装）**完全同构**。

### 1.3 响应：流式（`"stream": true`，缺省）

- **是 NDJSON，一行一帧，不是 SSE。**【官方源码】S2 `streamResponse`：`c.Header("Content-Type", "application/x-ndjson")`，每帧 `json.Marshal(val)` 后 **追加 `'\n'`** 写出，注释「Delineate chunks with new-line delimiter」。
- **没有 `data:` 前缀，没有 `event:` 行，没有 `[DONE]` 哨兵。** 终态用 `done: true` 表达。
- **【官方文档】** D2 的 200 响应同时声明两种媒体类型：`application/json` → `ChatResponse`（非流式）、`application/x-ndjson` → `ChatStreamEvent`（流式），与源码一致。
- **【官方文档】** D2 对 `ChatStreamEvent.done` 的描述逐字：「True for the final event in the stream」。

帧序列（**【官方文档】** D1 逐字）：

首帧/增量帧：

```json
{
  "model": "llama3.2",
  "created_at": "2023-08-04T08:52:19.385406455-07:00",
  "message": {
    "role": "assistant",
    "content": "The",
    "images": null
  },
  "done": false
}
```

终帧：

```json
{
  "model": "llama3.2",
  "created_at": "2023-08-04T19:22:45.499127Z",
  "message": {
    "role": "assistant",
    "content": ""
  },
  "done": true,
  "total_duration": 4883583458,
  "load_duration": 1334875,
  "prompt_eval_count": 26,
  "prompt_eval_duration": 342546000,
  "eval_count": 282,
  "eval_duration": 4535599000
}
```

带工具的终帧（**【官方文档】** D1「Streaming with tools」逐字）：

```json
{
  "model": "llama3.2",
  "created_at": "2025-07-07T20:22:19.19314Z",
  "message": { "role": "assistant", "content": "" },
  "done_reason": "stop",
  "done": true,
  "total_duration": 182242375,
  "load_duration": 41295167,
  "prompt_eval_count": 169,
  "prompt_eval_duration": 24573166,
  "eval_count": 15,
  "eval_duration": 115959084
}
```

结论：

1. **帧序列 = N 个 `done:false` 增量帧 + 1 个 `done:true` 终帧**，没有独立的开始帧。
2. **终帧的 `message.content` 是空串 `""`**（D1 明确），统计字段全在终帧顶层。
3. **`done_reason` 只在终帧出现，且可能缺失**（omitempty，见 §1.2）。
4. 中间帧的 `images` 可能是 `null`（D1 首帧示例有 `"images": null`；这是因为 `api.Message.Images` 是 `[]ImageData` + `omitempty`… 但 D1 示例里出现了 `null`，**与 S1 的 `omitempty` 不一致**）→ 【推断】这是文档示例的**手写产物**，实际取舍以 S1 的 `omitempty` 为准（值为空则整键不出现）；客户端两种都要容忍。
5. `done_reason` 的取值（**【官方文档】** D1 出现过的）：`"stop"`（含工具调用场景）、`"load"`（`messages: []` 装载模型）、`"unload"`（`messages: []` + `keep_alive: 0`）。**【官方源码】** S2 中该值来自 `cr.DoneReason.String()`。完整取值域 **【未证实】** —— D1/D2 都没给枚举。

### 1.4 逐字段映射表：`/api/chat` 响应 → 本网关 IR 事件/出站编码

本网关出站是「IR 事件流 → 入站协议编码器」。Ollama 面需要一个 **NDJSON 编码器**（对应 `responses.rs::ResponsesEncoder` 的角色）：

| IR 事件 / 字段 | Ollama NDJSON 输出 | 说明 | 证据 |
| --- | --- | --- | --- |
| `IrEvent::Start{ id, input_tokens }` | 不发帧（`id` 在 Ollama 面**无处安放**，Ollama 响应没有 message id 字段） | 【官方源码】S1 `ChatResponse` 无 id 字段 |  |
| `BlockStart(Text)` | 不发帧 | Ollama 没有 content block 概念 | 【推断】 |
| `TextDelta{ text }` | 一帧 `{model, created_at, message:{role:"assistant", content:text}, done:false}` | 一 delta 一帧 | 【官方文档】D1 |
| `ThinkingDelta{ text }` | 一帧，`message.thinking = text`（`content` 留空串或缺省） | `thinking` 是 message 内的独立字段 | 【官方源码】S1 `Thinking string \`json:"thinking,omitempty"\`` |
| `BlockStart(ToolUse)` / `ToolInputDelta` | **不发帧，只缓冲** | 因为 `arguments` 是**对象**，无法表达半个 JSON（见 §4.2） | 【推断，基于 S1/D2 的 `arguments: object`】 |
| `BlockStop(ToolUse{id,name})` | 一帧，`message:{role:"assistant", content:"", tool_calls:[{id, function:{index, name, arguments:{…}}}]}`，`done:false` | 整通调用一次性给出 | 【官方文档】D1 流式工具示例（该帧 `done:false`） |
| `BlockStop(Text/Thinking)` | 不发帧 | — | 【推断】 |
| `IrEvent::Usage{ input_tokens, output_tokens, cache_read_input_tokens, … }` | 只暂存，**不单独发帧**；终帧填 `prompt_eval_count` / `eval_count` / `prompt_eval_cached_count` | Ollama 无 usage 块概念 | 【官方文档】D1 终帧；【官方源码】S1 `Metrics` |
| `IrEvent::Stop{ reason }` | 只暂存 → 终帧 `done_reason` | IR 的 reason 已归一化到 Anthropic 词表，需要反归一化，见下 | 【推断】 |
| `IrEvent::Eos` | **终帧**：`{model, created_at, message:{role:"assistant", content:""}, done:true, done_reason?, total_duration?, …}` | 终帧必须发，否则客户端永远不结束 | 【官方文档】D1 |
| `IrEvent::Ping` | 不发（NDJSON 无 keep-alive 帧类型） | 长静默时 Ollama 客户端可能判空闲超时 **【未证实】**（D1/D2 未提 keep-alive） | 【推断】 |
| `IrEvent::Error{ kind, message }` | 一行 `{"error": message}`（HTTP 状态不可再改）或未写出任何帧时用真状态码 + `application/json` | 与 S2/S3 的实现一致 | 【官方源码】S2 `streamResponse`；【官方文档】D3 |

`done_reason` 反归一化建议（**【推断】**，依据是 D1 里工具调用场景的 `done_reason` 也是 `"stop"`）：

| IR `Stop.reason`（Anthropic 词表） | Ollama `done_reason` |
| --- | --- |
| `end_turn` | `stop` |
| `tool_use` | `stop` |
| `stop_sequence` | `stop` |
| `max_tokens` | `length`（**【未证实】**：D1/D2 未给枚举，此为常识性取值，落地前应实测） |
| 上游断流/无终态 | 不发 `done:true` 终帧；回 502 或发 `{"error":…}`（与票 26 的处置一致） |

非流式 `stream:false` 的组装（**【推断】**，范式照票 26 与 `responses.rs::ResponsesAssembler`）：同一个 NDJSON 编码器的「不写帧、只攒终态」版本 —— 把 TextDelta/ThinkingDelta 拼接、tool_calls 收集、Usage/Stop 记下，`Eos` 时一次性回一个 §1.2 形状的 `ChatResponse`（`done:true`，`content` 为全文，`message.tool_calls` 为全量数组）。

---

## 2. `GET /api/tags`

### 2.1 官方形状

**【官方文档】** D1 逐字：

```json
{
  "models": [
    {
      "name": "deepseek-r1:latest",
      "model": "deepseek-r1:latest",
      "modified_at": "2025-05-10T08:06:48.639712648-07:00",
      "size": 4683075271,
      "digest": "0a8c266910232fd3291e71e5ba1e058cc5af9d411192cf88b6d30e92b6e73163",
      "details": {
        "parent_model": "",
        "format": "gguf",
        "family": "qwen2",
        "families": ["qwen2"],
        "parameter_size": "7.6B",
        "quantization_level": "Q4_K_M"
      }
    }
  ]
}
```

字段级（**【官方源码】** S1 `ListModelResponse` / `ModelDetails`，这是 `omitempty` 的权威来源）：

| 字段 | 类型 | `omitempty`？ | 说明 |
| --- | --- | --- | --- |
| `models` | array | — | 顶层唯一键（D2 `ListResponse`） |
| `models[].name` | string | 否（无 omitempty） | **必须给** |
| `models[].model` | string | 否 | **必须给** |
| `models[].remote_model` / `remote_host` | string | 是 | 云端/远端模型才有；本网关不输出 |
| `models[].modified_at` | string（ISO 8601） | 否 | **必须给**；空值会序列化成 `"0001-01-01T00:00:00Z"` |
| `models[].size` | int（**字节**） | 否 | **必须给**（D2：`Total size of the model on disk in bytes`） |
| `models[].digest` | string | 否 | **必须给** |
| `models[].details` | object | 是（`omitempty`） | 但**给了就六个键全出**：`parent_model` / `format` / `family` / `families` / `parameter_size` / `quantization_level` 在 `ModelDetails` 里**都没有 `omitempty`** |
| `models[].capabilities` | string[] | 是 | S1 有、D2 的 `ModelSummary` **未列出**（文档滞后）→ 属可选加成 |

`name` 与 `model` 的差异：**【官方文档】** D1/D2 **都没有说明两者何时不同**（示例里恒等）。**【官方源码】** S2 的 `/api/ps` 处理器把同一个 `displayName` 同时写进 `Name` 与 `Model`；`/api/tags` 的 `ListHandler` 本轮**未逐行核对** → 【推断】本网关两者填同一个值即可。

`modified_at` 格式：**【官方源码】** S1 是 `time.Time`，Go 的默认 JSON 序列化 = RFC3339Nano（D1 示例 `2025-05-10T08:06:48.639712648-07:00` 带纳秒与偏移，符合）。

`/api/tags` 的**查询参数**：**【未证实】**（D1/D2 均未声明；D2 的 `/api/tags` 定义里没有 `parameters`）→ 按「无查询参数」实现。

### 2.2 本网关该列什么（建议形状）

**【推断】**（这是设计建议，不是官方事实）。依据 spec §7：对外集合 = **四个家族槽位名（`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`）+ 普通虚拟模型**；`/api/tags` 应与 `GET /v1/models` **同一集合**，否则「同一个网关在不同客户端里看到的模型不一样」。

建议：`name` 与 `model` 都填虚拟模型名（**带 `:latest` 后缀更符合 Ollama 习惯**，且让「`name` 不带 tag」的客户端也能命中；入站 `model` 解析时两种都要接受 —— **【官方文档】** D1「Model names」：「The tag is optional and, if not provided, will default to `latest`」）。

建议填充（避免撒谎，同时满足「无 `omitempty` 必须给值」）：

| 字段 | 建议值 | 理由 |
| --- | --- | --- |
| `name` / `model` | 虚拟模型名（四家族槽位用槽位名；普通虚拟模型用用户取的名） | 与 `/v1/models` 一致 |
| `modified_at` | 该绑定的最后修改时间（无则网关启动时间），UTC RFC3339 | 客户端多用于排序 |
| `size` | `0` | 网关没有本地权重文件；`0` 是唯一诚实的值 |
| `digest` | `sha256:` + `sha256(虚拟模型名)` 的 hex（**确定性**，不要留空串） | 字段必给；确定性可让客户端去重/缓存稳定，多个模型同为空串会互相碰撞 |
| `details.parent_model` | `""` | 无父模型 |
| `details.format` | `""` | **不要填 `"gguf"`**——那是谎报（本网关不托管权重） |
| `details.family` / `families` | 【推断】填四家族槽位名（家族槽位）或 `""`（普通虚拟模型）；`families` 给 `[]` 或同值数组 | 仅用于显示；避免与真实 llama/qwen 家族混淆 |
| `details.parameter_size` | `""` | 上游真实模型的参数量未知 |
| `details.quantization_level` | `""` | 同上 |
| `capabilities` | 【推断】`["completion","tools"]`（若该模型出站 wire 支持工具） | S1 有此字段；不填也不会挂 |

**同族端点缺口（必须记一笔）**：只做 `/api/tags` 是**半个 Ollama 面**。Ollama 客户端/工具通常还会打 `GET /api/version`（**【官方文档】** D1「Version」→ `{"version":"0.5.1"}`）、`POST /api/show`（模型详情）、`GET /api/ps`（运行中模型）。本票范围只要求 `chat` + `tags`，**这里只做记录，不扩范围**；若要「本机其他工具能直接用」，建议至少补 `/api/version` 与一个最小 `/api/show`（**【推断】**）。

---

## 3. `/v1/chat/completions`：Ollama 自己有没有这个面

**有，而且是官方明确支持的面。**【官方文档】D4 标题即「OpenAI compatibility」，整节讲 `/v1/chat/completions`。

**【官方源码】** S2 `GenerateRoutes()` 注册的全部 `/v1/*` 路由（逐字摘录）：

```go
r.POST("/v1/chat/completions", s.withInferenceRequestLogging("/v1/chat/completions", …middleware.ChatMiddleware(lookupThinking), s.ChatHandler)…)
r.POST("/v1/completions", s.withInferenceRequestLogging("/v1/completions", …middleware.CompletionsMiddleware(), s.GenerateHandler)…)
r.POST("/v1/embeddings", …middleware.EmbeddingsMiddleware(), s.EmbedHandler)
r.GET("/v1/models", middleware.ListMiddleware(), s.ListHandler)
r.GET("/v1/models/:model", …middleware.RetrieveMiddleware(), s.ShowHandler)
r.POST("/v1/responses", s.withInferenceRequestLogging("/v1/responses", s.responsesCompactionMiddleware(), …middleware.ResponsesMiddleware(lookupThinking), s.ChatHandler)…)
r.POST("/v1/responses/compact", s.ResponsesCompactHandler)
r.POST("/v1/audio/transcriptions", middleware.TranscriptionMiddleware(), s.ChatHandler)
r.POST("/v1/messages", s.withInferenceRequestLogging("/v1/messages", …middleware.AnthropicMessagesMiddleware(lookupThinking), s.ChatHandler)…)
r.POST("/v1/systemone", cloudPassthroughMiddleware(…), s.SystemOneHandler)
```

即 Ollama 一家同时提供 OpenAI Chat / OpenAI Responses / **Anthropic Messages**（`/v1/messages`）/ 自家 `/api/*` 四个面。

字段差异（**【官方文档】** D4 的「Supported features / Supported request fields」逐字结论）：

- **支持**：chat completions、streaming、JSON mode、reproducible outputs、vision、tools、reasoning/thinking control、`model`、`messages`（文本/图片/base64/parts 数组）、`frequency_penalty`、`presence_penalty`、`response_format`、`seed`、`stop`、`stream`、`stream_options.include_usage`、`temperature`、`top_p`、`max_tokens`、`tools`、`reasoning_effort`、`reasoning.effort`。
- **不支持**：`logprobs`、**`tool_choice`**、`logit_bias`、`user`、`n`、**图片 URL**（只支持 base64 data URI，S4 `decodeImageURL` 明确 `http(s)://` 报错）。
- `max_tokens` 在兼容面是**支持的**（映射到 `options.num_predict`，S4 `FromChatRequest`）。
- **`/v1/responses` 也存在，但只支持无状态**（D4：「Only the non-stateful flavor is supported (i.e., there is no `previous_response_id` or `conversation` support)」；`Added in Ollama v0.13.3`）。

SSE 与错误形状（**【官方源码】** S3 `ChatWriter.writeResponse` / `writeError`）：`Content-Type: text/event-stream`，逐帧 `data: {json}\n\n`，终态 `data: [DONE]\n\n`；错误用 **OpenAI 嵌套信封**（见 §5.4）。

### 3.1 本网关要不要提供？怎么接？（**【推断】**）

**要提供**——理由不是「学 Ollama」，而是**spec §1 已把 OpenAI Chat Completions 列为四个入站协议之一，§4.3 已写了它**。所以正确的框架是：

- **不是两套入站，也不是两套链路**：`/api/chat` 与 `/v1/chat/completions` 是**两个入站解码器（decoder）+ 两个入站编码器（encoder）**，共用**同一条 IR 与同一条出站链路**（spec §3「所有协议转换经 IR，绝不做 A 协议直转 B 协议」）。
- 两者的关系类似 `anthropic.rs` 与 `responses.rs`：各自 `decode_request` + 各自的流编码器，中间全是 IR。
- ⚠️ **`prototype/src/openai.rs` 目前只有出站**：文件头注释即「OpenAI Chat Completions 编解码：IR -> 出站请求、上游 SSE -> IR 事件」，`encode_request` 是 IR→OpenAI，`OpenAiDecoder` 是**上游 SSE→IR**（不是客户端入站）。**入站 `decode_request` 尚不存在**，实现 `/v1/chat/completions` 入站时要新写。
- Ollama 面与 OpenAI 面的**模型名解析应共用**（同一个虚拟模型表），但**错误信封必须分开**（票 32 的先例：错误信封按入站协议分形状）：`/api/*` 用 `{"error":"<字符串>"}`，`/v1/*` 用 OpenAI 嵌套形状。
- `stream` 缺省值**相反**，容易踩：Ollama 面缺省 `true`（D2），OpenAI 面 `stream` 缺省 `false`（OpenAI 官方语义）。两个 decoder 要各自处理，不要共用「取 stream」的助手函数。

---

## 4. 工具调用

### 4.1 工具定义（请求）

**【官方文档】** D1「Streaming with tools」逐字：

```json
"tools": [
  {
    "type": "function",
    "function": {
      "name": "get_weather",
      "description": "Get the weather in a given city",
      "parameters": {
        "type": "object",
        "properties": {
          "city": { "type": "string", "description": "The city to get the weather for" }
        },
        "required": ["city"]
      }
    }
  }
]
```

**【官方源码】** S1 `api.Tool{Type, Function{Name, Description, Parameters}}`；D2 `ToolDefinition`「`type` (always `function`)」「`parameters`: JSON Schema for the function parameters」。

→ **与 OpenAI Chat 的 `tools` 形状逐字相同**。映射到 IR：`IrRequest.tools` 在网关内统一存 Anthropic 形状（`{"name","description","input_schema"}`，见 `responses.rs::decode_tools`），所以 Ollama 入站也是 `parameters` → `input_schema` 的改名映射，**不是新形状**。

### 4.2 模型产出的工具调用（响应）

**【官方文档】** D1 逐字（注意 `arguments` 是**对象**，且文档示例**没有 `id`、没有 `index`**）：

```json
"tool_calls": [
  { "function": { "name": "get_weather", "arguments": { "city": "Tokyo" } } }
]
```

**【官方源码】** S1 的真实结构体（**与文档示例不一致，以源码为准**）：

```go
type ToolCall struct {
	ID       string           `json:"id,omitempty"`
	Function ToolCallFunction `json:"function"`
}
type ToolCallFunction struct {
	Index     int                       `json:"index"`       // ← 无 omitempty：永远序列化（含 0）
	Name      string                    `json:"name"`
	Arguments ToolCallFunctionArguments `json:"arguments"`   // ← 有序 map，序列化成 JSON 对象
}
```

要点：

1. **`arguments` 是 JSON 对象**，不是 JSON 字符串 —— 与 OpenAI Chat 的 `arguments`（字符串）**相反**。**【官方文档】** D2：「`arguments`: JSON object of arguments to pass to the function」。
2. **真实响应里有 `id` 与 `function.index`**，D1 的示例把它们省略了。【官方源码】S2 在解析器路径上给每个 tool call 补 id：`for i := range toolCalls { toolCalls[i].ID = toolCallId() }`，而 `toolCallId()` 生成 `"call_" + 8 位小写字母数字`（S2 `toolCallId()`）。→ 网关**不能假设客户端发来的 `id` 一定存在，也不能假设一定不存在**。
3. **流式分片**：
   - **【官方文档】** D1 的流式工具示例里，整个 `tool_calls` 数组（含完整 `arguments`）出现在**一个 `done:false` 帧**里。
   - **【官方文档】** D2 对 `ChatStreamEvent.message.tool_calls` 的描述却是「**Partial** tool calls, if any」。
   - → 【推断】文档示例是「通常一次给全」，但规范措辞允许分片。**入站客户端方向不受影响**（客户端是发起方）；**出站编码方向必须按「一次给全」实现**——因为 IR 的 `ToolInputDelta` 是 partial JSON 字符串，而 Ollama 的 `arguments` 是对象，**半个 JSON 无法表示为对象**，所以 Ollama 面必须**缓冲到 `BlockStop` 再一次性发整通调用**。这是 Ollama 面在工具调用上的**结构性限制**（对比 Anthropic 面可以逐 delta 转发）。
4. **`index`** 字段：Ollama 的 `function.index` 用于同轮多工具调用的配对；IR 里没有对应槽位（IR 用 `BlockStart.index` 表示块序号）。**【推断】** 出站编码时按数组下标填即可。

### 4.3 IR 对应关系（模型产出方向）

| Ollama | IR | 说明 |
| --- | --- | --- |
| `message.tool_calls[]` | 每个 → `IrBlock::ToolUse{ id, name, input }` | 放在 `role:"assistant"` 的 `IrMessage` 里 |
| `tool_calls[].function.name` | `ToolUse.name` | 直取 |
| `tool_calls[].function.arguments`（**对象**） | `ToolUse.input`（`serde_json::Value`） | **直接搬，不要 `from_str` 解析**（它已经是对象）。对照 `responses.rs::parse_arguments`：那里要处理字符串/对象两种；Ollama 入站只需对象分支，但为健壮可复用同一个函数 |
| `tool_calls[].id` | `ToolUse.id` | **可能缺失**（D1 示例就缺）→ 缺失时必须自己生成（如 `call_<8位>`，与 S2 的 `toolCallId()` 同形）；**不能生成空串**，因为 IR 靠它配 `ToolResult` |
| `tool_calls[].function.index` | 无对应（丢弃） | IR 用块序号 |

### 4.4 工具结果回传方向（客户端 → 网关）

**【官方文档】** D1「Chat request (With history, with tools)」逐字（这是 D1 唯一一处回传示例）：

```json
{
  "role": "tool",
  "content": "11 degrees celsius",
  "tool_name": "get_weather"
}
```

**【官方源码】** S1 `api.Message` 同时有：

```go
ToolName   string `json:"tool_name,omitempty"`
ToolCallID string `json:"tool_call_id,omitempty"`
```

→ **文档只记载 `tool_name`；源码额外接受 `tool_call_id`。** 官方 OpenAI 兼容面在收到 `tool_call_id` 时会反查工具名（S4 `FromChatRequest`：`if toolName == "" && msg.ToolCallID != "" { toolName = nameFromToolCallID(...) }`）。

**这是 Ollama 面映射到 IR 的最大保真陷阱**：IR 的 `ToolResult` **只有 `tool_use_id`、没有名字**：

```rust
IrBlock::ToolResult { tool_use_id: String, content: Value, is_error: Option<bool> }
```

而 Ollama 原生客户端**只发 `tool_name`**（D1 的示例就是）。所以入站必须：

1. 有 `tool_call_id` → 直接用（并可选按 S4 的做法反查名字做校验/兜底）。
2. 只有 `tool_name` → **在前面的 `role:"assistant"` 轮里按 `name` 反查最近一次同名 `ToolUse` 的 `id`**，填进 `tool_use_id`。查不到时 **【推断】** 退路：合成一个稳定 id（如 `call_name_<name>`）并把这条记成「配对可能失败」，或者直接返回 400 要求客户端带 `tool_call_id`（**推荐前者**，因为 Ollama 原生客户端不会带）。
3. 缺失的配对 id 若放任为空串，出站到 Anthropic 会 400（Anthropic 校验 `tool_use_id` 必须匹配），到 OpenAI Chat 会变成孤立的 `role:"tool"` 行 → 必须在这里补齐。

| Ollama 入站 | IR | 备注 |
| --- | --- | --- |
| `role:"tool"` 消息 | `IrMessage{ role:"user", blocks:[ToolResult{…}] }` | 对应 `openai.rs` 出站时把 ToolResult 提升成独立 `role:"tool"` 行的反向操作 |
| `content`（string） | `ToolResult.content` | 直取（IR 是 `Value`，字符串即可） |
| `tool_call_id` | `ToolResult.tool_use_id` | 首选 |
| `tool_name` | 仅用于**反查** id | IR 不存工具名 |
| （无 is_error 概念） | `is_error: None` | Ollama 面没有错误标记字段；**【官方源码】** S1 确认无对应字段 → 不要把 `content` 里的文本当错误信号猜 |
| `role:"assistant"` 带 `tool_calls` | `IrBlock::ToolUse`（同一轮可多个） | 顺序与数组一致 |

---

## 5. 流式 / 非流式 / HTTP 细节 / 错误形状

### 5.1 汇总表

| 维度 | `stream: true`（缺省） | `stream: false` |
| --- | --- | --- |
| 传输格式 | **NDJSON**，一行一个 JSON 对象，`\n` 分隔 | 单个 JSON 对象 |
| `Content-Type` | **`application/x-ndjson`** | **`application/json`**（S2 `c.JSON` → `application/json; charset=utf-8`；远端代理分支显式 `application/json; charset=utf-8`） |
| 终态表达 | 最后一帧 `done: true` | 响应体的 `done: true` |
| 有无 `[DONE]` | **无** | 不适用 |
| 有无 `data:` 前缀 | **无** | 不适用 |
| 统计字段位置 | 只在终帧（顶层） | 顶层 |
| 证据 | 【官方源码】S2 `streamResponse`；【官方文档】D2 的 `application/x-ndjson` + `ChatStreamEvent` | 【官方源码】S2 `writeChatResponse`；【官方文档】D2 的 `application/json` + `ChatResponse` |

### 5.2 非 200 的错误体

**【官方文档】** D3 逐字：

> Errors are returned in the `application/json` format with the following structure, with the error message in the `error` property:
> ```json
> { "error": "the model failed to generate a response" }
> ```

**【官方源码】** S2 的构造方式证实是**字符串**而非嵌套对象：`c.JSON(http.StatusNotFound, gin.H{"error": fmt.Sprintf("model '%s' not found", req.Model)})`、`c.AbortWithStatusJSON(http.StatusBadRequest, gin.H{"error": "missing request body"})`、`gin.H{"error": err.Error()}`。

**【官方文档】** D2 的 `ErrorResponse` schema 也确认：`error` 的类型是 **`string`**。

状态码（**【官方文档】** D3）：`200` / `400`（缺参数、JSON 非法）/ `404`（模型不存在）/ `429` / `500` / `502`（云端模型不可达）。

本网关会实际用到的：`400`（请求体非法、缺 `model`）、`404`（模型名解析不到，**建议逐字复用 `model '<name>' not found`**，因为 Ollama 客户端/工具对这条文案有既有预期）、`500`（内部错误）、`502`（上游断流/无终态，照票 26 的处置）。

### 5.3 流中错误

**【官方文档】** D3 逐字：

> If an error occurs mid-stream, the error will be returned as an object in the `application/x-ndjson` format with an `error` property. Since the response has already started, the status code of the response will not be changed.

示例（逐字）：正常帧若干行之后追加一行 `{"error":"an error was encountered while running the model"}`。

**【官方源码】** S2 `streamResponse` 的实现更细：如果**还一个字都没写**，会**改回** `Content-Type: application/json` 并用真实状态码 `c.JSON(status, …)`；只有**已经写出过帧**才退化成追加一行 `{"error":…}`。→ 网关的 Ollama 编码器应照抄这个「未写出则真状态码，已写出则追加错误行」的策略。

### 5.4 OpenAI 兼容面（`/v1/*`）的错误体是**另一种**形状

**【官方源码】** S4：

```go
type Error struct {
	Message string  `json:"message"`
	Type    string  `json:"type"`
	Param   any     `json:"param"`
	Code    *string `json:"code"`
}
type ErrorResponse struct {
	Error Error `json:"error"`
}
```

**【官方源码】** S3 `BaseWriter.writeError` 用 `openai.NewError(status, serr.Error())` 输出；S4 `NewError` 的 `type` 取值：`400 → "invalid_request_error"`、`404 → "not_found_error"`、其他 → `"api_error"`。

→ 即 **`{"error":{"message":…,"type":…,"param":null,"code":null}}`**。这与原生面的 `{"error":"<字符串>"}` **完全不兼容**，两者不能混用（正是票 32「错误信封按入站协议分形状」的同类问题）。

### 5.5 兼容面的 SSE 细节（供对照，不用于 `/api/*`）

**【官方源码】** S3 `ChatWriter.writeResponse`：`Content-Type: text/event-stream`；每帧 `data: <json>\n\n`；`chatResponse.Done` 时先发一个 finish chunk，若 `stream_options.include_usage` 为真再发一个纯 usage chunk（`choices: []`），最后 `data: [DONE]\n\n`。**【官方文档】** D4 确认 `stream_options.include_usage` 被支持。

### 5.6 空 `messages` 的特殊语义（网关必须决策）

**【官方文档】** D1 两例 + **【官方源码】** S2：

- `messages: []` → **装载模型**，回 200 `{"model","created_at","message":{"role":"assistant","content":""},"done_reason":"load","done":true}`。
- `messages: []` + `keep_alive: 0` → **卸载模型**，回 200 `{"…","done_reason":"unload","done":true}`。

→ 【推断】本网关**没有模型装载/卸载概念**，建议**直接本地回这两个 200 形状、不打上游**（否则会把一个空请求发给付费上游，且可能 400）。这是「保形状、不保语义」的最小处理。

---

## 6. 与现有 IR / 原型的缺口清单（**【推断】** + 待决）

| # | 缺口 | 现状 | 建议 |
| --- | --- | --- | --- |
| 1 | **`format`（JSON mode / JSON schema）无处安放** | `IrRequest` 没有任何 structured-output 字段；`responses.rs` 也是直接不建模；`openai.rs::encode_request` 从不发 `response_format` | ✅ **已于 2026-10-15 由[票 37](../issues/37-grilling-structured-output-slot.md) 定案**：IR 加归一化槽位 `ResponseFormat`（`Json` / `JsonSchema` / `Other`），表达不了的出站 wire **请求期 400**（不静默丢弃、不放行 + 警告），出站是否真生效由 spec §6 新增的「上游 × 出站协议」能力位把关。**更正本行原假设**：Anthropic **有**原生字段（`output_config.format`，GA，3 层、无 `name`/`strict` 包裹），所以「Anthropic 出站则拒绝」不成立——被拒的只有无 schema 的 JSON mode。一手事实见 [structured-output-wires.md](structured-output-wires.md) |
| 2 | **工具结果配对靠名字反查** | IR `ToolResult` 只有 id（见 §4.4） | 入站补齐 id（按名反查），并考虑给 IR 加可选的 `name` 便于日志与调试 |
| 3 | **Ollama 面必须缓冲工具调用** | IR 的 `ToolInputDelta` 是 partial JSON，Ollama 的 `arguments` 是对象 | Ollama 编码器在 `BlockStart(ToolUse)`→`BlockStop` 之间**只缓冲不发帧**（§4.2 第 3 点） |
| 4 | **`stream` 缺省方向相反** | Ollama 缺省 `true`；OpenAI Chat 缺省 `false` | 两个 decoder 各自处理，不要共用 |
| 5 | **usage 只有一份，且 `prompt_eval_cached_count` 是新字段** | IR `Usage` 有 `input_tokens`/`output_tokens`/`cache_read_input_tokens`/`cache_creation_input_tokens` | `prompt_eval_count → input_tokens`、`eval_count → output_tokens`、`prompt_eval_cached_count → cache_read_input_tokens`；`cache_creation_input_tokens` 无对应 → 略 |
| 6 | **同族端点只做一半** | 只有 `/api/chat` + `/api/tags` | 至少补 `/api/version`；`/api/show`、`/api/ps` 视客户端实测再定（§2.2） |
| 7 | **`created_at` 必须自己盖** | IR 的 `IrEvent::Start.id` 是上游给的；Ollama 响应需要 `created_at` | 编码器在首帧盖一个时间戳，**整个流复用同一个值**（OpenAI 兼容面的先例：S3 注释「OpenAI stamps one created value on every chunk」；Ollama 自己的 `created_at` 则是**一帧一个** —— D1 首帧与终帧时间戳不同。**【推断】** 按 Ollama 的文档示例，逐帧取当前时间即可） |

### 6.1 一句话版本（可直接抄进 spec §4.4 的草稿）

> **§4.4 Ollama 兼容（`/api/chat`、`/api/tags`）**
> 给本机其他工具留的入口。**传输是 NDJSON（`application/x-ndjson`），一行一帧，不是 SSE，没有 `[DONE]`**；终态是末帧的 `done:true`，统计字段（`prompt_eval_count` / `eval_count` / `prompt_eval_cached_count`）只在末帧顶层，`done_reason` 可能整键缺失。`message.content` 是字符串（不是 parts 数组），`message.tool_calls[].function.arguments` 是**对象**（与 OpenAI Chat 相反），因此 Ollama 面**无法流式转发部分工具参数**，必须缓冲到 `BlockStop` 一次性给出。`stream` **缺省为 true**（与 OpenAI 面相反）。错误信封是 `{"error":"<字符串>"}`（**不是** OpenAI 的嵌套形状），流中错误在已写出帧后追加一行 `{"error":…}`、未写出时用真实状态码。`GET /api/tags` 与 `GET /v1/models` 暴露同一集合（四家族槽位名 + 普通虚拟模型），`details.*` 按无权重文件的事实填空值。**`/v1/chat/completions` 不是第二个 Ollama 面**：它是 spec §4.3 的 OpenAI Chat 入站，与 `/api/chat` 共用同一条 IR 与出站链路，只是解码器与错误信封不同。

---

## 7. 未证实 / 需要实测的点

1. **`done_reason` 的完整取值域**：D1 只出现过 `stop` / `load` / `unload`；`length`（对应 `max_tokens`）**未在官方文档中出现**（§1.4 的映射表里已标【未证实】）。落地前用真 Ollama 跑一次 `num_predict: 1` 探针确认。
2. **`/api/chat` 的 `messages[].content` 是否也接受 parts 数组**：D1/D2 都写 string，S1 是 `Content string`（JSON 反序列化收到数组会直接报错）。**【推断】** 只接受字符串 —— 但 Ollama 官方**没有明说不接受数组**。
3. **`name` 与 `model` 在 `/api/tags` 里是否可能不同**：D1/D2 未说明；`/api/tags` 的 `ListHandler` 未逐行核对（§2.1）。
4. **`/api/tags` 是否接受查询参数**：D2 未声明（§2.1）。
5. **流式工具调用是否真会分片**：D1 示例一次给全，D2 措辞说可能 partial（§4.2 第 3 点）。对**入站**无影响；对**出站**我们已按「缓冲后一次给全」设计，故不需要实测也安全。
6. **`thinking` 在流式帧里怎么出现**：D1 的流式示例没展示 thinking 帧（只有非流式的 `thinking` 字段记载）。**【官方源码】** S1 `Message.Thinking` 存在且会被 `writeChatResponse` 累加 → 可以流式出现，但**具体是逐帧还是只在终帧**未证实。
7. **`docs.ollama.com/api/chat` 页面本身**本次抓取失败（连接错误），结论全部来自官方仓库同源文档 + OpenAPI，**不构成证据降级**，但若需要站点版本的措辞需重抓。
