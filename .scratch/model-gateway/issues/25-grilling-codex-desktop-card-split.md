# Codex 卡片：CLI 与桌面应用要不要拆开

Type: grilling
Status: resolved
Blocked by: —

## Question

票 24 结论：Codex **CLI** 用自定义 `model_provider` + `env_key` 免 ChatGPT 登录即可工作；但 Codex **桌面应用**的模型选择器按登录身份门控——没有官方登录态时，`config.toml` 里的自定义模型被藏起来（官方标为 not planned），请求可能仍走网关但用户在界面上选不到。

要定：

1. 客户端页把 Codex 做成**一张卡片还是两张**（CLI / 桌面应用）？
2. 卡片上「未登录」与「选择器被门控」怎么表达——普通提示 / 黄色警告 / 阻断性错误？
3. 要不要在卡片上直接给「登录一次官方账号（Free 即可）即可解锁选择器」的引导与按钮（网关只做跳转引导，不代登录）？

## 背景

- 已解析票据：[未登录态下能否只用网关凭据](24-research-client-login-state.md)、[前端信息架构与交互](17-grilling-ui-information-architecture.md)（客户端页四态卡片定义在该票 §4）。
- 两者共用 `%USERPROFILE%\.codex\config.toml`，所以「拆卡片」拆的是**界面表达**，不是配置文件。
- 证据：`research/client-login-state.md` §2.5；`vendor/cc-switch/docs/guides/codex-desktop-custom-model-visibility-zh.md:16-20`。

## Answer

2026-10-14 与用户三轮弹窗对话 + 三次本机实测定案（全程中文）。证据与复现步骤：[codex-desktop-model-picker-gating.md](../research/codex-desktop-model-picker-gating.md)。

### 前提被实测推翻

本票（承票 24 与 cc-switch 文档）假设「桌面应用的模型选择器按登录身份门控，没登录就藏起 `config.toml` 里的自定义模型」。本机实测**不成立**：

| 实验 | 做法 | 结果 |
| --- | --- | --- |
| 1 | 写 `model_provider` + `[model_providers.gateway-probe]`（`wire_api="responses"`），**不写模型目录**，完全重启应用 | 选择器只有官方模型，没有自定义条目 |
| 2 | 再补 `model_catalog_json`（指向含一条 `Gateway Probe` 的目录文件），完全重启应用 | **`Gateway Probe` 在「Codex」与「ChatGPT Work」两个模式里都出现**，全程零官方登录 |
| 3 | mock 监听 `127.0.0.1:8787`，应用里选中 `Gateway Probe` 发一条消息 | mock 收到 **`POST /v1/responses`**（`originator: Codex Desktop`）→ 请求确实按 `config.toml` 的 provider 走 |

机制：**写了 `model_catalog_json` 之后 Codex 只认目录文件里的模型**（`vendor/cc-switch/src-tauri/src/services/provider/codex_official_models.rs:3`）；第三方模型进选择器靠**模型目录**，与登录无关。cc-switch 那条结论对本机这一版（MSIX `OpenAI.Codex 26.1007.2314.0`）不成立，本机产物里 `not planned` 0 命中。

实验 3 还带回两条进代码的事实：请求头 `accept: text/event-stream` + body `"stream":true`（**必须按 SSE 回**，回普通 JSON 会被判 `stream disconnected before completion`），以及 `x-openai-internal-codex-responses-lite: true`。票 24 的 **U5 结案**。

### 定案

1. **不拆卡**：客户端页保持票 17 的**一张 Codex 卡 + 四态徽章**。CLI 与桌面应用对网关无行为差异（同一份 `~/.codex/config.toml`、都靠模型目录显示模型、都按 provider 发请求），配置写入与还原也只有一组动作。「拆两张卡」作废。
2. **不设黄色警告**：桌面应用独有的限制只剩一条——**云线程（durable thread）需要官方登录**（本机日志 `Sign in to ChatGPT to start a durable thread.`；模型显示与本地使用都不需要），按**普通说明**写在卡上。
3. **保留登录引导**：卡上给「登录一次官方账号（Free 即可）即可用桌面应用的云线程」的说明 + 一个按钮；按钮只做跳转或调起 `codex login`，**网关不代登录、不读不存官方凭据**。
4. **绝不触碰 `~/.codex/auth.json`**：切换与还原都只改 `config.toml`（cc-switch 默认会删它、导致官方登录态丢失——不学）。
5. **新结论 → 新票**：网关必须为 Codex 生成**模型目录文件**并把要暴露的模型写进去，否则 CLI 与桌面应用的选择器都看不到网关模型 → [Codex 模型目录：内容、并存与还原](31-grilling-codex-model-catalog.md)。
6. **新结论 → 新票**：Codex 侧的端到端验收（`/v1/responses` + SSE + 模型目录）此前没有票 → [原型：Codex 端到端](32-prototype-codex-e2e.md)（阻塞于票 31）。
7. 票 24、`research/client-login-state.md` §2.5 与 spec §8.3 的相关结论已就地更正。
