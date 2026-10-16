# OpenAI Chat 入站与入站错误信封收口

Type: task
Status: resolved
Blocked by: —

## Question

实现期收口后的代码评审（`cg-notes/code-review.md`）指出两条**阻断**之一：spec §1 目标 3 / §3 / §4.3 要求的 `POST /v1/chat/completions` **整条不存在**（全路由枚举里没有它，`openai.rs` 只有出站编解码，`lib.rs` 自述「OpenAI Chat 入站未做」；票 40 与票 57 的 Answer 都自认缺，但 39–63 的 M1–M4 里没有哪张票排过它）。打到该路径的请求只会落到空体 404。

本票补齐它，并把评审列出的**入站面错误信封**问题一并收口（它们同属一条链路）。

依据：spec §1 目标 3、§3、**§4.3**、§5、§10「错误表达」；`docs/adr/0001-self-built-thin-conversion-layer.md`；`cg-notes/code-review.md` 的「阻断 1」与三条「中」。

## 交付

- **`POST /v1/chat/completions` 入站**：解码 OpenAI Chat 请求 → IR（`messages` / `tools` / `tool_choice` / 单串 system → IR 的 system 块 / `max_tokens` / `temperature` / `stream` / `response_format` 走票 57 已就位的 `ResponseFormat::from_chat_request`），复用既有 `ingress::proxy` 的 `forward_ir` 链路，不另起一条转发路径。
- 流式 `data:` 帧 + `[DONE]`；非流式把同一批帧缓冲到终态再组装（与 §4.1 同范式）。
- **DeepSeek 的 usage 附在最后一个内容块、不单独发 chunk**（spec §4.3）。
- **错误信封按入站协议给形状**（spec §4.2 / §4.4）：OpenAI Chat 要 `{"error":{"message","type","code"}}`。
- **入站解析失败也要给信封**：语法错的 JSON / 缺 `content-type` 现在落到 axum 默认的 text/plain 400/415/422（评审「中」第一条）——按入站家族分流成上一条那三种形状。Ollama 面已用 `Bytes` 手解且是对的，照它的做法。
- **`/v1/*` 未匹配路径**也按入站家族给信封（评审「中」第三条）；空体 404 只留给真正无协议的路径。
- **`ingress::router` 的 fallback 面**：管理面以后谁加 fallback 会让 `Router::merge` 在启动时 panic（评审「中」第五条）——把两张脸的兜底合成一个，或加一条单测钉死「管理面无 fallback」。
- 顺手两条低：`lib.rs` 文档里关于 `mock` 特性的描述与实际不符，改准确；`proxy.rs` 每请求 `println!` 打 UA / `anthropic-version` / `anthropic-beta` 降到 debug 或只在 body 日志窗口开着时打。

## 收口验收

- 单测 / 回放：Chat 入站**流式与非流式各一条**（黄金帧）、**工具往返一条**、**错误信封三条**（三家入站各一）、**usage 附在最后内容块一条**。
- `pwsh -File ./check.ps1 -Quick -Replay` 全绿（含前端类型检查步）；票 41 / 51 / 56 的回放格**一格不许红**。
- 可选：`-Live` 打一条真实上游的 Chat 入站。
- 前端「模型与映射」页的协议列**不需要**新增取值（入站协议与出站 wire 是两回事）——若你认为界面要能看出「这个虚拟模型能用哪几种入站」，写进 Answer 的建议里，不要在本票顺手改界面。

## 不做

- **上游 `openai-responses` 出站**（评审的另一条阻断）：那是出站面，本票不碰（已在 `Wire::implemented()` 里标为「本版未实现」并挡住误配）。
- **不改 spec / ADR**：spec 与实现的口径差（6 个 Desktop 接管键、`request_frames` 表、`cache_write` 缺价口径、quirks 示例表）由编排者带用户决定，本票不写进 spec。

## Answer

### 做了什么

| 落点 | 内容 |
| --- | --- |
| `crates/gateway-core/src/openai.rs`（+717） | **入站半边**补齐（ADR-0007 第 2 条：三个编解码文件各自保持完整）：`decode_request`（Chat 请求 → IR）、`ChatEncoder`（IR 事件 → `data:` 帧 + `data: [DONE]`）、`ChatAssembler`（非流式攒成 `chat.completion`）；另加 2 条单测（请求映射逐字段 / 拒绝形状） |
| `crates/gateway-core/src/egress/upstream.rs` | 新增 `Inbound::OpenAiChat`（`as_str = "openai-chat"`、SSE、`format_origin = Wire::OpenAiChat`、错误信封走 OpenAI 嵌套那份）；`ClientEncoder::C` / `ClientAssembler::C` 接进**共用的** `forward` / `forward_buffered`（没有第二条转发路径）；Chat 的保活帧是 SSE 注释（`": keep-alive"`，Chat 的 SSE 没有 ping 帧型，也不能塞一个陌生 `data:` 帧） |
| `crates/gateway-core/src/ingress/proxy.rs` | `/v1/chat/completions`（+ 与另两张脸同款的别名 `/chat/completions`）挂进转发面；`messages` / `responses` / `count_tokens` 三处从 `Json<Value>` 换成 `Bytes` 手解（`parse_json`，票 56 的做法），错误按入站家族给信封；每请求打 UA / `anthropic-version` / `anthropic-beta` 的 `println!` 降为「body 日志窗口开着时才打」；路由表拆出 `routes()`（**不带兜底**） |
| `crates/gateway-core/src/ingress/ollama.rs` | `api_fallback` 从「只管 `/api/*`」扩到 `/v1/*`：未匹配路径按入站家族给信封（新增 `v1_family`：路径自带家族按路径，否则按客户端标识头判，Codex → OpenAI 系）；空体 404 只留给真正无协议的路径 |
| `crates/gateway-core/src/ingress/mod.rs` | 两张脸的兜底**合成一个**（`assemble`：`merge` 之后再挂唯一那条 fallback）+ 一条单测钉死「管理面将来自带 fallback 也不会在启动时 panic」；门面 re-export 补 Chat 三件套 |
| `crates/gateway-core/src/lib.rs` | `mock` 特性的描述改准确（评审「低」第一条：真正被挡的只有两个 bin 与 `tests/replay.rs`，其余集成测试照跑）+ 模块表补 Chat 入站 |
| `crates/gateway-core/tests/chat_inbound.rs`（新，11 条） | 全部走真实 HTTP（`proxy::router()` 挂本进程，上游是本进程的假 Chat 面）；黄金帧/黄金 JSON 在 `tests/fixtures/chat-inbound/`（手写，独立来源） |
| `crates/gateway-core/tests/upstream_live.rs` | 新增 `#[ignore]` 的真上游 Chat 入站一发（验收 8） |

**映射口径（Chat → IR）**：`system` / `developer` 轮 → IR 的 system 块；`content` 字符串或 parts 数组（`text` → Text、`image_url` 的 `data:` URL → Image{base64}，别种 URL → `Unknown` 原样带）；`assistant.tool_calls[]` → `ToolUse`（`arguments` 是**字符串**，复用 Responses 面那个「字符串/对象都吃」的解析，缺 id 合成 `call_gw_*`）；`role:"tool"` + `tool_call_id` → `ToolResult`（连续多条并进同一个 user 轮，否则配对失败）；`tools[].function.parameters` → `input_schema` 改名映射；`tool_choice` 三态 / `{type:"function"}` → Anthropic 词表；`max_tokens` 与 `max_completion_tokens` 都认（缺省 4096）；`stop` string|array 归一；`response_format` 走票 57 已就位的 `ResponseFormat::from_chat_request`；`n` / `logprobs` / `stream_options` / `user` 等 Chat 专有字段**丢弃**（不透传 `extra`：出站可能是 Anthropic，理由同 `responses::decode_request`）。`stream` 缺省 **false**（与 Ollama 面相反），但出站永远是 `stream:true`（票 26 决定 3）。

**响应口径（IR → Chat）**：首帧补 `role:"assistant"` 的 chunk（上游 Chat 帧里没有这个信号）、`object:"chat.completion.chunk"`、`id` 由编码器自造（上游的 `chatcmpl-…` 不进 IR）、`model` 回**虚拟模型名**；`ThinkingDelta` → `delta.reasoning_content`（DeepSeek 系那个键，出站解码器读的也是它，推理内容不吞）；`SignatureDelta` 丢弃；工具调用按 `delta.tool_calls[{index,id,type,function:{name,arguments}}]` 分片转；`finish_reason` 反归一化（`max_tokens`→`length`、`tool_use`→`tool_calls`、其余→`stop`）。

**usage 的落点（spec §4.3 的 DeepSeek 怪癖）**：**不单发 usage chunk** —— usage 跟着它之后的第一帧走（上游把 usage 附在最后一个内容块上时，就正好挂在那一帧）；若直到终态都没等到下一帧，就附在 `finish_reason` 那一帧上。非流式放进 `usage`（`prompt_tokens` / `completion_tokens` / `total_tokens` / `prompt_tokens_details.cached_tokens`，缓存写在顶层 `cache_creation_input_tokens`，与出站解码器同一处）。

### 三家入站错误信封的实际形状

| 入站家族 | 语法错的 JSON（`content-type: application/json`，body `{`） | 缺 `content-type` |
| --- | --- | --- |
| Anthropic（`/v1/messages`、`/v1/messages/count_tokens`） | 400 `{"type":"error","error":{"type":"invalid_request_error","message":"请求体不是合法 JSON：<解析器文本>"}}` | 不再是 415 text/plain：body 照常处理（`application/json` 或 `text/event-stream`） |
| OpenAI Chat（`/v1/chat/completions`、别名 `/chat/completions`） | 400 `{"error":{"message":"请求体不是合法 JSON：…","type":"invalid_request_error","code":null}}` | 照常处理；解析得了但查不到模型的请求回 404 `{"error":{"message":"未知虚拟模型 \"nope\"","type":"not_found_error","code":null}}` |
| OpenAI Responses（`/v1/responses`） | 同 Chat 那份嵌套信封（spec §4.2） | 同上 |
| Ollama（`/api/chat`） | 400 `{"error":"<解析器文本>"}`（单层，票 56 原有行为） | 照常处理（票 56 一直是 `Bytes` 手解） |

真上游那一发的实证（第一次跑 `-Live` 少带 quirk 头时拿到的就是本票的信封，逐字）：

```
{"error":{"message":"上游 400 Bad Request：{\"type\":\"error\",\"error\":{\"type\":\"MissingSessionID\",…}}","type":"invalid_request_error","code":null}}
```

`/v1/*` 未匹配路径：`/v1/messages/nope` → 404 Anthropic 信封；`/v1/chat/completions/extra` → 404 OpenAI 信封；`/v1/whatever` 无路径线索 → 默认 Anthropic 信封、带 `user-agent: codex_exec/…` → OpenAI 信封；`/definitely-not-a-protocol` 仍是**空体 404**（口径没扩大）。

### 逐条验收（真实命令与输出结论）

1. **Chat 入站流式 + 非流式（黄金帧）**：`cargo test -p gateway-core --test chat_inbound` → 11 passed。流式那发拿 `fixtures/text/openai.sse` 喂假上游，用 `golden::compare` 逐帧逐字段比 `tests/fixtures/chat-inbound/text.stream.sse`（5 帧：role 帧 / 2 帧正文 / `finish_reason:"stop"`+usage / `data: [DONE]`），并另断言末行是 `data: [DONE]`、全文没有 `event:`；非流式那发比 `text.json` 黄金 JSON（`id`/`created` 掩码），另断言**出站仍是 `stream:true` + `stream_options.include_usage:true`**。
2. **工具往返**：同套 `chat_round_trips_tool_calls_and_tool_results`（比 `tools.stream.sse`）：入站 `assistant.tool_calls` + `role:"tool"` 解成 IR 的 `ToolUse`/`ToolResult`，出站体里 `messages[1].tool_calls[0].id == messages[2].tool_call_id == "call_prev"`、`arguments` 字符串原样、`input_schema` 编回 `parameters`；回程 `delta.tool_calls` 两片 `arguments` 拼接后等于 `{"city":"上海"}`。
3. **错误信封三条**：同套 `parse_failures_get_family_shaped_envelopes` 覆盖上表四行（含语法错的 JSON 与缺 `content-type` 两种输入，并断言 `content-type` 一定是 `application/json`，不是 axum 的 text/plain）。
4. **DeepSeek usage 怪癖**：`deepseek_usage_rides_the_last_content_chunk`（比 `usage-quirk.stream.sse`）：手写的 DeepSeek 形状（usage 与最后一个内容块同帧、没有单独的 usage chunk）→ 出帧里 usage **只出现一次**且就在最后一帧正文上（`prompt_tokens:100` / `cached_tokens:80`），并断言**不存在** `choices: []` + usage 的单独 chunk；非流式同一发也拿到同一份 usage。真上游实测也复现了这条怪癖（见验收 8：usage 附在最后一个 `reasoning_content` 帧上）。
5. **`/v1/*` 未匹配路径**：`unknown_v1_paths_get_family_shaped_envelopes`（含 Codex UA 与无协议路径两条反例）。
6. **fallback 面 panic 风险**：`ingress/mod.rs::tests::two_faces_merge_into_one_fallback_without_panicking`（构造期就能炸，所以只构造不请求）；另外把两张脸的兜底真正合成一个（`assemble`），管理面以后自带 fallback 也只是被覆盖、不再 panic。
7. **门禁**：`pwsh -File ./check.ps1 -Quick -Replay` → **EXIT=0，末行「全部通过」**。unit **184 passed / 3 ignored**（本票新增 3 条单测）；`chat_inbound` **11 passed**；`ollama`（票 56）12 passed；回放套件 **22 passed**（票 41/51/56 的格子一格没红，`fixtures_replay_through_the_gateway` / `client_profile_override_and_models_exposure` / `chunking_invariance_and_header_injection` 全绿）；`replay-unit` 5 passed；web（`tsc --noEmit` + `vite build`）绿。说明：**第一次**跑门禁时 web 步红——本 worktree 的 `app/node_modules` 是空的，`pnpm --dir app build` 自己补装 24 个包时那一步失败（重跑即绿，此后两次门禁都 EXIT=0）；这不是本票改动引起的。
8. **可选 `-Live` 真上游 Chat 入站**：`cargo test -p gateway-core --test upstream_live -- --ignored --nocapture ticket64_chat_inbound` → **ok**（凭据按绝对路径默认值读，未复制、未打印、未入库；出站自动带 `x-opencode-session` quirk）。实测：

   ```
   ===== 票 64：真上游（opencode / deepseek-v4.1-flash / chat wire）的 Chat 入站 =====
   帧数 26（正文 4 字 / 推理 108 字）；finish_reason=["stop"]；
   usage={"prompt_tokens":37,"completion_tokens":73,"total_tokens":110,"prompt_tokens_details":{"cached_tokens":0}}
   test ticket64_chat_inbound_against_a_live_upstream ... ok
   ```

   同一条流里能看到 usage 就挂在最后一个 `reasoning_content` 帧上（跑挂的第一版留下了原样帧）：

   ```
   data: {…"choices":[{"index":0,"delta":{"reasoning_content":" four"},"finish_reason":null}],"usage":{"prompt_tokens":37,"completion_tokens":32,"total_tokens":69,…}}
   data: {…"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}
   data: [DONE]
   ```

### 遗留与坑

- **旧式 `function_call` / `role:"function"` 没做**：验收允许「tool_use / tool_result 或 function_call 成对」二选一，本票做的是新式 `tool_calls` + `role:"tool"`（`arguments` 字符串、`tool_call_id` 配对）。真要接旧客户端再加按名反查那一套（`ollama::pair_by_name` 是现成样板）。
- **usage 总挂在「下一帧」上**：极端情况下 usage 是流的第一帧（还没发过 role 帧），它会挂在 role 帧上；若整条流只有 usage 没有正文，则挂在 `finish_reason` 帧上。客户端读 `chunk.usage`，位置不影响解析（DeepSeek 自己也是附在内容帧上）。
- **`n` / `logprobs` / `stream_options` / `user` 等 Chat 专有字段丢弃**：与 Responses 面同一条判断（`extra` 会被 Anthropic 出站原样塞进请求体 → 上游 400）。要支持 `n>1` 得先给 IR 加多候选槽位，不是本票范围。
- **前端没改**（票里明确要求不改）：入站协议与出站 wire 是两回事。若要让「模型与映射」页看出「这个虚拟模型能用哪几种入站」，得先定一份入站能力的存储口径——留给新票。
- **`parse_json` 的 `Result<Value, Response>` 带 `#[allow(clippy::result_large_err)]`**：`Response` 就是 128 字节，装箱换不来什么（错误路径每请求最多走一次），注释里写了理由。
- **`/chat/completions` 别名**是顺手加的（与 `/messages` / `/responses` 同款，票 15 的 base URL 两份资料互相矛盾），已有一条断言钉住；若认为别名该留给新票，删掉是一行的事。
- 真上游那一发是**推理模型**：正文前先来 108 字 `reasoning_content`，`max_tokens:32` 时会被 `length` 截断——live 用例因此只看「有 delta 载荷 / 有 finish_reason / 有 usage」，不校验正文内容。

