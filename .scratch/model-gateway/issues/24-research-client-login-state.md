# 未登录态下能否只用网关凭据

Type: research
Status: resolved
Blocked by: —

## Question

本机现状（票 13）：Claude Desktop 1P 已登录、开发者模式已开，3P 走网关已端到端跑通；**Claude Code 2.1.296 未登录、无套餐**；**Codex CLI 0.162.1 已装、未登录**。

网关把 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`（或 `ANTHROPIC_API_KEY`）与四个 `ANTHROPIC_DEFAULT_*_MODEL` 写进 Claude Code 的 `settings.json`，把 `model_providers.*`（`base_url` / `env_key` / `wire_api="responses"` / `http_headers`）与 `model_provider` 写进 Codex 的 `config.toml` 之后，这两个客户端在**没有官方账号登录**的情况下能否启动并只用网关凭据发请求？Claude Desktop 3P 是否也不要求官方登录态？

要查清的事实：

1. Claude Code：无官方登录时只靠 base URL + token 能否启动并发请求；首次运行有没有强制的登录 / onboarding / 信任弹窗；有没有版本相关的门槛。
2. Codex CLI 同理：自定义 `model_provider` + `env_key` 之后还要不要 ChatGPT 登录；Store 版桌面应用与 CLI 有没有差别。
3. Claude Desktop 3P 是否也不要求官方登录态（本机 1P 已登录，两侧 userData 与登录态隔离已实测）。
4. 由此得到的事实基础：客户端卡片要不要把「未登录」当成阻断性错误；哪些步骤是必须让用户手动做一次的。

## 背景

- 已解析票据：[交付原型凭据并装好三个客户端](13-task-prototype-credentials-and-clients.md)、[前端信息架构与交互](17-grilling-ui-information-architecture.md)（客户端卡片文案等本票结论）。
- 安全边界：只读，不改用户现有客户端配置；不启动会要求登录的交互式会话；无法安全验证的写「未验证 + 精确步骤」。

## Answer

**结论：「未登录」对三个客户端都不是阻断性错误**；唯一例外是 **Codex 桌面应用的模型选择器**（按登录身份门控，官方标 not planned）。完整证据、本机实测与出处见产物 [client-login-state.md](../research/client-login-state.md)。

| 客户端 | 未登录能否启动 | 未登录能否发请求 | 强制登录 |
| --- | --- | --- | --- |
| Claude Code 2.1.299 | 能（本机零凭据走完 onboarding，启动 4 次） | 能，前提是凭据写在「首次运行向导之前可读」的位置（shell 导出 / `~/.claude/settings.json` 的 `env` / managed settings；**项目级 settings 不够**） | 无 |
| Codex CLI 0.162.1 | 能 | 能：自定义 `model_provider` + `env_key`（或 `experimental_bearer_token`）即免 ChatGPT 登录（隔离 `CODEX_HOME` 实测） | 无 |
| Codex 桌面应用 | 能 | 请求按 `config.toml` 走，但**选择器隐藏自定义模型** | 无（门控，非阻断） |
| Claude Desktop 3P | 能 | 能（票 15 已端到端实测） | 无，官方允许占位 key |

**三条进代码的硬约束**

1. **Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`**，不写 `ANTHROPIC_API_KEY`：后者需要一次 `/config` 交互批准，且曾拒绝过就静默失灵。
2. **Codex 不写 `requires_openai_auth = true`**（默认即 false、免登录）；写了 `env_key` 就必须保证那个环境变量真存在（`OPENAI_API_KEY` 顶替不了）；不写 `auth.json`（第三方 key 只进 `config.toml`）。
3. **凭据落点**：Claude Code 的 `~/.claude/settings.json` `env` 块合格；Desktop 3P 静态 key 允许占位。

**必须让用户手动做一次的步骤**：Desktop 开发者模式 + Configure Third-Party Inference；新机器首次运行向导（本机已完成）；每个新工作目录一次 trust dialog；**只有**想让 Codex 桌面应用选择器显示网关模型时，才需要登录一次官方账号（Free 即可）；三个客户端改配置后都要重启。另：**别用 `/logout` 清场**（官方说会重置首次运行状态）。

**未验证（需一次隔离实测，精确步骤见调研 §4）**：U1 Claude Code 在只有 `settings.json` env 块时真实发请求；U2 全新机器上 env 块能否抢在首次运行向导之前生效（决定「全自动」承诺能否成立）；U4 Codex CLI 真实发 `POST /v1/responses`；U5 Codex 桌面应用在有 `model_providers` 时请求是否真走网关。工具限制：OpenAI 官方 Codex 文档域在本机解析不到公网 IP，抓不到，Codex 侧缺官方文档层证据（靠二进制字符串 + cc-switch 实现互证）。

**对界面与后续票据的影响**

- 客户端卡片**不把「未登录」当阻断性错误**；Desktop 卡片仍按票 17 的四态方案。
- Codex 桌面应用与 CLI 的风险不同（选择器门控 vs 无），是否拆成两张卡片 → 新开 [票 25](25-grilling-codex-desktop-card-split.md)。
- 硬约束已写进 [spec §8 与 §8.3](../spec.md)。

**2026-10-14 更正（票 25 实测定案）**：本票与 `research/client-login-state.md` §2.5 引用的「Codex 桌面应用的模型选择器按登录身份门控」**不成立**——选择器只认 `model_catalog_json` 模型目录文件，写上目录后零官方登录也能看到自定义模型（「Codex」与「ChatGPT Work」两个模式都验证过）；且实测桌面应用确实按 `config.toml` 的 provider 发了 `POST /v1/responses`（**U5 结案**）。桌面应用真正需要官方登录的只有**云线程**（`Sign in to ChatGPT to start a durable thread.`）。证据：[codex-desktop-model-picker-gating.md](../research/codex-desktop-model-picker-gating.md)；卡片形状与引导结论见 [票 25](25-grilling-codex-desktop-card-split.md)。
