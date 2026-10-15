# 客户端接入网关的事实调研（Claude Desktop / Claude Code / Codex）

- 日期：2026-10-14
- 调研方式：只读一手来源（厂商官方文档、官方仓库源码）。第三方网关厂商文档（DeepSeek、千问AI平台）仅作为“网关侧如何适配客户端的实践证据”，凡属其对客户端行为的断言都单独标注为“厂商断言”。
- 证据等级：
  - **[官方文档]** Anthropic / OpenAI 官方文档
  - **[官方源码]** OpenAI Codex 仓库源码
  - **[厂商断言]** 第三方平台文档对客户端行为的描述（未在官方文档中证实）

---

## 0. 结论速览（硬约束）

| # | 约束 | 影响 |
|---|---|---|
| H1 | Claude Code 走 `ANTHROPIC_BASE_URL` 时，Anthropic Messages 协议是硬约束：必须实现 `POST /v1/messages`；`/v1/messages/count_tokens` 可选（缺失则退化为按字符估算）；`/v1/models` 仅在显式开启模型发现时需要 | 网关至少要能说 Anthropic Messages |
| H2 | Claude Code 的网关模型发现（`GET /v1/models?limit=1000`）只保留 `id` 中含 `claude` 或 `anthropic` 的条目（大小写不敏感） | 想让模型出现在 `/model` 选择器里，ID 必须含这两个子串，或用 `ANTHROPIC_CUSTOM_MODEL_OPTION` / 模型别名变量 |
| H3 | Claude Code 流式响应必须按序完整送达至 `message_delta` + `message_stop`；不得缓冲；长时间静默必须转发 SSE `ping` | 违反会被判定为连接中断 / 触发空闲超时 |
| H4 | Claude Desktop 3P（第三方推理）**确实存在官方配置入口**，`inferenceProvider` 是激活键；网关模式（`gateway`）要求 `POST /v1/messages`（含流式与工具调用），`GET /v1/models` 可选 | 与 Claude Code 同协议，可共用一套 Anthropic Messages 实现 |
| H5 | Claude Desktop 模型自动发现“只显示 ID 可识别为 Claude 的模型”；非 Claude ID 必须用 `inferenceModels` 显式列出**完整模型 ID**（裸别名如 `sonnet` 仍需发现才能解析） | 任意字符串 ID 只有在显式 `inferenceModels` 且为完整 ID 时才可用 |
| H6 | Codex 当前官方配置中 `wire_api` **只有 `responses` 一个合法值**，省略即默认；Chat Completions 不再受支持 | 网关必须实现 `POST /v1/responses`，`/v1/chat/completions` 不足以接入 |
| H7 | Codex 网关兼容性要求：`POST /v1/responses` + 增量 SSE + 以 `response.completed` 结束 + 回放式续轮 + function_call 与 function_call_output 配对 | `/v1/models` 或一次纯文本回复**不构成**兼容性证明 |
| H8 | Codex 客户端请求固定带 `store: false`、`stream: true`、`include: ["reasoning.encrypted_content"]`；非 OpenAI provider 时，除 `Reasoning`/`Compaction` 项外会清除 encrypted content | 网关与其上游必须能接受并回传加密推理项 |
| H9 | 两个客户端的认证头都要能配：Claude 侧 `ANTHROPIC_AUTH_TOKEN`→`Authorization: Bearer`，`ANTHROPIC_API_KEY`→`x-api-key`；Claude Desktop 3P 用 `inferenceGatewayAuthScheme` 在 `bearer` / `x-api-key` 间二选一；Codex 用 `env_key`（Bearer）或 `env_http_headers`（自定义头） | 一个网关最好两种头都收 |
| H10 | Claude Code 的 `~/.claude/settings.json` 的 `env` 块优先于 shell 导出；`ANTHROPIC_MODEL` 优先于 settings 的 `model`；`--model` / `/model` 优先于 `ANTHROPIC_MODEL` | 排查“配置不生效”时的顺序 |

---

## 1. Claude Desktop（第三方推理 / “3P”模式）

### 1.1 模式确实存在：官方名称与入口

**[官方文档]** 该模式官方称为 **Claude Desktop on third-party (3P)**：“a deployment mode of Claude Desktop that routes all model inference through a provider you configure”，provider 可以是 Google Cloud Agent Platform、Amazon Bedrock、Microsoft Foundry、**any compatible gateway you operate**，或直接是 Anthropic API。
来源：<https://claude.com/docs/third-party/claude-desktop/overview>

激活键：`inferenceProvider`。文档原文：“Selects the inference backend. **Setting this key activates third-party mode.** One of: `gateway`, `anthropic`, `bedrock`, `mantle`, `vertex`, `foundry`.”（Added in 1.2581.0）
来源：<https://claude.com/docs/third-party/claude-desktop/configuration>

UI 入口（单机/评估路径，官方步骤）：

1. 从 claude.com/download 安装 Claude Desktop；
2. 启动应用，**不要登录 Anthropic 账号**；
3. macOS 菜单栏 / Windows 左上角 ☰ → **Help → Troubleshooting → Enable Developer Mode**；
4. 然后 **Developer → Configure Third-Party Inference…** 打开配置窗口；
5. 填 provider / endpoint / 凭据 → **Apply Changes** → **Save & Restart**。

来源：<https://claude.com/docs/third-party/claude-desktop/installation>
另一处一致描述（Claude Code 文档侧）：<https://code.claude.com/docs/en/llm-gateway-connect>（“open Help → Troubleshooting → Enable Developer Mode… Then open Developer → Configure Third-Party Inference and enter your gateway base URL. An administrator-distributed configuration takes precedence and makes this form read-only”）。

### 1.2 配置文件路径（Windows 与其它平台）

**[官方文档]** 表（配置读取位置）：

| 平台 | 托管（MDM）位置 | 本地（用户）位置 |
|---|---|---|
| macOS | `/Library/Managed Preferences/<user>/com.anthropic.claudefordesktop.plist` | `~/Library/Application Support/Claude-3p/configLibrary/` |
| **Windows** | **`HKLM\SOFTWARE\Policies\Claude`（机器）/ `HKCU\SOFTWARE\Policies\Claude`（用户）** | **`%LOCALAPPDATA%\Claude-3p\configLibrary\`** |
| Linux | `/etc/claude-desktop/managed-settings.json` | `~/.config/Claude-3p/configLibrary/` |

本地位置是**目录**：`_meta.json` 记录当前生效的配置，每个配置是旁边一个 `<id>.json`；应用内配置窗口写到这里。

Windows 注册表细节（官方）：

- 值必须写成 **`REG_SZ`**，值名**直接挂在策略键下**（app 不读子键）；布尔/整数也接受 `REG_DWORD`（按十进制读）。
- 避免 `REG_EXPAND_SZ`：它会计入“机器策略存在”，但 app 读不到内容。
- 完全看不到 `REG_QWORD`、`REG_MULTI_SZ`、`REG_BINARY`。
- 两个 hive **不合并**：`HKLM\SOFTWARE\Policies\Claude` 下存在任何 `REG_SZ` / `REG_EXPAND_SZ` / `REG_DWORD` 时，app 完全忽略 `HKCU`。
- 对象/数组类型的键（如 `inferenceModels`、`inferenceCustomHeaders`、`inferenceGatewayOidc`）的值是**一个 JSON 文档字符串**。

来源：<https://claude.com/docs/third-party/claude-desktop/configuration>

配置在**启动时**生效；改完需完全退出并重启（1.46388.1 起运行中的应用会在下次配置复查时察觉变更，多数设置需重启）。

### 1.3 网关相关字段名

**[官方文档]** 汇总（`https://claude.com/docs/third-party/claude-desktop/configuration` 与
`https://claude.com/docs/third-party/claude-desktop/gateway`）：

| 键 | 类型 | 说明 |
|---|---|---|
| `inferenceProvider` | enum | `gateway` / `anthropic` / `bedrock` / `mantle` / `vertex` / `foundry`；设置即进入 3P 模式 |
| `inferenceGatewayBaseUrl` | string | 网关端点完整 URL |
| `inferenceGatewayApiKey` | string | 网关 API key |
| `inferenceGatewayAuthScheme` | enum | `bearer`（默认）/ `x-api-key`；决定凭据放在 `Authorization: Bearer` 还是 `x-api-key` 头 |
| `inferenceCredentialKind` | enum | `static` / `helper-script` / `interactive` / `vendor-profile` / `workforce` / `external-idp` |
| `inferenceCredentialHelper` / `...Windows` / `...Args` / `...TtlSec` / `...TimeoutSec` | string/int | 凭据助手脚本（stdout 只输出裸 token，或 `{"token":...,"headers":{...}}`） |
| `inferenceCustomHeaders` | object | 每个推理请求（以及模型发现请求）上附加的静态头；官方明确**不要放凭据** |
| `inferenceModels` | object[] | 显式模型列表，替代自动发现；条目形如 `[{"name": "bedrock/us.anthropic.claude-opus-5", "supports1m": true}]` |
| `modelDiscoveryEnabled` | boolean | 旧版本用于关掉 `/v1/models` 探测 |
| `modelCatalogEnabled` / `modelCatalogUrl` | boolean/string | Anthropic 签名模型目录的抓取（离线环境要关或指向镜像） |
| `inferenceStreamIdleTimeoutSec` | int | 网关 provider 专有，300–1800，默认 300；“只有 keep-alive ping 在流上”时额外等待的时间 |
| `toolSearchEnabled` | boolean | 默认 false；开启会让会话发送实验性 beta 头/字段 |
| `developerModeEnabled` | boolean | 是否允许开启 Developer Mode（2.37337.1） |
| `disableDeploymentModeChooser` | boolean | 阻止用户切回标准 claude.ai 模式 |

最小可用示例（Linux 文件形式，字段名通用）：

```json
{
  "inferenceProvider": "gateway",
  "inferenceGatewayBaseUrl": "https://gateway.example.com/v1",
  "inferenceGatewayApiKey": "sk-example",
  "inferenceCustomHeaders": { "X-Tenant-Id": "acme" }
}
```

### 1.4 请求打到哪些端点、用什么认证头

**[官方文档]** 网关 provider 的要求：

- `POST /v1/messages` **必须**，且**必须支持 streaming 与 tool use**。
- `GET /v1/models` **可选**：实现则 Claude Desktop on 3P 自动发现模型；不实现则必须显式设置 `inferenceModels`。
- 应当保留 prompt caching（转发 `cache_control`，不要改动 system prompt / tool list），否则每轮全量重算、`cache_read_input_tokens` 恒为 0。
- 认证头由 `inferenceGatewayAuthScheme` 决定：`bearer` → `Authorization: Bearer <credential>`（默认）；`x-api-key` → `x-api-key: <credential>`。
- 模型发现与推理请求都会带上 `inferenceCustomHeaders`（joined into the CLI's `ANTHROPIC_CUSTOM_HEADERS`）。

来源：<https://claude.com/docs/third-party/claude-desktop/gateway>

`/v1/models` 响应可选的增强字段（官方）：`anthropic_family_tier`（如 `sonnet` / `opus`，用于解析裸别名）、`is_family_default`、`display_name`、`description`、`supports_1m` 或 `max_input_tokens >= 1000000`。

### 1.5 模型 ID 接受什么：校验与可选列表

**[官方文档]** 关键原文与推论：

1. “When `inferenceModels` is unset, Claude Desktop on 3P populates the model picker from your gateway's `GET /v1/models` response. **Auto-discovery shows only models whose IDs are recognizably Claude.**”
2. “If your gateway does not implement `GET /v1/models`, give every `inferenceModels` entry **the full model ID your gateway accepts**; bare tier aliases such as `sonnet` rely on discovery to resolve. **When every entry is a full model ID, the app skips the `/v1/models` call automatically.**”
3. “If your gateway serves model aliases that don't match the discovery filter, developers can add those aliases manually with the model configuration variables.”（此句出现在 Claude Code 文档的模型发现章节）
4. 上下文窗口假定：标准条目对“会话识别为原生 1M 的模型”按 1M，其它（含不认识的 ID）按 200K；1M 条目恒为 1M。

来源：<https://claude.com/docs/third-party/claude-desktop/gateway>、<https://code.claude.com/docs/en/llm-gateway-protocol>

因此：

- **自动发现**路径受“_recognizably Claude_”过滤约束（官方未在该页给出过滤字符串；Claude Code 侧的等价过滤规则是 ID 含 `claude` 或 `anthropic`，见 2.4）。
- **显式 `inferenceModels`** 路径可填任意完整模型 ID（官方示例用了 `bedrock/us.anthropic.claude-opus-5` 这种带路由前缀的 ID），所以“任意字符串”在显式列表这一条路径上是可行的。
- 是否做**客户端校验**（拒绝非 Claude ID）：官方文档只描述了发现过滤与“裸别名需发现才能解析”，**没有**任何“必须匹配 `claude-opus-*` / `claude-sonnet-*` / `claude-haiku-*` 三档角色 ID”的官方表述。该断言来自第三方：
  - **[厂商断言]** 千问AI平台 CC Switch 章节：“由于千问AI平台模型 ID（如 `qwen3.7-max`）不是 Claude Desktop 识别的 `claude-sonnet-* / claude-opus-* / claude-haiku-*` 三档角色 ID，需开启**需要模型映射**”（<https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/claude-code>）。
  - **[厂商断言]** DeepSeek：“通过这样的映射，您在使用新版 Claude Desktop APP 的 developer 模式时，可以绕过 APP 对模型名的限制，只需改动 base_url 和 api_key”（<https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/claude_code>）。
  - 这两条描述的是**第三方网关产品自己的实现**（CC Switch 会写入桌面版第三方配置并做角色映射），**不能**当作 Anthropic 客户端硬校验的证据。

### 1.6 辅助端点：`count_tokens`、`/v1/models`、SSE 事件格式

- `/v1/messages/count_tokens`：**[官方文档]** Claude Desktop 3P 网关页**未提及**该端点；Claude Code 侧明确列为“optional”。→ 对 Desktop 而言 **不确定/未证实**。
- `/v1/models`：可选（见 1.4）。
- SSE：**[官方文档]** 网关页引用的 Anthropic streaming 文档要求；实测性要求包括：不要缓冲、必须抵达 `response.completed` 级别的终态（Codex 侧同义）、保留 `ping`（Claude Code 侧明确要求转发 `ping`，否则长思考期间会触发空闲超时）。
- **强制 egress 提示**：Code sessions 的 WebFetch 预检会直接访问 `api.anthropic.com`，除非设置 `skipWebFetchPreflight` 或禁用 WebFetch。

### 1.7 如果“3p”不是你以为的那个东西：实际替代路径

官方提供的其他把 Claude Desktop 指向自建网关的方式：

1. **Claude Desktop on 3P + `inferenceProvider: "gateway"`**（本文重点）。
2. **Claude apps gateway**（Anthropic 自托管网关，自身就是个 bootstrap server，可直接下发连接键）：<https://code.claude.com/docs/en/claude-apps-gateway>
3. **Bootstrap server**：设备上只放 `bootstrapUrl`（+ `bootstrapOidc` 等），其余配置在登录时从你自建 HTTPS 端点返回：<https://claude.com/docs/third-party/claude-desktop/bootstrap>
4. **企业 Admin Console / MDM**：<https://claude.com/docs/third-party/claude-desktop/mdm>、<https://claude.com/docs/third-party/claude-desktop/admin-console>
5. **系统代理层 / egress 代理**：官方只提供 `egressProxyUrl` / `egressProxyPacUrl`（**可达性**设置，不是 egress 控制），文档明确说它不是数据边界。

注意：标准（非 3P）Claude Desktop 的 Code tab “reads gateway routing from its third-party inference configuration, not from `ANTHROPIC_BASE_URL` or `settings.json`”（<https://code.claude.com/docs/en/llm-gateway-connect>）。

---

## 2. Claude Code

### 2.1 指向第三方 base_url 的全部方式

**[官方文档]** 两条变量是每个网关连接的必需项：`ANTHROPIC_BASE_URL` + 一个凭据变量。

凭据变量与头的映射（官方原文）：

| 变量 | 落在哪个头 | 适用 |
|---|---|---|
| `ANTHROPIC_AUTH_TOKEN` | `Authorization: Bearer <value>` | 网关说“bearer token / Authorization header” |
| `ANTHROPIC_API_KEY` | `x-api-key` | 网关说“API key / x-api-key” |
| `apiKeyHelper`（settings 键） | **同时**放 `Authorization` 与 `x-api-key` | 凭据轮转或来自 vault |

来源：<https://code.claude.com/docs/en/llm-gateway-connect>、<https://code.claude.com/docs/en/env-vars>

模型相关变量（官方 env-vars 页）：

- `ANTHROPIC_MODEL`：会话使用的模型名。
- `ANTHROPIC_DEFAULT_OPUS_MODEL` / `..._SONNET_MODEL` / `..._HAIKU_MODEL` / `..._FABLE_MODEL`：别名 `opus` / `sonnet` / `haiku` / `fable` 解析到的模型 ID；其中 `haiku` 还用于**后台任务**。
- `ANTHROPIC_DEFAULT_MODEL`：新会话默认模型（v2.1.236+）；**仅当没有配置文件设置 `model` 时生效**。
- `ANTHROPIC_SMALL_FAST_MODEL`：**[DEPRECATED]** 后台任务用的 Haiku 级模型。
- `ANTHROPIC_CUSTOM_MODEL_OPTION`（+ `_NAME` / `_DESCRIPTION` / `_SUPPORTED_CAPABILITIES`）：把一个非标准 / 网关专有模型 ID 加入 `/model` 选择器。
- `ANTHROPIC_BETAS`：额外 `anthropic-beta` 值。
- `ANTHROPIC_CUSTOM_HEADERS`：自定义头，`Name: Value` 格式，多行用换行分隔（JSON 里写 `\n`）。
- `ANTHROPIC_DEFAULT_*_MODEL_SUPPORTED_CAPABILITIES`：**只**在 provider 配置（`CLAUDE_CODE_USE_BEDROCK` / `_USE_VERTEX` / `_USE_FOUNDRY` / `_USE_MANTLE`）下生效，**在 `ANTHROPIC_BASE_URL` 网关后面无效**。

网关相关的 `CLAUDE_CODE_*`（官方文档中出现并有明确语义的）：

- `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`：开启 `/v1/models` 发现（默认**关闭**）。
- `CLAUDE_CODE_GATEWAY_MODEL_DISCOVERY_TIMEOUT_MS`：发现请求超时（默认 3 秒；v2.1.269+）。
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS`：覆盖上下文窗口假定（网关/自定义模型 ID 用）。
- `CLAUDE_CODE_AUTO_COMPACT_WINDOW`：把自动压缩阈值设到网关的实际上限（会被 clamp 到 ≥100000 且 ≤模型窗口）。
- `CLAUDE_CODE_MAX_OUTPUT_TOKENS`：输出上限；不认识模型时默认 32000、上限 128000。
- `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`：停止发送预发布能力与配对 beta 头（网关/上游拒收实验字段时用）。
- `CLAUDE_CODE_API_KEY_HELPER_TTL_MS`：`apiKeyHelper` 结果缓存时长（默认 5 分钟）。
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`：关掉走网关之外的背景流量。
- `CLAUDE_CODE_GATEWAY_HINT_HEADERS=1`：自定义 base URL 时**默认不发**网关提示头，需要显式开启（v2.1.273+）。
- `CLAUDE_CODE_ATTRIBUTION_HEADER=0`：省略 system prompt 的归属块（网关侧缓存兼容用，不是隐私开关）。
- `API_TIMEOUT_MS`（默认 600000）、`API_FORCE_IDLE_TIMEOUT`。
- `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`：宿主平台接管路由时，Claude Code 会忽略 settings 里的 provider/endpoint/认证变量（含 `ANTHROPIC_BASE_URL`、`ANTHROPIC_API_KEY`）。

**[厂商断言]** 未在官方 env-vars 页（我读取的片段）中证实的变量名，均由第三方文档使用：`CLAUDE_CODE_SUBAGENT_MODEL`、`CLAUDE_CODE_EFFORT_LEVEL`（DeepSeek / 千问AI平台示例）。`CLAUDE_CODE_AUTO_COMPACT_WINDOW` 已在官方文档出现（见上）。

### 2.2 settings.json 路径与字段

- Windows：`%USERPROFILE%\.claude\settings.json`（即 `C:\Users\<用户名>\.claude\settings.json`）。
- 项目级：`.claude/settings.json`（提交共享）、`.claude/settings.local.json`（个人，gitignore）。
- 托管：`managed-settings.json` / MDM / claude.ai console。
- 顶层辅助文件：`~/.claude.json`；Windows 上 `~/.claude` 即 `%USERPROFILE%\.claude`；可用 `CLAUDE_CONFIG_DIR` 改家目录位置。

网关写法（官方）：

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://llm-gateway.example.com",
    "ANTHROPIC_AUTH_TOKEN": "sk-gateway-key"
  },
  "apiKeyHelper": "~/bin/get-gateway-key.sh"
}
```

警告（官方）：不要把凭据放进项目的 `.claude/settings.json`（会提交）。

**[厂商断言]** 千问AI平台补充了两点官方文档未提的操作细节：写入 `settings.json` 后需**新开终端**；并把 `~/.claude.json`（Windows：`C:\Users\<用户名>\.claude.json`）的顶层字段 `hasCompletedOnboarding` 设为 `true`，否则启动时报 `Unable to connect to Anthropic services`。
来源：<https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/claude-code>

### 2.3 优先级

**[官方文档]** settings 文件之间的优先级（高→低）：托管 → `--settings` 命令行 → `.claude/settings.local.json` → `.claude/settings.json` → `~/.claude/settings.json`。

环境变量**不是**这一栈中的一层，而是逐键比较：

- `ANTHROPIC_MODEL`（shell 导出）**优先于**任何文件里的 `model`。
- `ANTHROPIC_DEFAULT_MODEL` **只在没有文件设置 `model` 时**生效。
- `--model` / `/model` **优先于** `ANTHROPIC_MODEL`。
- 同一个变量同时出现在 shell 和 settings 的 `env` 块时，**settings 文件的值生效**。
- 想“清掉”一个无法 unset 的变量（如 profile 里导出的 `CLAUDE_CODE_USE_VERTEX`），在 `env` 块里设为空字符串：`"CLAUDE_CODE_USE_VERTEX": ""`（Claude Code 视为未设置）。
- `env` 块不能设置某些变量（如 `CLAUDE_CONFIG_DIR`、OTel exporter 变量）。

来源：<https://code.claude.com/docs/en/settings>、<https://code.claude.com/docs/en/env-vars>

### 2.4 网关必须实现哪些端点 + 最小响应形状

**[官方文档]** Anthropic Messages 格式（由 `ANTHROPIC_BASE_URL` 选择）的端点表：

| 端点 | 必需性 | 备注 |
|---|---|---|
| `POST /v1/messages` | **必须** | 推理请求的路径是 `/v1/messages?beta=true`；网关应**按路径匹配**，不要按完整 URL |
| `POST /v1/messages/count_tokens` | 可选 | 缺失时 Claude Code 退回按字符估算，`/context` 显示近似值（无报错） |
| `GET /v1/models?limit=1000` | 可选 | 仅当 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`；默认超时 3 秒；**任何重定向都视为失败**（防凭据泄漏） |
| `HEAD /api/hello` | 可选 | 连接预热探测，可以拒绝，不影响功能；配置了 HTTP 代理或客户端证书时会被跳过 |

`/v1/models` 最小响应形状（官方示例）：

```json
{
  "data": [
    { "id": "claude-sonnet-4-6", "display_name": "Claude Sonnet 4.6", "description": "Default model for everyday coding tasks" },
    { "id": "claude-opus-4-8" }
  ]
}
```

发现过滤：**保留 `id` 中含 `claude` 或 `anthropic` 的条目**（大小写不敏感）；`vertex_ai/claude-sonnet-4-6`、`bedrock/anthropic.claude-sonnet-4-5` 这类前缀 ID 可以通过。缓存写到 `~/.claude/cache/gateway-models.json`（Windows：`%USERPROFILE%\.claude\cache\gateway-models.json`）。

发现请求的凭据头（官方，v2.1.248+ 同时发两个）：`Authorization: <ANTHROPIC_AUTH_TOKEN 或 apiKeyHelper 值>`；`x-api-key: <解析出的 API key>`。两个都解析不出来时**跳过发现**并写 `[gatewayDiscovery] skipped`。

必须原样转发的头（官方）：`anthropic-version`、`anthropic-beta`（**逐字转发，不要按白名单过滤**，集合随版本变化）；`anthropic-workspace-id`（仅 Claude Platform on AWS 上游）。网关可消费而不必转发的头：`Authorization` / `x-api-key`、`x-claude-code-session-id`、`x-claude-code-agent-id`、`x-claude-code-parent-agent-id`。

响应头（官方只要求这几个）：`content-type: text/event-stream`；`retry-after` 用**整数秒**（不要 HTTP date）；`x-should-retry` 透传；`anthropic-ratelimit-unified-*` 透传。

### 2.5 流式 SSE 要求（Claude Code）

**[官方文档]**：

1. 网关缓冲整个响应 → Claude Code **卡住**（stall）。
2. Claude Code 期望**完整、按序**的事件序列，直到 `message_delta` 与 `message_stop`。若 body 在某个 content block 开始之后、最终 `message_delta` 之前“干净地”结束，会被当作**连接中断**（用户看到 “The response above may be incomplete”，可能触发自动重试）。
3. Claude Code 在无字节超过“流空闲超时”后中止响应。长时间思考时，上游 SSE `ping` 可能是流上唯一的字节；**剥掉或缓冲 ping 的网关会中途触发超时**。上游不发 ping（如 Bedrock 二进制事件流）时，网关必须**自己发 ping**。
4. Bedrock InvokeModel 格式下，若网关把二进制 `application/vnd.amazon.eventstream` 转成 SSE 或改写 `Content-Type`，Claude Code 无法解析。

### 2.6 已知限制与常见报错（官方 troubleshooting 表节选）

| 现象 | 原因 | 处置 |
|---|---|---|
| `401` 报 token 无效 / 未识别 | 凭据不属于该网关，或落在网关不读的头里 | 核对变量↔头映射；换另一个变量 |
| `Your apiKeyHelper script is failing` | helper 输出里混有横幅/日志 | v2.1.227+ 只允许纯凭据输出 |
| `Connection refused ... (ConnectionRefused)` / `Can't reach the API server ... (ENOTFOUND)` | base URL 无人应答 / DNS 不通 | 先用 curl 验证 |
| `API returned an empty or malformed response (HTTP 200)` | 网关或中间代理返回了 HTML/登录页 | 修网关路由 |
| `400` 命名 `context_management` / `Extra inputs are not permitted` | 网关把 Anthropic 格式请求转发给拒收这些字段的上游 | `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` |
| `400` 命名 `thinking` 或 `adaptive`（如 `Input tag 'adaptive' found`） | 上游模型构建不接受 adaptive reasoning | 升级上游；Opus 4.6 / Sonnet 4.6 上可用 `CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING=1` |
| `400` 网关用**自己的措辞**报上下文/令牌超限（如 `ContextWindowExceededError`、`prompt token count of N exceeds the limit of M`） | 网关改写了上游的 too-long 错误，Claude Code 认不出来，**不会自动压缩重试** | `/compact` 恢复；预防：`CLAUDE_CODE_AUTO_COMPACT_WINDOW` 设为网关上限（会被 clamp 到 ≥100000）；并把 `CLAUDE_CODE_MAX_OUTPUT_TOKENS` 设到网关模型输出上限之下 |
| `403` HTML body 且网关日志里**没有**该请求 | 网关前的 WAF/反代按 XSS body 规则拦了请求体 | 对 `/v1/messages` 路径豁免请求体检查（AWS WAF: `CrossSiteScripting_Body`；nginx+ModSecurity: OWASP CRS body 规则） |
| 502/504 或 `Prompt is too long · automatic compaction failed` | 代理/LB 在汇总响应完成前关闭了连接 | 拉长代理与 LB 的响应超时，并保证 ping 透传 |
| `/fast` 报网络不可用但推理正常 | fast mode 可用性检查**直连 `api.anthropic.com`**，不跟随 `ANTHROPIC_BASE_URL` | 放行该域或设跳过变量 |
| 有网关凭据仍要求登录 | 凭据没放在首次运行向导前就读到的位置（shell 导出 / `~/.claude/settings.json` 的 `env` / managed） | 见 2.1 |
| 托管策略与网关凭据冲突 | managed settings 里的 `forceLoginMethod` / `forceLoginOrgUUID` / `forceLoginGatewayUrl` 与 `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` / `apiKeyHelper` **不能共存** | 由管理员移除其一 |
| 证书/TLS 错误但 curl 通过 | Claude Code 运行时不信同一个 CA | 设 `NODE_EXTRA_CA_CERTS` |

功能层面的已知降级（官方）：

- 走网关时 `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST` 之外的 provider 能力变量（`ANTHROPIC_DEFAULT_*_MODEL_SUPPORTED_CAPABILITIES`）**无效**。
- 自定义 base URL 时 `MCP tool search` **默认关闭**（`ENABLE_TOOL_SEARCH=true` 需网关转发 `tool_reference` 块）。
- v2.1.196 起，`ANTHROPIC_BASE_URL` 指向非 `api.anthropic.com` 时 **Remote Control 被禁用**；用网关凭据时 voice dictation 也不可用。
- 使用网关的会话不适用 HIPAA 配置。
- 走网关时 `ANTHROPIC_MODE` 之外的适配：若上游不认识模型 ID（网关别名），Claude Code 按“当前模型”处理，发送 adaptive reasoning 等字段（Bedrock/Vertex 格式下则退化为固定预算 thinking）。

来源：<https://code.claude.com/docs/en/llm-gateway-protocol>、<https://code.claude.com/docs/en/llm-gateway-connect>

---

## 3. Codex（OpenAI Codex CLI / IDE / 桌面）

### 3.1 `~/.codex/config.toml` 中 `model_providers` 的完整字段

**[官方文档]** `model_providers.<id>`（自定义 provider；内置 ID `openai`、`ollama`、`lmstudio` 保留、不可覆盖）：

| 字段 | 类型 | 官方说明 |
|---|---|---|
| `name` | string | 自定义 provider 的显示名 |
| `base_url` | string | provider 的 API base URL |
| `env_key` | string | 提供 provider API key 的环境变量名 |
| `env_key_instructions` | string | 可选的配置指引 |
| `experimental_bearer_token` | string | 直接内联 bearer token（不推荐，改用 `env_key`） |
| `requires_openai_auth` | boolean | 该 provider 使用 OpenAI 认证（默认 false） |
| `wire_api` | `responses` | “**`responses` is the only supported value, and it is the default when omitted.**” |
| `query_params` | map<string,string> | 附加到 provider 请求的查询参数（示例：Azure `{ api-version = "2025-04-01-preview" }`） |
| `http_headers` | map<string,string> | 静态 HTTP 头 |
| `env_http_headers` | map<string,string> | 由环境变量填充的头（自定义认证头用这个） |
| `request_max_retries` | number | 默认 4 |
| `stream_max_retries` | number | SSE 中断重试，默认 5 |
| `stream_idle_timeout_ms` | number | SSE 空闲超时，默认 300000 |
| `supports_websockets` | boolean | 该 provider 是否支持 Responses API 的 WebSocket 传输 |
| `supports_standalone_web_search` | boolean | 默认 false；声明支持独立 web search 端点（开发中，默认关闭） |
| `auth.command` / `auth.args` / `auth.timeout_ms` / `auth.refresh_interval_ms` / `auth.cwd` | string/array/number | 命令式 bearer token；**不要**与 `env_key`、`experimental_bearer_token`、`requires_openai_auth` 同时用 |
| `gateway_oauth.*`（`authorization_url`、`token_url`、`client_id`、`delivery`、`redirect_port`） | — | ChatGPT 登录 + 网关时的原生网关 OAuth（见 3.4） |
| `model_providers.amazon-bedrock.aws.profile` / `.region` | string | 内置 `amazon-bedrock` provider 的 AWS profile / region |

来源：<https://learn.chatgpt.com/docs/config-file/config-reference>（另见 `https://github.com/openai/codex/blob/main/docs/config.md` 指向的同一套文档）

顶层相关键：`model`、`model_provider`、`model_catalog_json`、`openai_base_url`（改内置 `openai` provider 的 base URL，**不要**定义 `[model_providers.openai]`）、`model_context_window`、`model_auto_compact_token_limit`、`model_reasoning_summary`、`model_verbosity`（**仅 Responses API 生效，Chat Completions provider 忽略**）、`hide_agent_reasoning`、`show_raw_agent_reasoning`。

官方配置样例（网关）：

```toml
model = "gpt-6-sol"
model_provider = "enterprise-gateway"
web_search = "disabled"

[model_providers.enterprise-gateway]
name = "Organization Gateway"
base_url = "https://gateway.example.com/v1"
wire_api = "responses"
env_key = "CODEX_GATEWAY_API_KEY"
```

自定义认证头（官方）：

```toml
env_http_headers = { "X-API-Key" = "CODEX_GATEWAY_API_KEY" }
```

来源：<https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway>

Windows 路径（官方）：

- 用户级：`%USERPROFILE%\.codex\config.toml`
- 系统级（主机默认）：**`%ProgramData%\OpenAI\Codex\`** 下的 `config.toml`
- Windows 里 TOML 路径用单引号保留反斜杠：`model_catalog_json = 'C:\ProgramData\OpenAI\Codex\models.json'`；Codex **不展开** `%ProgramData%`
- 项目级 `.codex/config.toml` **不能**覆盖：`openai_base_url`、`chatgpt_base_url`、`model_provider`、`model_providers`、`notify`、`profile`、`profiles` 等（会打印启动警告并忽略）

来源：<https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway>、<https://learn.chatgpt.com/docs/config-file/config-advanced>

### 3.2 走 `/v1/responses` 还是 `/v1/chat/completions`

**[官方文档]**：`POST /v1/responses`。原文：“Configure a gateway provider with `wire_api = "responses"`. For a base URL such as `https://gateway.example.com/v1`, the gateway must accept `POST /v1/responses` … **A working Chat Completions or Anthropic Messages endpoint doesn't establish Responses compatibility.**”

`/v1/models`：官方明确“Health and model-list endpoints are **optional operational aids**. They don't exercise a Codex conversation or prove tool support.”；在 ChatGPT 登录走网关的场景里则要求“A generic OpenAI `/v1/models` response is insufficient: **Codex expects a `models` array**”。

认证头：

- `env_key` → 作为 bearer（`Authorization: Bearer`），官方示例 auth 头即 `Authorization`（“Existing command-backed authentication places the gateway key in `Authorization`”）。
- `env_http_headers` → 自定义头（如 `X-API-Key`），值从环境变量读，不得写进 TOML。
- ChatGPT 登录 + 网关时，`Authorization` 携带 ChatGPT token，网关凭据改走独立头/ Cookie；网关需校验并**移除自己的凭据**，再把 ChatGPT 头原样转发到 `https://chatgpt.com/backend-api/codex`。

来源：<https://learn.chatgpt.com/docs/enterprise/gateway-compatibility>、<https://learn.chatgpt.com/docs/enterprise/sign-in-with-chatgpt-through-a-gateway>

**[厂商断言]** 千问AI平台称新版本 Codex 已不支持 `wire_api = "chat"`，并给出两类报错：`wire_api = "chat" is no longer supported`（改 `responses`）与 `unknown configuration field wire_api`（删掉该字段）；其 Chat/Completions 路径要求降级到 `@openai/codex@0.80.0`。
来源：<https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/codex>
（与官方 config-reference 的 `wire_api: "responses"` 唯一值一致，可视为已证实。）

### 3.3 网关必须保留的行为（官方兼容性清单）

**[官方文档]** 原文要求：

1. 接受 `POST /v1/responses`，并保留客户端用到的请求/响应字段。
2. **增量转发 SSE，不要缓冲整个答案**；保留事件类型与负载，包括成功的终态 `response.completed`；转发错误/失败事件。
3. 保留跨轮**回放式输入**（prior messages、tool calls、tool results）。启用 WebSocket / 增量传输时，还要验证其 `previous_response_id` 行为；纯 HTTP 无状态 Responses 路径可以只用回放输入、不依赖该字段。
4. 保留 function-call 项与配对的 `function_call_output` 项及其标识符；完整回路必须能跑通。
5. 认证：`env_key`、命令式 bearer、或 `env_http_headers` 自定义头；凭据走环境变量。
6. 模型路由：每个面向 Codex 的模型名必须路由到预期上游；用**网关记录**验证，不要靠模型自述。
7. 错误：区分客户端认证错误 / 未知模型路由 / 限流 / 上游故障，不要统统塌成 `500`。

官方“资格检查清单”：Responses 字段、增量 SSE + 成功终态、回放式续轮、`previous_response_id`（若该传输使用）、function call + 配对结果 + 最终答案、路由与元数据匹配、按用户归因+续期+吊销、有用的错误、脱敏诊断。

来源：<https://learn.chatgpt.com/docs/enterprise/gateway-compatibility>

### 3.4 已知限制

- **自定义别名必须配 model catalog**：`model_catalog_json` 指向本地 JSON；catalog 里的 `slug` 必须等于网关别名与 Codex 的 `model`。官方警告：自定义 catalog **替换**内置 catalog，所以必须包含用户需要的每个模型；没有匹配条目时请求虽能到达上游，Codex 会用**通用设置**。
- **catalog 与 requirements.toml**：`requirements.toml` 里的 `model_catalog_json` 只“钉路径”，**不分发文件**。
- **`wire_api` 只有 `responses`**（H6）。
- **`model_verbosity` 只在 Responses API provider 生效**。
- **ChatGPT 登录 + 网关**：`requires_openai_auth = true` 与 provider `auth` 命令块**不能同时存在**；网关需自己校验并剥离自己的凭据；原生网关 OAuth 需要浏览器+回环回调，**无 device-code 回退**。
- **Cloud/Work Cloud（“Local computer access with Work Cloud”）不承载自定义 provider**：官方表明确写 `model_provider, model_providers` “Not supported in managed Local computer access with Work Cloud requirements or configuration”；`sqlite_home`、`log_dir`、`model_catalog_json` 也“Not supported as managed cloud overrides”。
- **项目级 config 不能设置 provider/认证路由键**。
- **`amazon-bedrock` 内置 provider 只支持嵌套的 AWS profile/region 覆盖**。

### 3.5 客户端实际发出的请求（源码级证据）

**[官方源码]** `codex-rs/core/src/client.rs`（`build_responses_request`）：

```rust
include: ["reasoning.encrypted_content"]   // 恒定包含
store: false
stream: true
```

- `include` 另在 `output_token_replay_enabled && is_openai` 时追加 `"output.encrypted_content"`。
- 非 OpenAI provider（`!is_openai`）时：对所有 item 清 `internal_chat_message_metadata_passthrough`；除 `ResponseItem::Reasoning`、`Compaction`、`ContextCompaction` 外**清除 encrypted content**；`FunctionCall` 的 `encrypted_function_args` 被置空。
- 传输层：`WireApi::Responses` 是唯一分支；provider 支持时优先 WebSocket（`/responses`、beta 值 `responses_websockets=2026-02-06`），失败/`426 Upgrade Required` 时回退 HTTP。
- 请求会带一批 `x-codex-*` 头（`x-codex-installation-id`、`x-codex-turn-state`、`x-codex-routing-hint` 等）以及 `OpenAI-Beta`；粘性路由 token `x-codex-turn-state` 在同一轮内必须原样回传，**不得跨轮**。

来源：<https://github.com/openai/codex/blob/main/codex-rs/core/src/client.rs>

推论（对网关实现）：一个 Responses 网关必须能接受并回传 `reasoning` 项（含 `encrypted_content`），且不应依赖 `store: true` 的服务端状态；续轮靠客户端回放输入（或 `previous_response_id`，仅在启用 WebSocket/增量传输时）。

### 3.6 Gemini/其他格式的注意

**[官方文档]** Codex 不要求 Anthropic Messages 兼容；官方迁移表明确写：“Claude `/v1/messages` route, Bedrock InvokeModel shape, Anthropic headers… **Do not reuse these as proof of compatibility.** Codex needs `POST /v1/responses`”；且 “**Codex does not support `apiKeyHelper`**”。

来源：<https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway>

---

## 4. 三方文档中与官方一致 / 冲突的条目（网关侧实践）

**DeepSeek**（<https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/claude_code>）：
- Claude Code 走 `https://api.deepseek.com/anthropic`。其在网关侧做**模型名映射**：`claude-opus*` → `deepseek-v4-pro`；`claude-haiku*` / `claude-sonnet*` → `deepseek-flash`。deepseek 用官方文档列出的 `ANTHROPIC_DEFAULT_*_MODEL` 机制在客户端侧完成映射，与官方一致。
- 关于 Claude Desktop：DeepSeek 的表述是“app 对模型名的限制”，但其方案是在**自己的网关**上做名前缀映射。这属于厂商适配策略。

**千问AI平台**（<https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/claude-code>）：
- 明确 `/apps/anthropic` 兼容端点**只提供 `/v1/messages`，不提供 `/v1/models`**（探测返回 404），并说明“客户端会在配置的 base URL 后追加 `/v1/models`”与“在客户端的 Models 中手动添加模型即可跳过自动发现”，与官方 H2/H5 一致。
- 明确 base URL **不要以 `/v1/` 结尾**，否则出现 `/v1/v1/models`。
- Anthropic 兼容端点认证：`x-api-key` 或 `Authorization: Bearer` **二者选一**（<https://platform.qianwenai.com/docs/api-reference/chat/anthropic>）。
- Codex 侧使用 `base_url = "https://.../compatible-mode/v1"` + `env_key = "OPENAI_API_KEY"` + `wire_api = "responses"` + `model_catalog_json = "~/.codex/model-catalog.local.json"`（该 catalog 结构为 `{"models":[{"slug":...}]}`），与官方 catalog 机制一致（<https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/codex>）。

---

## 5. 未证实 / 不确定的条目

| 条目 | 状态 |
|---|---|
| Claude Desktop 3P 是否调用 `/v1/messages/count_tokens` | **不确定**：3P 网关官方页未提及；Claude Code 侧为可选 |
| “Claude Desktop 只接受 `claude-sonnet-* / claude-opus-* / claude-haiku-*` 三档角色 ID” | **未证实（官方层面）**：仅见于第三方网关厂商文档，且其方案是在自己的网关/插件里做映射 |
| Claude Desktop 3P 模型发现使用的具体“recognizably Claude”判定字符串 | **未证实**：官方只说“recognizably Claude”；Claude Code 侧的等价规则为 ID 含 `claude` 或 `anthropic` |
| `CLAUDE_CODE_SUBAGENT_MODEL`、`CLAUDE_CODE_EFFORT_LEVEL` | **未在官方 env-vars 页证实**（仅第三方示例使用） |
| Codex 是否强制要求 `/v1/models` | **不需要**：官方称其“optional operational aid”；但 ChatGPT-登录路径要求返回 `models` 数组 |
| Codex 的 `wire_api = "chat"` 何时被移除 | 官方只说唯一合法值为 `responses`；具体版本时间点见第三方 FAQ |
| Claude Desktop 3P 在 Windows 上是否有**除注册表外的**托管 JSON 文件 | **未证实**：官方 Windows 托管位置只列注册表；Linux 才有 `managed-settings.json` |

---

## 6. 来源清单（全部一手链接）

Anthropic / Claude：
- <https://claude.com/docs/third-party/claude-desktop/overview>
- <https://claude.com/docs/third-party/claude-desktop/installation>
- <https://claude.com/docs/third-party/claude-desktop/configuration>
- <https://claude.com/docs/third-party/claude-desktop/gateway>
- <https://code.claude.com/docs/en/settings>
- <https://code.claude.com/docs/en/env-vars>
- <https://code.claude.com/docs/en/llm-gateway>
- <https://code.claude.com/docs/en/llm-gateway-connect>
- <https://code.claude.com/docs/en/llm-gateway-protocol>
- <https://code.claude.com/docs/en/desktop>
- <https://support.claude.com/llms.txt>（帮助中心索引）

OpenAI / Codex：
- <https://learn.chatgpt.com/docs/config-file/config-reference>
- <https://learn.chatgpt.com/docs/config-file/config-advanced>
- <https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway>
- <https://learn.chatgpt.com/docs/enterprise/gateway-compatibility>
- <https://learn.chatgpt.com/docs/enterprise/roll-out-a-gateway>
- <https://learn.chatgpt.com/docs/enterprise/sign-in-with-chatgpt-through-a-gateway>
- <https://github.com/openai/codex/blob/main/codex-rs/core/src/client.rs>
- <https://github.com/openai/codex/blob/main/docs/config.md>

第三方平台（网关侧实践）：
- <https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/claude_code>
- <https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/codex>
- <https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/claude-code>
- <https://platform.qianwenai.com/docs/developer-guides/clients-and-developer-tools/codex>
- <https://platform.qianwenai.com/docs/api-reference/chat/anthropic>
- <https://platform.qianwenai.com/docs/api-reference/chat/openai-chat>
- <https://platform.qianwenai.com/docs/api-reference/chat/openai-responses>
