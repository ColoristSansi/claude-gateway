# 结构化输出（JSON mode / JSON schema）在 IR 里怎么安放

Type: grilling
Status: resolved (2026-10-15)
Blocked by: —

## Question

[票 34](34-research-ollama-inbound.md) 暴露一个跨入站协议的缺口：**结构化输出在 IR 里没有任何槽位**。Ollama 的 `/api/chat` 有 `format`（`"json"` 或一份 JSON schema），OpenAI Chat 面有 `response_format`，Responses 面有 `text.format`，而 `IrRequest` 一个字段都没有；出站也从不发 `response_format`（见 [research/ollama-inbound.md](../research/ollama-inbound.md) §6 缺口 1）。要定：

1. **IR 要不要加槽位**：加 `response_format: Option<Value>`（原样带形状、按出站协议映射），还是各自入站就拒绝、IR 不动？
2. **三条出站的支持面**：OpenAI Chat 有 `response_format`；Responses 是 `text.format`；Anthropic 没有对应字段（上游可能靠 `tools` 或 prompt 实现）。不可表达时怎么办——明确报错、还是放行并记一条警告？
3. **入站收到但出站不支持**：现在 `format` / `response_format` 会被**静默丢弃**（保真度事故），要改成「明确回 `{"error":…}`」还是「丢弃但记一条可观测的警告」？
4. **范围**：这一条是否只定契约（写进 spec §5 / §4.4），实现仍归实现阶段？还是本轮原型就要补。

## 背景

- [票 34](34-research-ollama-inbound.md)、[research/ollama-inbound.md](../research/ollama-inbound.md) §1.1（`options` / `format`）、§6 缺口 1。
- 已解析：[票 14](14-prototype-conversion-core.md)（IR 形状）、[票 32](32-prototype-codex-e2e.md)（Responses 入站的先例：不建模的字段怎么处置）。
- 结论要回写 spec §5（IR）与 §4.4，可能也要在 §4.3 补一句。

---

## Answer

**十条定案（2026-10-15）**。一手事实见 [research/structured-output-wires.md](../research/structured-output-wires.md)（含官方文档/源码出处、两家上游的逐变体实测与逐字错误串）。

### 先更正两条前提（票内假设被推翻）

1. **Anthropic 有原生结构化输出字段，且已 GA**：顶层 `output_config` → `format` → `{type:"json_schema", schema}`，**3 层、无 `name`、无 `strict`、无 `json_schema` 外层包裹**。旧名 `output_format` 已废弃，**必须** `anthropic-beta: structured-outputs-2025-11-13` 才认。硬约束：`additionalProperties` 必须为 `false`，否则 400。不禁止与 `tools` / `tool_choice` 同用；副作用是改 `output_config.format` 会让 prompt cache 失效、并自动注入一段 system prompt（多花 input token）。
2. **状态码不能当支持度**：commandcode 的 Anthropic 兼容端点对 `output_config.format`（含旧名 ± beta 头、连伪造的 `response_format`）**6 个变体全部 200 且逐字静默忽略**；同上游的 Chat wire 反而**真强制**。opencode 的 Anthropic 面因凭据 401 未测出。→ 「200 = 字段被认」这个假设当场作废，也是定案 4/5 存在的理由。

### 定案

1. **IR 加一个归一化槽位**（不再是「原样透传」）：

   ```rust
   pub format: Option<ResponseFormat>,   // IrRequest 新字段

   enum ResponseFormat {
       Json,                                                   // 无 schema 的 JSON mode
       JsonSchema { schema: Value,
                    name: Option<String>,                      // 入站有名字才填
                    strict: Option<bool> },                    // 入站声明了才填
       Other(Value),                                           // 没见过的形状，原样兜底
   }
   ```

   `undefined`（客户端没传）与 `None` 之外的显式空值按 spec §5 硬规则 1 分开处理。
2. **目标 wire 表达不了 → 请求期早拒 400**，错误体按入站协议给形状（Anthropic / OpenAI / Ollama 三种信封），**不静默丢弃**（票 34 判定的保真度事故），也不放行 + 警告。
3. **本票只定契约**：回写 spec §5 / §4.4 / §4.3 / §4.2 / §6，实现归实现阶段。原型没有 Ollama 面、三个客户端也都不发结构化输出字段，终点不需要它。
4. **新增「结构化输出能力位」，粒度 = 上游 × 出站协议**：两态 `native`（缺省）/ `ignore`，另记 `source`（`manual` / `selfcheck`）。被标 `ignore` 时，客户端要结构化输出 → 请求期 400 并说明原因（「该上游的该协议面会静默忽略结构化输出字段」）。它是票 23 表形状上的**正交新列**，不推翻 ADR-0004 的任何一条；也是 §6「哪些协议面服务哪些模型不进 quirks」那条先例的延续——wire 级事实进表，不进 quirks。
5. **能力靠自检判定，不靠状态码**：探针 = 带 schema 的最短请求 + **敌意提示词**（「只说 hello，不要 JSON」），回答不是被 schema 约束住的合法 JSON 就标 `ignore`。没有敌意提示词测不出忽略（模型本来就会吐 JSON）。
6. **OpenAI 的 `name` 固定填常量 `"response"`**：`name` 必填（`a-z A-Z 0-9 _ -`、≤64 字符），但 Anthropic 与 Ollama 的入站都没有名字；不提 `title`、不做消毒、不追虚拟模型名（它常含 `.` `/`，且随绑定变化）。
7. **只做「不补一定失败」的机械修补**：Anthropic 出站时递归补 `additionalProperties: false`（官方硬要求）；`strict` **不主动改**，客户端给什么发什么、没给就吃上游缺省（OpenAI 两条 wire 都缺省 `false`）。
8. **只发 GA 形状 `output_config.format`，网关不代发 `anthropic-beta`**：旧名 `output_format` 属于「上游自己老了」，等真碰到再按 quirk 加；不做「先 GA 失败再降级」的重试（会双发请求、计费与日志出现两次）。
9. **纯 JSON mode 在 Anthropic wire 上早拒 400**：`format:"json"` / `response_format:{"type":"json_object"}` 的语义是「合法 JSON 即可、不限形状」，而 `output_config.format` 只有 json_schema 一种类型、且必须 object + `additionalProperties:false`，写不出「任意 JSON」；硬凑一个最宽松 schema 只会反过来被上游 400。两条 OpenAI 出站照常支持 `json_object`。
10. **出站逐字形状**（`Other` 只在「与入站同形状」的 wire 上原样带，其余 400）：

| 出站 wire | 字段路径 | 层数 | 备注 |
| --- | --- | --- | --- |
| Anthropic | `output_config.format` → `{type:"json_schema", schema}` | 3 | 无 name / strict / 包裹；补 `additionalProperties:false` |
| OpenAI Chat | `response_format` → `json_schema` → `{name, schema, strict}` | 4 | `name` 填 `"response"`；`Json` 映射为 `{"type":"json_object"}` |
| OpenAI Responses | `text.format` → `{name, schema, strict}` | 3 | 同上但**少一层 `json_schema` 包裹**；字段名互斥：Chat 无 `text`、Responses 无 `response_format` |
| Ollama 入站 | 裸 schema 或 `"json"` | — | 只解不编（Ollama 面只有入站）；出站仍走上面三条 |

### 落点

spec §5（IR 槽位与 `ResponseFormat` 形状）、§4.4（`format` 的映射表与三类 400 条件、原「待决」行删除）、§4.3 / §4.2（出站映射各一句）、§6（上游 × 出站协议的能力位与 `source`）；GLOSSARY 加「结构化输出」「结构化输出能力位」；[research/ollama-inbound.md](../research/ollama-inbound.md) §6 缺口 1 标记已结。

### 留给实现阶段的待实测

流式下的行为（本轮实测只走 `stream:false`）；`additionalProperties:false` 是否要求**每个** object 层（本轮只验了顶层）；`json_object` 与 `tools` 同用；旧名 `output_format` 的原始 HTTP 逐字段形状（官方未给）；opencode 的 Anthropic 面（凭据 401，未测出）。

**本票不留新票，无新雾。**
