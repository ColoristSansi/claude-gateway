# 实现 `count_tokens` 与非流式入站

Type: grilling
Status: resolved
Blocked by: —

## Question

网关怎么实现 `POST /v1/messages/count_tokens` 与非流式 `/v1/messages`（`stream:false` / 缺省）？

## 为什么现在问

票 16 已证：Claude Desktop 的推理探针是**非流式** `{"model":…,"max_tokens":1}`，连接测试走 `/v1/messages/count_tokens`；网关不实现这两个端点，Desktop 就常驻「模型不可用」横幅（`ConfigHealth=config_model_rejected`）。票 14 的原型则是「非流式直接 400」，因为当时只按「三个客户端都流式」判断。票 19 已把它写进回归套件的**待覆盖清单**并明写「现在会失败」，本票负责让那些断言能成立。

## 需要定下来的

1. `count_tokens` 的返回值：真实向上游要 token 数（哪些上游有对应端点、没有时怎么估），还是本地估算；`input_tokens` 的语义与客户端会不会信。
2. 非流式的响应形状：完整 `message` JSON（含 `usage`、`stop_reason`）、错误信封、`content` 块的组装方式；thinking 与非流式同时出现时怎么办。
3. 非流式与前置的「逐模型 `supported_wires`」「usage 迟到」（README §2 第 1、2 条）怎么配合——OpenAI Chat 出站时 `input_tokens` 只能末尾才知道，非流式下反而正好。
4. `max_tokens:1` 这类极小请求的行为：是否要短路、是否照常走上游。
5. 入站 wire 的边界：本票是否只管 Anthropic Messages 入站，OpenAI Chat 入站的非流式是否顺带定义。

## 相关

- [票 16](../issues/16-research-desktop-model-id-validation.md)：Desktop 探针形态与 `config_model_rejected` 的来源。
- [票 19](../issues/19-grilling-test-strategy.md) 第 11 条：本票的产物会被套件里的待覆盖清单接住。
- [票 23](../issues/23-design-provider-table-granularity.md)：逐模型可用 wire 的粒度，决定「这个模型能不能接非流式」。

## Answer

**已解析（2026-10-15，与用户弹窗确认 6 项 + 2 项补充）。下列行为即实现依据；原型已按此实现并实测通过。**

### 0. 先把票 16 的归属改对（结论不变，只是归错了 provider）

两个装机版本（MSIX `Claude_2.37337.1.0` 与 `2.31226.0.0`，`app.asar` 逐字一致）里：

- `provider: gateway` 的探针链 = `/v1/models`（仅当 Models 列表为空）→ `POST <baseUrl>/v1/messages`，body `{model, max_tokens:1, messages:[{role:"user",content:"."}]}`，头 `anthropic-version: 2023-06-01`；结果**只看状态码**（`Irn` 不读 body）。
- `/v1/messages/count_tokens` 的唯一调用点是 **Azure AI Foundry provider**（label `"Azure AI Foundry"` 的那个 `probeInference`）；SDK 的 `countTokens()` 全包 **0 个调用点**。
- body 只被 `completeOneShot`（`Rrn` + `Lrn`，标题生成那类）解析：`content` 必须是数组，否则 `one-shot response had no content array`；`stop_reason` 取字符串或 null。
- 状态码分类（`Orn`）：401/403 → `auth`；400/404 且带 `probedModel` → `model_rejected`（→ `config_model_rejected` 横幅）；**其余（含 5xx / 超时）不是 model_rejected**。

→ 票 16 §6 与 spec §8.1 把 count_tokens 记在 gateway provider 名下是归错了；**「两个端点都该实现」这个结论不变**，只是对 Desktop 而言 count_tokens 不是必需（Claude Code 侧本来也是可选、缺失时退化为字符估算）。

### 1. `count_tokens`：Anthropic wire 转发上游，其余本地估算，任何失败都降级

- 路由：`POST /v1/messages/count_tokens`（+ 别名 `/messages/count_tokens`）。body `{model, messages, system?, tools?, thinking?, tool_choice?}`——没有 `max_tokens`、没有 `stream`。
- 判据与 `/v1/messages` 同源：未知虚拟模型 → 404 `not_found_error`；`messages` 为空 → 400 `invalid_request_error`。
- 出站形状 = `/v1/messages` 请求体去掉 `max_tokens` 与 `stream`；出站路径 = `path_anthropic` 的兄弟 `/messages/count_tokens`（**不新增表字段**）。
- 只有 `wire == anthropic` 才转发；**非 2xx / 连不上 / 10 秒超时 / 响应里没有 `input_tokens` 全部降级到本地估算**，这个端点永不因为上游而失败。
- 协议头只透传入站收到的 `anthropic-version` / `anthropic-beta`；不自动补 `token-counting-2024-11-01`（非 Anthropic 上游可能因未知 beta 直接 400，降级兜住）。
- 返回 `{"input_tokens": N}`，不写 `request_log`（表里没有端点列，写进去只会让 `/api/logs` 混进不可分辨的行）。

### 2. 本地估算口径（`ir::estimate_tokens`）

system + tools + messages 全算；ASCII 4 字符 ≈ 1 token（向上取整），非 ASCII 每字符 1 token；任何叫 `data` 的字段（base64）不计入；图片块按常量 **1500** token（`source` 里没有像素尺寸，Anthropic 的 `宽×高/750` 用不上）；结果最小 1。
理由：Desktop 真实负载 `input_tokens=34221` 的大头正是 system + tools，只算 messages 会严重低估。

### 3. 非流式 `/v1/messages`：出站永远流式，网关缓冲到终态再组装

- 入站 `stream` 缺省或 false → 缓冲路径；**出站仍是 `stream:true`**（两条 wire 一样）。一套编解码；Chat wire 迟到的 usage 正好在收敛后拿全。
- 组装形状：`{id, type:"message", role:"assistant", model:<客户端模型名>, content:[…], stop_reason, stop_sequence:null, usage:{…}}`；**`content` 一定是数组**（空也给）。
- 块语义与流式一致：text 拼文本；thinking 拼 thinking + signature，**没有 signature 的 thinking 块直接丢弃**（带回去下一轮会被官方校验拒）；tool_use 的 `input` 由 `partial_json` 拼完再 parse，parse 不了给 `{}`；未知块丢弃。
- usage：`input_tokens` / `output_tokens` 恒在（缺省 0），两个 cache 字段有才给；`stop_reason` 缺省 `end_turn`。
- 上游非 2xx → 就地转 Anthropic 信封、状态码保持（与流式同一套映射）；上游在流里报 error / 传输中断 / 没有终态事件 → **502 `api_error` JSON**（此时一个字节都还没发，不必伪装成 200）；正常 → 200 JSON。请求照常写 `request_log`。

### 4. `max_tokens:1` 探针：照常走上游，不短路

短路会让「这个模型能不能服务」变成假答案，而网关自己的槽位自检（票 17）走的正是这条路径。

### 5. 上游侧超时：统一超时不加，只给「探针档」封顶（用户选 1+3）

实测：`commandcode` 的 **Chat wire** + `deepseek/deepseek-v4.1-flash` + `max_tokens:1` **挂住不返回**（**直连 curl 25 秒也无响应，与网关无关**；同一形状 opencode 1.9 秒、commandcode 的 Anthropic wire 2.5 秒都正常）。因此：

- `max_tokens <= 16`（`PROBE_MAX_TOKENS`）的请求加上游上限 **20 秒**（`PROBE_DEADLINE`）：超时 → **504**「上游超时（20 秒内无响应）」；流式若响应头已发出则转成流内 `error` 帧。
- 其余请求仍只有全局 300 秒，不做统一超时。
- 已知副作用：这一档里合法但慢的请求会被砍；阈值与秒数都是可改常量。

### 6. 入站范围

本票只定 Anthropic Messages 入站。OpenAI Chat 入站、`/v1/responses`、Ollama 都不在票内（Codex 走 `/v1/responses`，属票 31/32）。

### 实现与实测（`prototype/`）

改动：`src/ir.rs`（`estimate_tokens` + 4 项单测）、`src/anthropic.rs`（`encode_count_request`、`AnthropicAssembler` + 5 项单测）、`src/upstream.rs`（`request()` 单一组装点、`forward_buffered`、`count_tokens_upstream`、`with_deadline`/`upstream_error`）、`src/main.rs`（两条 count_tokens 路由、非流式分支、`protocol_headers` 抽取）、`src/mock.rs`（`/anthropic/v1/messages/count_tokens` → 固定 **4242**，刻意与本地估算不同，用来分辨数字来源）。

实测（2026-10-15 本机）：

- `cargo test` **9/9**；`verify.ps1` **10/10**（流式无回归）；`cargo clippy` 只剩一条**既有**告警（`openai.rs` 的 `OpenAiDecoder` 手写 `Default`，本票未碰，留给票 29 的判据）。
- 非流式两 wire：`mock-*-text` 200 完整 message；tools → `text+tool_use` 且 `input` 解析成对象；thinking anthropic → `thinking+text` 带 signature、openai → 只留 text；usage 场景带 `cache_read_input_tokens`；abort 场景 502。
- count_tokens：mock anthropic wire → **4242**（转发生效）；mock openai wire → 本地估算 4；带 system + tools 的 body → 15（证明两者计入）；未知模型 404；别名可用；`cc-sonnet` → 本地估算（commandcode 直连该路由是 `404 not a registered API route`，**降级路径实测生效**）。
- 真实上游：opencode 与 commandcode 的流式与非流式都 200；`claude-haiku`（commandcode chat wire）`max_tokens:1` → 20.0 秒后 504；`oc-deepseek` 与 `cc-sonnet` 的 `max_tokens:1` → 200（探针档上限不影响正常上游）。

### 留给后续票的两条

1. **槽位绑定会撞上这条上游怪癖**：原型四个家族槽位里 `claude-sonnet` / `claude-haiku` 绑在 commandcode 的 Chat wire 上，正是 `max_tokens:1` 挂住的那一组 → Desktop 的推理探针会 504。选路/自检如何呈现由票 17 与票 32 处理。
2. 票 30 按本 Answer 补断言时，注意两个常量（`PROBE_MAX_TOKENS=16`、`PROBE_DEADLINE=20s`）与 mock 的 **4242** 都是断言锚点。
