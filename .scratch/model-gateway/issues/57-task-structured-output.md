# 结构化输出实现

Type: task
Status: resolved (2026-10-15)
Blocked by: 49, 51

## Question

按 [票 37](../issues/37-grilling-structured-output-slot.md) 的十条定案落结构化输出：IR 归一化槽位 + 三条出站映射 + 「上游 × 出站协议」能力位 + 带敌意提示词的自检探针。本票只实现契约，不改判据。

依据：spec §5 / §4.2 / §4.3 / §4.4 / §6、[票 37](../issues/37-grilling-structured-output-slot.md)、`research/structured-output-wires.md`。

## 交付

- IR 槽位 `format: Option<ResponseFormat>` = `Json` / `JsonSchema{schema,name,strict}` / `Other(Value)`（**归一化**，不原样透传）。
- 出站逐字形状：Anthropic `output_config.format`（3 层、无 `name`/`strict`/`json_schema` 包裹、递归补 `additionalProperties:false`）；Chat `response_format` → `json_schema`（4 层）；Responses `text.format`（3 层，`name` 必填）。OpenAI 两条的 `name` 一律填常量 `"response"`，`strict` 只透传客户端显式值。
- 表达不了 → **请求期 400**（不静默丢弃、也不放行加警告）：无 schema 的 JSON mode 打到 Anthropic wire 一律 400。
- 能力位（`native` 缺省 / `ignore` + `source`）挂在「上游 × 出站协议」上；标 `ignore` 时客户端要结构化输出 → 400 并说明。
- 自检探针：带敌意提示词判定能力位——**状态码骗人**（commandcode 的 Anthropic 兼容端点对六个变体全 200 且逐字静默忽略）。

## 收口验收

- 三条出站映射各一条黄金用例（逐字形状，含 `additionalProperties:false` 的递归补法）。
- 400 两条路径（Anthropic + JSON mode；`ignore` 能力位）。
- 探针在两条真实上游实测并给出能力位结论；commandcode 的 Anthropic 面判出 `ignore`。

---

## Answer

（2026-10-15，`task/57-structured-output`，基于集成分支 `f630830`，工作提交 `459fba2`；
收工前已并 `integration/model-gateway`（`42f33ff`）成 `c5e8559`，无冲突；门禁与两条真上游的
探针都在合并后的树上重跑过，全绿）

### 做了什么

契约一字未改，只把票 37 的十条定案落成代码。四个落点：

- **IR 槽位**（`crates/gateway-core/src/ir.rs`）：`IrRequest.format: Option<ResponseFormat>`，
  `ResponseFormat::{Json, JsonSchema{schema,name,strict}, Other(Value)}`。四条入站归一化器
  就在枚举旁边（`from_anthropic_request` / `from_chat_request` / `from_responses_request` /
  `from_ollama_request`），**字段路径写在归一化器里**，各 codec 只调一行：
  `anthropic::decode_request` 读 `output_config.format`、`responses::decode_request` 读
  `text.format`；Chat 与 Ollama 的入站端点还没做（Chat 入站未做、`/api/chat` 归票 56），
  两个归一化器是**给它们备好的公开接口**，本轮由单测钉住。
- **出站映射 + 早拒 + 探针**（新文件 `crates/gateway-core/src/egress/structured.rs`）：
  `apply(body, format, wire, cap, origin)` 一个纯函数管三条 wire 的逐字形状与两条 400；
  调用点只有两个 —— `egress::upstream::forward` 与 `forward_buffered`（出站请求的组装点），
  所以流式 / 非流式 / 探针三条出口共用同一套判据。
- **能力位**（`egress/provider.rs` + `store/schema.rs`）：类型 `StructuredOutput{native,ignore}` /
  `StructuredSource{manual,selfcheck}` / `StructuredCapability`，字段挂在 `Upstream` 上
  （键 = `Wire`），读写走新列 `upstreams.structured_output_json`，写入口 `set_structured_output`。
- **探针**（`structured::probe`）：带 schema 的最短请求 + 敌意提示词，判「回答有没有被 schema
  约束住」，结论按 `source = selfcheck` 落库；**判不出来（非 2xx / 超时 / 连不上）不落库**。

两处刻意的取舍（都为了不制造保真度事故）：

1. `output_config` **不进** `anthropic::decode_request` 的 `known` 名单：它仍走 `extra` 原样带过，
   出站映射只覆盖 `output_config.format`，**兄弟键 `effort` 因此不丢**（整份替换会把它悄悄吃掉）。
2. `Other(Value)` 带的是该入站协议**整份**字段值（Anthropic 是 `output_config`、Chat 是
   `response_format`、Responses 是 `text`、Ollama 是 `format`），所以「同形状就原样带」= 把它放回
   同一个字段。判据由 `apply` 的第 5 个参数给（`Inbound::format_origin()`），不在 IR 里再存一个来源标签
   —— spec §5 的 IR 形状一字不改。

### 三条映射的逐字形状（真跑输出，`structured::apply` 的返回体）

`format = JsonSchema{schema, name: Some("入站的名字"), strict: Some(true)}`，schema 两层嵌套
（顶层 object + `address` 子 object）：

```jsonc
// Anthropic：3 层，无 name / strict / json_schema 包裹；递归补 additionalProperties:false（子层也补）
{"output_config":{"format":{"type":"json_schema","schema":{"type":"object",
  "properties":{"name":{"type":"string"},"age":{"type":"integer"},
    "address":{"type":"object","properties":{"city":{"type":"string"}},"additionalProperties":false}},
  "required":["name","age"],"additionalProperties":false}}}}

// OpenAI Chat：4 层；name 一律常量 "response"（入站那个名字一个字都不发），schema 不补
{"response_format":{"type":"json_schema","json_schema":{"name":"response","schema":{…},"strict":true}}}

// OpenAI Responses：3 层（少一层 json_schema 包裹）；name 与 schema 同层
{"text":{"format":{"type":"json_schema","name":"response","schema":{…},"strict":true}}}
```

- `additionalProperties:false` **只在缺失时补**：客户端显式给了值（哪怕 `true`）一律不动
  —— 那是客户端的语义，改了就是我们替他做决定。递归下到 `properties` / `patternProperties` /
  `$defs` / `definitions` / `items` / `not` / `additionalProperties` / `anyOf` / `allOf` / `oneOf`。
- `strict` 只透传显式值：`Some(false)` 发 `false`、`None` 整个键不出现（吃上游缺省 `false`）。
- `Json`：Chat → `{"response_format":{"type":"json_object"}}`；Responses →
  `{"text":{"format":{"type":"json_object"}}}`；**Anthropic → 400**（见下）。

### 能力位落库位置与迁移版本

- 落点：`upstreams` 表新列 **`structured_output_json`**（形状
  `{"<wire>":{"mode":"native|ignore","source":"manual|selfcheck"}}`），与 `auth_json` /
  `path_templates_json` 同一套「键 = wire」的写法。**缺省 `{}` = 一面都没判过 = 一律 `native`**。
  选一列 JSON 而不是新表：粒度就是「上游 × wire」，跟旁边两列同粒度（spec §6「wire 级事实进表」）。
- 迁移：**v6**（`store/schema.rs::v6_structured_output_caps`），`SCHEMA_VERSION` 5 → 6，
  `ALTER TABLE upstreams ADD COLUMN structured_output_json TEXT NOT NULL DEFAULT '{}'`。
- 它是**运行期事实**，不在 `save_upstream` 的写入范围里（与 `last_pull` / `last_selfcheck` 同类）：
  改一次路径模板不该把自检判出来的结论冲掉。写入口只有 `set_structured_output`。
  读回：`Upstream::structured_output(wire)`（缺省 `native`）与 `structured_capability(wire)`（`None` = 没判过）。

### 400 两条路径（真实 HTTP，`ingress::proxy::router()` 挂在进程里）

| 路径 | 入站 | 出站 | 结果 |
| --- | --- | --- | --- |
| ① 无 schema 的 JSON mode 打到 Anthropic | `POST /v1/responses`，`text.format = {"type":"json_object"}` | anthropic | **400** `{"error":{"message":…,"type":"invalid_request_error","code":null}}` |
| ② 能力位 `ignore` + 客户端要结构化输出 | `POST /v1/messages`，`output_config.format` | anthropic（cap=ignore） | **400** `{"type":"error","error":{"type":"invalid_request_error","message":"该上游的 anthropic 面会静默忽略结构化输出字段（结构化输出能力位 = ignore，状态码照样 200）…"}}`，且**上游一个字节都没收到**（mock 的收到列表为空） |

另有一条 `Other` 的 400（换了形状就表达不了），与「客户端没要结构化输出时能力位无关」一起在单测里钉住。

### 探针在两条真上游的结论与原文

命令（联网，`#[ignore]`；凭据走绝对路径的 `.scratch/model-gateway/prototype-upstream.env`，**没进库、没进输出**）：

```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/57'
cargo test -p gateway-core --test upstream_live ticket57 -- --ignored --nocapture
```

→ `test result: ok. 1 passed; 0 failed`。提示词逐字（三条 wire 同一条）：
`Reply with exactly the word hello and nothing else. Do not output JSON.`

| 上游 / 模型 / 出站面 | status | 上游回答原文 | 能力位结论 |
| --- | --- | --- | --- |
| commandcode / `claude-sonnet-5-5` / **anthropic** | **200** | `hello` | **`ignore`**（`source=selfcheck`）|
| commandcode / `deepseek/deepseek-v4.1-flash` / openai-chat | 200 | `{"a":"hello"}` | `native`（`selfcheck`）|
| opencode / `deepseek-v4.1-flash` / openai-chat | 200 | `{"a":"hello"}` | `native`（`selfcheck`）|
| opencode / `claude-haiku-5-5` / anthropic | 401 | `{"type":"error","error":{"type":"api_error","message":"Upstream request failed: Go requires a credential"}}` | 未测出（不落库）|
| commandcode / `claude-sonnet-5-5` / openai-responses | 400 | `…"Model \"claude-sonnet-5-5\" must be called via /provider/v1/messages (Anthropic Messages shape)."…` | 未测出（该模型不走这个端点）|

**「状态码骗人」当场复现**：commandcode 的 Anthropic 面**200**，回答却是照敌意提示词直说的
`hello`——状态码说「收到」，内容说「没理它」。这正是能力位存在的理由，也是本票唯一硬判据
（`commandcode / anthropic` 必须判出 `ignore`，已断言）。
落库回读（测试里打的）：`commandcode / anthropic → ("ignore","selfcheck")`、
`commandcode / openai-chat → ("native","selfcheck")`、`opencode / openai-chat → ("native","selfcheck")`。

**opencode 的 Chat 面是间歇的（本轮新发现）**：第一发探针（22:38）撞上 **400**
`{"error":{…"message":"Upstream request failed: [invalid_request_error] This response_format type is unavailable now (request_id: …)"}}`
——不是「不支持」，是它自己的调度门。同一份 body 用 curl 直连逐变体复跑（密钥只从 env 读、不打印）：

```
curl -sS -m 120 -o body.json -w '%{http_code}' -X POST "$OPENCODE_BASE_URL/chat/completions" \
  -H "content-type: application/json" -H "authorization: Bearer $OPENCODE_API_KEY" \
  -H "user-agent: claude-gateway-prototype/0.0.1" -H "x-opencode-session: gw-0000000000000001" \
  --data-binary '{"model":"deepseek-v4.1-flash","max_tokens":512,
    "messages":[{"role":"user","content":"Reply with exactly the word hello and nothing else. Do not output JSON."}],
    "response_format":{"type":"json_schema","json_schema":{"name":"response",
      "schema":{"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false},
      "strict":true}}}'
```

四个变体（research 原样 `name=probe`+`strict` / 网关形状 `name=response`+`strict` /
`name=response` 不给 `strict` / 不带 `response_format` 的对照）全部 **200**，前三个
`content = '{"a":"hello"}'`（schema 压过了敌意提示词）、对照 `content = 'hello'`。
随后重跑探针那一发也是 200 → `native`。所以 `opencode / openai-chat` 的**结论是 `native`**，
那条 400 是上游的间歇门；live 测试因此**只打印不硬断言**这一个面（硬断言只有 commandcode 的两条：
Anthropic = ignore、Chat = native），免得把上游的抽风说成我们的判据。

### 每条验收的真实命令与输出结论

1) **三条出站映射的黄金用例**（`crates/gateway-core/tests/structured_output.rs`，19 条）：
```
cargo test -p gateway-core --test structured_output
```
→ `test result: ok. 19 passed; 0 failed`。逐条对应：
- `anthropic_outbound_is_three_layers_with_recursive_additional_properties`（3 层、只有两个键、
  子 object 也补 `additionalProperties:false`、显式 `true` 不动）、
  `chat_outbound_is_four_layers_with_the_constant_name`（4 层、`name` 常量、不给 `strict` 就不出现）、
  `responses_outbound_is_three_layers_under_text_format`（3 层、`name` 与 `schema` 同层、
  且**没有** `response_format`）、`json_mode_maps_to_json_object_on_both_openai_wires`。
- 入站四条：`anthropic_inbound_…` / `chat_inbound_…` / `responses_inbound_…` / `ollama_inbound_…`
  （含 `strict` 显式 `false` 与「没给」分开、`{"type":"text"}` 与缺字段都解成「没要」、
  认不出的形状进 `Other` 并带整份字段值）。
2) **400 两条路径**：`json_mode_on_the_anthropic_wire_is_rejected_with_400`（纯函数）+
  `json_mode_on_an_anthropic_binding_is_400_on_the_real_route`（真实 HTTP，OpenAI 信封）、
  `ignore_capability_rejects_any_structured_output_with_400`（三条 wire 全覆盖）+
  `ignore_capability_is_400_on_the_real_route_with_the_anthropic_envelope`（真实 HTTP，
  Anthropic 信封，且断言上游零收到）。另 `other_only_passes_through_on_the_same_shaped_wire`、
  `anthropic_passthrough_keeps_output_config_siblings`（`effort` 不丢）、
  `anthropic_inbound_lands_as_output_config_format_on_the_upstream`（200 正路，上游收到的逐字形状）。
3) **能力位与探针单测**：`capability_bit_is_per_upstream_and_per_wire_and_defaults_to_native`
  （缺省 native、按 wire 分开、改路径不冲掉、`manual` 覆盖 `selfcheck`）、
  `probe_marks_ignore_when_the_hostile_prompt_wins_and_native_when_the_schema_wins`（两个方向 +
  落库 + 探针形状）、`probe_without_a_verdict_does_not_touch_the_capability_bit`、
  `probe_uses_the_chat_shape_on_the_chat_wire`；迁移在 `store/tests.rs::v6_adds_the_structured_output_capability_column`
  （造 v5 老库升上来，缺省 `{}`、脏键忽略）。
4) **两条真上游实测**：见上一节的表（命令与原文都在那里）。
5) **门禁**：`pwsh -File ./check.ps1 -Quick -Replay`（worktree 根，`CARGO_TARGET_DIR` 指本票自己的
   `cg-target/57`）→ `全部通过。`（fmt + clippy 两套特性 + 单元 5/108/14/8/4/14/19 条 + 回放
   **21 passed** + 回放器切法 5 条；票 41 的回放套件一条没红 —— `format: None` 时映射是空操作，
   出站体逐字不变）。

### 遗留与坑

- **上游 Responses 面仍未实现**：绑到 `openai-responses` 还是 502「尚未实现（M3 起接）」
  （`egress/upstream.rs` 的 `wire_not_implemented` 两处，票 49 留给本票的那句）。本票只交付
  `text.format` 的**映射**（纯函数 + 黄金用例 + 探针在用），整个 Responses 出站 wire
  （请求编码 + SSE 解码）不在本票交付里。今天也没有可用样本：commandcode 的 `/responses`
  对 `claude-sonnet-5-5` 直接 400「必须走 /messages」。
- **opencode 的 Anthropic 面仍未测出**（401 `Go requires a credential`），与研究 §3.3 一致；
  凭据不是 Anthropic 面的。它的能力位因此是「没判过」（`None`），不是 `native`。
- **探针的 `max_tokens` 是 512**（`STRUCTURED_PROBE_MAX_TOKENS`）：敌意提示词只要一个词，但推理型
  模型要先思考；与 research §3.2 的实测探针同档位便于逐字比对。它**不吃**探针档的 20 秒上限
  （那是 `max_tokens <= 16` 的档），超时由调用方传的 `deadline` 给。
- **判据本身的两处已知窄口**（都写在代码注释里，没改判据）：`conforms` 只认 `type` / `required` /
  `properties` 三件事；`additionalProperties` 的递归只下到 Anthropic 支持的那些容器。
- **Chat 入站（`/v1/chat/completions`）还没有端点**：`ResponseFormat::from_chat_request` 已就位并被
  单测钉住，接端点时调一行即可（与票 56 的 Ollama `format` 同一情形）。

