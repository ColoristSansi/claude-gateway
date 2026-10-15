# Ollama 兼容面

Type: task
Status: resolved
Blocked by: 51

## Question

按 spec §4.4 落 Ollama 兼容面：`/api/chat`、`/api/tags`、`/api/version`、`/api/ps`、`/api/show`，外加「装载 / 卸载」特权语义。与 Anthropic / OpenAI 三个入站**共用同一条 IR 与出站链路**，只有解码器与错误封装不同。

依据：spec §4.4、[票 34](../issues/34-research-ollama-inbound.md)、[票 36](../issues/36-grilling-ollama-endpoint-scope.md)、`research/ollama-aux-endpoints.md`。

## 交付

- `/api/chat`：**NDJSON 一行一帧**（不是 SSE），末帧 `done:true` 才是终态；`message.content` 是字符串、`tool_calls[].arguments` 是**对象** → 工具调用缓冲到 block stop；回传只有 `tool_name` → 按名反查补 `tool_use_id`（否则 Anthropic 出站 400）；错误体 `{"error":"…"}`。
- `/api/tags`：与 `/v1/models` 同一集合，`name` / `model` 都填**裸虚拟模型名**、`digest` 用名字的 sha256、不谎报 gguf；入站解析接受并剥掉 `:latest`。
- `/api/version`：回真实版本串 + 顺带实现 `HEAD`；`/api/ps` 回 `{"models":[]}`（空数组非 `null`）。
- `/api/show`：只填能诚实填的键——必给 `details` 六子键 + `model_info:{}` + `modified_at`；`modelfile` / `parameters` / `template` / `license` / `thinking` 整键省略。
- 装载 / 卸载（空 `messages`；`keep_alive:0`）**本地回假终态不打上游**；`messages` 非空时 `keep_alive` 忽略。
- 未知名统一 404 `model '<name>' not found`；其余 `/api/*`（`pull`/`create`/`copy`/`delete`/`blobs/*`/`generate`/`embed`/`embeddings`）统一 404。

## 收口验收

- 回放 / 单测：五个端点 + 两处怪癖（NDJSON 终态、按名反查补 id）+ 装载卸载 + 错误信封。
- `ollama run` 或 curl 实测一条问答走通（本机没有 Ollama 客户端时用 curl 记录）。

## Answer

### 做了什么

| 落点 | 内容 |
| --- | --- |
| `crates/gateway-core/src/ingress/ollama.rs`（新，1362 行含 16 条单测） | 解码器 `decode_request`、NDJSON 编码器 `OllamaEncoder`、非流式组装器 `OllamaAssembler`、五个 handler、`/api/*` 兜底 |
| `crates/gateway-core/src/egress/upstream.rs` | 新增 `Inbound::Ollama`（+ `as_str` / `stream_content_type`）：流式 Content-Type 换 `application/x-ndjson`、错误信封换单层、**不发保活帧**（NDJSON 没有 ping 帧型）；编码器 / 组装器接进共用的 `forward` / `forward_buffered` |
| `crates/gateway-core/src/ingress/proxy.rs` | 从 `proxy()` 里抽出 `forward_ir`（四个入站共用的尾段：上游 → 三层校验 → 凭据 → 出站）；`router()` 挂 `ollama::router()` + `/api/*` 兜底；`Gateway` 加 `started_at`；`client_identity` / `dump_inbound` 提为 `pub(crate)` |
| `crates/gateway-core/tests/ollama.rs`（新，826 行 / 12 条） | 全部走真实 HTTP（`proxy::router()` 挂在本进程，上游是本进程的假 Anthropic 面） |
| `crates/gateway-core/src/responses.rs` | `parse_arguments` / `push_block` 提为 `pub(crate)` 复用（工具连发要并进同一个 user 轮） |
| `Cargo.toml` | 加 `sha2 = "0.10"`（`digest` 用；已在锁文件里，无新下载） |

**口径差（本票要求的记录）**：spec §4.4 的「其余 `/api/*` 一律 404」**只就 Ollama 面而言，不覆盖管理面**。`/api/*` 被两张脸共用，实现是「五个具体路由 + 一条兜底」：兜底是 axum 的 `fallback`，**只吃没被任何具体路由匹配**的路径，且只对 `/api/` 开头的路径回 Ollama 信封（别的路径维持原来的空体 404）。因此管理面 `/api/status`（票 43）、`/api/logs`、`/api/logs/filters`、`/api/logs/body-recording`、`/api/usage/summary`（票 53）以及原型自用的 `/api/routes`、`/api/echo` 都不会被吃；实测前四个 GET 路由全 200（见验收 ②⑩），集成用例还把 `ingress::router`（管理面 + 转发面合并后的那份）整份挂起来跑了一遍。兜底 404 的**文案是本网关自己定的**（`{"error":"/api/pull is not implemented by this gateway"}`）：真 Ollama 对未知路径回 gin 的纯文本 `404 page not found`，而对已实现的 `pull`/`create` 根本没有「未实现」这一态可比，用本面信封至少客户端能解析。

### 逐字段映射最终形态

**请求 `/api/chat` → IR**（`decode_request`）

| Ollama | IR | 取舍 |
| --- | --- | --- |
| `messages[].role` | `system` → `IrRequest.system`；`user`/`assistant` 原样；**`tool` → `user` 轮 + `ToolResult`** | 连续多条 `tool` 用 `push_block` 并进**同一个** user 轮（拆开会配对失败） |
| `messages[].content` | `Text`（字符串；空串不建块） | 只接受字符串（D2/S1） |
| `messages[].images` | `Image{source:{type:base64, media_type:<按 base64 前缀嗅探>, data}}` | 裸 base64 没 media_type，按前缀认 png/jpeg/gif/webp，认不出给 `image/png`（Anthropic 出站要求合法值） |
| `messages[].thinking` | `Thinking{signature:None}` | — |
| `messages[].tool_calls[]` | `ToolUse`（assistant 轮） | `arguments` 是对象，走 `parse_arguments`（字符串/对象都吃）；`id` 缺就合成 `call_gw_N`，**不留空串** |
| `tool_call_id` / `tool_name` | `ToolResult.tool_use_id` | `tool_call_id` 首选；只有 `tool_name` 时按名反查**最近一次同名且还没配过对**的 `ToolUse.id`（同名两次配到两个不同 id，避免串对）；查不到给 `call_name_<name>` + 一行 warn，**不 400**；`is_error` 恒 `None` |
| `tools[]` | `{name, description, input_schema}` | `parameters` → `input_schema` 的改名映射；无名工具丢掉 |
| `tool_choice` | — | Ollama 没有这个概念 |
| `options.temperature` / `top_p` | 直取 | — |
| `options.stop`（string \| array） | `IR.stop` 数组 | 归一 |
| `options.num_predict` | `max_tokens` | `-1`/缺省 → 4096（票 26/32 先例） |
| `options.seed` / `top_k` / `min_p` / `repeat_*` / `presence_penalty` / `frequency_penalty` | **丢弃** | 票 34 草稿建议进 `extra`，但 `anthropic.rs` 会把 `extra` 原样塞进请求体（这些键在那里是非法顶层字段 → 上游 400），而本端口岸没有任何 wire 读 `extra`；静默丢一个采样旋钮好过让整条请求 400。装载参数（`num_ctx`/`num_gpu`/…）与已废弃 `typical_p` 同理 |
| `think`（bool \| string） | **丢弃** | IR 的 `thinking` 是 Anthropic 形状（`{"type":"enabled","budget_tokens":N}`），bool 转过去必然上游 400；不编 budget，如实忽略（thinking 仍由上游模型自己决定） |
| `format` | `ResponseFormat::from_ollama_request`（票 57 已备好） | 裸 schema → `JsonSchema`、`"json"` → `Json`、空串/`null` → 无。**只解不编**，出站走三条 wire 的映射；`"json"` 打 Anthropic 是请求期 400（票 57 定案 2）——本票补了两条断言 |
| `stream` | 不进 IR，决定编码方式 | **缺省 `true`**（与 OpenAI 面相反，两处 decoder 没共用助手） |
| `keep_alive` | 无 IR 字段 | `messages` 非空时一律忽略；空 `messages` 时 `0`/`"0"`/`"0s"` = 卸载，`-1`（永不卸载）不是 |
| `truncate` / `shift` / `logprobs` / `top_logprobs` / `runner` / `all_manifests` | 忽略 | `runner`/`all_manifests` 在 `/api/show` 里也是接受并忽略（绝不返回另一套 `ShowManifestsResponse`） |

**响应 → NDJSON 帧 / 单个 JSON**（`OllamaEncoder` / `OllamaAssembler`）

| IR 事件 | 输出 | 取舍 |
| --- | --- | --- |
| `Start` | 不发帧 | 响应里没有 message id 字段 |
| `BlockStart(Text)` | 不发帧 | Ollama 没有 content block 概念 |
| `TextDelta` | 一帧 `{model, created_at, message:{role, content:text}, done:false}` | `model` = **请求里那个名字**（官方回显 `req.Model`；`:latest` 只在查表时剥） |
| `ThinkingDelta` | 一帧 `content:""` + `thinking:text` | 实测官方逐帧发且 `content` 键照发空串 |
| `BlockStart(ToolUse)`～`BlockStop` | 只缓冲，`BlockStop` 发一帧 `tool_calls:[{id, function:{index:0, name, arguments:{…}}}]` | `arguments` 是对象 ⇒ 半个 JSON 表示不出来；`index` 无 omitempty 恒发（一帧只带一通完整调用，数组下标即 0）；**终帧不重复给** tool_calls（客户端会累加） |
| `Usage` / `Stop` | 只暂存 | `prompt_eval_count` / `eval_count`（`int`+omitempty ⇒ 0 不出现）、`prompt_eval_cached_count`（`*int` ⇒ 给了就照发，含 0——实测 0.40.4 真回 0） |
| `Eos` | 终帧 `{model, created_at, message:{role,content:""}, done:true, done_reason?, 统计…}` | `done_reason`：`max_tokens` → `length`，其余 → `stop`；上游没给 `Stop` 就整键缺失 |
| `Error` | 追加一行 `{"error": message}` | 帧已经写出去了，状态码不变；未出帧的失败（上游非 2xx / 连不上）走真状态码 + `application/json` |
| `Ping` | 不发 | NDJSON 没有保活帧型 |
| `stream:false` | 同一批事件攒成**一个** `ChatResponse`（`application/json`） | 与票 26 的非流式同范式 |

**五个端点 + 装载卸载的最终形状**

- `/api/tags`：集合 = `BindingTable::exposed()`（四家族槽位名在前 + 普通虚拟模型，跨档案去重，与 `/v1/models` 逐字同一份）；`name` = `model` = 裸虚拟模型名；`size: 0`；`digest` = 名字的 **sha256 裸 hex**（不带 `sha256:` 前缀——实测官方两个端点的 `digest` 都是裸 hex，**销掉 ollama.md §12 冲突 3**）；`details` 六子键恒在，`format: ""`（不谎报 gguf）、`parent_model: ""`、`parameter_size`/`quantization_level: ""`；家族槽位 `family` = 槽位名、`families` = `[槽位名]`，普通虚拟模型 `""` / `[]`（**销掉冲突 4：families 是数组**）；`capabilities: ["completion","tools"]`；`modified_at` = **网关启动时刻**（绑定的最后修改时间库里没存，spec §4.4「无则网关启动时间」）RFC3339 UTC。
- `/api/version`：`{"version":"0.13.3"}`，唯一键；`HEAD` 由 axum 的 `get()` 自动兜住（官方同一 handler，实测 HEAD 200 + content-length）。
- `/api/ps`：`{"models":[]}`；**显式压掉 HEAD → 405**（实测官方 `/api/ps` 只有 GET，HEAD 回 405 `Allow: GET`）。
- `/api/show`：只给 `details`（六子键）/ `model_info:{}` / `modified_at`；13 个可省略键一个不给（含 `capabilities`——**销掉 ollama.md §12 冲突 2：以 spec 为准，show 省略 capabilities，tags 才给**）；请求体用 `model`，`name` 是兜底（两者都给 `model` 胜出），`verbose` 接受并忽略。
- 装载 / 卸载：空（或缺）`messages` → 200 单个 JSON 对象（**实测官方即便 `stream` 缺省 true 也回 `application/json` 的单对象**），`done_reason` = `load` / `unload`，统计字段全不填；**模型名先解析**（实测未知名 + 空 `messages` 官方回 404，不是 load）。
- 404/400 文案（逐字，全部实测过）：`model '<name>' not found`（回显请求里的名字，含 `:latest`）、`missing request body`、`model is required`；非法 JSON 用解析器文本（官方那边是 gin bind 的文本）。

### 三项「待实测」的处置（**全部做了实测**，不是猜）

本机装有 Ollama **0.40.4**，服务在 11434 上跑着（`granite4.2:3b` / `gpt-oss:20b`），所以三项都真跑了探针（只读探针；`/api/ps` 探针前是空的，load/unload 一发一收，没留驻留状态）：

1. **`done_reason` 取值域** —— 实测到 4 个值：`stop`（正常收尾与工具调用）、**`length`**（`options.num_predict:1` 截断，`"done_reason":"length"`）、`load`、`unload`。所以 `max_tokens → length` 从【未证实】升为**实测证实**；没有观测到第 5 个值。探针：`curl -s http://127.0.0.1:11434/api/chat -d '{"model":"granite4.2:3b","stream":false,"options":{"num_predict":1},"messages":[{"role":"user","content":"Count from 1 to 100."}]}'` → `...{"done":true,"done_reason":"length",...}`。
2. **流式帧里 `thinking` 的位置** —— 实测：**逐帧发**，每帧形如 `{"model":…,"created_at":…,"message":{"role":"assistant","content":"","thinking":"Okay"},"done":false}`；`content` 键**照发空串**；进入正式回答后 thinking 键整键消失（omitempty）。我们的编码器就是照这个形状写的。
3. **结构化输出的流式行为** —— 实测：**没有专用帧型**，`format`（裸 schema 与 `"json"`）之下的流就是普通 `content` 增量帧，末帧照旧 `done:true` + `done_reason:"stop"`；`think:false` 被接受（用于把思考帧压掉看清 content 帧）。例：`curl -sN http://127.0.0.1:11434/api/chat -d '{"model":"granite4.2:3b","stream":true,"think":false,"format":{"type":"object","properties":{"city":{"type":"string"}}},"messages":[…]}}'` → 若干 `"content":"{"` / `"content":"\n"` / … → `{"…","done":true,"done_reason":"stop","prompt_eval_count":27,"prompt_eval_cached_count":26,…}`。`additionalProperties:false` 是否要求每层、`json_object`+`tools` 同用、旧名 `output_format` 属票 37/57 的范围，本票没动。

**顺带测到的三件事**（写进代码注释与上面）：① 0.40.4 的流式工具调用可能**把整通 `tool_calls` 放在终帧**（`done:true` 里），而 D1 文档是放在 `done:false` 帧——我们按 D1/spec 在 `BlockStop` 发、终帧不重复（客户端是跨帧累加的，两种都吃得下；重复给会执行两次）；`function.index:0` 与 `id` 实测都在，id 不一定是 `call_`+8 位。② `/api/ps` HEAD = 405。③ `/api/tags` 的 `digest` 是裸 hex、`capabilities` 真存在（实测 `["tools","thinking","completion"]`）。

### 逐条验收（真实命令与结论）

1. **回放 / 单测**：`cargo test -p gateway-core --lib ollama` → **16 passed**（解码边界、两处怪癖、装载卸载的形状、错误信封、`digest` 稳定性、`:latest`、`keep_alive`、`format`）；`cargo test -p gateway-core --test ollama` → **12 passed**（五个端点 + 两处怪癖 + 装载卸载 + 错误信封 + 兜底与管理面共存）。
   `cargo test -p gateway-core --test ollama` 的真实尾部：
   ```
   test chat_streams_ndjson_and_ends_with_a_terminal_done_frame ... ok
   test tool_call_is_buffered_whole_and_tool_results_pair_by_name ... ok
   test empty_messages_load_and_unload_without_touching_the_upstream ... ok
   test tags_mirrors_v1_models_with_honest_fields ... ok
   test version_and_ps_are_shape_exact ... ok
   test show_fills_only_the_honest_keys ... ok
   test error_envelopes_are_ollama_shaped ... ok
   test format_is_decoded_into_the_ir_and_mapped_outbound ... ok
   test unknown_api_paths_are_404_and_the_management_plane_survives ... ok
   ... test result: ok. 12 passed; 0 failed
   ```
2. **curl 实测一条问答走通**（两条都跑了；脚本在 target 目录，不進库）：
   - **mock 上游**（`gateway-harness` + `mock-upstream` 两个独立进程，`mock-anthropic-text`）：
     ```
     $ curl -sN http://127.0.0.1:28861/api/chat -H 'content-type: application/json' --data-binary @b1.json
     {"model":"mock-anthropic-text","created_at":"2026-10-15T14:55:40Z","message":{"role":"assistant","content":"你好，"},"done":false}
     {"model":"mock-anthropic-text","created_at":"2026-10-15T14:55:40Z","message":{"role":"assistant","content":"这是 mock 上游的纯文本回放。"},"done":false}
     {"model":"mock-anthropic-text","created_at":"2026-10-15T14:55:40Z","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":12,"eval_count":18}
     HTTP/1.1 200 OK / content-type: application/x-ndjson / cache-control: no-cache / transfer-encoding: chunked
     ```
     同一发 `stream:false` → 单个 JSON（`{"…","content":"你好，这是 mock 上游的纯文本回放。","done":true,…}`）；工具场景（`mock-anthropic-tools`）→ 文本帧 + **一帧完整工具调用**（`{"id":"toolu_mock_1","function":{"index":0,"name":"get_weather","arguments":{"city":"上海"}}}`，`done:false`）+ 终帧（无重复 tool_calls）；`messages:[]` → `done_reason:"load"`、`+keep_alive:0` → `"unload"`；「装载 / 卸载绝不打上游」这条由集成用例断言（`empty_messages_load_and_unload_without_touching_the_upstream`：三次装载/卸载之后 `mock.len() == 0`），curl 里能看到的只是两个 200 与上游没收到请求（网关 stdout 只打印 `→ load（本地假终态）`）。
   - **真实上游**（凭据文件按**绝对路径**只交给 harness，未复制、未打印、未入库）：`oc-deepseek`（opencode + openai-chat wire）
     ```
     {"model":"oc-deepseek","created_at":"…","message":{"role":"assistant","content":"","thinking":"我们需要"},"done":false}
     …（思考帧若干）
     {"model":"oc-deepseek","…","message":{"role":"assistant","content":"1"},"done":false}
     {"model":"oc-deepseek","…","message":{"role":"assistant","content":"+"},"done":false}
     {"model":"oc-deepseek","…","message":{"role":"assistant","content":"等于"},"done":false}
     {"model":"oc-deepseek","…","message":{"role":"assistant","content":"2。"},"done":false}
     {"model":"oc-deepseek","created_at":"2026-10-15T14:56:09Z","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":41,"prompt_eval_cached_count":0,"eval_count":47}
     HTTP/1.1 200 OK / content-type: application/x-ndjson
     非流式同一发：{"model":"oc-deepseek","message":{"role":"assistant","content":"2+2等于4。","thinking":"…"},"done":true,"done_reason":"stop","prompt_eval_count":41,"prompt_eval_cached_count":0,"eval_count":52} <- 200
     ```
     另外：`/api/version` GET/HEAD = 200/200、`/api/ps` GET/HEAD = 200(`{"models":[]}`)/405、`/api/show` 只给三件套、`{:404 "model 'nope' not found"}`、`:latest` 也 404 且回显原名、空 body 400 `missing request body`、`{}` 400 `model is required`、`/api/{pull,create,delete,generate,embed,embeddings,blobs/…,whatever}` 全 404 单层信封、`GET /v1/models`、`/api/status`、`/api/logs`、`/api/logs/filters`、`/api/usage/summary` 全 200（兜底没吃管理面），`POST /v1/messages` 未知模型仍是 Anthropic 信封 404。
3. **三项待实测**：见上，三项都实测完（`length` / 逐帧 thinking / 普通 content 帧）。
4. **门禁**：`pwsh -File ./check.ps1 -Quick -Replay` → **exit 0，`全部通过`**。单元测试 137 passed / 3 ignored（本票新增 16）；集成测试各套 14 / 8 / 4 / 4 / **12（本票）** / 14 / 19 / 1 全 passed；回放套件 **22 passed**（票 41/51/53/57 的格子全绿，其中票 51 的 `client_profile_override_and_models_exposure` 在本次重构中被我改坏过一次——日志的 `client` 列被我错写成「命中那条绑定的 profile」，落回 global 就丢了客户端档案——已改回从 `user-agent` 判归属，现绿）；回放器单测 5 passed。

### 遗留与坑

- **`/api/*` 兜底与兜底的兜底**：axum 的 `Router::merge` 在**两边都设了 fallback 时会 panic**（实测过这条规则）。本票把 `api::router`（无 fallback）+ `proxy::router`（有 fallback）合并，方向是「一边默认一边自定义」，安全；但**以后谁给管理面加 fallback，`ingress::router` 会在启动时 panic**——那时应把两张脸的兜底合成一个。已在 `ollama::api_fallback` 与 `ingress/mod.rs` 注释里写明。
- **未写出任何帧的「流中」失败**：我们的架构在等到上游 2xx 后才建响应流，所以「上游 2xx 但一个帧都没发就断」这一档，客户端拿到的是 200 + 一行 `{"error":…}`；官方在 `Content-Type` 还没写出时会改回真状态码。窄边角、要改就得在转发链上先偷看第一帧（会动票 41 的共用路径），本票按「已写出 → 追加错误行」实现并在此记一笔。
- **`think` 与 `seed`/采样旋钮被丢弃**：不是遗漏，理由见上（IR 的 `thinking` 是 Anthropic 形状；`extra` 会被 Anthropic 出站原样塞进请求体）。若将来 IR 加一层「wire 无关的采样旋钮」，这里应改成映射而不是丢。
- **`/api/tags` 的 `modified_at` 一律是网关启动时刻**：`virtual_models` 表没有时间列（票 51 的 schema），要「该绑定的最后修改时间」得先加列；spec §4.4 允许「无则网关启动时间」。
- **上游 500 透传成 500**（不是 502），Ollama 信封；与另两张脸同一口径（未改动）。
- 探针脚本与输出留在 `C:/Users/ZHANQUAN/AIGC/Project/cg-target/56/`（`curl-56.sh` / `curl-56-live.sh` / `curl-56.out` / `curl-56-live.out`），不在 worktree 里、不入库。
