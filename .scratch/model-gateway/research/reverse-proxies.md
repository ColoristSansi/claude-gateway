# 反代/网关四项目调研：协议转换实现

调研日期：2026-10-14 · 工作目录 `C:\Users\ZHANQUAN\AIGC\Project\Claude Gateway`

**信源 = 源码本身**（四个仓库均 `git clone --depth 1` 成功落地于 `.scratch/model-gateway/vendor/`）。
下文每条结论都给仓库内相对路径。文中的"未验证"标注表示我读了文档/注释但没在代码里逐条确认，
或代码里没有任何对应实现。版本号取自各仓 `package.json` / 首个 tag 的 HEAD，非 README 自述。

四个仓库都克隆成功，无一个需要退回 web_fetch。`vendor/` 下另有 `cc-switch`、`CodexPlusPlus`
两个目录（上一轮遗留，不在本次任务清单内），未纳入。

| | dsh-our-free-model | deepseek-harness-codearts | Astra-Gate | zen-gate |
|---|---|---|---|---|
| 语言/运行时 | JS (ESM), Node | TS → `lib/`, Node ^22.19\|\|>=24 | C# .NET 10 (Native AOT) + TS/Electron | Go 1.26 |
| 形态 | DSH 插件 + 独立包 | DSH 插件 | 本机常驻服务 + CLI + Electron + Docker | 桌面托盘程序 |
| License | MIT | MIT | MIT | MIT |
| 代理的上游 | OpenCode Zen 免费车道 + EAC + Kilo + 13 家账号渠道 + 聚合 76 家 | 10+ 家账号渠道（CodeArts/Buddy/Qoder/TRAE/Cline/MiniMax/Gemini/ZCode/Loomy/Raccoon…） | 任意 OpenAI/Anthropic/DeepSeek/OpenRouter 等 + 订阅(OAuth)账号 | OpenCode Zen 免费车道 + Kilo + 用户自加 OpenAI/Anthropic 兼容上游 |
| 对外伪装成 | OpenAI Chat + OpenAI Responses + **Anthropic Messages** | OpenAI Chat + OpenAI Responses（**无 Anthropic 入口**） | OpenAI Chat + Responses + **Anthropic Messages** + **Gemini** | OpenAI Chat + Responses + **Anthropic Messages** |
| Anthropic↔OpenAI 双向转换 | **是（双向）** | OpenAI→各家协议单向（含 Anthropic 上游）；无 Anthropic 入口 | **是（双向，四协议 IR）** | **是（双向）** |

---

## 1. zen-gate — 协议最齐、机制注释最完整

- **语言/部署**：Go 1.26，单二进制。Windows/`macOS`/Linux 桌面托盘（`getlantern/systray` + WebView2/WKWebView），
  另有 `.deb` / AppImage；`go.mod` 仅一个直接依赖。License MIT（`LICENSE`）。
- **代理什么 / 伪装成什么**：上游是 OpenCode Zen **免密**车道（`Authorization: Bearer public`）
  与 Kilo 免费池，再加上用户自加的 OpenAI/Anthropic 兼容上游（`internal/relay/presets.go` 13 个免费商预设）。
  对外同时提供 `/v1/chat/completions`、`/v1/responses`、`/v1/messages` 三条线协议
  （`internal/gateway/server.go:440-446`）。
  `/v1/models` **只有 OpenAI 一种形状**（`server.go:518 handleListModels` + `:546 modelCard`：
  OpenAI 字段之外附加 `context_window`/`max_output_tokens`/`input_modalities`/`reasoning`/
  `supported_reasoning_levels`/`capability_source`；**不按 `anthropic-version` 切换**，
  与 Astra / deepseek-harness-codearts 的做法不同）。
  另有 `/v1/codex-catalog`（`server.go:699`）。
- **关键实现**
  - **统一 IR 在 lane 包**：三种 wire 各自 Decode 成 `lane.Message`/`lane.Part`（`internal/lane/types.go`），
    再投影回目标 wire。**hub-and-spoke，不是两两直转**：`internal/lane/messages.go` 的
    `ToChatMessages` / `ToClaudeMessages` / `ToResponseInput` / `ToToolDefs(tools, style)`
    （`style` 取 `chat` / `claude` / `flat`）。Anthropic 入口在
    `internal/gateway/anthropic.go:58 convertAnthropicMessages` + `:78 anthropicParts`。
  - **鉴权头改写**：`internal/relay/relay.go:38 authHeaders` —— Anthropic 协议写 `x-api-key` +
    `anthropic-version: 2023-06-01`；否则 `Authorization: Bearer <key>`。端点拼接同文件 `endpointOf`。
    入站侧 `internal/gateway/server.go:460 authorized` 同时接受 Bearer 与 `x-api-key`，并
    按 key 解析出 agent 标签（`:474 agentOf`），因此每个 Agent 可发独立 key 并单独统计。
  - **SSE 流式转发**：`internal/gateway/sse.go` 的 `sseWriter`。关键点是
    `typedEvent()`（`:49`）**从 marshal 后的 JSON 顶层 `type` 字段反推 `event:` 行**——
    注释明确记录：结构化解码能避免把嵌套的 `content_block`/`delta` 里的 `type` 误当事件名（issue #48）。
    `flushed` 标志（`:11`）用来判断 200 是否已发出，决定失败走 HTTP 错误还是流内 `error` 事件。
  - **错误与重试**：`internal/lane/client.go:694 Complete` 是主循环。逐候选模型尝试，
    仅当**尚未向调用方吐出任何内容**（`sawAny`）才允许切换模型——否则会重复输出。
    `:800 switchableFailure` 决定哪些错误类值得换模型（Quota/Region/Server/Timeout/Empty/Transport），
    `:787 egressRetryable` + `:779 egressBudgetFor` 决定哪些值得**换出口 IP 重试同一模型**。
    `internal/gateway/routing.go:151 providerFallback` 是"免费车道耗尽 → 单向回落到用户自己的 key"，
    且仅当 `uerr.Exhausted`（`client.go:753`）才触发。
  - **限流/额度管理**：`internal/lane/throttle.go` 的 `ThrottleBook`——每模型 429 冷却台账。
    默认冷却只有 **10 秒**（`quotaCooldownNoHeaderSec`），注释给了实测依据：92 次限流样本里最短 0.6s，
    共用凭据的 429 是瞬时拥塞而非模型属性，60s 停车会把好模型误报成不可用；
    上限 2 小时，`RecoveryETA` 用历史 episode 中位数估算恢复时间。
    另有订阅出口池（每个请求轮换一个健康出口，因为额度按出口 IP 计）。
  - **模型 ID 映射**：命名空间 `provider/model`；`providerRoute`（`server.go:630` 附近）反解，
    `trimNamespaced`（`routing.go:175`）剥前缀供日志用。免费车道 id 自身不带斜杠，故不会与聚合行冲突。
- **Anthropic Messages ↔ OpenAI Chat 双向转换：有**
  - 入站 Anthropic：`internal/gateway/anthropic.go` 全文。
    `anthropicParts` 逐块映射：`text`→`TextPart`、`image`(base64)→`ImagePart`(data URL)、
    `document`→`FilePart`(仅 base64；注释说明 url source 会把网关出口暴露给第三方)、
    `tool_use`→`ToolCallPart`、`tool_result`→`ToolResultPart`（含 `is_error`）；
    `anthropicStreamState.consume`（`:310`）把 IR chunk 反向渲染成
    `content_block_start/delta/stop`，并用 `blockMap` 维护 lane 块序 → Anthropic 索引（**两种协议块序不同，必须重编号**）；
    `reasoning` 块渲染成 Anthropic 的 `thinking` + `thinking_delta`。
  - 出站 Anthropic：`internal/relay/relay.go:143` 用 `lane.ToClaudeMessages` 编码，
    `wire = "messages" that style = "claude"`。
  - 流解码器：`internal/lane/stream.go` 的 `Decoder` 单类吃三种 wire（`:188` 按 `d.wire` 分派），
    `decodeMessages`（`:331`）/ `decodeChat`（`:228`）/ `decodeResponses`（`:454`）→ 统一 `Chunk`。
    这是本仓库最值得抄的一处结构：**一个解码器、三条 wire、一套 chunk 词汇**。
- **对 Claude Code / Codex / DSH 的适配**：`internal/agents/` 一目录一个适配器，
  都是"改配置文件 + 先备份 + 关闭还原"：
  - Claude Code：`agents/claude.go` —— 改 `~/.claude/settings.json` 的 `env`，
    写 `ANTHROPIC_BASE_URL`（注释点明：Anthropic SDK 自己会追加 `/v1/messages`，所以这里只给 origin）、
    `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_MODEL` 及 opus/sonnet/haiku 三个映射。
  - Codex：`agents/codex.go` —— 往 `~/.codex/config.toml` **文本插入**一个带 `# >>> zen-gate managed block`
    标记的 `[model_providers.zen_gate]` 块（`wire_api = "responses"`，注释记录当前 Codex 拒绝 `chat` provider），
    外加 `model_catalog_url` 指向 `/v1/codex-catalog`；注释详述"TOML 不允许重复键，重复一次 Codex 整份配置拒解析"，
    并有 `TestCodexEnableRepairsAlreadyBrokenConfig` 等回归用例。目录里还注入一段中文/跟随用户语言的 system 提示词。
  - 其余：opencode（`agents/opencode.go`，`@ai-sdk/openai-compatible`）、dsh（`agents/dsh.go`，
    `api: openai-completions` provider profile）。
- **可复用设计点**
  1. `typedEvent`：Anthropic 的 `event:` 名从**顶层 `type`** 取，绝不用字符串扫描（会撞嵌套 type）。
  2. `flushed` 单标志决定"错误走 HTTP 还是走流内事件"，是 SSE 代理必备的状态位。
  3. 冷却时间"短默认 + 历史中位数估恢复"，而不是长黑名单。
  4. 换模型/换出口两类重试分开建模（`switchableFailure` vs `egressRetryable`），且都受 `sawAny` 闸门约束。
  5. 配置文件注入用**成对标记注释** + 幂等重写 + 修复已被写坏的文件。

---

## 2. Astra-Gate — IR 最规范、仿真 Anthropic 客户端最彻底

- **语言/部署**：`Astra.sln` 是 .NET 10 解决方案，`src/Astra.Server` 编译成
  **Native AOT 自包含二进制**（`AGENTS.md` 的 Native AOT 节，约 40MB，目标机不需要 .NET 运行时）；
  Node/pnpm workspace 另出 `cli/`（npm 包 `astragate`，bin `astra`）、`web/`（Vite+React 控制台）、
  `desktop/`（Electron）、`docs/`；Docker 镜像基于 `mcr.microsoft.com/dotnet/runtime-deps`。
  License MIT（`LICENSE`）。
- **代理什么 / 伪装成什么**：本地 17321 端口（被占则漂移，真实端口写 `~/.astra/runtime.json`，
  文档明说别硬编码端口）。四条线协议**同时**对外：
  `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1beta/models/{model}:generateContent`
  （`src/Astra.Gateway/Http/GatewayEndpoints.cs:20-40`），另有 `/v1/models`（按 `anthropic-version` 切形状）、
  `/v1beta/models`、`/muse-code/models`。
- **关键实现**
  - **IR 是明文契约**：`src/Astra.Gateway/Protocol/Ir.cs` 一个文件定义全部。
    请求侧 `UnifiedRequest`（`System` 独立成 `List<ContentPart>`，注释标注"消息中间出现的 system 会丢位置，lossy"）、
    `UnifiedMessage`/`ContentPart`（Text/Image/File/ToolCall/ToolResult/**Reasoning**）；
    `ReasoningPart` 带 `Origin`（`ApiProtocol?`）——**签名与加密载荷只对产生它的协议有效，换协议时编码器丢弃签名只留文本**，
    这是我在四个仓库里看到对"thinking 签名不可跨协议搬运"处理得最明确的一处。
    响应侧 `UnifiedStreamEvent`：`MessageStart → (BlockStart, deltas…, BlockStop)* → Usage? → MessageStop`，
    块索引由解码器统一分配。契约 `IProtocolCodec`（`:308`）= `DecodeRequest` / `EncodeRequest` /
    `CreateResponseDecoder` / `CreateResponseEncoder` / `EncodeError` / `DecodeError` ——
    **错误体也按协议分派**，所以客户端永远收到自己那套错误形状。
  - **鉴权头改写 + 仿真 Claude Code**（本仓库最有价值的原创部分，两处）：
    - `src/Astra.Gateway/Pipeline/ClaudeOAuthHeaders.cs`：出站头策略两模式。
      **Relay**（调用方就是 Claude Code）——按白名单逐字转发客户端头（`RelayedHeaders` 数组，
      注释说明转发更多会泄漏矛盾身份、更少会剥掉真 CLI 会发的东西），只换凭据，
      并把 `anthropic-beta` 合并到订阅下限之上（`:87 MergeRelayBeta`；注释有实测依据：
      同一 CLI 的 auth-token 模式比 OAuth 模式**恰好少两个 beta token**，haiku 又多一层差异，
      所以**不能写成固定列表**）。配套的 `:109 PromoteCacheControlTtl` 把 body 里 `cache_control`
      补上 `ttl: "1h"` —— 头与体必须成对，注释明说"只给一个会被拒"。
      **Compat**（别的客户端）——只给最小 beta，不冒充。
    - `src/Astra.Gateway/Pipeline/ClaudeCodeMimicry.cs`：**主动伪造 Claude Code 身份**。
      头：钉死 UA `claude-cli/2.1.281 (external, cli)` + 全套 `X-Stainless-*` 运行时指纹
      （`StainlessBase`，OS/arch/node 三元组按 accountId 稳定哈希选 `HostProfiles`，同账号恒定）+ `X-App: cli` + 完整 beta 集；
      body：把 `system` 重写成**规范三块**（计费归属块 `x-anthropic-billing-header: cc_version=<ver>.<fp>; cc_entrypoint=cli;`、
      `You are Claude Code, Anthropic's official CLI for Claude.` 身份块、带 5m cache 断点的扩展块），
      调用方原 system 降级成历史首轮 user/assistant 对，并补 CLI ≥2.1.78 形状的
      `metadata.user_id`（JSON 字符串）。指纹算法 `CcFingerprint` 逐字节复刻真实 CLI：
      `sha256(salt + 首条 user 消息的第 4/7/20 字节 + 版本)[..3]`，salt 常量注释注明"不得更改"。
      文件头明确写这是**有意欺骗 Anthropic 的客户端校验**、默认关闭、由 `subscription.mimic_claude_code` 开关控制、
      可能违反条款、风险由账号自负。
  - **SSE 流式转发**：`GatewayPipeline.StreamAsync`（`src/Astra.Gateway/Pipeline/GatewayPipeline.cs:729`）+
    `Protocol/Sse.cs` 的 SSE 读写器。同协议走 **pass-through 快路径**，但解码器仍并行运行以观察 usage/首字延迟/结束原因
    （`Ir.cs:10-12` 注释）。非流式请求**走同一条事件流**，由编码器的 `BuildJson()` 聚合 ——
    即流式与非流式只有外壳差别。`IdleTimeoutReader`（`:1199`）做上游空闲超时；`UpstreamIdleTimeoutException` 单独成型。
    `:842 HandleNonSseBodyAsync` / `:927 CloseArrayAsync` / `:936 BufferAsync` 处理"上游对 `stream:true` 回了非 SSE"的退化情形。
  - **错误与重试**：`SendAsync`（`:468`）一个 `while(true)` 循环里叠了三层补救：
    ① 上游 415 且发过压缩体 → 关压缩重发（`:526`）；
    ② 401/403 且是订阅 → `ResolveAfterUnauthorizedAsync` 刷新令牌后**原请求重发一次**（`:535`）；
    ③ 429 且 failover → `RateLimitResetOf`（`:687`，优先读 `anthropic-ratelimit-* reset` 头，其次 `Retry-After`，
    再兜底 5 分钟，钳在 [1min, 7d]）+ `FailOverAsync` **换账号**继续（`:564`）。
    限流并发下的 `AccountScheduler.Lease` 有 in-flight 槽位，失败必 `Dispose` 不泄漏（`:582 finally`）。
  - **多账号池 / 额度**：`Pipeline/UpstreamAuthResolver.cs` 的 `SubscriptionSupport`：
    `SelectAccount`（pinned → current → 第一个可用 → 第一个启用，逐级兜底）、
    `IsUsable` = enabled && Active && 不在冷却期、`SwitchModeOf`（manual/failover/balanced）、
    `ClientPolicyOf`（Claude 订阅默认 `ClaudeCodeOnly`，非 Claude Code 请求 **403 且不出本机**）、
    `MimicClaudeCodeOf`。balanced 模式按 `stickyKey` 把同一会话钉在同一账号上。
    计费在 `src/Astra.Core`，金额一律 **INTEGER 纳美元**（`AGENTS.md`："Never store money as a float"），
    SQLite(WAL) 落库。
- **Anthropic Messages ↔ OpenAI Chat 双向转换：有（且是四协议全互通）**
  - `Protocol/Anthropic/AnthropicRequestCodec.cs`（33KB，入站→IR）、`AnthropicResponseEncoder.cs`（IR→Anthropic SSE/JSON）、
    `AnthropicResponseDecoder.cs`（Anthropic 上游→IR）、`AnthropicCodec.cs`（组装）。
    对称地有 `Protocol/Chat/*`、`Protocol/Responses/*`、`Protocol/Gemini/*`。
  - `Protocol/GatewayCodecs.cs` 是注册表。`AGENTS.md` 明令：
    "Protocol conversion is hub-and-spoke, never pairwise"，加协议 = 扩 IR + 写 codec + 加 golden fixture
    （`tests/Astra.Gateway.Tests/Fixtures`）。
- **客户端适配**：`src/Astra.Clients/Adapters/` 一客户端一适配器，**23 个**
  （codex / claude-code / claude-desktop / gemini-cli / opencode / grok-build / pi / hermes-agent /
  minimax-code / copilot-cli / vscode-copilot / crush / qwen-code / droid / kimi-code / zed /
  vscode-insiders / vscodium / omp / mimo-code / deepseek-harness / workbuddy / muse-code）。
  还原语义很讲究（`AGENTS.md` 第 6 条 + `Config/ClientConfigApplier.cs`）：
  每项改动按 keyPath 记原值，首次备份**永久保留** + 20 份滚动备份，还原时**跳过用户自己改过的值**（drift 检测），
  `astra-server restore-all` 能离线还原。Claude Desktop 走的是第三方 `Claude-3p/configLibrary/<uuid>.json`
  布局 + 两个 `claude_desktop_config.json` 里 `deploymentMode = "3p"`（`ClaudeDesktopClientAdapter.cs:9-14`，
  注释注明"并非 Anthropic 官方文档格式，是按 cc-switch 实测"），且**模型清单只暴露 sonnet/opus/haiku 三个 role id**
  （`GatewayEndpoints.cs:76-81`）。公共头注入 `ClaudeCodeDetector.cs` 判定调用方是不是真 Claude Code。
- **可复用设计点**
  1. `Ir.cs` 单文件 IR + `IProtocolCodec` 六方法契约 + "错误体也按协议分派"，是四仓库里最干净的抽象。
  2. `ReasoningPart.Origin`：把"签名不可跨协议"编码进类型，而不是靠编码器自觉。
  3. 头/体成对改写（beta ↔ cache_control ttl）写成两个互相引用的函数，注释点明不成对会被拒。
  4. 客户端的 per-keyPath 原值记录 + drift 跳过 + 离线 restore-all；这是"改用户配置"类功能的安全底线。
  5. 端口漂移 + `runtime.json` 记录真实端口，禁止脚本硬编码。

---

## 3. dsh-our-free-model — 三种对外协议都自己实现，Anthropic 入口最完整（含 count_tokens）

- **语言/部署**：纯 JS ESM，Node。**双重形态**：既是 DSH 插件（`package.json` 的 `main: ./index.js` +
  `cordis.patch.yml`），也有独立包（`packages/standalone/`，含自建管理页与 `cli.mjs`）。
  v2.1.0。License MIT（`LICENSE`，Copyright 2026 Ebony-Vinyl）。
- **代理什么 / 伪装成什么**：四条车道——① 匿名免密免费车道（打 OpenCode Zen，
  `Authorization: Bearer public`）；② EAC 协付渠道（登录态）；③ Kilo 免费池；④ 聚合渠道
  （用户自带 key 接 76 家，`src/providers/registry.json` 之外还有 `vendor/channel-pack` 的 13 家账号渠道）。
  对外伪装成 **OpenAI Chat + OpenAI Responses + Anthropic Messages**，三条都自己实现。
- **关键实现**
  - **路由与转发**：`src/forward.js` 是独立监听器。路由表在 `:330-368`：
    `OPTIONS` 免鉴权 → `/` 与 `/health` **故意放在鉴权之前**（注释：探活不该要 key，且只回一个空计数、
    模型花名册在鉴权路由上）→ 401 → `GET /v1/models` → `POST /v1/chat/completions` → `/v1/responses`
    → `/v1/messages` → `/v1/messages/count_tokens`（纯本地估算）→ 404。
    `serveCompletion`（`:609`）是三个端点共用的生命周期壳：绑定 `req.aborted`/`socket.close`/`res.close`
    **三个**信号到同一个幂等 abort（注释解释 Linux 上 `res.close` 会来太晚），并在第一段有效 delta 时打 trace 点。
  - **模型分流**：`src/upstream.js:119 endpointFor` —— 模型 id 决定上游路径，**同一上游不同模型走不同 wire**：
    `muse-spark-*`→`/zen/v1/responses`，`union-alpha`→`/zen/v1/messages`，其余→`/zen/v1/chat/completions`；
    `:126 wireFor` 决定编解码用哪套。
  - **鉴权头改写**：入站 `bearerOf`（`forward.js:110`）**同时接受 `Authorization: Bearer` 与 `x-api-key`**，
    `keyMatches`（`:103`）用 `crypto.timingSafeEqual` 常量时间比较。
    出站 `upstream.js:138 gatewayHeaders` 伪造真实桌面客户端指纹：
    `Bearer public` + `user-agent: opencode/1.18.31`（注释：网关校验 UA 版本 ≥1.17）+
    `x-opencode-client/session/request/project` 四个头。
    聚合渠道按 provider 的 `format` 决定头（`src/providers/index.js:158 defaultHeaders`）：
    `anthropic-messages` → `x-api-key` + `anthropic-version: 2023-06-01`，其余 → `Bearer`；
    且 `auth.kind === 'none'` 的 keyless provider **绝不发任何凭据头**。
    registry 里声明的 transport 头（如 `x-opencode-client`、MiniMax 的 `anthropic-beta`）放在最前，
    让协议/凭据头能赢冲突（注释写了这个顺序理由）。
  - **SSE 流式转发**：不自建 SSE 库，直接 `res.write`。两个关键件：
    `sendSse`（`:665`）写 `data: JSON\n\n`；
    `sseFrame`（`src/anthropic.js:321`）写**带 `event:` 名**的 Anthropic 帧
    （`event: <type>\ndata: <json>\n\n`）。
    还有 **SSE 心跳**：`startHeartbeat`（`:690`）每 15s（`SSE_HEARTBEAT_MS`）写 `: ping\n\n` 注释帧。
    注释给了实测依据：推理模型静默 70 秒，传输层会先死；且明确说明心跳**不会**重置 pi-ai 的
    content-idle 看门狗（那个只看有效内容 delta）——这种"我知道它治不了什么"的标注很值钱。
  - **错误与重试 / 降级链**：`src/combos.js` 是"一个模型名 = 一条有序链"的实现：
    `combo:` 前缀的虚拟目录行，策略 `fallback`（永远从头）或 `round-robin`（带 `stickyLimit` 轮转游标）。
    要害在 `:25-29` 的注释：链的能力元数据取各成员的**交集而非并集**——
    "按头部声明 vision 会把图片交给读不了图的兜底成员；按头部声明 1M 窗口会在 128K 的成员上越界"。
    链上第一个能出字的模型即答案，切换发生在任何内容交付之前（`combo-advance-test.mjs` 覆盖）。
    另有 `src/recovery.js` 的断流续写。**未验证**：我没找到 429 冷却台账一类的东西，
    看起来重试与限流处理不在这一层（可能在 `adapter.js` 的渠道实现里）。
  - **会话 / 多账号 / 额度**：多账号池在 `vendor/channel-pack`（13 家账号渠道，含 `account-pool.ts`、
    `account-probe.ts`、`buddy-credits.ts` 等），另有 `src/eac.js`(`:209` 打
    `${credential.base}/chat/completions`)、`src/eac-login.js`、`src/eac-user.js`。
    免密车道的额度按**会话**计，所以 `upstream.js` 做会话铸造：
    `sessionForConversation`（`:84`）用 harness 会话 id 的 sha256 派生**稳定**的
    `ses_<12hex><14base62>`，注释给了代价说明："每请求新会话会耗尽额度并表现为 429 且 retry-after 递增"；
    `requestIdFor(session, turnSeed)`（`:92`）让**同一轮的多次重试共用同一个 request id**。
    `mintSessionId`/`mintRequestId` 在无会话上下文时兜底（时间前缀 + 单调计数器 + base62）。
  - **工具指纹门**：`upstream.js:187 applyFingerprint` 是必须点名的一处。
    上游免费车道要求 `body.tools` 里声明小写的 `bash/glob/grep/read` 四件套，否则 403。
    做法不是硬塞，而是**多级**：调用方真有的 quartet 名字规范化大小写（上游把 `Bash`+`bash` 当重复）；
    缺的槽优先**从真能代跑的工具里提拔**（`QUARTET_DONORS = { bash: ['pwsh'] }`——因为
    dsh 在 Windows 上 shell 工具叫 `pwsh`）；实在没有才插一个自述
    "currently unavailable and must not be used" 的诱饵。注释有实测记录：只有 `pwsh` 时插 `bash` 诱饵，
    一个 run 里被模型调用 **24 次**，每次都是 unknown tool。返回 Map 用于**响应侧把名字还原**调用方的拼写
    （`restoreToolName`，`:254`），这是"提拔的工具还能被真正执行"的前提。
  - **本地假回答**：`src/local-optimize.js` —— Claude Code 会问的 5 类请求
    （配额探测、会话标题、建议、命令前缀、文件路径提取）**在进程内直接回答，不花一个上游 token**，
    且用**同一个帧生成器**渲染（`messagesEndpoint` 里 `for (const frame of anthropicFrames(chunk, localState))`），
    所以"本地答案与生成答案字节同形"。
- **Anthropic Messages ↔ OpenAI Chat 双向转换：有，而且是唯一提供 count_tokens 的**
  - 编解码器：`src/anthropic.js`。文件头有一张**逐块的映射表**（`:17-45`），这是全四仓库最好的文档实践：
    Anthropic 请求块 → harness 消息形状、harness finish chunk → Anthropic 帧，两栏对照写清。
  - 入站：`fromAnthropicRequest`（`:222`）。要点：
    `system`（字符串或块数组）压平成一条（`flattenSystem`，`:89`）；
    `tool_result` **拆成独立 `role:'tool'` 行**（`:261`），因为 harness 的 v4 形状如此，
    而"toClaudeMessages 会把相邻 user 侧行重新合并"——所以往返稳定；
    `thinking` 保留为 `reasoning` 块（注释解释：所有 projector 出站时会丢 reasoning，
    所以留着不会把旧思考回放给上游，但丢掉会抹掉历史里的推理）；
    `redacted_thinking` **直接丢**（`:28`，理由是"不透明密文、绑定产生它的账号"）；
    `thinking.budget_tokens` 分档成 `light/balanced/deep`（`:197`），阈值刻意用整数 4096/16384
    且注释说明理由："正好发 8192 的意思是'默认'，不是'deep'"。
  - 出站流：`anthropicFrames(chunk, state)`（`:453`）。块纪律写得很细：
    `message_start` 只发一次且在首个 chunk 的帧之前；
    文本/思考块**同时只开一个**，另一种 delta 到达时先关前者；
    **tool 块例外**——每个由自己的 block-end 关闭，所以并行工具调用可以并发流式输出
    （注释：若为了开新块而关旧块，会把一个调用的 JSON 拆到两个同 id 块里，客户端会执行两次）。
    另一个真实细节：**tool_use 块在知道名字之前不能宣布**，所以 `tool-call-delta` 的参数片段先存进
    slot 的 `held`，名字到齐再开块并倒出（`:498-509`）；如果名字始终没来，就**不宣布这个块**（`:532`），
    因为"宣布一个没有名字的调用等于交给客户端一个跑不了的调用"。
    `stop_reason` 还有 `truncated` 压过 `tool_use` 的规则（`:683`）：
    "上限截断赢了工具调用，否则客户端会执行一个被截断的调用"。
  - 出站非流：`anthropicMessage`（`:658`）。内容顺序遵循 Anthropic"thinking 在答案之前"；
    注释直言 **thinking 块不带 `signature` 字段**，理由："这侧没有有效签名，空字符串是重放客户端无法验证的谎"。
  - 计数：`countTokens`（`:793`）= 字符/4 + 每图固定 1024 + 工具 schema 字符/4 + 每工具 8 帧开销 + 固定 16 底噪
    （注释：底噪是为了"空 body 不能回 0"，否则客户端拿它做除数会炸）。
  - 另有一条**跨协议**的 Anthropic 上游通道：`src/aggregate.js:651 anthropicFrames(payload, model)`
    处理"provider 原生只讲 Anthropic"的情形，`:26-35` 注释说明原则：
    "Anthropic 的 SSE 是另一套事件词汇，诚实的做法是让它端到端跑 messages wire，
    而不是把事件翻译成 chat 再翻回来"。
- **对 Claude Code / Codex 的适配**：`index.js:2144-2202` 生成可复制的"接入配方"，
  按**实际绑定的端口与真实要求的 key** 渲染：Claude Code 给
  `export ANTHROPIC_BASE_URL=... / ANTHROPIC_AUTH_TOKEN=... / ANTHROPIC_MODEL=...`，
  并直接写着 `# the gateway speaks /v1/messages natively — no translation shim needed`；
  Codex 给 `~/.codex/config.toml` 片段；另给 OpenAI SDK / Anthropic SDK / curl / 局域网变体。
  文档自述"模型选择器里直接出现免费模型"。
- **安全的两个独立门**（值得抄）：本机监听**只绑回环**，且 `resolveLoopbackBind`（`forward.js:160`）
  会**把主机名解析成 IP 再逐个校验**，注释给出攻击面："`localhost` 被当字符串匹配、却由 OS 解析，
  一个 hosts 文件就能把免密额度交给整个子网"（issue #19）。
  LAN 中继（`startLanRelay`，`:492`）是第二扇门：默认关、绑 `0.0.0.0`、
  **`/health` 也要鉴权**（注释：未鉴权的存活应答等于向整个子网宣告"这台机器在代理"）、
  用**独立的 lanKey**（不是本机 key，泄了只需轮换）、只放行 5 条白名单路径、
  入口把 lanKey **换成**本机 key 再转发（两把 key 永不通用）、
  `x-ofm-relay-hop` 跳数标记让"中继端口配成本机端口"这种自环直接 508。
- **可复用设计点**
  1. `anthropic.js` 的两张映射表（请求块级、finish 帧级）+ 一个纯函数编解码器（文件头明写
     "no I/O, no sockets, no timers, no randomness"）→ 编解码可以脱网单测。
  2. tool_use 块"名字未到不开块、始终无名则不宣布"，以及"截断压过 tool_use"，是 Anthropic 线上
     两个具体的正确性坑。
  3. 工具指纹门的三级策略（规范化 → 提拔真工具 → 才插诱饵）+ 名字还原 Map。
  4. 会话铸造 / 请求 id 稳定化，让"重试不额外烧额度"。
  5. SSE 注释帧心跳，并诚实标注它**治不了** content-idle 看门狗。
  6. 双门安全模型（本机免密门 / 局域网独立凭据门 + 白名单 + 跳数标记）。

---

## 4. deepseek-harness-codearts — 对外**没有** Anthropic 入口，但上游侧协议覆盖最广

- **语言/部署**：TypeScript，`tsc` 编译到 `lib/`，客户端 bundle 用 esbuild（`plugin-src/client/build.mjs`）；
  Node `^22.19.0 || >=24.0.0`；pnpm workspace。DSH 插件（`package.json` 的 `name` 是 `dsh-codearts-auth`，
  v0.2.0-rc.10）。License MIT（`LICENSE`，Copyright 2026 Jet）。
- **代理什么 / 伪装成什么**：上游 10+ 家账号渠道（CodeArts、Buddy、WorkBuddy、LobsterAI、Qoder 国际/中国版、
  TRAE、Cline、Loomy、Raccoon、MiniMax、Gemini、ZCode、OpenCode、Accio、OfficeAce——`src/` 里
  每家有 `<name>-adapter.ts` + `-auth.ts` + `-credits.ts` + `-product.ts` 的家族）。
  **对外只伪装成 OpenAI Chat Completions 与 OpenAI Responses**。
- **关键实现**
  - **路由与转发**：`src/openai-gateway/server.ts:452 requestHandler`。路径就四条：
    `GET /v1/models`、`GET /v1/reasoning-efforts`（**自创只读端点**，见下）、
    `POST /v1/chat/completions`、`POST /v1/responses`，其余 404。下载 `messages.ts` 里没有
    `/v1/messages` 的影子——**这是四者中唯一不提供 Anthropic 入站的**。
    默认绑 `127.0.0.1`，容器场景下可配 `DSH_OPENAI_GATEWAY_HOST`，**但拒绝通配地址**
    （`AGENTS.md` 的 protocol-wire 笔记详述该判据被两轮审计证伪过两次的history：
    最终必须是"先字符闸门 fail-closed（只放行 `[A-Za-z0-9.:%_-]`）→ 再通配识别"，
    且判据串与 listen 串必须分成两个函数）。
    `keepAliveTimeout = 120000` / `headersTimeout = 130000` 显式设置（`server.ts:496-499`，
    注释：不设就落 Node 默认 5 秒，对"人读着回复、几分钟后再问一句"完全不够）。
  - **鉴权**：`server.ts:158 authorized` —— 只认 `Authorization: Bearer <key>`，**不收 `x-api-key`**
    （与另外三个仓库相反，因为它只服务 OpenAI 协议客户端）。
    key 由 `auth.ts` 的 `loadOrCreateApiKey` 在 `~/.dsh` 下生成/读取，且**在创建网关时解析而非 start 时**
    （注释：设置页要在网关没跑时也能读到 key 去配置外部客户端）。刻意只经 `createOpenAiGateway()`
    暴露，不另开读 key 通道（注释：多一个入口就多一处"读到的和网关在用的不是同一个"）。
  - **SSE 流式转发**：`src/openai-gateway/stream.ts` 的 `toOpenAiSse`（`:162`）与
    `collectOpenAiCompletion`（`:225`）——**流式与非流式共用同一个 `consumeChunk` 状态机**（`:107`），
    只有外壳不同。要点：
    * 结束原因映射集中在 `finishReason()`（`:46`）：`stop/tool-calls/max-tokens/aborted/error` → OpenAI 拼写。
    * usage 只在真有值时发（`:214`），缓存与推理明细同时给**官方字段与私有兼容字段**
      （`prompt_tokens_details.cached_tokens` + `completion_tokens_details.reasoning_tokens`
      + DeepSeek 系私有 `prompt_cache_hit_tokens`），注释说明为什么两套都要（老消费者读私有、Codex 只认官方）。
    * **流已发 200 后状态码改不了**，所以把该有的 404/429 放进 error 对象的 `status` 字段供客户端读
      （`:196`），并把上游"模型不存在"从 502 翻成 404——注释给了代价："502 会被客户端当可重试故障白耗额度"。
    * 流正常结束但没收到 finish → 补一个 `incomplete_stream` 错误帧，不装作成功。
  - **错误与重试**：`withRequest`（`server.ts:302`）是 `/v1/chat/completions` 与 `/v1/responses`
    **共用**的生命周期壳：绑定请求 abort、统一错误翻译。注释明说必须共用，否则"同一个错误在一个 URL 回 404、
    在另一个回 502"。流式路径的错误在 `toOpenAiSse` 内部翻成流内 error 帧；
    非流式在 `collectOpenAiCompletion` 里抛 `OpenAiGatewayError`。
    `failureToOpenAiError`（`:282`）有一条踩坑注释：`LlmError` 的 `status` **不是顶层属性**，
    只在 `e.failure.status`，只读顶层等于没读、404 会退化成 502 被反复重试。
    重试本身由宿主 dsh 的 llm runtime 负责，网关层**不重试**（`AGENTS.md`：网关是无状态转发）。
  - **模型 ID 映射**：路由 id 是 `<provider>/<model>`，由 `messages.ts` 的 `parseModelRoute` 纯字符串切片
    （**不做任何归一化**——`messages.ts:126-135` 的注释明确记录这个尺寸：
    大小写混用的目录 + 从不拒绝未知 id 的适配器 ⇒ 白名单判据必须忽略大小写，否则防线为零）。
    响应 id 由网关生成（`resp_<uuid>`），且注释强调"本网关无状态，这个 id 只能用于关联，
    `GET /v1/responses/{id}` 一律 404"。
  - **思考档位翻译（本仓库最有普适价值的一处）**：`messages.ts:200 normalizeReasoningEffort`。
    问题：各 provider 的档位 id 是**上游私有 wire 值**（TRAE `light/high/extra_high`、
    LobsterAI `off/high/xhigh`、Raccoon `on/off`），而 OpenAI 协议客户端只有固定 8 档词汇
    （Codex/CC Switch 都是 `none…ultra`），**客户端表达不出私有 id**。
    实例报障：`does not support reasoning effort "high"` 让整轮对话不可用。
    结论是**翻译而非拒绝**：按强度序就近落到模型声明的同族档位；完全不认识的值（拼错的 `banana`）
    仍然 400。返回值是**三态** `string | undefined | null`，其中 `null` = "用户传了但本模型不适用 → 明确不下发"，
    而 `undefined` = "用户没传"——注释用一整段解释为什么这两者**必须**分开：
    下游 `toGenerateOptions` 以 `!== undefined` 判定"是否给了值"，用 `undefined` 表示"不下发"会被当成"没给"
    而回退读原始 body，**归一化的结论被原样抵消、缺陷第二轮复发**。
    但它**不能静默**：`translated` 记 info、`unexpressible` 记 warn
    （`server.ts:353 logEffortNotice`，理由：加了翻译层之后 translated 是正常路径，用 warn 会刷满日志），
    并开一个自创只读端点 **`GET /v1/reasoning-efforts`**（`server.ts:211`）把
    8 个规范名逐一的结局列出来（原样/就近/无法表达/不下发），让"猜错也不会失败、也不必猜"。
    `AGENTS.md` 还有一条相关纪律：展示出口（含 `detail` 字段）与分类出口（不含）
    必须分成 `errorDetail` / `errorDetailForClassify` 两个函数——
    因为无条件加 `detail` 腿会**翻转错误码**（含 "out of credits" 时 INVALID_REQUEST → QUOTA_EXCEEDED，
    含超限措辞时触发溢出压缩）。
  - **多账号池 / 额度**：`src/account-pool.ts`（117KB）、`src/account-probe.ts`、`src/credits.ts`、
    `src/provider-credits.ts`、`src/token-ledger.ts`、`src/dead-model-store.ts`、
    `src/captcha-pool.ts`（验证码池）、`src/auto-checkin.ts`、`src/serial-queue.ts`。
    `AGENTS.md` 里有大量硬教训值得单独一读，例如：
    续期三件套缺一不可（启动先跑一轮 / lead-time 判据 / 回写账号池），
    且"`refreshable` 只是凭据材料的镜像，**绝不能当续期门禁**"——
    把它当过滤条件会把一次瞬时失败固化成**单向门**，账号从此永不自愈；
    "判终态前先重读凭据"（并发下服务端烧掉的是旧 refresh_token，磁盘上躺着一份可用的新的）；
    `InvalidDPoPHeader` 不算 refresh_token 失效。
  - **body 上限**：`BODY_LIMIT = 300MiB`（`server.ts:91`），由 `DEFAULT_IMAGE_LIMITS`
    的 200MiB 原图 × 4/3 base64 反推，注释要求"改图片声明时必须回来核对"，
    且有纯算术用例锁住这个关系。超限行为也有踩坑记录：**必须 413，不能销毁 socket**
    （原先 reject 后 destroy，413 永远发不出去，客户端只看到连接重置；对 Codex 这种每轮回传全量历史的
    客户端，历史一旦越限就再也回不去，因为它的自动压缩只看 token 不看字节——会话永久不可用）。
- **Anthropic Messages ↔ OpenAI Chat 双向转换：入站无，出站有（单向）**
  - **入站 Anthropic：不存在。** 网关只认 `/v1/chat/completions` 与 `/v1/responses`
    （`server.ts:452` 的路由表就是全部路由），`messages.ts` 是"OpenAI 消息 → DSH 内部 `Message`"。
    所以 **Claude Code / Claude Desktop 不能直连它**。
  - **出站到 Anthropic 上游：有。** 走的是"每个 provider 家族一个 adapter"的路子，不是 IR：
    `src/zcode-anthropic.ts`、`src/minimax-messages.ts`、`src/gemini-messages.ts`、`src/accio-messages.ts`、
    `src/opencode-messages.ts`、`src/message-shape.ts`、`src/sse.ts`。
    以 `src/zcode-anthropic.ts` 为例：`toAnthropicMessages(wire)`、`toAnthropicTools(tools)`、
    `consumeAnthropicSse(body, options)` —— 即 DSH 内部形状 → Anthropic 请求体、
    Anthropic SSE → DSH `StreamChunk`。
    `AGENTS.md` 记有该路的实测约束：`ZCODE_PLAN_MESSAGES_URL` 用 `/api/v1/zcode-plan/anthropic/v1/messages`，
    `anthropic-version: 2023-06-01` 必须带。
  - **消息形状归一化**：`src/message-shape.ts` 的 `normalizeHarnessMessages` /
    `detectMessageShape`——DSH 0.1.7 把工具结果从 `role:'user'` 内嵌 `tool-result` 块改成
    **一等 `role:'tool'` 消息**，判据用**形状而非版本号**（注释：0.1.7 没有任何协议协商机制，
    所以只能让代码同时认两种形状），且"两种形态同时出现时以 `tool-role` 为准"
    （升级期会话可能混合）。修法是**降级到旧形状**让既有 5 个序列化实现零改动，
    落点 6 处（`sse.ts` 的 `resolveToolPairing` + 5 个 adapter 的 `serializeMessages`）。
    注释里有实测数字：真实 session 修复前保留 **0** 条工具调用，修复后 **512** 条。
    这是一条**通用教训**：agent 消息模型的破坏性变更会静默剔掉工具调用，
    而症状表现为"没有工具调用就认为对话结束"或"循环思考"。
- **可复用设计点**
  1. **三态归一化返回值**（`string | undefined | null`）+ 把"为什么 `undefined` 和 `null` 不能混用"
     写进注释——这是"下游会把上游的修正吃掉"这类 bug 的通用防御写法。
  2. `GET /v1/reasoning-efforts` 这种**自创只读对照表端点**：客户端协议里没有位置承载的信息，
     就单开一个端点给人看、给脚本抄，并保证与 `/v1/models` 同源。
  3. 流式与非流式共用同一个 chunk 状态机，只有外壳不同。
  4. "流已发 200 → 状态码放进 error.status 字段" + 私有兼容字段与官方字段同时发。
  5. `withRequest` 单一错误出口，防止同一错误在两个 URL 上给出不同状态码。
  6. 把"未鉴权存活应答 = 向子网宣告自己在代理"这类推理写进代码注释，而不是只写进文档。
  7. 网关无状态 → 响应 id 只用于关联，并在注释里禁止拿它回查。

---

## 横向结论

### A. Anthropic ↔ OpenAI 双向转换，四个仓库里的三种做法

| 做法 | 仓库 | 结构 | 代价 |
|---|---|---|---|
| 单文件 IR + 每协议一个 codec | Astra-Gate | `Ir.cs` + `AnthropicRequestCodec/ResponseEncoder/ResponseDecoder` | 抽象成本最高，换来四协议互通 + 错误体也按协议分派 |
| 统一 chunk 词汇 + 一个解码器 + 每 wire 一个 projector | zen-gate | `lane.Decoder`（chat/messages/responses 三选一）+ `ToChatMessages`/`ToClaudeMessages`/`ToResponseInput` | 中等，最贴合"网关"场景 |
| 纯函数编解码器 + 映射表文档 | dsh-our-free-model | `src/anthropic.js`（入/出/流/非流/计数全在一个文件） | 单协议最快，但每加一条协议就多一个同形文件 |
| 每 provider 家族一个 adapter | deepseek-harness-codearts | `zcode-anthropic.ts` / `minimax-messages.ts` / … | 上游覆盖最广，但**没有入站 Anthropic**，协议转换方向单一 |
| 对比：Anthropic 上游直接透传 | dsh-our-free-model `src/aggregate.js:26-35` | 让只讲 Anthropic 的 provider 端到端跑 messages wire | 最少损失；注释明确反对"翻成 chat 再翻回来" |

**四者共性（可当作共识）**：
1. 都不做两两直转，都经过一个中间表示（IR / harness message / lane.Message）。
2. 都把 `tool_result` 提升成一等 `role:'tool'` 行，且都在出站时**重新合并**回 Anthropic 的 user 块
   （`toClaudeMessages` 的 `appendBlock` 合并同角色相邻轮 / `ToClaudeMessages` 的 `resultsFirst`）。
3. 都给 `tool_use`/`tool_call` 兜底造 id（`call_<hex>` / `call_<ulid>`），因为上游常常不给。
4. 都**丢弃跨协议的 thinking 签名**（`ReasoningPart.Origin`、dsh 不写 `signature` 字段、
   zen-gate 只搬文本）。

### B. 仿真/伪装的浓度排序（从最激进到只做必要改写）

1. **Astra-Gate `ClaudeCodeMimicry.cs`** — 伪造完整 Claude Code 身份：钉死 UA + 全套 Stainless 指纹 +
   规范三块 system（含逐字节复刻的计费指纹 `sha256(salt + 首条 user 消息的第 4/7/20 字节 + 版本)[..3]`）+
   `metadata.user_id`。默认关闭、文件头明写可能违反条款、风险自负。配套 `ClaudeOAuthHeaders` 区分
   Relay（真 CLI，白名单转发 + beta 合并 + cache_control ttl 同步）与 Compat（别的客户端，只给最小 beta）。
   **这是唯一一个把"仿真 Anthropic 客户端"当成一等公民做的项目**。
2. **dsh-our-free-model `upstream.js`** — 伪造 OpenCode 桌面客户端指纹（UA + 四个 `x-opencode-*` 头）+
   会话/请求 id 铸造 + 工具四件套指纹门（含"提拔真工具"与"插自述不可用的诱饵"两级）。
3. **zen-gate** — 不做调用方身份伪装，只做"advertise 正确的 base URL 让 SDK 自己拼路径"，
   靠 Codex 侧 `requires_openai_auth` / `model_catalog_url` 这类**上游自己认的配置项**接入，
   而不是伪造 CLI 指纹。
4. **deepseek-harness-codearts** — 不做调用方伪装；但有反向的一条：`tencent-fingerprint.ts` 的
   `rewriteTencentContentFingerprints` 是**改写内容级风控指纹**（腾讯系会把 system 里的客户端模板句整轮拦死），
   `AGENTS.md` 记有"指纹在第二条 system 也会被拦 ⇒ 必须改写所有 system 消息，不只 `messages[0]`"。

### C. 配置注入 / 客户端适配的共同安全底线

- 三家（Astra、zen-gate、dsh-our-free-model）都做"改配置文件接管客户端"，且都遵守同一条底线：
  **写前备份 / 成对标记 / 幂等重写 / 关闭即还原 / 跳过用户自己改过的值**。
  - Astra 最严：per-keyPath 原值 + 永久首写备份 + 20 份滚动 + drift 跳过 + 离线 `restore-all`。
  - zen-gate 次之：`backupFile` / `latestBackup` + `# >>> … <<<` 成对标记 + 坏文件修复用例。
  - dsh-our-free-model：不直接改 Claude Code 配置，而是**生成可复制配方**（把"改配置"留给用户），
    只对自己的 LAN 中继做配置管理。
- 只有 Astra 与 zen-gate 真正接管 Claude Code；接管手段都是 `ANTHROPIC_BASE_URL` +
  `ANTHROPIC_AUTH_TOKEN`（zen-gate 还写 `ANTHROPIC_MODEL` 与 opus/sonnet/haiku 三个映射）。
- Codex 侧的共识是 **`wire_api = "responses"`**（zen-gate `agents/codex.go` 注释：
  当前 Codex 拒绝 `chat` provider）+ 注入模型目录（`model_catalog_url` / `model_catalog_json`）。
  zen-gate 还记录了一个具体陷阱：TOML 不允许重复键，重复一次 Codex 整份配置拒解析。

### D. 我**没有**验证 / 没读到的东西

- `dsh-our-free-model` 的 `src/adapter.js`（40KB）与 `vendor/channel-pack/pack.js`（数万行，是打包产物）
  只做了 grep 级阅读；13 家账号渠道各自的协议细节没有逐个读。
- `dsh-our-free-model` 的 429 冷却/重试台账没有找到对等物——重试与限流处理可能在
  `adapter.js` 内部；**未确认**。
- `deepseek-harness-codearts` 的 `src/sse.ts`（68KB）、`src/index.ts`（188KB）、
  `src/jet-hub-rpc.ts`（310KB）只做了 grep 级阅读；`account-pool.ts` 的 117KB 也没通读，
  账号池结论主要来自 `AGENTS.md` 的项目笔记（那些笔记本身是该项目自己的实测记录，非二手转述）。
- Astra-Gate 的 `GatewayPipeline.cs` 通读了架构与上游发送/重试部分，
  但 `StreamAsync` 到 `BufferAsync` 之间的 200 行只做了行级定位，没逐行读。
- zen-gate 的 `internal/lane/workbuddy*.go`（约 50KB）、`internal/gateway/admin.go`（52KB）未读。
- 四个仓库的 README 自述功能与代码行为我都尽量分开标注；但"模型清单里出现哪些模型"
  这类运行期事实（依赖上游当时状态）**无法从源码验证**，README 里的数字（如"76 家服务商"、
  "13 个免费商预设"）我只核到对应注册表文件存在，没有逐条清点数量。

### E. 克隆与落地位置

```
.scratch/model-gateway/vendor/dsh-our-free-model/          (GitHub Ebony-Vinyl/dsh-our-free-model)
.scratch/model-gateway/vendor/deepseek-harness-codearts/   (Gitee iJetLi/deepseek-harness-codearts)
.scratch/model-gateway/vendor/Astra-Gate/                  (GitHub 239573049/Astra-Gate)
.scratch/model-gateway/vendor/zen-gate/                    (GitHub LAGcomcom/zen-gate)
```

全部 `git clone --depth 1` 成功，无一个需要退回 `web_fetch`；因此本文没有任何"仅凭网页摘要"的结论。
