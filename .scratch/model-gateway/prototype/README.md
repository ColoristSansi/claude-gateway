# 原型：协议转换核心 + mock 上游（票 14）

一次性原型，只为回答一个问题：**IR + 编解码 + SSE 逐帧转发能不能立起来**。
不是产品代码：无前端、无 DPAPI、无配置写入引擎、无管理 API 鉴权。数据库文件叫 `gateway.db`，随时可删。

## 怎么跑

```powershell
cd .scratch\model-gateway\prototype
cargo run
```

- 网关 `http://127.0.0.1:8787`（`POST /v1/messages`、`POST /v1/messages/count_tokens`、`GET /v1/models`、`GET /api/routes`、`GET /api/logs`、`GET /healthz`）
- mock 上游 `http://127.0.0.1:8788`（回放 `fixtures/`）
- 凭据读 `../prototype-upstream.env`；可用 `GATEWAY_ENV` / `GATEWAY_DB` / `GATEWAY_FIXTURES` / `GATEWAY_PORT` / `GATEWAY_MOCK_PORT` 覆盖。
- 票 30 新增的旋钮：`GATEWAY_PING_SECS`（上游静默超过它就向前发 `ping`，默认 15）、`GATEWAY_PROBE_SECS`（探针档 `max_tokens<=16` 的上游上限 → 504，默认 20）、`GATEWAY_LOG_BODY=1`（开 body 日志，1 小时自动关）；窗口另可用 `POST /__bodylog {"seconds":N}` 拨（测试用，`0` = 立即关）。
- 启动时打印整张路由表；运行时状态看 `GET /api/routes`、`GET /api/logs`、`GET /api/echo`（mock 收到的请求回显）。
- 回归门禁（**单一入口，代替已退役的 `verify.ps1`**）：`pwsh -File .\check.ps1`（不带参数 = fmt + clippy + 单元测试 + 回放集成测试；另有 `-Quick` / `-Replay` / `-Live` / `-Smoke`，见「票 29」一节）。
- 重录一份真实上游 fixture：`pwsh -File .\record.ps1 -Scenario tools -Wire openai -Model <模型> -Url <上游 URL> -KeyVar <env 键名>`（字节原样落盘 + 写 meta.json；`-DryRun` 只打印）。

```powershell
# mock 五场景 × 两种 wire（把 MODEL 换成 mock-anthropic-text / mock-openai-abort / …）
curl.exe -N -X POST http://127.0.0.1:8787/v1/messages -H "content-type: application/json" `
  -d '{"model":"MODEL","max_tokens":256,"stream":true,"messages":[{"role":"user","content":"你好"}]}'
```

## 代码结构

| 文件 | 内容 |
|---|---|
| `src/ir.rs` | 中间表示：请求 + 12 种流式事件 |
| `src/anthropic.rs` | Anthropic Messages 编解码；入站请求解码、出站请求编码、上游 SSE 解码、客户端 SSE 编码 |
| `src/openai.rs` | OpenAI Chat 编解码；IR→Chat 请求、Chat SSE→IR |
| `src/sse.rs` | SSE 帧解析（跨 chunk 半帧缓存、CRLF）与帧序列化 |
| `src/replay.rs` | 回放器：读 fixture 字节 + `meta.json`，按字节偏移切块（frame / half / byte / whole / crlf） |
| `src/provider.rs` | 声明式供应商表：`base_url` + 认证 + 每 wire 路径模板 + `quirks` 枚举（含双向守卫单测） |
| `src/store.rs` | SQLite 建表、路由表、请求日志 |
| `src/upstream.rs` | 出站发送、quirk 执法、流式泵（解码→IR→重编码）、中断处理 |
| `src/mock.rs` | 假上游：按「场景 × wire」回放 fixture（可按切法切块）+ 请求回显；另有四个负例场景：`err401/403/429/500`（直接回状态码）、`hang`（先回头再静默）、`hang-nohdr`（连头都不回）、`hardcut`（发到中途硬切连接） |
| `fixtures/<场景>/<wire>.sse` | 6 场景 × 2 wire + `hardcut` × 2 + 真录的 `keepalive`（Anthropic）= 15 份帧序列（字节原样，含切法约定） |
| `fixtures/expected/<场景>.<wire>.sse` | 结构级黄金文件，**15 格全有**；回放测试逐帧逐字段比，**运行时不写** |
| `tests/` | 集成测试：共用底座（起网关）+ `replay.rs`（回放）+ `common/golden.rs`（结构级比对） |
| `check.ps1` | 回归门禁单一入口；`verify.ps1`（已退役删除） |
| `record.ps1` | 录制工具：跑真实上游一次，写 `.sse` + `meta.json`（人工触发，绝不自动重录） |
| `desktop-3p.ps1` | 票 15 的配置写入引擎最小版：`-Mode gateway` / `standard` / `restore`，首写前整目录备份 |

## 结论记录

### 1. 被 `quirks` 吸收的差异（声明式，转换代码里没有供应商名）

| quirk | 上游 | 差分 |
|---|---|---|
| `ClientUserAgent` | opencode | 必须自称（形如 `my-coding-agent/1.0`），不接受通用 SDK UA |
| `SessionIdHeader` | opencode | 同一段对话必须带稳定会话 ID 头（网关按首条 user 文本派生，FNV-1a） |
| `ClaudeModelsOnAnthropicOnly` | commandcode | 该上游 Claude 模型只有 `/messages`；wire 配错必须**早拒**而不是等上游 400 |

认证方式（`bearer` / `x-api-key`）与路径模板（opencode 的协议族前缀）也全部落在表里，没进代码。

### 2. 声明式表吸收不了、必须写进代码的差异

1. **model 与 wire 的绑定是逐模型的，不是逐上游的。** opencode 把 `claude-haiku-5-5` 只挂在 `/messages` 上，发 Chat 端点直接 400 `ModelProtocolUnsupported`；commandcode 用 `supported_endpoints` 字段表达同一件事。→ 供应商表需要一个**逐模型 `supported_wires`** 字段（`/v1/models` 已带，可作为自动发现来源），quirk 只是它的特例。
2. **usage 字段名与到达时机不同。** Anthropic：input 在 `message_start`、output 在 `message_delta`；OpenAI：`prompt_tokens`/`completion_tokens` 在 `finish_reason` **之后**单独一帧。→ 编码器必须把 usage 与 stop_reason 缓冲到终态再发 `message_delta`，否则 token 计数丢失（已实现）。
3. **OpenAI wire 没有 `message_start` 的对应事件**，而 Anthropic 客户端协议要求它是第一帧。→ 编码器必须补发（已实现）。代价：Chat wire 上 `message_start.usage.input_tokens` 只能是 0，真实 input 计数到末尾才到。
4. **`tool_result` 的载体不同。** Anthropic 挂在 user 轮的 content block 里，OpenAI 必须是独立 `role:"tool"` 行，且要排在同轮文本之前（已实现）。
5. **thinking 的签名不可跨协议搬运。** Anthropic 的 `signature_delta` 在 Chat wire 上没有落点，Chat 的 `reasoning_content` 也换不回签名。→ 直通丢弃（已实现，直通 Anthropic 时保留）。
6. **入站的 Anthropic 协议头必须透传。** `anthropic-version` / `anthropic-beta` 一丢，prompt caching、thinking 这类 beta 能力静默失效。→ 入站头→出站头是代码，不是 quirk（已实现；本原型只透传这两个）。
7. **上游错误信封要就地转成 Anthropic 信封**，状态码保持（401→`authentication_error`、403→`permission_error`、429→`rate_limit_error`、其余→`api_error`）。已实现，见 `/api/logs`。

### 3. 实测挖出的上游事实（票 14 新增，与 `research/providers.md` 的差异）

- **opencode Go 的 Anthropic 端点与 Chat 端点凭据不同**：同一个 Bearer key 打 `/zen/go/v1/chat/completions` 正常，打 `/zen/go/v1/messages` 返回 **401 `Go requires a credential`**；`/zen/v1/messages`（Console 面）返回 **401 `Missing API key`**。`providers.md` §3 只写了「统一 Bearer」，需按此更正。
- **commandcode 的 plan 门在调用时才拦**：本机计划里只有 `claude-sonnet-5-5` 可用（200）；`claude-opus-5`、`claude-opus-5-5`、`claude-sonnet-5`、`claude-sonnet-4-6`、`claude-fable-5`、`claude-fable-5-1` 全部 **403 `MODEL_NOT_IN_PLAN`**。`/v1/models` 的 89 个模型 ≠ 可用模型。
- opencode Chat 端点实测可用：`deepseek-v4.1-flash`、`deepseek-v4-pro`、`glm-5.3`、`kimi-k3`、`qwen3.8-max`、`minimax-m3`。
- commandcode 的 `deepseek/deepseek-v4.1-flash` 走 `/chat/completions` 200，且它的 `supported_endpoints` 明确**不含** `/messages`。
- **commandcode 没有 `/messages/count_tokens`**：直连返回 `404 not a registered API route`（票 26 实测）→ 网关的 count_tokens 降级路径在真实上游上生效。
- **commandcode 的 Chat wire + `deepseek/deepseek-v4.1-flash` + `max_tokens:1` 挂住不返回**：直连 curl 25 秒无响应（与网关无关）；同一形状 opencode 1.9 秒、commandcode 的 Anthropic wire 2.5 秒都正常。这正是 Desktop 推理探针的形状 → 票 26 给「探针档」（`max_tokens <= 16`）加了 20 秒上游上限。

因此本原型的四个家族槽位绑定为（用户选定：四个槽位都用 deepseek 4.1 flash，两家上游各两个；`GET /api/routes` 可见）：

| 槽位 | 上游 | 真实模型 | wire |
|---|---|---|---|
| `claude-fable` | opencode | `deepseek-v4.1-flash` | openai |
| `claude-opus` | opencode | `deepseek-v4.1-flash` | openai |
| `claude-sonnet` | commandcode | `deepseek/deepseek-v4.1-flash` | openai |
| `claude-haiku` | commandcode | `deepseek/deepseek-v4.1-flash` | openai |

另留两条真实上游验证条目：`oc-deepseek`（opencode / Chat）与 `cc-sonnet`（commandcode / Anthropic 直通，`claude-sonnet-5-5`），以及三条负例 `oc-anthropic-unauth`(401)、`cc-opus-no-plan`(403)、`cc-opus-on-chat`(400 quirk 早拒)。

### 4. 原型没做的 / 已知取舍

- 非流式请求**已实现**（票 26，见 §6）；入站仍只支持 Anthropic Messages（`/v1/messages`）；OpenAI Chat 入站、Responses、Ollama 都没做。
- 只「解码再编码」，不做字节直通：直通更快，但那样 IR 就没被验证。
- mock 的「异常中断」是**流在终态前正常结束**，不是 TCP 硬切；网关的判据是「没有终态事件」，两种情形都覆盖（**票 30 补了真硬切**：`hardcut` 场景在字节发完后产一个 Err，连接被真正切断）。
- 密钥明文在内存；**入站明确不鉴权**（本机单用户，票 30 第 4 条把它写成断言：无凭据也必须 200）；日志默认不写 body，`GATEWAY_LOG_BODY=1` 或 `POST /__bodylog` 开启、1 小时自动关，且 API / 库 / 诊断面都查不到密钥（票 30 第 7 条的断言）。
- 头透传（`anthropic-version` / `anthropic-beta`）有集成断言了（票 30 第 10 条）：mock 回显出站请求头，Anthropic 直通必须带、Chat wire 必须不带。
- quirk 覆盖（票 29 更新）：两个头类 quirk 现在**真有集成断言**（mock 回显出站请求头）；`ClaudeModelsOnAnthropicOnly` 是守卫型 quirk（wire 错配早拒），由 `src/upstream.rs` 的 guard 覆盖、无 fixture——豁免理由写在 `fixtures/quirk-exempt.json`，票 23 已决定删除它（删除随实现阶段）。

## 5. 票 15：Claude Desktop 3P 端到端（已过）

```powershell
# 网关先跑起来，然后：
pwsh -File .\desktop-3p.ps1 -Mode gateway     # 写入 3P 配置（首次会整目录备份）
# 完全退出 Claude Desktop 再启动
pwsh -File .\desktop-3p.ps1 -Mode standard    # 摘掉网关字段 → 回 1P 官方模式
pwsh -File .\desktop-3p.ps1 -Mode restore     # 用首次备份整目录还原（大退路）
```

实测（用户验收）：菜单精确显示四个家族名，四个都流式正常回答，切回 1P 后官方登录态完好。

从这一轮拿到的新事实：

1. **`inferenceGatewayBaseUrl` 不要带 `/v1`**。Desktop 打的是 `<baseUrl>` + `/v1/messages`（网关打印的真实路径）。官方示例里的 `.../v1` 会得到 `/v1/v1/messages`。
2. **`inferenceModels` 全是完整 ID 时 Desktop 跳过 `/v1/models`**：整个会话期间网关没收到过该请求。
3. **3P 与 1P 的登录态隔离**：3P 侧 accountId 与 1P 侧 `lastKnownAccountUuid` 是两个值，compartments 不互相覆盖 → 切回 1P 只需摘 `inferenceProvider`，不需要还原备份。
4. 真实负载保真：`claude-fable` 单次 `input_tokens=34221`（Desktop 整套 system prompt + 工具定义）过 OpenAI Chat wire 编解码无碍。

入站路由为此加了 `/messages` 与 `/models` 两个别名（`inferenceGatewayBaseUrl` 到底带不带 `/v1` 两份资料矛盾），`messages` 里会打印真实路径 + `user-agent` / `anthropic-version` / `anthropic-beta`。

## 6. 票 26：`count_tokens` 与非流式入站（已过）

定案与依据见[票 26](../issues/26-grilling-count-tokens-and-non-streaming.md) 的 Answer；这里只记实现落点与怎么验。

| 文件 | 加了什么 |
|---|---|
| `src/ir.rs` | `estimate_tokens`：system + tools + messages 全算，ASCII 4 字符/token、非 ASCII 每字符 1 token、`data`（base64）不计、图片常量 1500 |
| `src/anthropic.rs` | `encode_count_request`（去掉 `max_tokens`/`stream`）；`AnthropicAssembler`（IR 事件 → 完整 message，无 signature 的 thinking 块丢弃） |
| `src/upstream.rs` | `request()` 成为三个出口的唯一组装点；`forward_buffered`（非流式缓冲）；`count_tokens_upstream`（10 秒超时、失败返回 `None`）；`with_deadline` / `upstream_error`（探针档 20 秒 → 504） |
| `src/main.rs` | `/v1/messages/count_tokens` + 别名 `/messages/count_tokens`；非流式分支；`protocol_headers` 抽取 |
| `src/mock.rs` | `/anthropic/v1/messages/count_tokens` → 固定 **4242**（刻意与本地估算不同，用来分辨数字来源） |

```powershell
# 非流式（Desktop 探针形状；走真实上游时注意下面的探针档怪癖）
curl.exe -s -X POST http://127.0.0.1:8787/v1/messages -H "content-type: application/json" `
  -d '{"model":"mock-anthropic-text","max_tokens":1,"messages":[{"role":"user","content":"."}]}'
# count_tokens：anthropic wire 转发（mock → 4242），openai wire 本地估算
curl.exe -s -X POST http://127.0.0.1:8787/v1/messages/count_tokens -H "content-type: application/json" `
  -d '{"model":"mock-openai-text","messages":[{"role":"user","content":"你好 world"}]}'
```

实测（2026-10-15）：`cargo test` 9/9、`verify.ps1` 10/10（流式无回归）；非流式两 wire 都出完整 message，tools / thinking / usage / abort 四个场景逐项验过；count_tokens 的四条路径（转发 / 本地 / 404 / 降级）都验过；真实上游 opencode 与 commandcode 的流式与非流式都 200。

（`verify.ps1` 已于票 29 退役删除，上面那 10/10 是当轮的历史记录。）

## 7. 票 29：回归基建（录制 / 回放 / 断言 / 入口）

形态由[票 19](../issues/19-grilling-test-strategy.md) 定死，这一节只记落点与怎么跑。

| 件 | 干什么 |
|---|---|
| `record.ps1` | 录制：curl 打真实上游一次，`.sse` 字节原样落盘 + 写 `meta.json`；`-DryRun` 只打印；重录必须人工加 `-Force` |
| `src/replay.rs` | 回放：`meta.json` 的 `chunks`（字节偏移）优先，否则内建 `frame` / `half` / `byte` / `whole` / `crlf` |
| `src/mock.rs` | 按当前切法逐块发；`POST <mock>/__chunks {"scheme":"half"}` 拨切法；收到的请求进 `GET /api/echo` |
| `src/provider.rs` | `Quirk::all()` + `acts_on()`：枚举值 ⇄ fixture `quirks` 标签双向守卫；豁免只许写进 `fixtures/quirk-exempt.json` 并带理由 |
| `tests/replay.rs` | 集成层：测试自起网关与 mock，十格走真实 HTTP；断言 HTTP 状态码、首末帧、帧序列；有黄金文件就逐帧逐字段比 |
| `tests/common/golden.rs` | 结构级比对 + 字段级掩码（`message.id`）；**没有任何写黄金文件的代码路径** |
| `check.ps1` | 单一入口：`-Quick`（fmt + clippy + 单测）/ `-Replay` / `-Live`（`#[ignore]` 真上游）/ `-Smoke`（打印 `smoke.md` 等人签字）；不带参数 = Quick + Replay |

```powershell
pwsh -File .\check.ps1              # 门禁全跑（约 20 秒）
pwsh -File .\check.ps1 -Quick       # 秒级
pwsh -File .\check.ps1 -Replay      # 只跑回放集成测试
```

`fixtures/<场景>/<wire>.meta.json` 字段：`scenario` / `outbound_wire` / `upstream` / `model` / `origin`（`authored` 还是 `recorded`）/ `recorded_at` / `request` / `expect`（HTTP 状态 + 末帧）/ `quirks`（本 fixture 覆盖的 quirk 标签）/ `masks` / `chunks` / `changed_why`。

实测（2026-10-15 票 29）：`check.ps1` 全绿——fmt / clippy `-D warnings` 无警告、单元测试 **23** 个、回放集成测试 1 格（内含十格断言 + 五种切法不变性 + 头类 quirk + 认证头 + 路径模板）全过。两条反验也真跑过：往 `fixtures/expected/text.anthropic.sse` 里塞一帧，测试立刻报「帧数不同：实际 8 帧，黄金 9 帧」；抽掉 `fixtures/text/anthropic.sse`，守卫报「没有对应的字节文件」。录制工具也用真实上游验过一遍（opencode Go 的 Chat 面**要求 `x-opencode-session`**，不补该头直接 400 `MissingSessionID`——正是 `SessionIdHeader` 这条 quirk 存在的理由）。

## 8. 决策 ⇄ fixture 台账（票 19 §9 / 票 30 第 10 条）

§2 那 7 条「声明式表吸收不了、必须写进代码的差异」逐条对到覆盖它的 fixture 或测试。回放集成测试里的 `ledger_has_no_gaps` 读这张表：目标必须真实存在（文件，或 `文件::函数`），空目标或写「未覆盖」都让门禁变红。

| # | README §2 的差异 | 覆盖它的 fixture / 测试 |
| --- | --- | --- |
| 1 | model 与 wire 的绑定是逐模型的 | `src/store.rs::init` + `src/upstream.rs::guard` |
| 2 | usage 字段名与到达时机不同 | `fixtures/usage/anthropic.sse` + `fixtures/usage/openai.sse` |
| 3 | OpenAI wire 没有 message_start，必须补发 | `fixtures/text/openai.sse` + `tests/replay.rs::fixtures_replay_through_the_gateway` |
| 4 | tool_result 的载体不同 | `fixtures/toolresult/anthropic.sse` + `fixtures/toolresult/openai.sse` + `tests/replay.rs::tool_roundtrip_multiturn_and_partial_input` |
| 5 | thinking 签名不可跨协议搬运 | `fixtures/thinking/anthropic.sse` + `fixtures/thinking/openai.sse` |
| 6 | 入站的 Anthropic 协议头必须透传 | `tests/replay.rs::chunking_invariance_and_header_injection` |
| 7 | 上游错误信封要就地转成 Anthropic 信封 | `tests/replay.rs::error_envelopes_and_route_boundaries` + `src/mock.rs::error_status` |

**明确未覆盖的两处**（写了原因，不是漏掉）：

- **`: keep-alive` 注释帧**：2026-10-15 真录两次（普通生成与长生成，分别打 opencode 的 Chat 面与 commandcode 的 Anthropic 面）都**没录到**注释帧——两个真实上游都不发它。按票 19 §2「字节只存真录到的那一份」不手编；解析路径由 `src/sse.rs` 的单元测试 `comment_frames_do_not_swallow_the_next_frame` 覆盖。真录到的那一个 `event: ping` 帧已作为 `fixtures/keepalive/anthropic.sse` 收下（40KB / 888 行，字节原样）。
- **客户端档案覆盖**（同一槽位按客户端走不同上游）：原型没有客户端档案概念（票 10 的设计），按用户 2026-10-15 的决定**不在原型里造**，归实现阶段。

## 9. 票 30：铺满回归套件（已过）

`tests/replay.rs` 从一个格子铺成十一个，每个 `#[tokio::test]` 各起一份网关（端口按进程 id + 序号现算，所以并行跑不打架）：

| 格子 | 守什么 |
| --- | --- |
| `fixtures_replay_through_the_gateway` | 15 格台账矩阵：状态码、首帧、末帧、index 连续性、逐帧逐字段黄金 + 分场景语义断言（拼完的工具入参、usage 三个数、签名留/丢、断流错误帧、ping 不被吞） |
| `chunking_invariance_and_header_injection` | 五种切法不变性 + 头类 quirk + 认证头 + 路径模板 + 协议头「直通带、Chat 不带」 |
| `tool_roundtrip_multiturn_and_partial_input` | `tool_result` 多轮入站编到两种 wire 的位置（Chat 提升成 `role:tool` 且排在文本前）+ 非流式拼完的 `tool_use.input` |
| `error_envelopes_and_route_boundaries` | 401 / 403 / 429 / 500 的信封与状态码、未知模型（含 `claude-opus-6`）、`model` 去空白 + 大小写敏感、无凭据必须 200 |
| `concurrency_and_session_isolation` | 并发两段对话不串会话头、同段对话稳定、两个「客户端」同打一个槽位 |
| `first_frame_latency_and_silence_ping` | 首帧延迟（证明无整包缓冲）+ 上游静默时向前发 `ping` |
| `probe_tier_timeout_is_504` | 探针档 `max_tokens<=16` 的上游上限 → 504（流式与非流式），旋钮 `GATEWAY_PROBE_SECS` |
| `count_tokens_and_non_streaming` | 转发（4242）/ 本地估算 / 上游 404 降级 / 缺 `input_tokens` 降级 / 非流式完整 message / 非流式断流 502 |
| `logs_are_metadata_only_and_redacted` | 默认不写 body、开关开了能捞到、2 秒窗口到点自动关、API 与库里反查已知密钥 |
| `ledger_has_no_gaps` | 上面 §8 那张表：7 行都在、目标都存在 |

黄金文件的来历要说清：**离线**用一次性脚本（跑完即删）把网关对每份 fixture 的输出抓下来，逐份人工核对语义后冻结（`text.anthropic.sse` 与票 29 冻结的那份逐字节相同，说明这一轮没改行为）；测试代码里**没有任何**写黄金的路径。

## 10. 票 32：Codex 端到端（已过）

Codex 的 `wire_api` 唯一合法值是 `responses`，所以这一票给网关加了**第二个入站协议**：`POST /v1/responses`。出站不变（还是路由表里的 anthropic / openai-chat），转换仍是「解成 IR 再编回去」。

新增：

- `src/responses.rs`：Responses ⇄ IR。入站解 `instructions` / `input` 项（message / function_call / custom_tool_call / function_call_output / reasoning）/ `tools`（function 与 custom）/ `tool_choice` / `max_output_tokens`；出站编 `response.created` → 各类 `.delta` / `.done` → **`response.completed`**。
- `src/upstream.rs`：入站编码器与组装器都变成两态（Anthropic / Responses），错误信封按入站协议给形状（Responses 用 OpenAI 的 `{"error":{...}}`，Anthropic 那份 Codex 读不懂）。
- `codex-gateway.ps1`：生成 Codex 模型目录 + 写 `config.toml`，四种模式 `gateway` / `show` / `restore`（摘键）/ `rollback`（整份，且只在没被外部改过时允许），外加 `-Mode selftest`。
- `tests/replay.rs` 第 ⑫ 格 `responses_inbound_contract`：文本 / 工具 / 推理 / 非流式 / 两种错误信封 / 续轮配对，共 8 组断言。

### 实测（2026-10-15，真实上游 commandcode + opencode）

| 路径 | 结果 |
| --- | --- |
| `codex exec`（CLI 0.162.1，文本） | `provider: claudegateway` / `model: claude-sonnet`，答对，`/api/logs` 记为 `claude-sonnet → commandcode / deepseek/deepseek-v4.1-flash / openai / 200` |
| `codex exec`（工具往返） | 模型发 `exec_command` → Codex 执行 → 错误文本回灌 → 模型据此作答；上游侧 `messages` 是 `[system, user, assistant(tool_calls), tool]`，配对成功 |
| Codex **桌面应用**（真实 `~/.codex`） | 选择器里看到四个家族名；发问作答正确；无报错。网关日志 4 条 `Codex Desktop/0.162.0-alpha.17.2` 的 `/v1/responses`，`claude-fable` 落 opencode、`claude-sonnet` 落 commandcode，全 200 |

### 这一票挖出来的事实

1. **没有 `instructions` 字段**：CLI 把系统提示当 `input` 里的 `role:"developer"` 消息发（两条，含 skills 与 permissions 说明），所以 `developer` / `system` 必须归到 IR 的 system，不能丢。
2. **中性模板确实不产生自由格式工具**：`shell_type="shell_command"` 下 Codex 发的是普通 JSON-schema 函数 `exec_command` / `write_stdin` / `request_user_input` / `view_image` / `get_goal` / `create_goal` / `update_goal`，另有一个 `namespace multi_agent_v1`（嵌套工具组）与一个 `web_search`。**后两者本原型丢弃**（转发层认识 `function` 与 `custom` 两种）。
3. **`x-openai-internal-codex-responses-lite` 不分流**：网关不读这个头，两端一律回全量 Responses SSE，CLI 与桌面应用都过。「按它降级」这件事没有证据支持，不做。
4. **`reasoning` 项按 summary 回，不带 `encrypted_content`**：上游不是 OpenAI，拿不到可验证的加密推理，伪造只会骗自己。实测 Codex 接受这种 reasoning 项并按 summary 渲染（`mock-openai-thinking` 那次输出里能看到思考文本）。输入侧的 `reasoning` 项一律丢弃。
5. **`previous_response_id` 不实现**：Codex 固定 `store:false`，续轮靠回放 `input`（第 ⑫ 格的续轮断言就是这么发的）。收到该字段也只当没看见。
6. **桌面应用会改写 `config.toml`，而且会插进我们的范围里**：实测它启动时补了 `[plugins."computer-use@openai-bundled"]` 与 `[windows] sandbox = "elevated"`，后者**正好落在我们 provider 哨兵之间**——按「哨兵围出的区域」删就会连它一起删掉。所以认领改成**逐行**：只删 `model_provider` / `model` / `model_catalog_json` 三个顶层键、`[model_providers.claudegateway]` 这张表、以及两对哨兵行；别的东西一个字节不动。`-Mode selftest` 就是拿这个场景当回归（插入 → 外部写手插表 → 摘除，必须回到原样且保住那张表，且重写幂等）。
7. **还原默认是「摘键」不是「整份回滚」**：实测那一轮里桌面应用自己改了 3 处，整份回滚会把它们一起抹掉。`rollback` 现在会先比「两边摘掉网关内容之后是否一致」，不一致就拒绝。
8. **Codex 自己的 Windows 沙箱会拦住起进程**（`CreateProcess … blocked by policy`，`pwsh` / `cmd` / `bash` 都拦）。这是 Codex 侧的策略，与网关无关；工具**往返**本身是通的（错误文本照样回灌给了模型）。

### 刻意没做 / 已知缺口

- `namespace`（多智能体）与 `web_search` 工具：丢弃，不上报也不转发。
- 接管键的**冲突检测**：票 28 的规则（磁盘值非网关所写即放弃并提示）没实现，这里是「以网关为准直接覆盖」；首写快照保住退路。
- 客户端档案覆盖（同一槽位按客户端走不同上游）：原型没有这个概念（票 10 的设计，票 30 台账里也记了未覆盖）。
- `max_output_tokens` 缺省时填 4096（Codex 实测不带这个字段）。


