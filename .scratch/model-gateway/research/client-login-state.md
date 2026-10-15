# 未登录态下能否只用网关凭据（票 24 调研）

- 日期：2026-10-14
- 问题：Claude Code / Codex CLI（含 Codex 桌面应用）/ Claude Desktop 3P，在**完全没有官方账号登录**的前提下，只靠网关凭据能否启动、能否发请求、首次运行有没有强制弹窗、有没有版本门槛。
- 调研方式：只读一手来源。优先级为「本机已装客户端的产物」>「官方文档」>「本仓库已有调研与第三方开源实现（cc-switch）」。
- **安全边界遵守情况**：全程未修改任何用户配置文件（`~/.claude/`、`~/.codex/`、`%LOCALAPPDATA%\Claude-3p\`），未启动任何交互式会话（未运行裸 `claude`、未进 `codex` 交互模式、未启动 Claude Desktop），未创建/切换分支，未 push。唯一一次「实验」在临时目录 `%TEMP%\cgx-probe-*` 内用 `CODEX_HOME` 隔离完成，跑完已删除（已核实目录不存在）。**第二次执行的复核实验同样在临时目录 `%TEMP%\cg-verify-*` 内用 `CODEX_HOME` 隔离完成，跑完即删**（`Test-Path = False`，见附录 A.6）。
- 证据等级：
  - **[本机实测]** 本机产物或本机可复现的非交互命令输出
  - **[官方文档]** Anthropic / Claude Code / Claude Desktop 官方文档
  - **[官方二进制]** Codex CLI 0.162.1 发行二进制中的符号与字符串
  - **[第三方实现]** 本仓库 `vendor/cc-switch` 的代码与文档（注明其性质）
- **复核状态**：本文件已由第二次执行（2026-10-14 晚）独立复核：§1–§3 的**本机产物断言逐条重验为真**，并用隔离 `CODEX_HOME` **重跑了 §2.2 的 A/B/D/G 四组探针**（结果一致），另补了可编程检测手段与一批新证据。仅修正一处错误：§4 的 U3 括号里原写「`.claude.json` 里 `hasTrustDialogAccepted` 键不存在」，实际该键**按目录存在**。全部新增内容见文末 [附录 A](#附录-a独立复核记录2026-10-14-晚第二次执行本票-24)。

---

## 0. 结论摘要

| 客户端 | 未登录能否启动 | 未登录能否发请求 | 首次运行强制登录/弹窗 | 版本门槛 |
|---|---|---|---|---|
| **Claude Code 2.1.299** | **能**（本机实测：无任何凭据完成 onboarding，启动 4 次） | **能**，前提是凭据写在「首次运行向导之前就能读到」的位置（shell 导出 / `~/.claude/settings.json` 的 `env` / managed settings） | **没有强制登录**；有一次性的首次运行向导与按目录的信任提示（trust dialog），二者都不是登录 | 无「某版本起强制登录」。只有 managed settings 里的 `forceLoginMethod` / `forceLoginOrgUUID` 会造成启动拒绝，本机不存在 |
| **Codex CLI 0.162.1** | **能** | **能**。自定义 `model_provider` + `env_key`（或 `experimental_bearer_token`）即可，**不需要 ChatGPT 登录**（本机隔离实测 A/E/F/H 组） | 无（非交互命令直接可用；交互式 TUI 首次会问登录方式，但被自定义 provider 绕过） | 本机未发现强制登录门槛；`requires_openai_auth = true` 是唯一会强制要 Codex 账号凭据的开关 |
| **Codex 桌面应用**（Store `OpenAI.Codex 26.1007.2314.0`） | 能（App 本身可开） | 请求会按 `config.toml` 走，但**模型选择器按登录身份门控**：无官方登录态时自定义模型被藏起来 | 官方把「在桌面 GUI 暴露自定义供应商模型」标记为 **not planned** | — |
| **Claude Desktop 3P** | **能，且不需要官方登录** | **能**（票 15 已端到端实测） | 静态 API key 模式官方明说「No per-device preparation is required」，且允许 placeholder key | — |

**一句话**：**「未登录」对这三个客户端都不是阻断性错误**，唯一真正的例外是 **Codex 桌面应用的模型选择器**——它把「有没有官方登录态」当成放行自定义模型的门禁，而 CLI 不受影响。

---

## 1. Claude Code（`claude` 2.1.299，native，`C:\Users\ZHANQUAN\.local\bin\claude.exe`）

### 1.1 本机状态：确实从未登录成功，但会话照常建立

| 事实 | 值 | 出处 |
|---|---|---|
| 版本 | `2.1.299 (Claude Code)`（由 2.1.296 自动更新而来） | `claude --version`；`~/.claude/.last-update-result.json`（`version_from: 2.1.296` → `version_to: 2.1.299`） |
| onboarding 已完成 | `hasCompletedOnboarding = true` | `C:\Users\ZHANQUAN\.claude.json` 顶层字段 |
| onboarding 版本 | `lastOnboardingVersion = "2.1.296"`，`firstStartVersion = "2.1.296"`，`migrationVersion = 14` | 同上 |
| 启动次数 | `numStartups = 4` | 同上 |
| **官方账号凭据：完全不存在** | `oauthAccount` 键**不存在**；`primaryApiKey`、`subscriptionType`、`hasAvailableSubscription`、`forceLoginMethod` **全部不存在** | 同上，逐键核实 |
| **磁盘上没有任何凭据文件** | `%USERPROFILE%\.claude\.credentials.json` **不存在** | `Test-Path` 实测 |
| API key 批准记录：不存在 | `customApiKeyResponses` 键**不存在** | `.claude.json` 逐键核实 |
| `/login` 曾被执行且**被中断** | 会话 `49ae17ae-…` 中 `/login` → `<local-command-stdout>Login interrupted</local-command-stdout>` | `~/.claude/projects/C--Users-ZHANQUAN/49ae17ae-0edc-4fb0-9ff7-a956d36f213b.jsonl:5-6` |
| **该会话的计费状态为零** | `totalCostUSD: 0`、`totalAPIDuration: 0`、`modelUsage: {}`、`permissionMode: auto`、`entrypoint: cli`、`version: 2.1.296` | 同文件 `:3-4`、`:8` |
| `/login` 也在历史里 | `{"display":"/login", …}` | `~/.claude/history.jsonl:1` |
| 三份 `.claude.json` 备份**均无** `oauthAccount` / `customApiKeyResponses` | `numStartups` 3 → 3 → 4 | `~/.claude/backups/.claude.json.backup.1791919476273`、`.1791953229257`、`.1791953371001` |

**读法**：这台机器上 Claude Code **完整走完了 onboarding、建立了真实会话、接受了 `/login` slash 命令并把它中断**，全过程磁盘上没有任何官方凭据。所以「未登录」不会把 Claude Code 挡在启动门外。**[本机实测]**

### 1.2 凭据必须放在哪：这是本票最可执行的一条

官方疑难表里有一行专门描述「curl 能通但 Claude Code 还要你登录」：

> The CLI has no credential of its own: a reachable base URL isn't one, and in an interactive session an `env` block in a project's `.claude/settings.json` or `.claude/settings.local.json` applies only after the first-run wizard and trust prompt.
> **Fix**: Set `ANTHROPIC_AUTH_TOKEN` somewhere Claude Code reads before first-run setup: **a shell export, the `env` block in `~/.claude/settings.json`, or managed settings**.
> — [官方文档] <https://code.claude.com/docs/en/llm-gateway-connect>（Troubleshoot gateway errors 表）

对网关的直接含义（本仓库票 13 第 31 行的计划正好落在允许集合里）：

| 位置 | 首次运行向导之前读得到吗 | 结论 |
|---|---|---|
| `%USERPROFILE%\.claude\settings.json` 的 `env` 块 | **是** | ✅ 网关该写这里 |
| shell 导出（`$env:ANTHROPIC_AUTH_TOKEN`） | 是 | ✅ 但只对当前终端，且背景会话不保证继承 |
| managed settings | 是 | 本机不存在，网关不该依赖 |
| 项目 `.claude/settings.json` / `.claude/settings.local.json` | **否**（过向导与信任提示之后才生效） | ❌ 不能当唯一凭据来源 |

同一页还给了优先级：「When both a shell export and a settings-file `env` block set the same variable, **the settings-file value applies**」。与 `client-integration.md:25` 的 H10 一致。

### 1.3 `ANTHROPIC_AUTH_TOKEN` vs `ANTHROPIC_API_KEY`：网关应优先写前者

官方「Authentication precedence」序列（<https://code.claude.com/docs/en/authentication>）**[官方文档]**：

1. 云厂商凭据（`CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / `_FOUNDRY`）
2. **`ANTHROPIC_AUTH_TOKEN`** — 发成 `Authorization: Bearer <value>`
3. **`ANTHROPIC_API_KEY`** — 发成 `X-Api-Key`
4. `apiKeyHelper`
5. `CLAUDE_CODE_OAUTH_TOKEN`
6. Anthropic profile / federation 凭据
7. `/login` 拿到的订阅 OAuth

关键差异（这是「必须让用户手动做一次」的**唯一**来源）：

| 变量 | 生效方式 | 出处 |
|---|---|---|
| `ANTHROPIC_AUTH_TOKEN` | 「With `ANTHROPIC_AUTH_TOKEN`, the variable takes precedence **immediately**.」 | llm-gateway-connect「Conflicts with an existing login」 |
| `ANTHROPIC_API_KEY` | 「With `ANTHROPIC_API_KEY`, you are **prompted once in interactive mode to approve the key** before it takes over.」；「In interactive mode, you are prompted to approve the key once before it overrides your subscription. To use your subscription instead, run `unset ANTHROPIC_API_KEY`」；「The key needs a one-time approval in interactive sessions, and a **previously declined key is ignored without asking again**」→ 补救入口是 `/config` 的 `Use custom API key` | authentication；env-vars；llm-gateway-connect 疑难表；`~/.claude/cache/changelog.md:1649`（`customApiKeyResponses` 相关修复） |

**结论**：网关写 `ANTHROPIC_AUTH_TOKEN` → 零手动步骤；写 `ANTHROPIC_API_KEY` → 用户必须在 Claude Code 里批准一次，且**曾拒绝过就不会再问**（会静默失灵）。所以票 13 里「`ANTHROPIC_AUTH_TOKEN`（或 `ANTHROPIC_API_KEY`）」这组二选一**不是等价的**，应定成 `ANTHROPIC_AUTH_TOKEN`，`ANTHROPIC_API_KEY` 只作为头兼容的兜底（网关如需 `x-api-key` 收凭据，那是**网关侧**的事，与客户端写哪个变量无关）。

### 1.4 首次运行：有向导、有信任提示，但都不是登录

- 首次运行确实有一次性向导与 trust dialog；`/logout` **会重置**这个状态：「Logging out also resets your first-launch setup state, so the next time you run `claude` it walks you through login and setup again」（authentication）。→ **界面文案必须提醒用户不要为了让网关卡生效去点 `/logout`。**
- 本机 `hasCompletedOnboarding` 已经是 `true`，所以这台机器不会再走一遍向导；但**新机器上**网关要么保证凭据在向导前可读（1.2 的做法），要么设 `hasCompletedOnboarding`。本仓库已有调研记了一条对应的外部断言：`client-integration.md:227`「**[厂商断言]** …把 `~/.claude.json` 的 `hasCompletedOnboarding` 设为 `true`，否则启动时报 `Unable to connect to Anthropic…`」。
- trust dialog 是按目录的、且不影响启动本身（只影响项目级 `env`、hooks、`.mcp.json`）；`changelog.md` 有多条 trust dialog 修复（`:1083`、`:3138`、`:6746`、`:7463`）。**精确的「新目录首次启动时弹几次、能否预置」未做本机实测**（见 §4 未验证项）。

### 1.5 版本门槛：没有「某版本起强制登录」

`~/.claude/cache/changelog.md`（987 KB，本机自带，418 个版本段，最新 `## 2.1.296`）里通检后的事实：

- **不存在**任何「必须登录才能用」的版本门槛；`requires login` / `must log in` 检索 **0 命中**。
- 会**拒绝启动**的只有 managed settings 驱动的策略，changelog 原文（`:1107`）：
  > Improved the startup refusal when managed settings require a sign-in (`forceLoginMethod` or `forceLoginOrgUUID`) **and an API key, token or `apiKeyHelper` is configured**: it now names the credential in use, where it is set, and how to remove it.

  即「策略强制登录」与「网关凭据」**互斥**——但这是 managed settings 独有的场景。文档同结论（authentication「Restrict login to your organization」；llm-gateway-connect 疑难表最后几行）。
- **本机没有 managed settings**：`C:\ProgramData\ClaudeCode\managed-settings.json`、`C:\Program Files\ClaudeCode\managed-settings.json`、`%USERPROFILE%\.claude\managed-settings.json`、`HKLM:\SOFTWARE\Policies\Claude`、`HKCU:\SOFTWARE\Policies\Claude` —— **全部 `Test-Path = False`**。（与票 15 评论第 73 行的实测一致。）**[本机实测]**
- 明确承认「走网关且没有 Anthropic 账号」是一类被支持的正常用户，changelog `:466`：
  > Fixed mods staying off for people who reach Claude through a gateway (`ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN`) **and have no Anthropic account**

  以及 `:3803`：`Fixed background jobs on LLM gateway auth (ANTHROPIC_AUTH_TOKEN + ANTHROPIC_BASE_URL) coming back "Not logged in" after the daemon respawns them`（说明曾经有过网关凭据下被判「Not logged in」的 bug，已修）。
- 与网关相关的两个**行为开关**（不是门槛，但会被用户误当成「不生效」）：
  - `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` 才去拉网关 `/v1/models`，**默认关**（env-vars；changelog `:5316`）。
  - `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` 会顺带关掉自动更新（llm-gateway-connect）。

### 1.6 `ANTHROPIC_DEFAULT_*_MODEL`：确切名单是「4 个家族 + 1 个默认」

票 13 说网关要写「四个 `ANTHROPIC_DEFAULT_*_MODEL`」。按 <https://code.claude.com/docs/en/env-vars> 逐条核对，**家族变量正好四个**，另有一个不属家族：

| 变量 | 语义 | 出处 |
|---|---|---|
| `ANTHROPIC_DEFAULT_OPUS_MODEL` | `opus` 别名解析到的 ID；Plan Mode 激活时 `opusplan` 也用它 | env-vars |
| `ANTHROPIC_DEFAULT_SONNET_MODEL` | `sonnet` 别名解析到的 ID；`opusplan` 非 Plan Mode 时用它 | env-vars |
| `ANTHROPIC_DEFAULT_HAIKU_MODEL` | `haiku` 别名解析到的 ID，**并用于后台功能**（会话标题等） | env-vars |
| `ANTHROPIC_DEFAULT_FABLE_MODEL` | `fable` 别名解析到的 ID，且是第三方 provider 上自动回退时被识别为 Fable 的依据 | env-vars |
| `ANTHROPIC_DEFAULT_MODEL` | **新会话默认模型**（v2.1.236+）；不是家族变量 | env-vars；changelog `:3148` |
| `ANTHROPIC_SMALL_FAST_MODEL` | **[DEPRECATED]** 后台任务的 Haiku 级模型 | env-vars；changelog `:5072`、`:8600` |

每个家族还有配套的 `_NAME` / `_DESCRIPTION` / `_SUPPORTED_CAPABILITIES` 变体（changelog `:6231`、`:5574`）；`changelog.md:3148` 另记 `ANTHROPIC_DEFAULT_MODEL` 与 `/model` 的覆盖关系（`/model` 选择会覆盖并跨重启持久化，`ANTHROPIC_MODEL` 不会）。

**与 Desktop 3P 的交叉印证**：票 15 实测的 Desktop 模型菜单**精确是四项** `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`（`issues/15-prototype-desktop-3p-e2e.md:42`），与这四个家族一一对应。**网关的「四家族映射」说法核实无误**；是否额外写 `ANTHROPIC_DEFAULT_MODEL` 是产品选择，不是协议要求。

---

## 2. Codex

### 2.1 本机状态（`codex-cli 0.162.1`，`%APPDATA%\npm\node_modules\@openai\codex`）

| 事实 | 值 / 命令 | 出处 |
|---|---|---|
| 版本 | `codex-cli 0.162.1` | `codex --version` **[本机实测]** |
| **未登录（退出码 1）** | `codex login status` → stderr `Not logged in`，`exit=1` | 同上 |
| `doctor` 判定 auth 失败 | `[XX] auth  no Codex credentials were found - Run codex login or provide an API key through a supported auth env var.`（总计 `20 ok / 4 notes / 3 warn / 1 fail`，`exit=1`） | `codex doctor --summary` |
| **没有 `auth.json`** | `%USERPROFILE%\.codex\` 下无 `auth.json` | 目录列举 |
| **`config.toml` 里没有 provider 配置** | 只有 `notify` / `[desktop]` / `[marketplaces.*]` / `[plugins.*]` / `[mcp_servers.*]`，**没有 `model_provider`、没有 `model_providers`** | `C:\Users\ZHANQUAN\.codex\config.toml` |
| `config.toml` 由**桌面应用**写入 | `BROWSER_USE_CODEX_APP_VERSION = "26.1007.21434"`、`CODEX_CLI_PATH`、`[desktop] followUpQueueMode` 等 | 同上 |
| 桌面应用已装且未运行 | `[ok] desktop the desktop application is installed` / `not running` | `codex doctor --summary` |

`codex login --help` 的三个非交互入口（**[本机实测]**，`--help` 原文）：
- `--with-api-key` — `Read the API key from stdin (e.g. \`printenv OPENAI_API_KEY | codex login --with-api-key\`)`
- `--with-access-token` — 从 stdin 读 access token
- `--device-auth`、子命令 `status`

二进制里另有一条**已废弃的旧写法**提示（说明 `codex login --api-key` 不能再用）：
> `The --api-key flag is no longer supported. Pipe the key instead, e.g. \`printenv OPENAI_API_KEY | codex login --with-api-key\``
> `Choose one login credential source: --with-api-key or --with-access-token`
> — **[官方二进制]** `codex.exe` 字符串

### 2.2 隔离实测：自定义 provider 不需要 ChatGPT 登录

方法：在 `%TEMP%\cgx-probe-<random>` 建 `config.toml`，用 `CODEX_HOME` 指向它，`codex doctor --summary` 读取判定。**没有触碰 `~/.codex/`，没有发任何推理请求，跑完删目录（已核实不存在）。** provider 的 `base_url` 是 `http://127.0.0.1:9/v1`（黑洞端口），所以 doctor 的 provider 可达性检查必然失败——这只影响 `reachability` 行，不影响 `auth` 行。

| 组 | config 关键内容 | auth 判定原文 | 结论 |
|---|---|---|---|
| **A** | `env_key = "PROBE_KEY"` + `requires_openai_auth = false`，`PROBE_KEY` 已设 | `[ok] auth is provided by the active model provider` | ✅ **不需要 Codex 登录** |
| **E** | 同上但 **`requires_openai_auth` 整行省略**，`PROBE_KEY` 已设 | `[ok] auth is provided by the active model provider` | ✅ **字段默认就是「不需要 OpenAI 登录」** |
| **F** | `requires_openai_auth = false` + `experimental_bearer_token = "sk-inline-token"`（**无 `env_key`**） | `[ok] OpenAI auth is not required for the active model provider` | ✅ 内联 token 可用，不需要环境变量 |
| **G** | `requires_openai_auth = false`，**完全没有任何凭据字段** | `[ok] OpenAI auth is not required for the active model provider` | ⚠️ 不阻断；请求会裸发（网关将收到 401） |
| **H** | `env_key` + `env_key_instructions` | `[ok] auth is provided by the active model provider` | `env_key_instructions` 只影响提示文案 |
| **B / C** | `env_key = "PROBE_KEY"` 但 **`PROBE_KEY` 未设**（即使 `OPENAI_API_KEY` 已设也一样） | `[XX] active model provider auth env var is missing - Set PROBE_KEY for the active model provider.` | ❌ **声明了 `env_key` 就构成硬性要求**；`OPENAI_API_KEY` **不能**替代 provider 自己的 `env_key` |
| **D** | `requires_openai_auth = true` + `PROBE_KEY` 已设 | `[XX] no Codex credentials were found - Run codex login or provide an API key through a supported auth env var.` | ❌ **`requires_openai_auth = true` 会强制要 Codex 账号凭据，provider 的 `env_key` 救不了** |

（全部 `exit=1`，因为 `reachability` 那一栏在黑洞端口下必然失败；`auth` 行才是本实验的读数。）

**这三条对网关最重要**：
1. 自定义 provider **默认**不要求 OpenAI 登录 → 网关不必让用户先跑一次 `codex login`。
2. **不要写 `requires_openai_auth = true`**，否则整个方案退化成「必须有 ChatGPT 账号」。
3. **写了 `env_key` 就必须保证那个环境变量真实存在**，否则 Codex 在 provider 校验阶段直接报错（B 组）。若不想依赖用户去设环境变量，`experimental_bearer_token`（F 组，内联）是可行替代。

### 2.3 字段清单（来自本机 0.162.1 二进制，最权威）

`codex.exe`（v0.162.1）里 `ModelProviderInfo` 的序列化字段（**[官方二进制]** 原样字符串，`struct ModelProviderInfo with 21 elements`）：

```
base_url  model_catalog_url  env_key  env_key_instructions
experimental_bearer_token  gateway_oauth  aws  wire_api  capabilities
query_params  request_max_retries  stream_max_retries  stream_idle_timeout_ms
websocket_connect_timeout_ms  requires_openai_auth
supports_websockets  supports_standalone_web_search
```

→ 票 13 里要写的 `base_url` / `env_key` / `wire_api = "responses"` / `http_headers` 都是真实字段（`http_headers` 的字段名在二进制里体现为 `header env var` / `env_http_headers` 一类字符串，见下）。同时确认了 `requires_openai_auth`、`env_key_instructions`、`experimental_bearer_token`、`gateway_oauth` 是**真字段**，不是社区传说。

其余从二进制取到的相关字符串：`auth.json`、`.auth storage mode`、`.auth file`、`OPENAI_API_KEY`、`CODEX_API_KEY`、`CODEX_ACCESS_TOKEN`、`env var` / `bearer token env var` / `header env var` / `<redacted>`、`Not logged in` / `Logged in` / `Successfully logged out`、`ChatGPT backend requests require Codex backend auth`、`Daybreak requires a signed-in ChatGPT account`。

注：**`~/.claude/cache/changelog.md` 那样的 Codex 版 changelog 本机不存在**，所以「有没有版本门槛」这条对 Codex 只有 0.162.1 这一个切片的证据。

### 2.4 `auth.json` 是否必需？不是——第三方实现的行为可直接印证

**[第三方实现]** `vendor/cc-switch`（一个成熟的第三方供应商切换器，本仓库已克隆）：

- `docs/guides/codex-official-auth-preservation-guide-zh.md:18`：
  > v3.20.1 起切换逻辑又简化了一步：**第三方 API Key 一律只写进 `config.toml`，永远不会写入 `auth.json`**。

- 同文件 `:133-145` 给出的直连第三方 provider 写法（**没有 `auth.json`、没有 `codex login`**）：
  ```toml
  model_provider = "custom"
  [model_providers.custom]
  name = "DeepSeek"
  base_url = "https://api.deepseek.com"
  wire_api = "responses"
  experimental_bearer_token = "sk-..."
  ```
- `src/config/codexProviderPresets.ts:88-89` 的模板统一写 `wire_api = "responses"` 与 `requires_openai_auth = ${requiresOpenAiAuth}`；`:3292` 注释写明「`requires_openai_auth` 必须是 false：keyless + true 会被后端安全闸…」，`src/config/codexProviderPresets.test.ts:8,18,26,42-43` 断言第三方预设必须是 `requires_openai_auth = false`。
- 另有预设使用 `env_key = "OPENAI_API_KEY"`（`:1509-1512`），印证 `env_key` 是通行写法。

→ 与 §2.2 的 A/E 组完全一致：**Codex CLI 走第三方 provider 不需要 ChatGPT 登录，也不需要有 `auth.json`。**

### 2.5 **Codex 桌面应用 ≠ Codex CLI**（这是本票最重要的差异）

> **2026-10-14 更正（票 25 实测）**：本节引用的「无官方登录 → 选择器隐藏自定义模型」在本机装的那一版桌面应用上**不成立**。选择器只显示 `model_catalog_json` 目录文件里的模型：写上目录后零官方登录也能看到自定义模型（「Codex」与「ChatGPT Work」两个模式都验证），且请求确实按 `config.toml` 的 provider 发（mock 收到 `POST /v1/responses`）。桌面应用真正需要登录的是**云线程**。证据见 [`codex-desktop-model-picker-gating.md`](./codex-desktop-model-picker-gating.md)。以下原文保留作历史记录。

**[第三方实现]** `vendor/cc-switch/docs/guides/codex-desktop-custom-model-visibility-zh.md:16-20`：

> 这**不是 CC Switch 的本地配置问题**……而是 **Codex 桌面应用（上游闭源客户端）自身的模型门控行为**。
> Codex 桌面应用的模型选择器会按你**当前的登录身份**来决定放行哪些模型：**当它检测不到官方 ChatGPT / Codex 登录态时，会把选择器强制回落到官方默认模型，把你通过 `config.toml` 配置的自定义模型藏起来**（思考等级也会一并回落到官方默认）。官方已把「在桌面 GUI 里暴露自定义供应商模型」标记为 **not planned**，因此 CC Switch 无法从桌面 GUI 层面彻底修复它。

同文件 `:20`：**命令行 `codex` 的 `/model` 与请求路由都能正常识别 `config.toml` 里的自定义供应商，唯独桌面 GUI 的选择器受这层门控限制。** `:37` 还记了一条副作用：「关闭时直连切到第三方会**删除 `auth.json`**。很多人第一次切到第三方就这样丢了官方登录态」。

**对本机的意义**：本机 Store 版 Codex 桌面应用（`OpenAI.Codex 26.1007.2314.0`）**从未官方登录过**（无 `auth.json`）。按上述门控，**即使网关把 `model_providers` 写进 `~/.codex/config.toml` 并被 CLI 正确读取，桌面应用的模型选择器里也不会出现网关的模型**。这不是网关的 bug，是上游行为。

**结论**：网关的 Codex 支持应**明确区分「Codex CLI」与「Codex 桌面应用」两张卡片**。CLI 卡片可以承诺「无需登录，开箱即用」；桌面应用卡片必须写明「官方已把 GUI 暴露自定义模型标为 not planned；要么登录一次官方账号（Free 即可），要么接受模型选择器看不到网关模型（请求仍可按 config.toml 走）」。

---

## 3. Claude Desktop 3P（`Claude_2.37337.1.0`，MSIX，`%LOCALAPPDATA%\Claude-3p\`）

### 3.1 3P 不要求官方登录态 —— 官方文档直说

**[官方文档]** <https://claude.com/docs/third-party/claude-desktop/gateway>（= 票 15 用的那份的 `.md` 版）：

- 凭据三选一，其中**静态 API key 是默认且零准备**：
  > | Proof of concept, or your gateway already issues per-team keys | **Static API key** (`inferenceGatewayApiKey`) | A long-lived secret distributed in the managed profile. |
  > **Static API key**: **No per-device preparation is required.** Generate an API key in your gateway and place it in the managed configuration as `inferenceGatewayApiKey`.
- 并且**明确允许占位 key**：
  > | Gateway API key | your gateway key (**or a placeholder if your gateway has none**) |
- 另两种（SSO/`inferenceGatewayOidc`、`inferenceCredentialHelper`）都是可选的进阶路径，不是登录要求。
- 交叉印证 —— `authentication` 文档：
  > Claude Desktop and cloud sessions **do not call `apiKeyHelper` or read these environment variables**: they use OAuth, **except desktop sessions running a third-party inference configuration, which authenticate with that configuration's credential**.
- `llm-gateway-connect` 文档：
  > The desktop app reads gateway routing from its **third-party inference configuration**, not from `ANTHROPIC_BASE_URL` or `settings.json`.

### 3.2 本机实测：3P 确实在用网关凭据，且与 1P 登录态隔离

| 事实 | 值 | 出处 |
|---|---|---|
| 3P 凭据由 3P 配置提供，**不是** OAuth | `{"env":{"ANTHROPIC_AUTH_TOKEN":"prototype-local-key","ANTHROPIC_API_KEY":"","CLAUDE_CODE_HOST_AUTH_ENV_VAR":"ANTHROPIC_AUTH_TOKEN","ANTHROPIC_BASE_URL":"http://127.0.0.1:8787"},…}` | `%LOCALAPPDATA%\Claude-3p\host-creds-3edbc898-745a-45f4-82c5-b9aa2d9ac9d7.json` |
| 3P 启动即宣称 3P 模式 + 网关 provider | `[custom-3p] 3P mode active { provider: 'gateway' }` | `%LOCALAPPDATA%\Claude-3p\logs\main.log:13`（另 `:421`、`:1453`） |
| 凭据来源自称「managed config」 | `[custom-3p] Credentials loaded from managed config { provider: 'gateway', mcpServerCount: 0 }` | 同 `:11`（另 `:419`、`:1451`、`:2782`） |
| 3P 用**独立 userData** | `user_data_location: 'third_party'` | 同 `:107`、`:526` |
| 3P 身份是**新造出来的**（`<none>` → 合成 org） | `[sessions-bridge] identity reevaluate: <none> → 00000000-0000-4000-8000-000000000001:65328857-475d-49b4-bbac-25a17542c30b` | 同 `:224`、`:651`、`:1670` |
| 3P 自己的 accountUuid | `65328857-475d-49b4-bbac-25a17542c30b` | `%LOCALAPPDATA%\Claude-3p\config.json` → `lastKnownAccountUuid`；`cowork-enabled-cli-ops.json` → `ownerAccountId` |
| **1P 的 accountUuid 是另一个** | `e2ef09a3-9194-4b50-9713-726b79356f10` | `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\config.json` → `lastKnownAccountUuid` |
| 3P 的 org UUID 是**全零占位** | `orgId: '00000000-0000-4000-8000-000000000001'` | `main.log:224` 及 `[CCDScheduledTasks] Initialized { accountId: …, orgId: '00000000-…-000000000001' }` |
| 3P 部署模式 | `"deploymentMode": "3p"` | `%LOCALAPPDATA%\Claude-3p\claude_desktop_config.json` |
| 已端到端跑通 | 四家族流式问答全 200，四家族映射生效，零 4xx/5xx | `issues/15-prototype-desktop-3p-e2e.md:39-47` |

**读法**：3P 的 `65328857-…` 与合成的全零 org 是 **3P 自己生成的本地身份**，与 1P 的 `e2ef09a3-…` 是两个不同的值——这印证了票 15 第 53 行「3P 与 1P 的 userData 与登录态确实隔离」，也说明 **3P 运行不需要 1P 的登录态**。日志里那句 `claude.ai account active and logged in`（`:64`、`:479`、`:1498`）**不能**读成「3P 登录了 claude.ai」——同一次启动里 identity 才刚从 `<none>` 变出来，且凭据是 `provider: 'gateway'`。

**未完全排除**：`claude.ai account active and logged in` 这行的确切判定条件没能从产物里证伪，属于**未验证**（§4）。

### 3.3 3P 的协议要求（票 15 已结论，此处只列与本票相关的）

- `inferenceGatewayBaseUrl` **不要带 `/v1`**，Desktop 实际打 `<baseUrl>/v1/messages`（`issues/15:51`；`claude.com` 文档中示例的 `/v1` 后缀会变成 `/v1/v1/messages`）。
- `inferenceModels` 项**全是完整 ID 时 Desktop 跳过 `GET /v1/models`**（`issues/15:52`；文档原文：「When every entry is a full model ID, the app skips the `/v1/models` call automatically」）。
- `inferenceGatewayAuthScheme` 默认 `bearer`，可取 `x-api-key`（文档 configuration 键表）；票 15 用的是 `bearer` + `inferenceCredentialKind: static`。

---

## 4. 未验证项（明确标注，并给「需要一次实测 + 精确步骤」）

| # | 未验证的事 | 为什么没验 | 需要的实测与精确步骤 |
|---|---|---|---|
| U1 | **Claude Code 在「无任何官方凭据 + 只有 `~/.claude/settings.json` 的 `env` 块」时，第一个请求能否真的打到网关并拿到回复** | 禁止跑裸 `claude`（会进交互/要求登录），所以只验到了「启动与 onboarding 不被阻断」，没验到真实请求 | 在**临时目录**里做，不碰用户配置：① `$t="$env:TEMP\cc-probe"; New-Item -ItemType Directory $t`；② 写 `$t\settings.json` = `{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:8787","ANTHROPIC_AUTH_TOKEN":"probe"}}`；③ 起网关并让它记录请求；④ `$env:CLAUDE_CONFIG_DIR=$t`（官方：凭据目录可改，见 authentication「Log in with multiple accounts」）后跑 **一次非交互**请求（`claude -p "hi"` 或 `--bare`）；⑤ 看网关是否收到 `POST /v1/messages`，并确认 header 是 `Authorization: Bearer probe`；⑥ 删除 `$t` 并 `Remove-Item Env:CLAUDE_CONFIG_DIR`。**关键**：必须用 `CLAUDE_CONFIG_DIR` 隔离，否则会污染 `~/.claude.json`。 |
| U2 | **新机器（`hasCompletedOnboarding` 不存在）首次启动时，`~/.claude/settings.json` 的 `env` 块是否真的抢在向导之前生效** | 本机 `hasCompletedOnboarding` 已是 `true`，无法复现「全新机器」；文档说要，外部厂商断言说还要显式设 `hasCompletedOnboarding`（`client-integration.md:227`） | 同 U1 的隔离法，但用**全新空** `CLAUDE_CONFIG_DIR`（里面没有 `.claude.json`），跑 `claude -p`，观察：是否弹出向导/主题选择、是否报 `Unable to connect to Anthropic services`、网关是否收到请求。若失败，再试在同一临时目录的 `.claude.json` 里预置 `{"hasCompletedOnboarding":true}` 复跑对比。**这条决定了网关的「首次运行」文案能不能承诺全自动。** |
| U3 | **trust dialog 在新目录首次启动时的实际次数与可预置性** | 同上，需要交互式启动才能看到 | 在临时目录起 `claude`，记录弹窗次数与文案；再试在 `.claude.json` 的 `projects["<path>"].hasTrustDialogAccepted` 预置 `true` 后复跑，看是否消失。**（2026-10-14 晚更正）**：该键在本机 `%USERPROFILE%\.claude.json` 里**按目录存在**——`projects["C:/Users/ZHANQUAN/AIGC/Github"]`、`["…/Desktop"]`、`["…/AIGC/Github/Provide Gateway"]` 都是 `true`，而 `projects["C:/Users/ZHANQUAN"]`（家目录）是 `false`，与官方「家目录的信任只保留当次会话、不写盘」一致。原生文案也已在二进制里取到：确认按钮 `Yes, I trust this folder`，取消 `No, continue without these permissions` / `No, exit`（见附录 A.3）。 |
| U4 | **Codex CLI 真实发出一个 Responses 请求**（`POST /v1/responses`）在无 `auth.json` 时是否成功 | 禁止跑 `codex exec`（会真的发请求并写 rollout） | 用 `CODEX_HOME` 指向临时目录跑：`$env:CODEX_HOME="$env:TEMP\codex-probe"` + 写入 `model_provider` / `env_key` 的 `config.toml` + 设好那个环境变量，然后 `codex exec "hi"`，看网关是否收到请求、header 是否带 provider token；跑完删目录。**这条能把 §2.2 的 doctor 级结论升级成端到端结论。** |
| U5 | **Codex 桌面应用读取 `~/.codex/config.toml` 的 `model_providers` 后，请求是否真的走网关，还是只在选择器里被隐藏** | 需要启动桌面应用并肉眼观察；被安全边界禁止 | 手动：把 `model_providers` 写进 `~/.codex/config.toml`（先备份），重启 Codex 桌面应用，看 ① 选择器里有没有网关模型 ② 发一条消息后网关日志有没有请求。cc-switch 文档断言「请求按 config.toml 走，只有 GUI 门控」（`codex-desktop-custom-model-visibility-zh.md:20`），但这是**第三方断言，未在官方文档证实**。 |
| U6 | **Codex CLI 0.162.1 之前的版本门槛** | 本机只有一个版本切片，且 Codex 官方 changelog 本机不存在 | 需要时抓 <https://github.com/openai/codex/releases> 与 `CHANGELOG.md` 通检 `requires_openai_auth` / `env_key` 的引入与默认值变更。 |
| U7 | **3P 日志 `claude.ai account active and logged in` 的确切判定条件** | 需要读应用内部资源（`app\resources\ion-dist`）或源码，未做 | 在 MSIX 安装目录 `app\resources\ion-dist` 里 grep 该字符串，回溯它的判定分支（票 15 曾用同样手法挖出官方原文）。同时可 grep 1P 与 3P 的 LevelDB 找 `oauth:tokenCache*` 键；**本次对两份 `Local Storage\leveldb` 做 `oauth:[A-Za-z0-9_]+` 检索，两边都是 0 命中**，所以那个具体检查**不具结论性**（token cache 可能存在别处或键名不同）。 |
| U8 | **Claude Code 官方文档的可达性** | — | 已用 `code.claude.com`（文档已从 `docs.claude.com` 迁移，后者 302 到这里）。**Codex 侧官方文档 `developers.openai.com/codex/config-reference` 会跨域跳到 `learn.chatgpt.com`，而该主机名在本机解析到非公网 IP，抓不到** —— 所以 Codex 的官方文档这一层证据缺失，改用「本机二进制字符串 + cc-switch 实现 + 官方 README」三者互证。需要时请在能访问该域的机器上补。 |

---

## 5. 对网关与界面文案的影响

### 5.1 客户端卡片**不要**把「未登录」当阻断性错误

| 客户端 | 卡片应显示 | 理由 |
|---|---|---|
| Claude Code | **不需要官方登录**，正常（绿） | §1.1 本机实测 + §1.5 无版本门槛 + 官方明确承认无账号的网关用户（changelog `:466`） |
| Codex CLI | **不需要官方登录**，正常（绿） | §2.2 隔离实测 A/E/F/H |
| Codex 桌面应用 | **单独的提示（黄）**：「桌面应用的选择器需要官方登录态来放行自定义模型；官方标记为 not planned。CLI 不受影响。」 | §2.5，第三方实现的一手观察 |
| Claude Desktop 3P | **不需要官方登录**，正常（绿） | §3.1 官方文档 + §3.2 本机实测 |

「未登录」只有在**两个**条件下才该升级成错误：
1. Claude Code 检测到 **managed settings 里存在 `forceLoginMethod` / `forceLoginOrgUUID`** —— 这两者与网关凭据**不能共存**，启动会被拒。文案应直接指向「移除策略里的这两项」。（§1.5）
2. Codex 目标 provider 被写成了 **`requires_openai_auth = true`** —— 这是网关自己写出来的配置错误，应在写入前就避免。（§2.2 D 组）

**检测手段可以编程化（2026-10-14 晚新增，卡片四态不必靠猜）**：

| 客户端 | 读什么（非交互） | 未登录时的样子 |
|---|---|---|
| Claude Code | `claude auth status` | JSON `{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty","configDirectory":"…\\.claude"}`，**退出码 1** |
| Codex CLI | `codex login status` | stdout 空、stderr `Not logged in`，**退出码 1** |
| Codex CLI（更细） | `codex doctor --summary` | 能区分「provider 自带凭据」/「缺 `env_key` 指向的环境变量」/「需要 Codex 凭据」三种（附录 A.2） |
| Claude Desktop 3P | `%LOCALAPPDATA%\Claude-3p\configLibrary\_meta.json` + 对应 `<id>.json` | 与登录无关，只反映网关配置是否生效（附录 A.3 末表） |

### 5.2 必须让用户手动做一次的步骤（完整清单）

| # | 步骤 | 只在哪一种情况下需要 |
|---|---|---|
| M1 | 开启 Claude Desktop 的 Developer Mode（Help → Troubleshooting）并走 `Developer → Configure Third-Party Inference` | 只有**不用 MDM 推送配置**、要在应用里手填时才需要。票 15 已实测走通。（`client-integration.md:39-48`） |
| M2 | 在 Claude Code 里用 `/config` 打开 `Use custom API key` 批准一次 | **只有网关写 `ANTHROPIC_API_KEY` 时**才有。写 `ANTHROPIC_AUTH_TOKEN` 就没有这一步。→ **网关应只写 `ANTHROPIC_AUTH_TOKEN`，从而消灭 M2。**（§1.3） |
| M3 | 新机器上完成一次首次运行向导（主题选择等） | 凭据已按 §1.2 预置时**可能**不需要（U2 未验证）；本机 `hasCompletedOnboarding` 已为 `true`，不会再弹 |
| M4 | 对每个新工作目录接受一次 trust dialog | 默认需要，按目录一次（U3 未验证可预置性） |
| M5 | 官方登录一次（Free 即可） | **只有想让 Codex 桌面应用的模型选择器显示网关模型时**才需要；Codex CLI 不需要。（§2.5） |
| M6 | 重启客户端 | Claude Code 读启动期环境变量、Codex 在启动时读 `config.toml`、Desktop 需要 `Save & Restart`；三者都要重启才生效 |

### 5.3 需要写进界面文案的「反直觉」提醒

1. **不要点 `/logout`** 来「清干净」——它会**重置首次运行状态**，下次启动又走一遍向导+登录（§1.4，官方原话）。
2. **不要给 Codex 写 `requires_openai_auth = true`**；写完 `env_key = "X"` 就必须保证环境变量 `X` 真实存在，否则 Codex 报 `active model provider auth env var is missing`（§2.2 B/D）。
3. **Codex 桌面应用看不到网关模型不是配置错了**，是上游门控（§2.5）。
4. Claude Code 走网关时，`Remote Control` / `/schedule` / claude.ai 连接器 / 语音听写 / fast mode 的可用性会变化（`ANTHROPIC_BASE_URL` 指向非 Anthropic 主机 + 凭据变量被设）——界面别承诺这些功能（llm-gateway-connect「Slack, cloud sessions, and Remote Control」；changelog `:5156`、`:4180`）。
5. 网关模型要出现在 Claude Code 的 `/model` 选择器里，需显式开 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`（**默认关**），或改用 `ANTHROPIC_CUSTOM_MODEL_OPTION`（env-vars；changelog `:5316`）。
6. 本仓库已有调研 `client-integration.md:16-17`（H1/H2）关于「模型 ID 必须含 `claude`/`anthropic` 才会被 `/model` 保留」的约束，与本票不冲突，仍适用。

---

## 6. 来源清单

### 本机产物（**[本机实测]** / **[官方二进制]**）
- `%USERPROFILE%\.claude.json`（`hasCompletedOnboarding`、`lastOnboardingVersion`、`numStartups`、`firstStartVersion`、`migrationVersion`、`firstStartTime`；`oauthAccount`/`customApiKeyResponses`/`primaryApiKey` 均不存在）
- `%USERPROFILE%\.claude\backups\.claude.json.backup.{1791919476273,1791953229257,1791953371001}`
- `%USERPROFILE%\.claude\projects\C--Users-ZHANQUAN\49ae17ae-0edc-4fb0-9ff7-a956d36f213b.jsonl:3-8`
- `%USERPROFILE%\.claude\history.jsonl:1`
- `%USERPROFILE%\.claude\settings.json`、`%USERPROFILE%\.claude\.last-update-result.json`
- `%USERPROFILE%\.claude\cache\changelog.md`（987 162 字节，418 个版本段，最新 `## 2.1.296`；本文件引用的行号：`:466`、`:1107`、`:1649`、`:2375`、`:2422`、`:2473`、`:3148`、`:3803`、`:4180`、`:5072`、`:5156`、`:5316`、`:5574`、`:6231`、`:8600` 等）
- `%USERPROFILE%\.codex\config.toml`、`%USERPROFILE%\.codex\` 目录列举（无 `auth.json`）
- `%APPDATA%\npm\node_modules\@openai\codex\{package.json,README.md,bin\codex.js}`、`node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe`（333 479 728 字节）
- `%LOCALAPPDATA%\Claude-3p\{config.json,claude_desktop_config.json,developer_settings.json,host-creds-3edbc898-745a-45f4-82c5-b9aa2d9ac9d7.json,cowork-enabled-cli-ops.json,configLibrary\_meta.json}`
- `%LOCALAPPDATA%\Claude-3p\logs\main.log:11,13,64,107,224,419,421,479,526,651,1451,1453,1498,1670,2782`
- `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\config.json`
- 非交互命令：`claude --version`；`codex --version` / `codex --help` / `codex login --help` / `codex login status` / `codex doctor --summary` / `codex debug --help`
- 隔离实验：`%TEMP%\cgx-probe-*`（`CODEX_HOME` 隔离，已删除）

### 官方文档（**[官方文档]**）
- <https://code.claude.com/docs/en/llm-gateway-connect>（凭据变量、头映射、设置文件位置、首次运行前生效范围、疑难表含 `forceLoginMethod` 互斥行）
- <https://code.claude.com/docs/en/authentication>（登录方式、凭据优先级 1–7、`/logout` 重置首次运行、凭据存储位置、Desktop 不读 env）
- <https://code.claude.com/docs/en/env-vars>（`ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY` / `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU,FABLE}_MODEL` / `ANTHROPIC_DEFAULT_MODEL` / `ANTHROPIC_SMALL_FAST_MODEL`[DEPRECATED] / `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY` / `CLAUDE_CONFIG_DIR` / `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`）
- <https://claude.com/docs/third-party/claude-desktop/gateway>（静态 API key 零准备、允许 placeholder、`inferenceModels` 全完整 ID 时跳过 `/v1/models`、SSO 为可选项）
- <https://code.claude.com/docs/en/settings>、<https://code.claude.com/docs/en/settings-reference>
- 注意：`docs.claude.com/en/docs/claude-code/*` 已 302 到 `code.claude.com/docs/en/*`；`developers.openai.com/codex/*` 跨域跳到 `learn.chatgpt.com`（本机不可达，见 U8）

### 本仓库内
- `.scratch/model-gateway/issues/13-task-prototype-credentials-and-clients.md:29-32,43-44,54`
- `.scratch/model-gateway/issues/15-prototype-desktop-3p-e2e.md:39-47,51-54,67-77`
- `.scratch/model-gateway/research/client-integration.md:16-25,39-48,227`
- `.scratch/model-gateway/vendor/cc-switch/docs/guides/codex-desktop-custom-model-visibility-zh.md:16-20,37`
- `.scratch/model-gateway/vendor/cc-switch/docs/guides/codex-official-auth-preservation-guide-zh.md:18,133-145,179`
- `.scratch/model-gateway/vendor/cc-switch/src/config/codexProviderPresets.ts:88-89,1509-1512,3292`；`.../codexProviderPresets.test.ts:8,18,26,42-43`

---

## 7. 与「票据 24 三个小问」的逐条对答

1. **Claude Code 只靠 `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` 能否启动并发请求？首次运行有没有强制登录/onboarding/信任弹窗？有没有版本门槛？**
   - 启动：**能**，本机已实测「零凭据完成 onboarding + 建立 4 次会话」。
   - 发请求：**能**，前提是凭据在「首次运行向导之前可读」的位置（`~/.claude/settings.json` 的 `env` 是网关的正确落点；项目级 settings **不行**）。真实请求链路是 U1 未验证项。
   - 弹窗：**没有强制登录**；有一次性首次运行向导（`hasCompletedOnboarding`）与按目录的 trust dialog（U3 未验证细节）。
   - 版本门槛：**无**。只有 managed settings 的 `forceLoginMethod`/`forceLoginOrgUUID` 造成启动拒绝，本机不存在任何 managed settings。
   - 额外结论：**优先写 `ANTHROPIC_AUTH_TOKEN`**；`ANTHROPIC_API_KEY` 需要一次性交互批准且曾拒绝后静默失效。
2. **Codex 指定自定义 `model_provider` + `env_key` 后还要不要 ChatGPT 登录？Store 桌面应用与 CLI 有没有差别？**
   - CLI：**不要**。本机在隔离 `CODEX_HOME` 下实测，`env_key` + 默认（或显式 false）的 `requires_openai_auth` 判定为 `auth is provided by the active model provider`。
   - 三个陷阱：`requires_openai_auth = true` 会强制要 Codex 凭据；声明了 `env_key` 就必须真的设那个环境变量（`OPENAI_API_KEY` 不能顶替）；不写 `env_key` 而用 `experimental_bearer_token` 也可行。
   - 桌面应用 vs CLI：**有明确差别**。CLI 完整识别自定义 provider；桌面应用的模型选择器按登录身份门控，无官方登录态时**隐藏**自定义模型，官方标为 not planned。请求路由与 GUI 可见性是两回事（后者为第三方断言，U5）。
3. **Claude Desktop 3P 是否也不要求官方登录态？**
   - **不要求**。官方文档：静态 API key 模式「No per-device preparation is required」，且允许 placeholder key。本机实测：3P 用独立 userData（`user_data_location: 'third_party'`）、凭据来自 3P 自己的配置（`provider: 'gateway'`）、身份从 `<none>` 新造、accountUuid 与 1P 不同。

---

## 附录 A：独立复核记录（2026-10-14 晚，第二次执行本票 24）

复核边界与正文 §0 相同：只读 + 非交互命令；未改任何客户端配置文件，未启动交互式会话（未跑裸 `claude`、未进 `codex` 交互模式、未启动 Claude Desktop），未写注册表。唯一写入发生在 `%TEMP%\cg-verify-*`（隔离 `CODEX_HOME`），已在同一次运行内删除。写入情况逐条记录在 A.6。

### A.1 逐条重验为真的本机断言

| 断言（正文位置） | 复核结果 | 复核出处 |
|---|---|---|
| `~/.claude/settings.json` 只有 `autoUpdatesChannel`；无 `.credentials.json` | ✅ 一致（settings.json 37 字节） | `%USERPROFILE%\.claude\settings.json`、`Test-Path .claude\.credentials.json = False` |
| `oauthAccount` / `customApiKeyResponses` / `primaryApiKey` 不存在 | ✅ 一致 | 通读 `~/.claude.json`（10 119 字节 / 315 行）逐键核实 |
| `hasCompletedOnboarding=true`、`lastOnboardingVersion="2.1.296"`、`numStartups=4` | ✅ 一致 | 同上 `:292-294`、`:2` |
| `/login` 曾被执行且被中断 | ✅ 一致（`Login interrupted`） | `…\projects\C--Users-ZHANQUAN\49ae17ae-….jsonl:5-6`；`history.jsonl:1` |
| changelog 的三条原文（`:466`、`:1107`、`:3803`） | ✅ **逐字一致**；文件 987 162 字节 / 8 789 行 / 418 个 `## ` 段，与正文所记完全相同；`requires login`、`must log in` 各 **0 命中** | `%USERPROFILE%\.claude\cache\changelog.md` |
| 版本已由 2.1.296 自动更新到 2.1.299 | ✅ 一致（`claude --version` → `2.1.299 (Claude Code)`） | `.last-update-result.json`：`version_from 2.1.296` → `version_to 2.1.299`，`outcome success` |
| 3P 日志 `main.log` 的 `:11, :13, :64, :107, :224` | ✅ **逐字一致**（文件 3 035 行） | `%LOCALAPPDATA%\Claude-3p\logs\main.log` |
| 3P 与 1P 的 accountUuid 不同 | ✅ 一致（3P `65328857-475d-49b4-bbac-25a17542c30b`；1P `e2ef09a3-9194-4b50-9713-726b79356f10`） | 两侧 `config.json` 的 `lastKnownAccountUuid` |
| 3P 部署模式 | ✅ `deploymentMode = "3p"` | `%LOCALAPPDATA%\Claude-3p\claude_desktop_config.json` |
| 3P 凭据来自 3P 自己的配置而非 OAuth | ✅ `env.ANTHROPIC_AUTH_TOKEN`（19 字符，以 `prot` 开头）+ `env.CLAUDE_CODE_HOST_AUTH_ENV_VAR = ANTHROPIC_AUTH_TOKEN` + `env.ANTHROPIC_BASE_URL = http://127.0.0.1:8787`；`env.ANTHROPIC_API_KEY` 为空串（另有 `expiresAt` / `pid` / `procStart`） | `%LOCALAPPDATA%\Claude-3p\host-creds-3edbc898-745a-45f4-82c5-b9aa2d9ac9d7.json` |
| 本机无 managed settings | ✅ 四个路径全为 False | `%ProgramData%\ClaudeCode\managed-settings.json`、`%ProgramData%\ClaudeCode`、`C:\Program Files\ClaudeCode`、`%USERPROFILE%\.claude\managed-settings.json` |
| Codex 未登录、无 `auth.json`、`config.toml` 无 provider 段 | ✅ 一致 | `codex login status`（exit 1）；`~/.codex` 目录列举；`config.toml` 通读 |

### A.2 重跑 §2.2 的隔离探针（独立复现，结论一致）

方法：`%TEMP%\cg-verify-<8hex>\config.toml` + `CODEX_HOME` 指向该目录，跑 `codex doctor --summary`；provider 为 `base_url = "http://127.0.0.1:9/v1"`（黑洞端口，故 `reachability` 必然 ✗，与 `auth` 行无关）。**全程未触碰 `~/.codex/`。**

| 组 | config 关键内容 | `auth` 行原文 | 结论 |
|---|---|---|---|
| A | `env_key = "PROBE_KEY"`，`PROBE_KEY` 已设，**不写** `requires_openai_auth` | `✓ auth  auth is provided by the active model provider` | ✅ 不需要 Codex 登录 |
| E | 同 A 重跑一次 | 同上（可复现） | ✅ 确定性 |
| B | `env_key` 已写但 `PROBE_KEY` **未设** | `✗ auth  active model provider auth env var is missing - Set PROBE_KEY for the active model provider.` | ❌ 声明 `env_key` = 硬要求 |
| D | `requires_openai_auth = true` + `PROBE_KEY` 已设 | `✗ auth  no Codex credentials were found - Run codex login or provide an API key through a supported auth env var.` | ❌ **唯一会强制要 OpenAI/Codex 凭据的开关** |
| G | **完全没有任何凭据字段** | `✓ auth  OpenAI auth is not required for the active model provider` | ⚠️ 不阻断（请求会裸发，网关将收到无凭据请求） |

另观察到 `✓ websocket  Responses WebSocket is not enabled for the active provider`（自定义 provider 默认不开 WebSocket 通道，与 `client-integration.md` 的 Responses 要求不冲突：HTTPS 回退仍可用）。

### A.3 新增事实（正文未记）

**Claude Code（`%USERPROFILE%\.local\bin\claude.exe`，2.1.299，269 231 264 字节）——本机产物字符串：**

1. **登录态可脚本化查询**（卡片可直接用）：
   ```json
   {"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty",
    "analyticsDisabled": false, "projectsDirectory": "…\\.claude\\projects",
    "configDirectory": "…\\.claude"}
   ```
   命令 `claude auth status`，未登录时**退出码 1**。二进制内另有说明文字 ``'claude auth status' reports the Anthropic API as apiProvider "firstParty"``。
2. **`auth` 子命令族**：`s.command("auth").description("Manage authentication")`，其下 `login`（`Sign in to your Anthropic account`）带 `--email <email>` / `--sso` / `--console`（`Use Anthropic Console (API usage billing) instead of Claude subscription`）/ `--claudeai`；另有 `gateway auth is unpinned; run \`claude auth login\` to pin`。→ 网关「已回官方 / 一键还原」的文案可以直接指向 `claude auth login`。
3. **登录界面有显式的「不登录继续」出口**：字符串 `continue without logging in`、`to go back to login options`，组件 prop `allowContinueWithoutLogin`（默认 `!1`，即**只在特定条件下出现**）、`firstPartyLoginAllowed`、`consoleProfileLoginAllowed`、`allowedProviders`。→ 「未登录不是死路」在 UI 层有直接证据；出口的出现条件见 A.5 U10。
4. **网关认证模式下 onboarding 被代码直接置为完成**：
   ```js
   if(Ie()==="gateway"){if(XBt())return await Re((T)=>({...T,hasCompletedOnboarding:!0,lastOnboardingVersion:{…}}))
   ```
   → 认证模式为 `gateway` 时不经登录就把 `hasCompletedOnboarding` 置真。
5. **trust dialog 确切文案**：确认 `Yes, I trust this folder`；取消 `No, continue without these permissions`（有可授予权限时）/ `No, exit`；正文含 `This folder runs commands to mint HTTP headers (headersHelper), declared in …`、`These will apply without asking. Only proceed if you trust this configuration.`；埋点 `tengu_trust_dialog_shown`。持久化键即 `hasTrustDialogAccepted`（A.1 更正）。
6. **会话认证来源会被显式播报**（三段同源）：`ANTHROPIC_API_KEY is set, so this session is using API-key auth — unset it (or run in a shell without it) …`、`ANTHROPIC_AUTH_TOKEN is set, so this session is using API-key auth — …`、`ANTHROPIC_BASE_URL is set and does not point at api.anthropic.com, so this session is using a custom endpoint — …`。→ 把 `ANTHROPIC_AUTH_TOKEN` 归入「API-key auth」是官方自己的措辞，印证 §1.3。
7. **不存在** `CLAUDE_CODE_SKIP_ONBOARDING` / `skipOnboarding` 一类跳过开关（各 0 命中）→ 别指望用环境变量跳过首次向导。
8. **错误文案对照表**（可复用到错误模板）：`Not logged in · Please run /login`、`Authentication required · Sign in again to continue.`、`Invalid API key · Fix external API key`、`Invalid auth token · Fix external auth token`、`OAuth token revoked · Please run /login`、`Login expired · Please run /login`、`Your organization has disabled API key authentication · …`。

**Windows 凭据管理器（新增检查）**：`cmdkey /list` 全量输出（98 行）中**没有任何**目标名含 `Claude` / `Codex` / `Anthropic` / `OpenAI` 的条目 → 与「`.credentials.json` 不存在」互为佐证：本机确实没有存下任何官方凭据。（二进制里出现存储模式名 `Windows Credential Manager` / `windows-credm`，但本机未留下条目；官方 `authentication` 文档记的是 Windows 用 `%USERPROFILE%\.claude\.credentials.json`。）

**Codex CLI（`…\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe`，0.162.1，333 479 728 字节）——补充 §2.3：**

- **`wire_api = "chat"` 已是硬错误**（与 `client-integration.md` H6 一致，这次拿到 v0.162.1 的原文）：
  > `` `wire_api = "chat"` is no longer supported. How to fix: set `wire_api = "responses"` in your provider config. More info: https://github.com/openai/codex/discussions/7782 ``
- `codex doctor` 的 provider 感知文案（即 A.2 实测输出的来源）：`model provider requires OpenAI auth: …`、`OpenAI auth is not required for the active model provider.`、`provider auth env var: … (present)`、`provider auth env var: … (missing)`、`active model provider auth env var is missing.`、`auth is provided by the active model provider.`、`Set … for the active model provider.`
- `codex login status` 的取值集合：`Not logged in` / `Logged in using ChatGPT.` / `Logged in using an API key - …` / `Logged in using workload identity` / `Successfully logged out` / `Error checking login status: …`。
- `codex login --with-api-key` 必须从 stdin 喂：`--with-api-key expects the API key on stdin. Try piping it, e.g. \`printenv OPENAI_API_KEY | codex login --with-api-key\`.`、`No API key provided via stdin.`、`On a remote or headless machine? Use \`codex login --device-auth\` instead.`
- 需要账号的功能各自报错：`... requires ChatGPT authentication or API key authentication; run \`codex login\` or set CODEX_API_KEY.`、`Not signed in. Please run 'codex login' to sign in with ChatGPT, then re-run 'codex cloud'.`

**Claude Desktop 3P（`app.asar` 49 152 598 字节 + 官方文档）——新增证据：**

- **「要不要交互式登录」由配置键决定，静态 key 路径不进入登录分支**：asar 内的配置 schema 里，`inferenceGatewayBaseUrl` 的 `flatKey` 即 `inferenceGatewayBaseUrl`，`readers: ["desktop","m365","code"]`，`support:{enabled:{scopes:["3p"], availableInVersion:"1.2581.0"}, executionTarget:"desktop"}`；凭据层有 `needs_interactive` / `interactiveKind` / `signInRequiredMessage` / `sourceCanReauth`，只在 `inferenceCredentialKind` 取交互式（`interactive`）时才抛 `signInRequiredMessage`。票 15 用的是 `static` → **不经过登录步骤**。
- 官方文档（<https://claude.com/docs/third-party/claude-desktop/gateway>）的凭据三选一表：静态 API key「**No per-device preparation is required**」；「Gateway API key」一栏明写「**or a placeholder if your gateway has none**」；SSO（`inferenceGatewayOidc`）与 `inferenceCredentialHelper` 都是可选进阶路径。
- 菜单项与窗口标题原文均为 `Configure Third-Party Inference…`（asar 内 `{label: "Configure Third-Party Inference…", id:"9GRz7bC+rr", visible: QJ(), click: …}`），与文档「Developer → Configure Third-Party Inference…」一致。
- 应用自带 changelog 用 `surface: "3p"` 标注 3P 专属条目 → 3P 是官方一等发布面（与票 15 的结论一致）。

**Claude Desktop 3P 当前落盘状态（新观察，直接关系卡片「四态」）：**

| 事实 | 值 |
|---|---|
| 当前生效配置项 | `configLibrary\_meta.json` → `appliedId = 9f1c2b3a-4d5e-4f60-8a71-000000000001`（name `Claude Gateway (prototype)`）；entries 另含 `ac278ce7-9506-4f19-b147-25637015682c`（name `Default`） |
| **两个 profile 文件当前都是 `{}`** | `configLibrary\9f1c2b3a-….json` 与 `configLibrary\ac278ce7-….json` 内容均为 `{}` → **此刻 3P 没有网关字段** |
| 首次写入前的整目录备份仍在 | `Claude-3p\configLibrary.backup-20261014-191819\`（其 `_meta.json` 的 `appliedId` 为 `Default`）；指针 `configLibrary.backup.path` 指向它 |
| 3P setup 日志无登录痕迹 | `logs\custom3p-setup.log` 只有两条 Datadog 存储告警（`No storage available for session`） |

→ 对界面设计的两条含义：① Desktop 卡片的「当前值 / 期望值」比对对象是 `configLibrary\_meta.json` + 对应 `<id>.json`，**不是**注册表、不是 `%APPDATA%\Claude`、不是 `claude_desktop_config.json`；② `{}`（存在但空）与「文件不存在」是两种不同状态，还原必须按 `configLibrary.backup.path` 指针走整目录还原（票 15 的 `desktop-3p.ps1 -Mode restore` 已这么做）。`{}` 的成因未验证（A.5 U9）。

### A.4 复核期间对正文的直接改动

| 位置 | 改动 |
|---|---|
| 文件头 | 增加「复核状态」说明 |
| §5.1 末 | 增加「检测手段可以编程化」表（`claude auth status` / `codex login status` / `codex doctor --summary` / `configLibrary`） |
| §4 U3 | **更正**：`hasTrustDialogAccepted` 按目录存在（原写「不存在」），并补上原生按钮文案 |
| 附录 A | 新增本记录 |

### A.5 本次新增的未验证项

| # | 未验证的事 | 为什么没验 | 精确步骤 |
|---|---|---|---|
| U9 | **`configLibrary\*.json` 为何当前是 `{}`** | 需启动 Desktop 或改文件才能复现，被只读边界挡住 | 对比 `configLibrary.backup-20261014-191819\ac278ce7-….json` 与当前文件；若要复现，跑 `desktop-3p.ps1 -Mode gateway` 后立刻读回（**会写用户文件，需用户同意**） |
| U10 | **`allowContinueWithoutLogin` 的出现条件**（什么状态下登录界面才给「不登录继续」出口） | 需交互式启动 Claude Code，被边界挡住 | 在隔离 `CLAUDE_CONFIG_DIR` 下分别以「无任何凭据」/「只设 `ANTHROPIC_AUTH_TOKEN`」/「只设 `ANTHROPIC_API_KEY`」启动 `claude`，记录该选项是否出现 |
| U11 | **Desktop 3P 在完全没有 1P 登录态的机器上能否启动到可用**（正文 §3.1 是文档结论 + 本机 1P 已登录下的实测） | 本机 1P 已登录；注销 1P 属破坏性操作 | 另开 Windows 用户账户或全新机器：装 Desktop → **不登录** → Help → Troubleshooting → Enable Developer Mode → Developer → Configure Third-Party Inference… → 填 baseUrl + 任意 key（官方允许 placeholder）→ Save & Restart → 发一条消息；同时看 `%LOCALAPPDATA%\Claude-3p\logs\main.log` 是否出现 `[custom-3p] 3P mode active`。**注意**：`claude.ai account active and logged in` 这行在 1P 已登录的机器上也出现，不能当判据（U7） |

### A.6 复核期间的写入情况（诚实记录）

| 被触碰的东西 | 性质 | 说明 |
|---|---|---|
| `~/.codex/tmp/arg0/codex-arg0<id>\{apply_patch.bat, applypatch.bat, .lock}` | **非配置**（临时脚本目录） | 每次运行 `codex` 命令都会新建一个 arg0 目录并淘汰上一个 |
| `~/.codex/*.sqlite-shm`（goals/logs/memories/queue/state） | **非配置**（SQLite 共享内存） | 运行 `codex doctor` 时被触碰时间戳 |
| `~/.claude.json`、`~/.claude/settings.json`、`~/.codex/config.toml`、`~/.codex/auth.json`（不存在）、`%LOCALAPPDATA%\Claude-3p\**` | **未改动** | 在 `claude --version` / `claude auth status` / `codex --version` / `codex login status` / `codex doctor` 前后做了文件清单 + 长度 + 时间戳比对，差异只有上两行 |
| `%TEMP%\cg-verify-*` | 探针目录，**已删除** | `Test-Path = False` 已核实 |
