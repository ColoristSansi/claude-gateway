# Ollama 入站（`/api/chat`、`/api/tags`）的形状与映射

Type: research
Status: resolved
Blocked by: —

## Question

给本机其他工具留的 Ollama 入口到底长什么样？形状按 Ollama 官方 API 走，但要落到本网关的 IR 上。

要查清并给出映射表：

1. **`POST /api/chat`**：请求字段（`model` / `messages` / `stream` / `tools` / `options`）与响应形状——是 NDJSON 一行一帧还是 SSE？`done` 怎么表达终态？`message.content`、`message.tool_calls`、以及末尾那帧的 `prompt_eval_count` / `eval_count` / `done_reason` 各在哪个位置。
2. **`GET /api/tags`**：列表项的字段（`name` / `model` / `size` / `details.*`），以及本网关该列什么——四个家族槽位？全部虚拟模型？
3. **`/v1/chat/completions`**：网关要不要同时提供这个 OpenAI 兼容面？要的话它与 Ollama 面是两套入站、还是共用 IR 的同一条链路（注意：出站已经有 OpenAI Chat 编解码，别把入站出站搞混）。
4. **工具调用**：Ollama 面的 `tools` 与 `tool_calls` 形状，以及它跟 IR `ToolUse` / `ToolResult` 的对应关系。
5. **流式与非流式**：非流式（`stream:false`）回哪个形状。

## 背景

- spec §4.4 目前只写了一句「形状按 Ollama 官方 API 走，映射到 IR 后走同一条出站链路」，本票把那句铺开。
- 已解析：[票 14](14-prototype-conversion-core.md)（IR 与两条出站的编解码已经立起来）、[票 26](26-grilling-count-tokens-and-non-streaming.md)（非流式的处理范式：出站仍 `stream:true`，缓冲到终态再组装）、[票 32](32-prototype-codex-e2e.md)（`/v1/responses` 入站是新加入站协议的完整先例，含错误信封按入站协议分形状）。
- 产出写回 spec §4.4；若发现需要新的入站协议分支，再决定要不要开实现票。

## Answer

（2026-10-15，AFK 调研，产出 [research/ollama-inbound.md](../research/ollama-inbound.md)，564 行，每条结论标了来源与证据等级）

1. **`POST /api/chat`**：**NDJSON 一行一帧，不是 SSE**（`application/x-ndjson`，无 `data:` 前缀、无 `[DONE]`）。终态 = 末帧 `done:true`，末帧 `message.content` 是空串，`prompt_eval_count` / `eval_count` / `prompt_eval_cached_count` / 各 duration 在**顶层**（Go 内嵌 Metrics），`done_reason` 也在顶层且**带 omitempty、可能整键缺失**。`stream` **缺省 true**（与 OpenAI 面相反）。`message.content` 是字符串不是 parts 数组；`options.num_predict → max_tokens`、`options.stop` 接受 string|array、`format` 在 IR 无槽位。
2. **`GET /api/tags`**：`{models:[{name, model, modified_at, size, digest, details:{parent_model, format, family, families, parameter_size, quantization_level}}]}`，这些键在源码里**都没有 omitempty → 必须给值**。建议与 `/v1/models` 暴露**同一集合**，`size:0`、`digest` 用虚拟模型名的确定性 sha256、`details.*` 填空串（**不谎报 `format:"gguf"`**）。
3. **`/v1/chat/completions`**：Ollama **自己就提供**这个面（另有 `/v1/responses`、`/v1/messages` 等）。网关按 spec §4.3 本来就该提供，**不是两套入站**：与 `/api/chat` 共用同一条 IR 与出站链路，只解码器与错误信封不同。已核实 `prototype/src/openai.rs` 是**纯出站**编解码，**入站 decoder 尚不存在**（属实现阶段，见 §11 范围）。
4. **工具调用**：`tools` 与 OpenAI Chat 逐字相同（改名映射进 IR `input_schema`）；响应 `arguments` 是**对象**（与 OpenAI Chat 相反）→ Ollama 面**无法表达半个 JSON**，必须缓冲到 `BlockStop` 一次性发。回传方向文档只写 `tool_name`，源码另接受 `tool_call_id`；**入站必须按名字反查前面 assistant 轮的 `ToolUse.id`** 补上，否则 Anthropic 出站 400。
5. **流式 / 非流式 / 错误**：`stream:false` 回单个 JSON，服务端实现就是**缓冲流式帧再组装**（与[票 26](26-grilling-count-tokens-and-non-streaming.md) 范式同构）。错误体是 `{"error":"<字符串>"}`（**不是** OpenAI 的嵌套形状）；流中出错：未写出任何帧 → 真实状态码 + `application/json`，已写出帧 → 追加一行 `{"error":…}`、状态码不变。

**未证实**（已记进 spec §4.4 的「待实测」）：`done_reason` 完整取值域（`length` 属推断）、`messages[].content` 是否接受 parts 数组、`/api/tags` 的 `name` vs `model` 与查询参数、流式帧里 `thinking` 的位置。

**顺带发现的缺口 → 新开两票**：[票 36](36-grilling-ollama-endpoint-scope.md)（同族端点范围与「装载/卸载」特权语义）、[票 37](37-grilling-structured-output-slot.md)（`format` / `response_format` / `text.format` 在 IR 里怎么安放）。spec §4.4 已按本票铺开，两处待决指向上面的新票。
