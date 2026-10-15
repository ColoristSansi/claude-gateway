# 原型：Claude Desktop 3P 端到端

Type: prototype
Status: resolved
Blocked by: 13, 14

## Question

Claude Desktop 在 3P 模式下能不能通过本网关正常流式问答，并且四个家族名映射到不同上游时都生效？这是用户为本期定的验收标准。

## 验收标准（用户原话）

「Claude Desktop 3P 能正常流式问答 + 四个家族映射生效。」

## 要做的事

1. 配置写入引擎最小版：写 `%LOCALAPPDATA%\Claude-3p\configLibrary\`（`inferenceProvider=gateway` + `inferenceGatewayBaseUrl` + `inferenceGatewayApiKey` + `inferenceGatewayAuthScheme` + `inferenceModels`），设置 `_meta.json` 的 `appliedId`；**写入前备份原文件**。
2. 四个家族槽位分别绑到不同上游，在 Desktop 的模型菜单里逐个验证。
3. 一键切回 1P（移除网关字段 + 从备份还原），确认能回到官方登录态。
4. 记录实测结果：Desktop 到底认不认非 claude-* 的 ID（喂给票据 [16](16-research-desktop-model-id-validation.md)）。

## Answer

### 结论：验收标准达成

用户原话两条都实测过关——「Claude Desktop 3P 能正常流式问答 + 四个家族映射生效」。

### 产物

- **配置写入引擎（最小版）**：`.scratch/model-gateway/prototype/desktop-3p.ps1`，三个模式：
  - `-Mode gateway`：整目录首次备份 → 写 `<固定 GUID>.json`（`inferenceProvider=gateway` + `inferenceGatewayBaseUrl` + `inferenceGatewayApiKey` + `inferenceGatewayAuthScheme=bearer` + `inferenceCredentialKind=static` + 四个 `inferenceModels` 条目）→ 把它加进 `_meta.json` 的 `entries` 并置 `appliedId`。固定 id 使重复执行幂等。
  - `-Mode standard`：从 configLibrary 里**所有** profile 摘掉 6 个网关字段（`inferenceProvider` 是激活动词，一摘即回标准模式）。
  - `-Mode restore`：按首次备份整目录还原（大退路，本次未实跑）。
- 备份：`%LOCALAPPDATA%\Claude-3p\configLibrary.backup-20261014-191819`，指针文件 `configLibrary.backup.path`（备份只做一次，后续写入不会覆盖原始状态）。
- 网关侧两处小改：入站加 `/messages`、`/models` 别名路由（base URL 语义两份资料矛盾，先让两种写法都能通），并在 `messages` 里打印真实路径与 `user-agent` / `anthropic-version` / `anthropic-beta`。

### 实测证据

| 项 | 结果 |
|---|---|
| 步骤 1 写入生效 | `_meta.json.appliedId` 指向新 profile；Desktop 重启后网关收到 UA 为 `Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Claude/2.37337.1 … Electron/44.7.0 … MSIX` 的请求，HTTP 200 |
| 步骤 2 模型菜单 | 用户实测菜单**精确**是四项：`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku` |
| 流式问答 | `claude-fable` 回答逐字流式出现（用户实测） |
| 四家族映射 | 四个家族名各发一句，**四个都正常回复**；`GET /api/logs` 证明按路由表分落两家上游（fable/opus → opencode `deepseek-v4.1-flash`，sonnet/haiku → commandcode `deepseek/deepseek-v4.1-flash`），全部 `status=200` |
| 真实负载保真 | `claude-fable` 实测 `input_tokens=34221` —— Desktop 的整套 system prompt + 工具定义全量过了 OpenAI Chat wire 编解码，没有崩 |
| 步骤 3 切回 1P | 摘字段后重启 → 用户确认「已回到标准模式、已登录、无网关痕迹」；1P 侧 `oauth:tokenCache` / `oauth:tokenCacheV2` / `lastKnownAccountUuid` 完好未被 3P 碰过 |
| 失败请求 | 全程零 4xx/5xx：没有未知模型 404、没有非流式 400、没有上游错误信封 |

### 新增事实（写进 spec / 研究文档用）

1. **`inferenceGatewayBaseUrl` 不要带 `/v1`**。Desktop 实际打的是 `<baseUrl>` + `/v1/messages`（网关打印的真实路径是 `/v1/messages`，UA 是 Desktop）。官方示例里的 `https://gateway.example.com/v1` 会得到 `/v1/v1/messages`；`research/client-integration.md` §4 千问那条「base URL 不要以 `/v1/` 结尾」对 Desktop 同样成立。base URL 语义至此定案。
2. **`inferenceModels` 全是完整 ID 时 Desktop 确实跳过 `/v1/models`**：整个启动 + 四轮问答期间，网关一次 `GET /v1/models` 都没收到（别名路由 `/models` 同样没被打过）。
3. **3P 与 1P 的 userData 与登录态确实隔离**（票据 13 的强证据本轮实测确认）：3P 侧 accountId 是 `65328857-…`，1P 侧 `lastKnownAccountUuid` 是 `e2ef09a3-…`。→ 切回 1P **不需要**从备份还原，摘 `inferenceProvider` 就够；「从备份还原」只在要按字节回到原状时才用。
4. `%LOCALAPPDATA%\Claude-3p\configLibrary\` 确未被 MSIX 重定向，写入即生效（再次确认票据 13）。

### 未验 / 取舍

- **非 claude-\* 的模型 ID 是否被拒收没测**：本轮 `inferenceModels` 四项全是 claude-* 家族名，只证明了「四个家族名可用」，票据 16 的正式实验（混入非法名看是否整组 fail-all）留给它自己。观察记录已作为评论喂给票据 16。
- `/v1/models` 自动发现路径没测（用显式列表绕过去了）。
- 非流式入站仍 400：本轮全程没出现非流式请求，Desktop 3P 只发流式。
- 网关无入站鉴权，`inferenceGatewayApiKey` 填的是任意串。
- `-Mode restore` 写了但未实跑；切回 1P 走的是「摘字段」。
- 用户收尾选择：**机器留在 1P**（网关进程已停），想再看 3P 就 `desktop-3p.ps1 -Mode gateway` + 重启 Desktop。

## Comments

- 2026-10-14（票 13 实测补充）：本机 Claude Desktop 是 **Microsoft Store / MSIX** 包 `Claude_2.37337.1.0`（`Claude_pzs8sxrjxfjjc`）。
  - `%LOCALAPPDATA%\Claude-3p\` 已存在（当前为空，`configLibrary` 尚未生成），且 `Packages\...\LocalCache\Local\Claude-3p` 不存在 → 该路径**未**被 MSIX 重定向，第 1 步的写入目标成立。
  - ⚠️ 但 **`%APPDATA%\Claude` 不存在**：Roaming 被重定向到 `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\`（`claude_desktop_config.json`、`config.json`、`logs\` 都在那里）。第 1 步「写入前备份原文件」与第 3 步「从备份还原 1P」都要按这个 LocalCache 路径做，不能假设 `%APPDATA%\Claude\`。
  - 开发者模式已开（`developer_settings.json` = `{"allowDevTools": true}`）。

- 2026-10-14（**1P/3P 切换机制，实测挖出**，供第 1、3 步用）：
  - 激活的**唯一来源**是 `%LOCALAPPDATA%\Claude-3p\configLibrary\`：`_meta.json` 的 `appliedId` 指向一份 `<id>.json`，后者里的 `inferenceProvider` 是激活动词。本机 `HKLM\SOFTWARE\Policies\Claude` 与 `HKCU\SOFTWARE\Policies\Claude` **都不存在**（没有策略覆盖）。
  - 应用自带文档原文（从安装包 `app\resources\ion-dist` 里挖出）：*"When this key is not set, the app launches in standard mode unless another setting asks for 3P mode."* → **切回 1P = 取消应用 / 移除 configLibrary 里那份 3P 配置**。
  - 3P 与 1P 跑**两套 userData 目录**：3P = `%LOCALAPPDATA%\Claude-3p\`（`main.log`：`user_data_location: 'third_party'`）；1P = MSIX 重定向下的 `…\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\`。1P 的登录态（`oauth:tokenCache`、`lastKnownAccountUuid`）仍在后者且未被 3P 覆盖 → **第 3 步的「从备份还原」大概率可以省掉，只做「取消应用」就够**（强证据，**尚未实测**；实测后再改这条）。
  - 新版 3P 引导窗口（路由 `/setup-desktop-3p`）**没有**「退出第三方模式」入口；`disableDeploymentModeChooser` **未设置**，即不是被策略锁死，只是界面无入口。
  - 本机当前状态（2026-10-14 起）：3P 已激活，profile `ac278ce7-9506-4f19-b147-25637015682c`，`inferenceGatewayBaseUrl` / `inferenceGatewayApiKey` 都是占位 `"0"` → 网关不可达（`Gateway discovery failed: ERR_CONNECTION_CLOSED`）、模型选择器为空。用户当次决定**暂不切回标准模式**。
