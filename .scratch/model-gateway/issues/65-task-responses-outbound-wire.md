# 上游 OpenAI Responses 出站 wire

Type: task
Status: resolved
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

## Answer

**做了什么**：上游 `openai-responses` 的出站编解码接上了（请求编码 + 上游 SSE 解码），`egress/upstream.rs`
四处恒 502 的早拒换成真实现；票 64 那轮针对这条 wire 的临时措施（`implemented()` 标「本版未实现」+ 界面禁选
+ 保存 400）一并撤掉。

### 落点与理由

- **落在 `crates/gateway-core/src/responses.rs`**，不新开 `egress/` 文件。ADR-0007 第 2 条：三个编解码文件
  **各自含入站与出站、不硬拆两半**；票 64 的先例正是把 Chat **入站**补进 `openai.rs` 同一个文件（而不是
  塞进 `egress/`）。Responses 这半边同理：`encode_request()`（IR → 上游请求体）与 `ResponsesDecoder`
  （上游 SSE → IR）都写在 `responses.rs` 既有的入站解码 / 客户端编码旁边；`egress/upstream.rs` 只按 wire
  选边（`Decoder` 新增 `R` 变体，四处 match 各加一条），与 Anthropic / Chat 两条 wire 完全同构。
- **复用票 57 的三层形状**：`text.format` 仍由 `egress/structured.rs::apply` 落，一个字没改；出站编码器
  **不写 `text`**（免得把结构化输出字段吃掉）。`openai.rs` 的 `tool_result_text()` / `data_url()` 改成
  `pub(crate)` 给 Responses 出站复用（不复制第二份）。
- **字段口径**（spec §4.2 / §6，方向是**发给上游**）：
  - 系统提示 → 顶层 **`instructions`**；Codex 入站那套 `input[].role:"developer"`（票 32 的实测）**不是**
    出站的事，没照抄。
  - `input` 是平铺 item 列表：文本/图片轮收成 `{"role","content":[parts]}`，`function_call` 与
    `function_call_output` **各占一条**（配对键一路是 `call_id`）；工具定义的 IR `input_schema` → `parameters`；
    `max_output_tokens`；`store:false`（续轮只做输入回放、从不发 `previous_response_id`，别让上游留会话）。
  - 有损项**不编不猜**：`stop`（Responses 没有 stop 参数）、`thinking`（拿不到可验证的 `encrypted_content`）、
    `IrBlock::Unknown`。
- **解码**：上游 `output_index` → IR **顺序**块号（与 `OpenAiDecoder` 同一套做法）；`output_text.delta` /
  `reasoning_summary_text.delta` / `function_call_arguments.delta` 逐片进 IR；`output_item.done` 收块；
  `response.completed` → usage + `Stop`（出现过工具调用就 `tool_use`，否则 `end_turn`）+ `Eos`；
  `response.incomplete` → `max_tokens`；`response.failed` 与顶层 `error` 帧 → IR 错误事件。
  **推理项懒开块**：真录帧里 commandcode 的 `reasoning` 项只有 `encrypted_content`、`summary` 是空的，
  先开块就等于给客户端一个空 thinking 块（无 signature）→ 有 summary 正文才开，没有就整块不出现。
- **`wire_not_implemented` → `no_path`**：只剩「`pathTemplates` 缺这一面」一种情形（以前对缺路径的
  Anthropic / Chat 也误用那句「本版未实现」的文案），按 wire 报清楚。
- **临时措施整条撤掉**（不是「翻真留空」——三条 wire 全实现后那套机制没有使用者，留着是死代码）：
  `Wire::implemented()` / `unimplemented_note()` / `UNIMPLEMENTED_WIRE_NOTE`、两个 API 的
  `unimplementedWires` 字段、两条保存路径的 `reject_unimplemented` 早拒、前端 `api.ts` 的两个类型字段与
  `ModelsPage` / `UpstreamsPage` 的「本版未实现」标注 + `disabled`。

### 请求体与帧的真样（真上游）

真上游跑的是 `tests/upstream_live.rs::ticket65_responses_outbound_streams_and_buffers_against_a_live_upstream`
（客户端面 = Anthropic，模型是从真清单里挑的、声明了 `/responses` 的那条；请求体与上游帧从 obs 的 body 开关读回）：

```text
===== 票 65：真上游 Responses 出站（commandcode / gpt-6-luna / openai-responses）=====
（清单 92 条，其中声明 /responses 的 73 条）
[流式] 客户端面帧序列：message_start → content_block_start → content_block_delta → content_block_delta → content_block_stop → message_delta → message_stop
[流式] 正文："收到收到"
[非流式] content=String("收到收到") stop_reason="end_turn" usage={"input_tokens":25,"output_tokens":35,"cache_read_input_tokens":0}

[日志行 1] 发给上游的请求体（非流式那发也一样是 stream:true）：
{"model":"gpt-6-luna","input":[{"role":"user","content":[{"type":"input_text","text":"只回四个字：收到收到"}]}],"max_output_tokens":512,"stream":true,"store":false,"instructions":"你是简洁的助手。"}
[上游帧词表] response.created → response.in_progress → response.output_item.added(reasoning) → response.reasoning_summary_part.added → response.reasoning_summary_text.done → response.reasoning_summary_part.done → response.output_item.done → response.output_item.added(message) → response.content_part.added → response.output_text.delta ×2 → response.output_text.done → response.content_part.done → response.output_item.done → response.completed
```

- 非流式那发的出站请求体与流式**逐字相同**（`stream:true`）——票 26 决定 3：出站永远流式，非流式只是把流缓到终态。
- 上游的 `reasoning` 项（空 summary + `encrypted_content`）**没有**在客户端面变成空 thinking 块，正是上面那条懒开块的判据。
- 票 57 见过 commandcode 的 Responses 面回「必须走 `/messages`」：那是**模型级**的（`claude-sonnet-5-5`），
  不是这个端点不认。本票挑的 `gpt-6-luna` 直接 200，请求体与帧词表如上（真清单里 73 条声明了 `/responses`）。
  opencode 的 Responses 面**没打**（票 64 只验过它的 Chat 面），见「遗留」。

### 验收逐条（真实命令与结论）

1. **单测 / 回放五条** —— `cargo test -p gateway-core --lib responses::` → `12 passed`（新增 6 条）、
   `cargo test -p gateway-core --test responses_outbound` → `10 passed`：
   - 流式黄金 `upstream_responses_streams_into_anthropic_frames` ⇄ `fixtures/responses-outbound/text.stream.sse`
     （上游 Responses 帧 → IR → Anthropic 七帧：`message_start` / `content_block_start` / 两片 delta /
     `content_block_stop` / `message_delta`（带上游 usage）/ `message_stop`）；
   - 非流式黄金 `upstream_responses_non_streaming_buffers_to_one_message` ⇄ `fixtures/responses-outbound/text.json`；
   - 工具往返 `upstream_responses_round_trips_function_calls`：出站 `input[1]=function_call{call_id:toolu_prev}`、
     `input[2]=function_call_output{call_id:toolu_prev}` 成对（断言 `call_id` 相等 + arguments 是字符串 +
     `input_schema`→`parameters`），回程 ⇄ `tools.stream.sse`（`stop_reason=tool_use`、`input_json_delta` 分片原样）；
   - `text.format` 三层逐字 `outbound_text_format_is_three_layers_verbatim`（真出站体：
     `{"format":{"type":"json_schema","name":"response","schema":{…}}}`—— Anthropic 入站没给 `strict` 就整键不出现；
     Responses 入站显式 `strict:true` 才透传）+ 单测 `responses::tests::text_format_keeps_the_three_layer_shape_verbatim`；
   - 错误信封 `upstream_failure_keeps_the_inbound_envelope_shape`：同一条上游、同一个模型，Anthropic 面拿
     `{"type":"error","error":{"type":"api_error",…}}`，Responses 面拿 `{"error":{"message","type","code":null}}`
     —— 按**入站**协议给形状，不是把上游 Responses 的错误形状原样回。
   - 另加 `missing_path_template_is_reported_against_that_face`（`pathTemplates` 缺那一面 → 502 且报 wire 名）。
2. **真上游**：见上（流式 + 非流式各一发 200，请求体与帧词表原样贴出）。凭据用绝对路径读进变量、只打
   长度与掩码，未入库（live 测试自带 `dir_contains_bytes` 明文反查）。
3. **门禁**：`pwsh -File ./check.ps1 -Quick -Replay` → **全部通过**（exit 0）：fmt / clippy /
   `unit`（`190 passed`）+ 前端类型检查与构建步 / mock 特性 clippy / replay（`22 passed`，含
   `fixtures_replay_through_the_gateway`、`responses_inbound_contract`、`client_profile_override_and_models_exposure`、
   `tool_roundtrip_multiturn_and_partial_input`、`error_envelopes_and_route_boundaries`、`ledger_has_no_gaps`）。
   回放矩阵（票 41/51/56/64 那些格）一格没红。
4. **前端**：撤掉禁选后 `pnpm --dir app build` → `✓ built in 82ms`（`tsc --noEmit` + vite 全过）；后端真 HTTP
   证据（`cargo test -p gateway-core --test binding_save_api responses_wire -- --nocapture`）：

   ```text
   [票 65] GET /api/bindings?client=global → wires=["anthropic","openai-chat","openai-responses"]，还带 unimplementedWires 吗：false
   [票 65] PUT /api/bindings/claude-sonnet?client=global wire=openai-responses → 200，saved=true，读回：resp / gpt-5-responses / openai-responses
   [票 65] POST /api/virtual-models wire=openai-responses → 200，triple="resp / gpt-5-responses / openai-responses"
   ```

   （`PUT /api/bindings/{slot}` 就是界面那个下拉点「存」发的请求；重启后读回仍一致。）
5. **反向验证**（改坏两处，都真红，已还原）：
   - 编码器：把系统提示改成 Codex 入站那套（`input` 里插 `role:"developer"`）→
     `responses::tests::encodes_ir_into_an_upstream_responses_request` FAILED；集成那条也红，差异是
     `left: Null / right: "你是助手"`（即 `sent["instructions"]` 没了）。
   - 解码器：`response.completed` 不报 `Eos` → `responses_outbound` 里 4 条 FAILED，黄金报
     `帧数不同：实际 6 帧，黄金 7 帧`，客户端面收到
     `{"type":"error","error":{"type":"upstream_abort","message":"上游在终态事件前断流…"}}`。

### 遗留与坑

- **只发 `.done`、不发 `.delta` 的上游**不在支持面内（解码器把 `output_text.done` / `content_part.done`
  当收尾、不取里面的正文）——真流式上游都发 delta；代码里留了 `shortcut:` 注释，碰到就补。
- opencode 的 Responses 面**没实测**（只验了 commandcode）。它的文档表把 `grok-*` / `gpt-6-luna` 等
  声明成 `openai-responses`，现在这些模型在前端可绑、会走本票这条编码器，但真打要另开一次验收。
- `stop`（上游没有该参数）、thinking 回放、`IrBlock::Unknown` 三条有损项按上面口径丢弃，未做「尽力映射」。
- `no_path` 分支在 `Upstream::new()` 会铺默认模板的前提下只能靠「库里这一面没有模板」触发，
  测试里是显式清空 `path_templates` 造的；真实触发面只有手工改库 / 老数据。
- Responses 的缓存写（`cache_creation`）上游词表里没有 → 一律 `None`、不编 0（缓存读读
  `input_tokens_details.cached_tokens`）。
