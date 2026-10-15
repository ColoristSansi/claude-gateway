# 结构化输出（JSON mode / JSON schema）在三条出站 wire 上的字段事实

范围：只记录事实。每条断言后标 `【官方文档】`/`【官方源码】`/`【实测】`/`【推断】` 并给出处。
调研时间：2026-10-15。密钥不出现在本文件中，一律用环境变量名代替（见 §5）。

---

## 0. 与票 37 原假设相反的两条

1. **【推翻】** 原假设「Anthropic 没有对应字段」在 2026-10-15 **不成立**。Anthropic Messages API 有**原生**结构化输出字段
   `output_config.format`，且已 **GA**（不需要 beta header）。`output_format` 是**旧名**，已废弃，只在带
   `structured-outputs-2025-11-13` beta header 时还认。
   【官方文档】<https://platform.claude.com/docs/en/build-with-claude/structured-outputs>（页头 `featureMetadata.status: ga`）
2. **【实测推翻】** 本项目两个真实上游**都不履行** Anthropic wire 上的结构化字段：commandcode 的
   `/provider/v1/messages` 对 `output_config.format` 返回 200 但**静默忽略**（输出与不带字段时逐字相同）；opencode 的
   `/messages` 用现有凭据直接 401（凭据不通，无法判定）。见 §3.3。

---

## 1. A. Anthropic Messages API

### 1.1 字段名与逐字段形状

当前（GA）字段：**顶层 `output_config` → `format` → `{type, schema}`**，共 **3 层**，`format` 内只有两个键。

```json
"output_config": {
  "format": {
    "type": "json_schema",
    "schema": { "type": "object", "properties": {...}, "required": [...], "additionalProperties": false }
  }
}
```
【官方文档】<https://platform.claude.com/docs/en/build-with-claude/structured-outputs>（cURL 示例与「Send it in `output_config.format`」步骤）

SDK 类型定义（逐字段，`total=False` + `Required`）：

```python
class JSONOutputFormatParam(TypedDict, total=False):
    schema: Required[Dict[str, object]]   # "The JSON schema of the format"
    type:   Required[Literal["json_schema"]]

class OutputConfigParam(TypedDict, total=False):
    effort: Optional[Literal["low","medium","high","xhigh","max"]]
    format: Optional[JSONOutputFormatParam]
```
【官方源码】<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/json_output_format_param.py>、
<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/output_config_param.py>

要点（与 OpenAI 明显不同）：
- **没有 `name`**，**没有 `strict`**，**没有 `json_schema` 外层包裹**。
  Anthropic 的 `output_config.format` 内部直接就是 `{type, schema}`；OpenAI Chat 是 `{type, json_schema:{name, schema, strict}}`（多一层），
  OpenAI Responses 是 `{type, name, schema, strict}`（同层但多两个键）。【官方源码】同上
- `output_config` 第二个键 `effort`（推理力度）与结构化输出无关。【官方源码】同上

### 1.2 旧字段 `output_format`

- 官方原文：「The `output_format` parameter has moved to `output_config.format`, and beta headers are no longer required.
  The `output_format` parameter is deprecated and will be removed in the future. To use it anyway, add the
  `structured-outputs-2025-11-13` beta header. **Without it, the API returns a 400 error.**」
  【官方文档】<https://platform.claude.com/docs/en/build-with-claude/structured-outputs>（Migrating from the beta 一节）
- beta 名字逐字：`structured-outputs-2025-11-13`，出现在服务端 `AnthropicBeta` 枚举里。
  【官方文档】<https://platform.claude.com/docs/en/api/http/beta/messages/create>（`anthropic-beta` 取值列表）
- **未找到**旧 `output_format` 的原始 HTTP 逐字段形状（官方指南只给了 SDK 用法 `output_format=ContactInfo`，
  API reference 页面过大未取到该段）。【未找到】

### 1.3 流式 / 非流式

- 两者都支持。官方原文：「Structured outputs also work with streaming. As responses arrive in stream events, you need
  to accumulate the full response before deserializing the JSON.」【官方文档】同 1.1 页（Streaming 折叠块）
- 非流式就是 1.1 的示例，直接返回。【官方文档】同上
- 官方给的两个「输出不保证符合 schema」的例外：`stop_reason: "refusal"`（仍 200、仍计费）与
  `stop_reason: "max_tokens"`；以及 `enum`/`const` 字符串**大小写不保证**。
  【官方文档】同 1.1 页（Invalid outputs 一节）

### 1.4 限制

显式上限（适用于所有带 `output_config.format` 或 `strict: true` 的请求）：

| 限制 | 值 |
| --- | --- |
| 单请求 `strict: true` 的工具数 | 20（非 strict 工具不计） |
| 可选参数总数 | 24（跨所有 strict 工具 schema 与 JSON 输出 schema 合计） |
| 联合类型参数总数（`anyOf` 或类型数组） | 16 |

【官方文档】同 1.1 页（Schema complexity limits → Explicit limits）

另有内部语法编译大小限制，超出报 400 `"Schema is too complex for compilation."`，以及 **180 秒**编译超时。
【官方文档】同上（Additional internal limits）

**JSON Schema 子集**：

- 支持：全部基本类型；`enum`（仅字符串/数字/布尔/null，不支持复杂类型）；`const`；`anyOf`、`allOf`
  （`allOf` 与 `$ref` 组合不支持）；**`$ref`、`$def`、`definitions`**（**外部 `$ref` 不支持**）；`default`；
  `required` 与 `additionalProperties`（对象上**必须**为 `false`）；字符串格式 `date-time`/`time`/`date`/`duration`/`email`/`hostname`/`uri`/`ipv4`/`ipv6`/`uuid`；数组 `minItems` 仅支持 0 和 1。
- 不支持：递归 schema；外部 `$ref`；数值约束（`minimum`/`maximum`/`multipleOf`）；字符串约束（`minLength`/`maxLength`）；`minItems` 0/1 之外的数组约束；`additionalProperties` 非 `false` 的任何取值。
- **用了不支持的特性 → 400，并带细节**（不是静默降级）。
- 正则：不支持反向引用、前后瞻、`\b`/`\B`、大范围 `{n,m}`；复杂 pattern 可能 400。

【官方文档】同 1.1 页（JSON Schema limitations 两段折叠块 + Pattern support）

**没有 `strict` 类开关**——Anthropic 侧 schema 校验是强制的，`strict` 是**工具**上的开关（`tools[].strict`），不是 `output_config.format` 上的。
【官方源码】1.1 的 `JSONOutputFormatParam`（只有 `schema`/`type`）

### 1.5 与 `tools` / `tool_choice` 同用

**不禁止，且官方明确设计为可同用。** 官方原文：「JSON outputs and strict tool use solve different problems and work together」
——JSON outputs 控制「说什么」，strict tool use 校验「怎么调函数」；示例里同一个请求同时带 `output_config` 与 `tools`。
【官方文档】同 1.1 页（Using both features together）

已知副作用（同页）：
- schema / 工具集合变化会使编译后的语法缓存失效（24 小时滑动缓存）；只改 `name`/`description` 不失效。
- 使用结构化输出会**自动注入一段 system prompt**，输入 token 变多且计费；改 `output_config.format` 会**使该会话的 prompt cache 失效**。
- 有 `mid-conversation-output-config-2026-07-01` 这样的 beta 名存在，说明「会话中途改 output config」本身另有 beta 门控。
  【官方文档】<https://platform.claude.com/docs/en/api/http/beta/messages/create>

### 1.6 模型与平台

- `supportedModels`（官方页面 featureMetadata 列出）：`claude-fable-5-1`、`claude-mythos-5-1`、`claude-fable-5`、
  `claude-mythos-5`、`claude-mythos-preview`、`claude-opus-5-5`、`claude-opus-5`、`claude-opus-4-8`、`claude-opus-4-7`、
  `claude-opus-4-6`、`claude-sonnet-5-5`、`claude-sonnet-5`、`claude-sonnet-4-6`、`claude-sonnet-4-5-20250929`、
  `claude-opus-4-5-20251101`、`claude-haiku-5-5`、`claude-haiku-4-5-20251001`。
- 平台：Claude API / Claude Platform on AWS / Google Cloud / Microsoft Foundry 均 **ga**；Amazon Bedrock 只在**旧**
  integration（Opus 4.6 及更早）上可用，**在 Claude in Amazon Bedrock 上不可用**。
  【官方文档】同 1.1 页（页头 featureMetadata）

### 1.7 第三方兼容端点认不认这个字段

- 官方文档**没有**关于「第三方 Anthropic 兼容代理是否支持」的说明。【未找到】
- 实测（本项目两个真实上游）：**不认**。commandcode 的 `/provider/v1/messages` 静默丢弃；opencode 的 `/messages`
  凭据不通（401）。见 §3.3。【实测】

---

## 2. B. OpenAI Chat 与 Responses 的逐字段形状

### 2.1 Chat Completions `response_format`（顶层字段名就是 `response_format`）

三种变体，**逐字**：

```json
{"type": "text"}
{"type": "json_object"}
{"type": "json_schema",
 "json_schema": { "name": "...", "description": "...", "schema": {...}, "strict": true }}
```

- Chat 的 `response_format` 类型 = `ResponseFormatText | ResponseFormatJSONObject | ResponseFormatJSONSchema`。
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/chat/completion_create_params.py>（`response_format: ResponseFormat`）
- **`json_schema` 外层包裹存在**（这就是与 Responses 的关键差别）。官方 OpenAPI 描述原文：「Setting to
  `{ "type": "json_schema", "json_schema": {...} }` enables Structured Outputs」。
  【官方源码】<https://github.com/openai/openai-openapi/blob/master/openapi.yaml>（`ResponseFormatJsonSchema`，约 L70987）
- `json_schema.name`：**必填**（`required: [name]`），限制原文「Must be a-z, A-Z, 0-9, or contain underscores and dashes,
  with a maximum length of 64」。【官方源码】同 yaml（`json_schema.properties.name`）与
  <https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/response_format_json_schema.py>
- `json_schema.schema`：`$ref: ResponseFormatJsonSchemaSchema`；**注意 Chat 的 spec 里 `schema` 不在 `required` 列表内**
  （只 `required: [name]`），而 SDK 里也是 `schema: Dict[str, object]`（非 Required）。Responses 侧则把 `schema` 列为 required。
  【官方源码】同 yaml 与同 SDK 文件 —— 这是 spec 层的一处不一致，值得记一笔。
- `json_schema.strict`：`anyOf: [{type: boolean, default: false}, {type: "null"}]` → **缺省 false**。
  【官方源码】同 yaml（`ResponseFormatJsonSchema.json_schema.properties.strict`）
- `{"type":"text"}`：默认值，官方描述「Default response format. Used to generate text responses.」
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/response_format_text.py>
- `{"type":"json_object"}`：官方描述「An older method of generating JSON responses. Using `json_schema` is recommended for
  models that support it. **Note that the model will not generate JSON without a system or user message instructing it to do so.**」
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/shared_params/response_format_json_object.py>

### 2.2 `json_object` 的「提示词里必须有 JSON」限制：**仍在**（官方现行文档）

官方原文：「When using JSON mode, you must always instruct the model to produce JSON via some message in the conversation,
for example via your system message. If you don't include an explicit instruction to generate JSON, the model may generate
an unending stream of whitespace and the request may run continually until it reaches the token limit. **To help ensure you
don't forget, the API will throw an error if the string "JSON" does not appear somewhere in the context.**」
【官方文档】<https://developers.openai.com/api/docs/guides/structured-outputs>（JSON mode 一节）

【实测】两个上游都按这条拒：错误串见 §3.2，`"Prompt must contain the word 'json' in some form to use 'response_format' of type 'json_object'."`

### 2.3 哪些模型不支持

- 「Structured Outputs with `response_format: {type: "json_schema", ...}` is only supported with the `gpt-4o-mini`,
  `gpt-4o-mini-2024-07-18`, and `gpt-4o-2024-08-06` model snapshots **and later**.」
- 「Older models like `gpt-4-turbo` and earlier may use JSON mode instead.」
- 对比表：Structured Outputs 兼容 `gpt-4o-mini`、`gpt-4o-2024-08-06` 及更新；JSON Mode 兼容 `gpt-3.5-turbo`、`gpt-4-*`、`gpt-4o-*` 及兼容的 GPT-5 模型。
【官方文档】同 2.2 页（Supported models / Structured Outputs vs JSON mode 表）

`strict: true` 的 schema 要求（官方）：**所有字段必须 `required`**；对象必须 `additionalProperties: false`；
不支持组合关键字（`allOf`/`not`/`dependentRequired`/`dependentSchemas`/`if`/`then`/`else`）；根节点不能是 `anyOf` 且必须是 object；
对象属性总数 ≤ 5000、嵌套 ≤ 10 层；属性名+定义名+enum 值+const 值总字符串长度 ≤ 120000 字符；enum 值 ≤ 1000 个；
微调模型上还额外不支持 `minLength`/`maxLength`/`pattern`/`format`/`minimum`/`maximum`/`multipleOf`/`patternProperties`/`minItems`/`maxItems`。
「If you turn on Structured Outputs by supplying `strict: true` and call the API with an unsupported JSON Schema, you will receive an error.」
【官方文档】同 2.2 页（Supported schemas 各小节）

### 2.4 Responses `text.format`：**同形但少一层 `json_schema` 包裹**

```json
"text": { "format": { "type": "json_schema", "name": "...", "description": "...", "schema": {...}, "strict": true } }
```

- 官方 OpenAPI 组件名 `TextResponseFormatJsonSchema`，`required: [type, schema, name]` —— **`name` 必填**，扁平结构，**没有** `json_schema` 包裹。
  【官方源码】<https://github.com/openai/openai-openapi/blob/master/openapi.yaml>（`TextResponseFormatJsonSchema`，约 L76643）
- 官方 SDK 类型 `ResponseFormatTextJSONSchemaConfigParam`：`name: Required[str]`（同样 a-z/A-Z/0-9/下划线/短横线，最长 64）、
  `schema: Required[Dict[str, object]]`、`type: Required[Literal["json_schema"]]`、`description: str`、`strict: Optional[bool]`。
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_format_text_json_schema_config_param.py>
- `strict` 缺省同样是 **false**（yaml 里 `default: false`）。【官方源码】同 yaml
- 三种变体的 oneOf：`TextResponseFormatConfiguration = ResponseFormatText | TextResponseFormatJsonSchema | ResponseFormatJsonObject`
  —— 即 `{"type":"text"}` 与 `{"type":"json_object"}` 在 Responses 下也放在 `text.format` 里。
  【官方源码】同 yaml（`TextResponseFormatConfiguration`）
- 官方示例（Responses）：`text: { format: { type: "json_schema", name: "math_response", schema: {...}, strict: true } }`
  【官方文档】同 2.2 页（Step 2: Supply your schema in the API call）

### 2.5 两个 wire 的字段名是互斥的（不是一个字段的别名）

- Responses 的创建参数里**只有 `text: ResponseTextConfigParam`，没有 `response_format`**。
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_create_params.py>
- Chat 的创建参数里**只有 `response_format`，没有 `text`**。
  【官方源码】<https://github.com/openai/openai-python/blob/main/src/openai/types/chat/completion_create_params.py>
- 【实测】把 Responses 形状的 `text.format` 打到 Chat 端点：**200 但被忽略**（敌意提示词下输出仍是 `hello`，见 §3.2）。

### 2.6 能否与 `tools`/`tool_choice` 同用

- 在官方 Structured Outputs 指南与 Function calling 指南中**没有找到任何互斥说明**；文档把 Structured Outputs 描述为两种形态
  （function calling / `json_schema` response format）并分别成节。
  【官方文档】<https://developers.openai.com/api/docs/guides/structured-outputs>、
  <https://developers.openai.com/api/docs/guides/function-calling>
- 找到的相关原文只有 strict 的**缺省值差异**：「If you omit `strict`, the default depends on the API: Responses requests will
  attempt to normalize your schema into strict mode when possible, and will fall back to non-strict, best-effort function calling
  if the schema cannot be made compatible... Chat Completions requests remain non-strict by default.」
  【官方文档】<https://developers.openai.com/api/docs/guides/function-calling>（strict mode 一节）
- 结论：**没有文档化的互斥**；但也**没有**一句「可以同用」的明确背书。【推断】

---

## 3. C. 实测：两个真实上游认不认这些字段

方法：`curl` 直连上游，`stream:false`，`max_tokens` 16～512，只比较状态码与**输出内容**。
密钥只从 `.scratch/model-gateway/prototype-upstream.env` 读入环境变量（`OPENCODE_API_KEY` / `COMMANDCODE_API_KEY`），
本文件与任何输出都不含密钥明文。

上游路径与模型（取自原型源码，非猜测）：
- opencode：base `https://opencode.ai/zen/go/v1`，`path_anthropic=/messages`，`path_openai=/chat/completions`，模型 `deepseek-v4.1-flash`
- commandcode：base `https://api.commandcode.ai/provider/v1`，`path_anthropic=/messages`，`path_openai=/chat/completions`，模型 `deepseek/deepseek-v4.1-flash`
- 出站认证两家都是 `Authorization: Bearer`；opencode 另有 quirk：`user-agent: claude-gateway-prototype/0.0.1` 与 `x-opencode-session`（会话 ID 形如 `gw-<16 hex>`）
【官方源码（本仓库）】`.scratch/model-gateway/prototype/src/main.rs` L160-L199、`.scratch/model-gateway/prototype/src/provider.rs` L109-L127、`.scratch/model-gateway/prototype/src/upstream.rs` L136-L164

### 3.1 结果表：上游 × wire × 变体 → 状态码

| 上游 | wire | 变体 | 状态码 | 结论 |
| --- | --- | --- | --- | --- |
| opencode | chat | baseline（不带格式字段） | 200 | 通 |
| opencode | chat | `response_format:{"type":"json_object"}`，提示词无 "json" | 400 | 拒，旧限制仍在 |
| opencode | chat | `response_format:{"type":"json_schema",...}` | 200 | **接受** |
| opencode | chat | 同上 + 畸形 schema（`properties` 是字符串） | 400 | 上游自己校验 schema |
| opencode | chat | `text.format`（Responses 形状） | 200 | **接受但忽略**（见 3.2 判别） |
| opencode | anthropic | baseline | 401 | `Go requires a credential`（凭据不通） |
| opencode | anthropic | `output_config.format` | 400 | `Model is unavailable.`（该端点不服务此模型） |
| commandcode | chat | baseline | 200 | 通 |
| commandcode | chat | `response_format:{"type":"json_object"}`，提示词无 "json" | 400 | 拒，旧限制仍在 |
| commandcode | chat | `response_format:{"type":"json_schema",...}` | 200 | **接受** |
| commandcode | chat | 同上 + 畸形 schema | 200 | **上游不校验 schema**（与 opencode 相反） |
| commandcode | chat | `text.format`（Responses 形状） | 200 | **接受但忽略** |
| commandcode | anthropic | baseline | 200 | 通（需 claude 系模型，见下） |
| commandcode | anthropic | `output_config.format` / `output_config.format`+`strict` / 旧 `output_format`±beta / 伪字段 `response_format` | 全 200 | **全部静默忽略** |
| commandcode | anthropic | 模型换成 `deepseek/deepseek-v4.1-flash` | 400 | 该端点只服务 Claude 模型 |

### 3.2 Chat wire 的判别探针（关键证据）

敌意提示词 `Reply with exactly the word hello and nothing else. Do not output JSON.`，`max_tokens: 512`，schema 要求必填 `a: string`：

| 上游 | 变体 | `finish_reason` | `choices[0].message.content` |
| --- | --- | --- | --- |
| opencode | 对照（不带格式字段） | stop | `hello` |
| opencode | `response_format.json_schema`（strict:true） | stop | `{"a":"hello"}` |
| opencode | `text.format`（Responses 形状） | stop | `hello` |
| commandcode | 对照（不带格式字段） | stop | `hello` |
| commandcode | `response_format.json_schema`（strict:true） | stop | `{"a":"hello"}` |
| commandcode | `text.format`（Responses 形状） | stop | `hello` |

结论：两家在 **Chat wire 上真的强制 `response_format.json_schema`**（敌意提示词被 schema 压过）；
而 **Responses 的 `text.format` 形状在 Chat wire 上被完全忽略**（退回纯文本，与对照逐字相同）。【实测】

错误串逐字（截前 200 字符）：

- opencode Chat，`json_object` 无 "json" 字样：
  `{"error":{"param":"","type":"invalid_request_error","code":"invalid_request_error","message":"Upstream request failed: [invalid_request_error] Prompt must contain the word 'json' in some form to use 'response_format' of type 'json_object'. (request_id: …)"}}`
- commandcode Chat，`json_object` 无 "json" 字样（外层再包一层 JSON 字符串）：
  `{"error":{"message":"{\"message\":\"invalid request error trace_id: …\",\"type\":\"invalid_request_error\"}\n","type":"invalid_request_error"}}`
- opencode Chat，畸形 `json_schema`：
  `{"error":{"param":"","type":"invalid_request_error","code":"invalid_request_error","message":"Upstream request failed: [invalid_request_error] This response_format type is unavailable now (request_id: …)"}}`
- opencode Anthropic：`{"type":"error","error":{"type":"api_error","message":"Upstream request failed: Go requires a credential"}}`
- commandcode Anthropic，非 Claude 模型：
  `{"type":"error","error":{"type":"invalid_request_error","message":"Model \"deepseek/deepseek-v4.1-flash\" is not supported on this endpoint. Use /provider/v1/chat/completions for OpenAI and OSS models…`

### 3.3 Anthropic wire 的判别探针（关键证据）

commandcode `/provider/v1/messages`，模型 `claude-sonnet-5-5`，同一个敌意提示词，`max_tokens: 512`：

| 变体 | 状态码 | `stop_reason` | `content[0].text` |
| --- | --- | --- | --- |
| 对照（不带格式字段） | 200 | end_turn | `hello` |
| `output_config.format` = `{type:"json_schema",schema:{...}}` | 200 | end_turn | `hello` |
| `output_config.format` + 额外 `strict:true` | 200 | end_turn | `hello` |
| `output_format`（旧名）+ `anthropic-beta: structured-outputs-2025-11-13` | 200 | end_turn | `hello` |
| `output_format`（旧名），不带 beta | 200 | end_turn | `hello` |
| 伪字段 `response_format:{"type":"json_object"}` | 200 | end_turn | `hello` |

结论：**字段被完全忽略**，且伪造的字段名也**不报错**（不区分「不认识的字段」与「认识的字段」）。
因此「Anthropic wire 状态码 200」不能作为结构化输出可用的证据。【实测】

opencode `/messages`：用 `.env` 里的凭据拿不到授权（401 `Go requires a credential`），
换 `output_config.format` 变体是 400 `Model is unavailable.` —— **该 (上游, wire) 组合对本项目当前凭据不可用**，
无法测出字段是否被认；本条是「未测出」，不是「不支持」。【实测】

### 3.4 复现命令形状（密钥用环境变量名占位）

```bash
set -a; . ".scratch/model-gateway/prototype-upstream.env"; set +a
UA="claude-gateway-prototype/0.0.1"; SID="gw-0000000000000001"

# --- Chat wire：判别探针（对照 vs response_format vs Responses 形状） ---
curl -sS -o out.json -w '%{http_code}\n' -X POST "${OPENCODE_BASE_URL%/}/chat/completions" \
  -H "content-type: application/json" -H "authorization: Bearer $OPENCODE_API_KEY" \
  -H "user-agent: $UA" -H "x-opencode-session: $SID" \
  --data-binary '{"model":"deepseek-v4.1-flash","max_tokens":512,
    "messages":[{"role":"user","content":"Reply with exactly the word hello and nothing else. Do not output JSON."}],
    "response_format":{"type":"json_schema","json_schema":{"name":"probe",
      "schema":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false},
      "strict":true}}}'

# --- Anthropic wire：GA 字段 + 旧字段 + beta 头 ---
curl -sS -o out.json -w '%{http_code}\n' -X POST "${COMMANDCODE_BASE_URL%/}/messages" \
  -H "content-type: application/json" -H "authorization: Bearer $COMMANDCODE_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "anthropic-beta: structured-outputs-2025-11-13" \
  --data-binary '{"model":"claude-sonnet-5-5","max_tokens":512,
    "messages":[{"role":"user","content":"Reply with exactly the word hello and nothing else. Do not output JSON."}],
    "output_config":{"format":{"type":"json_schema",
      "schema":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false}}}}'
```

（`OPENCODE_API_KEY` / `COMMANDCODE_API_KEY` 从 env 文件读入，命令与文件里都不出现明文。）

---

## 4. D. Anthropic 官方推荐的替代做法

票里的前提「Anthropic 没有原生字段，官方推荐用 tool use 强制 `tool_choice`」在本仓库之外**未找到当前官方页面的对应推荐**：
现在的 `Structured outputs` 页把旧做法替换成了 `output_config.format`，并把 `strict` 挂在**工具**上
（`tools[].strict`，页名 Strict tool use）——即「拿 JSON」走 `output_config.format`，「保证工具入参合法」走 `strict: true`。
【官方文档】<https://platform.claude.com/docs/en/agents-and-tools/tool-use/strict-tool-use>、
<https://platform.claude.com/docs/en/build-with-claude/structured-outputs>（开头的两个 feature 定义）

因此 **D 项的前提问法在本日期已失效**：不需要靠 `tool_choice` 强制工具来拿 JSON。
【官方文档】同上；【推断】「旧页面曾这样建议」这一点未能取得可引用的存证，不予断言。

---

## 5. 未找到 / 未测出（明确留白）

1. 旧 `output_format` 的原始 HTTP 逐字段形状（官方只在迁移一节给了文字与 SDK 用法）。【未找到】
2. Anthropic 官方对「第三方 Anthropic 兼容代理是否支持 `output_config`」的任何说明。【未找到】
3. OpenAI 官方对 `response_format`/`text.format` 与 `tools`/`tool_choice` **同用**的正面或禁止性表述。【未找到】
4. opencode 的 Anthropic wire 是否认 `output_config.format`（凭据不通，401/`Model is unavailable.`，未测出）。【未测出】
5. 两家上游是否在**流式**下同样忽略/履行这些字段（本轮全部 `stream:false`）。【未测出】

---

## 6. 出处清单

| 标签 | URL |
| --- | --- |
| Anthropic 结构化输出指南 | <https://platform.claude.com/docs/en/build-with-claude/structured-outputs>（`docs.claude.com`/`docs.anthropic.com` 同路径会 302 到这里） |
| Anthropic beta 头总表 | <https://platform.claude.com/docs/en/api/beta-headers> |
| Anthropic Messages（含 beta 名枚举） | <https://platform.claude.com/docs/en/api/http/beta/messages/create> |
| Anthropic Strict tool use | <https://platform.claude.com/docs/en/agents-and-tools/tool-use/strict-tool-use> |
| anthropic-sdk-python 类型 | <https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/json_output_format_param.py>、`.../output_config_param.py` |
| OpenAI 结构化输出指南 | <https://developers.openai.com/api/docs/guides/structured-outputs>（`.md` 后缀可得纯 Markdown） |
| OpenAI Function calling 指南 | <https://developers.openai.com/api/docs/guides/function-calling> |
| OpenAI 官方 OpenAPI 规范 | <https://github.com/openai/openai-openapi/blob/master/openapi.yaml>（`ResponseFormatJsonSchema` ≈L70987，`TextResponseFormatJsonSchema` ≈L76643） |
| openai-python 类型 | `.../src/openai/types/shared_params/response_format_json_schema.py`、`.../shared_params/response_format_json_object.py`、`.../shared_params/response_format_text.py`、`.../responses/response_format_text_json_schema_config_param.py`、`.../responses/response_create_params.py`、`.../chat/completion_create_params.py` |

抓取过程中的失败与重定向：`docs.claude.com`/`docs.anthropic.com` → 302 到 `platform.claude.com`（跨域，需直接访问目标）；
`https://platform.claude.com/en/api/messages` 与 `/en/api/messages` 404（正确路径是 `/docs/en/api/...`）；
`developers.openai.com/llms.txt` 与 `/api/reference/.../*.md` 多条超时或 404；`raw.githubusercontent.com` 用 `web_fetch` 会超时，改用 `curl` 全部 200。
