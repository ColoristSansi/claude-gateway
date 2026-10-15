# 订阅制 AI 服务的「登录态 → 可编程 API」凭据调研

- 日期：2026-10-14
- 范围：Qoder、CodeBuddy / WorkBuddy（身份先确认）、Trae、Kiro、Cursor、Windsurf、GitHub Copilot、Gemini CLI / Code Assist（含 Antigravity）、Qwen Code、Claude Code
- 调研方式：一手优先（厂商官方文档、厂商官方仓库源码、被抓取的官方客户端二进制/包内常量）；社区逆向项目源码作为「该机制确实可运行」的存在性证据，绝不当作官方事实。
- 证据等级（每个结论都带其一）：
  - **[官方文档]** 厂商自己发布的文档
  - **[官方源码]** 厂商自己开源的仓库源码
  - **[社区逆向]** 第三方项目通过抓包/逆向客户端得到的结论（可能随版本失效）
  - **[第三方报道]** 媒体/博客，未回溯一手
  - **[未证实]** 本次调研没有取得任何一手或可复现证据

> 重要前提：本文回答的是「技术上怎么把登录态变成 API」，不是「这么做合不合法」。合规结论单独放在 §13，那里的每一条都引用厂商条款原文。

---

## 0. 结论速览

| 服务 | 登录态形态 | 凭据落盘（Windows） | 自动刷新 | 上游调用是否需要客户端指纹 | 凭据获取难度 |
|---|---|---|---|---|---|
| **Qoder** | OAuth 设备流（PKCE）/ PAT | `%USERPROFILE%\.qoder\.auth\user`（CLI，AES-128-CBC）；`%APPDATA%\com.qoder.app.stable\auth.v1.dat`（桌面端，DPAPI+Chromium os_crypt） | 有（dt-/drt- 前缀路由） | **需要**（COSY 签名 + cosy-machine* 头） | 中（PAT 最易；设备流次之；本机文件需解密） |
| **CodeBuddy（国内）** | 浏览器 SSO `cli-external-link` → 轮询 token | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\*.info`；中继侧 `~/.codebuddy-session.json` | 有（401 刷新后重试一次） | **需要**（UA 必须是 `CodeBuddyIDE/<ver>`，另有 `x-device-token`） | 中偏难（新版 accessToken 进程内存密钥加密） |
| **WorkBuddy（海外）** | 同上（同协议，不同 endpoint） | 同上目录；中继侧 `~/.workbuddy-ai-session.json` | 有 | **需要**（`WorkBuddy/...` UA + x-stainless-*） | 中（JWT 手动导入即可；无 x-device-token 绑定） |
| **Trae** | 桌面客户端登录；网页 `work.trae.cn` Cookie | `%APPDATA%\<Trae CN\|TRAE SOLO CN\|Trae\|TRAE SOLO>\User\globalStorage\storage.json`（key `iCubeAuthInfo://icube.cloudide`，自定义 tc 加密） | 有（refreshToken → 新 token） | 部分（`Cloud-IDE-JWT` + `X-Cloudide-Token` + `x-uid`） | 难（tc 格式解密需硬编码 salt） |
| **Kiro** | 桌面 Social Auth（本地回调 19876-19880）/ AWS Builder ID 设备码 | `%USERPROFILE%\.aws\sso\cache\kiro-auth-token.json`；`%USERPROFILE%\.kiro\oauth_creds.json`；`%USERPROFILE%\.local\share\kiro-cli\data.sqlite3` | 有（`refreshToken` 端点） | 轻度（UA 含 `KiroIDE-<ver>-<machineId>`） | **易**（明文 JSON，refreshToken 可粘贴） |
| **Cursor** | 浏览器 OAuth；CLI `agent login` 写 keychain | `%APPDATA%\Cursor\User\globalStorage\state.vscdb`（`cursorAuth/accessToken`、`cursorAuth/refreshToken`…） | 有（`api2.cursor.sh/oauth/token`） | 未证实（chat 链路未取得一手证据） | 中（读 SQLite 即可；chat 协议未证实） |
| **Windsurf** | Firebase 密码登录 / Devin（auth1_token）/ `windsurf://` 深链 | `%APPDATA%\Windsurf\User\globalStorage\state.vscdb`（`windsurfAuthStatus` 明文含 `sk-ws-01-*` API Key；`secret://windsurf_auth.sessions` 为 DPAPI） | 有（refresh_token，提前 5 分钟） | **需要**（connect-protocol-version / Referer / UA / sec-ch-ua*） | 中（API Key 明文可读，但换号需改客户端） |
| **GitHub Copilot** | GitHub OAuth 设备流 / `gh auth login` / 细粒度 PAT | AIClient-2-API 走环境变量 `GITHUB_COPILOT_API_KEY`（PAT），不落盘 | 有（GitHub token → Copilot token，`refresh_in`） | **需要**（Editor-Version / Editor-Plugin-Version / UA / X-GitHub-Api-Version） | **易**（PAT 或设备流） |
| **Gemini CLI / Code Assist** | Google OAuth（本地回调）/ `GEMINI_API_KEY` / Vertex | `%USERPROFILE%\.gemini\oauth_creds.json`；Antigravity 为 `%USERPROFILE%\.antigravity\oauth_creds.json` | 有（1 小时级 access token） | **需要**（UA `GeminiCLI/<ver>/<model> (platform; arch)` + `X-Goog-Api-Client`） | **易**（明文 JSON） |
| **Qwen Code** | RFC 8628 设备流 + PKCE S256 | `%USERPROFILE%\.qwen\oauth_creds.json`（0600，原子写） | 有（refresh_token，400/401 则清空并要求重新 /auth） | 未证实（token 返回 `resource_url` 作为 API 基址） | **易**（明文 JSON + 官方 client_id 已公开在源码） |
| **Claude Code** | OAuth（claude.ai / platform.claude.com） | `%USERPROFILE%\.claude\.credentials.json`（`claudeAiOauth.*`） | 有（到期前 5 分钟） | 有（`anthropic-beta: oauth-2025-04-20` 等） | **易**（明文 JSON）——但**合规风险最高**，见 §13 |

判定「指纹」是否真的被校验的依据，见各服务小节；`[社区逆向]` 的项目普遍报告：不带指纹头会被 400/403 或静默过滤。

---

## 1. Qoder（阿里，qoder.com / qoder.com.cn）

**是什么**：阿里云/阿里巴巴的 agentic 编码平台，产品线含桌面 App、JetBrains 插件、Qoder CLI、Agent SDK、Cloud Agents（[docs.qoder.com](https://docs.qoder.com/llms.txt)）。

### 1.1 登录方式
- CLI 交互登录：`qoder` 启动后 `/login`（别名 `/signin`），选择「Login with Qoder Platform (Browser)」或直接使用 PAT；无图形界面时（`CI`、`SSH_CONNECTION`、`DEBIAN_FRONTEND=noninteractive`、`BROWSER=www-browser`、Linux 无 `DISPLAY`/`WAYLAND_DISPLAY`/`MIR_SOCKET`）自动跳过打开浏览器，改为在终端打印 URL 让用户手动粘贴浏览器完成（[官方文档](https://docs.qoder.com/cli/authentication.md)）。
- 设备授权细节（PKCE S256、双区 URL 参数差异：国内带 `redirect_uri+client_id+machine_id`，国际仅 `client_id+machine_id`）来自社区实现（[shuishuipingan/qoder2api-hub](https://github.com/shuishuipingan/qoder2api-hub)），非官方文档。
- **PAT（Personal Access Token）是一条官方明路的可编程凭据**：在 <https://qoder.com/account/integrations> 创建，通过环境变量 `QODER_PERSONAL_ACCESS_TOKEN` 使用；官方明确「同时存在环境变量与 `/login` 登录态时，**PAT 优先**」（[官方文档](https://docs.qoder.com/cli/authentication.md)）。
- 配置目录可用 `QODER_CONFIG_DIR` 覆盖，默认 `~/.qoder`（[官方文档](https://docs.qoder.com/cli/authentication.md)）。

### 1.2 凭据落盘位置与格式（Windows）
| 来源 | 路径 | 形式 |
|---|---|---|
| Qoder CLI 国际版 | `%USERPROFILE%\.qoder\.auth\user` | AES-128-CBC，key = `machine_id` 前 16 字符 |
| Qoder CLI 国内版 | `%USERPROFILE%\.qoder.cn\.auth\user` | 同上 |
| Qoder 桌面 App | `%APPDATA%\com.qoder.app.stable\auth.v1.dat`（国内 `com.qoder.cn.app.stable`） | Chromium `v10` 布局；`Local State` 的 os_crypt 密钥经 DPAPI 解出后 AES-256-GCM |
| 模型目录缓存 | `%USERPROFILE%\.qoder*\.models\<uid>\catalog-v6` | QMC 解密（HKDF + AES-256-GCM） |

上表全部来自 [qoder2api-hub README](https://github.com/shuishuipingan/qoder2api-hub) 的「扫描本机已登录凭证」小节 —— **[社区逆向]**，且该项目的做法是「只读探测 + 看板两步确认导入，永不静默采用」。第三方 opencode 插件亦独立记录了 `~/.qoder/.auth/user` 这个路径（[zxcrf/opencode-qoder-auth](https://registry.npmjs.org/opencode-qoder-auth)）。

### 1.3 token 结构
社区实现给出的家族与生命周期（**[社区逆向]**，原文见 qoder2api-hub「凭证家族与生命周期」）：

```
accessToken:   dt- (OAuth 设备流, ~30天)   或 jt- (PAT 交换, 24小时)
refreshToken:  drt- (~1年, 旋转)           或 jrt- (48小时)
personalToken: pt- (长期兜底, 看板导入)
```
- 刷新按 refreshToken 前缀路由：`drt-` → `deviceToken/refresh`，`jrt-` → `jobToken/refresh`；失败回落 PAT 重新交换。
- access token 轮换后 COSY 会话自动重建。
- CLI 侧官方文档只说「浏览器登录后 CLI 会在后台自动刷新 token」，未公布 token 名称与有效期（[官方文档](https://docs.qoder.com/cli/authentication.md)）。

### 1.4 调用接口（非公开）
推理链路（**[社区逆向]**，qoder2api-hub「请求链路（COSY 签名推理）」）：

```
POST {gateway}/algo/api/v2/service/pro/sse/agent_chat_generation?…&Encode=1
  body   = Qoder 自定义 Base64（标准 B64 三段轮转 + 字母表映射, '=' → '$'）
  header = Authorization: Bearer COSY.{payloadB64}.{md5(payload\ncosyKey\ndate\nbody\npath)}
           + RSA(1024) 包裹的 AES-128 会话密钥
```
- 主机：国际版 `api1.qoder.sh`（`api2`/`api3` 为官方故障切换域名），国内版 `gateway.qoder.com.cn`。
- 模型清单：`GET /algo/api/v2/model/list?Encode=1`，**GET 也必须带与签名一致的 `{}` body，否则 403**。
- 账号/额度：`/api/v1/userinfo`、`/api/v2/quota/usage`、`/api/v2/user/plan`；活动平台 `/sash/api/v1/me/*`。
- 客户端版本常量：`cosy-version: 1.1.64`（对齐官方 0.4.3 桌面端）；`User-Agent: Qoder`、`Cosy-ClientType: 10`、`Cosy-Version`、`cosy-machine*` 六头。
- **指纹校验的判断**：官方桌面端在拉活动列表前会 spawn 自带风控桥 `resources/umid/runtime-info.exe prod --account-stdin` 取真实机器身份；初始实现发送**派生**的六个 `cosy-machine*` 头时，服务端把「可领取状态」的 Credits 活动**整条静默过滤**（列表为空、无报错）。逐头隔离实测结论是「六个机器头单独任一个出现时活动可见，**全套一起发**即被过滤」（[qoder2api-hub issue #10 复盘](https://github.com/shuishuipingan/qoder2api-hub)）。这是「上游确实在做客户端/设备指纹校验」的强证据。**[社区逆向]**

### 1.5 额度与限流
- 官方 CLI：`/usage` 面板显示 Plan 类型、到期日、Plan Credits 已用/总量、Add-on Credits、Org Resource Package、API 累计时长、wall 时长、代码改动行数；用量比例 **≥80% 显示警告色，≥95% 显示错误色**（[官方文档](https://docs.qoder.com/cli/usage.md)）。
- 非公开额度接口：`/api/v2/quota/usage`（聚合基础额度 + 赠送/签到额度）、`/api/v2/user/plan`（套餐名）。**[社区逆向]**
- 超额表现：社区实现观察到上游把故障包装成 `418/5xx + provider_error`，也会在 HTTP 200 的 SSE 信封里投递 `statusCodeValue=418`；`429 usage exceeds frequency limit` 仅在真频控时出现。**[社区逆向]**

### 1.6 已有开源实现
| 项目 | 做什么 | 凭据获取方式 |
|---|---|---|
| [Liki4/qodercli2api](https://github.com/Liki4/qodercli2api) | Qoder OAuth → OpenAI/Anthropic API | 自建 OAuth 设备流 |
| [shuishuipingan/qoder2api-hub](https://github.com/shuishuipingan/qoder2api-hub) | 双区多账号 OpenAI 兼容网关中枢：COSY 签名、设备指纹隔离、OAuth 设备授权、PAT 导入、每日本机凭证扫描导入、定时调度、Web 看板 | 三种：设备流 / PAT / 只读扫描本机 auth 文件 |
| [mmqz/cpa-multi-plugins](https://github.com/mmqz/cpa-multi-plugins) | CLIProxyAPI 的 Qoder 双区域插件（CN/Intl 常量表、设备授权、PAT 交换、COSY 签名） | 插件形态 |
| [zxcrf/opencode-qoder-auth](https://github.com/zxcrf/opencode-qoder-auth) | opencode 插件，复用本机已登录的 `qodercli`（SDK 流式；`mode: cli` 时直接跑 `qodercli --print`） | **不自己管凭据**，依赖本机 CLI 已登录 |

**账号池/轮换**：qoder2api-hub 的做法最具参考价值 —— 以账号 UID 稳定哈希派生 `cosy-machineid`/`cosy-machinetoken`/会话标识（「同一账号长期固定在同一台虚拟物理设备」），多账号机器码彼此隔离；账号级短冷却（15s，单账号池 3s）后换号；`refreshToken` 前缀路由刷新；定时 09:00/21:00 保活。

---

## 2. CodeBuddy / WorkBuddy（腾讯）—— 身份先确认

### 2.1 结论：WorkBuddy 是腾讯的产品，与 CodeBuddy 同源
- 用户给的两种猜测都部分成立，但**不是**「腾讯 CodeBuddy」与「某个叫 WorkBuddy 的编码工具」两个无关产品，而是**同一厂商（腾讯）的两个产品，共用同一套认证协议与同一批后端域名**：
  - **CodeBuddy** = 腾讯的 AI 编码助手，后端 `https://copilot.tencent.com`（国内）；官方文档入口为 [codebuddy.cn/docs](https://www.codebuddy.cn/docs/enterprise/)（该页面标题为「**WorkBuddy** - AI Agent 办公新范式」）。
  - **WorkBuddy** = 腾讯的 AI Agent 办公产品（同一站点 `codebuddy.cn` 托管其文档），海外站为 `https://www.workbuddy.ai`；Windows 上其客户端数据目录名是 **`CodeBuddyExtension`**。
  - 知名多账号签到工具的 README 标题即为「TRAE / **WorkBuddy** 每日积分自动签到」，正文把 WorkBuddy 与 TRAE 并列，且注明 WorkBuddy 凭据来自 `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\*.info`（[devilardis/auto-checkin](https://github.com/devilardis/auto-checkin)）。
  - 主流反代项目明确写「同一个 proxy，两个平台」：国内 **CodeBuddy**（`copilot.tencent.com`）与海外 **WorkBuddy AI**（`www.workbuddy.ai`），并声明「两个平台使用**完全相同的认证协议**（`cli-external-link`，`/v2/plugin/` 前缀）」（[Zhengyuuuui/workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api)）。
- 腾讯云开发者社区另有文章「初识 WorkBuddy 与 CodeBuddy 体验感想」与「WorkBuddy：让 AI 从『会聊天』进化到『会干活』」，佐证二者是同期姊妹产品（[腾讯云社区](https://cloud.tencent.com.cn/developer/article/2760498)）**[第三方报道]**。

> 因此下文把二者合并叙述，只在 endpoint / UA / device-token 上区分。

### 2.2 登录方式
- **协议名 `cli-external-link`**：中继服务先取 `state`，打开浏览器到官方登录页 → 用户完成 SSO → 中继轮询取回 token → 自动刷新（**[社区逆向]**，workbuddy2api「技术细节」）。
  - 国内登录 URL：`https://copilot.tencent.com/login?platform=VSCode&state={state}`
  - 海外登录 URL：`https://www.workbuddy.ai/login?platform=workbuddy-ai&state={state}`
- 用户侧另一种方式：直接登录官方客户端/IDE，客户端会把登录态写到本地文件（见 2.3），中继只读导入。
- **不是** OAuth device flow（无 `device_code` 轮询语义），**不是**扫码，是浏览器 SSO + 回跳轮询。

### 2.3 凭据落盘位置与格式（Windows）
| 来源 | 路径 | 字段 |
|---|---|---|
| 官方客户端扩展 | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\*.info` | `accessToken`、`uid`、`domain`、`expires_at`、昵称（`username`/`nickname`）**[社区逆向]** |
| 中继会话文件 | `%USERPROFILE%\.codebuddy-session.json`（国内）/ `%USERPROFILE%\.workbuddy-ai-session.json`（海外） | 中继自己写 |
| 设备令牌（仅国内） | `%USERPROFILE%\.codebuddy-device-token.json` | `x-device-token` |

**一个关键的工程坑（新版客户端加密）**：新版客户端会把 `accessToken` 加密存储，形如 `{"$wbEncrypted": 1, "envelope": ...}`，算法为 **AES-256-GCM**，密钥 = `SHA-256(atRestSecretKey)`，而 `atRestSecretKey` **随客户端安装内置、只存在于运行中的客户端进程内存**；社区脚本因此通过**扫描 WorkBuddy 进程内存**（正则 `\{"version":1,"atRestSecretKey":"([A-Za-z0-9+/]{43}=)"`）取回密钥再解密字段，并且明说「**WorkBuddy 客户端未在运行、无法解密**」。同时注明「**官方无续期接口**，token 过期后需重开客户端再导入」（[auto-checkin extract_credentials.py](https://github.com/devilardis/auto-checkin)）。**[社区逆向]**

### 2.4 token 结构
- 明文/解密后是 **JWT**（中继侧 `data/token.json` 保存 JWT，并支持「导入凭证（JWT）」；[EchoPing07/Buddy-2API-Go](https://github.com/EchoPing07/Buddy-2API-Go)）。
- 刷新：401 时刷新后重试一次（同上）；会话过期表现为 `TOKEN_EXPIRE` / `12153` / `Offline user session not found` → 需重新登录。
- **国内版必须携带 `x-device-token`，海外版不存在该头**：项目声明「国内版 `x-device-token`：通过环境变量 / `--device-token` 注入……一设备一账号，勿共用」「海外版**不存在** `x-device-token`：官方客户端本身不产该头（已三轮抓包 + 二进制层验证），仅需 JWT 认证」（[workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api)）。**[社区逆向]**

### 2.5 调用接口（非公开）与指纹
- 后端：国内 `https://copilot.tencent.com`，海外 `https://www.workbuddy.ai`；认证 platform 分别为 `VSCode` 与 `workbuddy-ai`。
- 已知路径：
  - `POST /v2/billing/meter/checkin-activity-status`、`/v2/billing/meter/*`
  - `GET /billing/meter/get-user-resource-summary`（余额/资源包；注意该路由**不带 `/v2`**，与客户端 desktop 实现一致）
  - `GET /v2/accounts`（账号列表含昵称）
  - `GET /v3/config?repos=`（模型清单）
  （前三条见 [auto-checkin/diagnose_workbuddy.py](https://github.com/devilardis/auto-checkin)；`/v3/config` 见 Buddy-2API-Go）
- 必需请求头（**[社区逆向]**，两份独立实现互相印证）：
  ```
  Authorization: Bearer <token>
  X-User-Id: <uid>
  X-Domain: <domain>
  Content-Type: application/json
  User-Agent: WorkBuddy/1.0            # 海外（auto-checkin 实测：默认 UA 被网关拦截）
  ```
  国内反代对齐官方 IDE 客户端的头集：`CodeBuddyIDE/4.12.0` UA、`x-ide-*`、`x-product-*`、`X-Product: SaaS`、`X-Product-Version`；海外对齐 `WorkBuddy/5.5.2 ... CLI/2.137.1`，并带 `x-stainless-*`（workbuddy2api「官方客户端指纹模拟」）。
- **UA 是硬校验**：`User-Agent` 必须是 `CodeBuddyIDE/<version>` 形式，否则上游返回 `HTTP 400 / code 12403 check ua, get coding copilot version error`（Buddy-2API-Go 原文；workbuddy2api 的「同步上游模型列表」示例也复述了同一约束）。`x-device-token` 缺失/共用则触发**设备关联风控**（见 §13）。
- 另有会话级指纹：`conversation_id` 复用、`previous_response_id` 链式传递、`b3` 追踪链延续、大请求体 gzip、去掉 httpx 默认注入的 `accept-encoding`（workbuddy2api）。

### 2.6 额度与限流
- 余额/资源包：`/billing/meter/get-user-resource-summary`；中继侧有 `/v1/credits`（缓存 60s，`?refresh=1` 强刷）与消耗速率告警阈值。
- 限流：429 → 熔断器（`/v1/breaker`，可按 model 手动清除）；全局限速参数 `--rate-qps`（默认 1.0）、`--rate-burst`（5）、`--rate-jitter`（3.0s）——作者显然把「QPS 太高」当作主要风控触发因素。
- 模型倍率：`/v3/config` 的 `models[].credits` 与 `modelPromotions` 折扣活动，支持时段窗口（夜间折扣/限时免费），Buddy-2API-Go 会按请求时刻实时评估。
- **扫描到「一个平台两套域名」的凭证不互通**：`cn`（`copilot.tencent.com`）/ `global`（`www.codebuddy.ai`）切换后需重新扫码登录（Buddy-2API-Go）。

### 2.7 已有开源实现（这一族最多）
| 项目 | 定位 | 账号数 | 备注 |
|---|---|---|---|
| [Zhengyuuuui/workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api) | 双域反代网关，OpenAI/Anthropic/Responses 三协议 | 多账号 | 指纹模拟、`x-device-token` 注入、脱敏、压缩、熔断、限速 |
| [EchoPing07/Buddy-2API-Go](https://github.com/EchoPing07/Buddy-2API-Go) | Go 单二进制 + Web 面板 | **单账号（明确拒绝号池）** | 项目声明「明确不接受『号池』相关的建议与提交」 |
| [Sliverkiss/CodeBuddy2api](https://github.com/Sliverkiss/CodeBuddy2api)、[wicm84266964/Buddy2api](https://github.com/wicm84266964/Buddy2api)、[cyl2361341082-alt/Buddy2api](https://github.com/cyl2361341082-alt/Buddy2api)、[ShouZhuo0413/codebuddy2api](https://github.com/ShouZhuo0413/codebuddy2api)、[Sliverkiss/workbuddy2api](https://github.com/Sliverkiss/workbuddy2api) | 同族上游契约来源 | — | 被 Buddy-2API-Go 在「致谢与参考」中逐项标注用途 |
| [kuops/opencode-codebuddy-auth](https://github.com/kuops/opencode-codebuddy-auth) | opencode 认证插件 | — | 实时模型列表接口参考 |
| CLIProxyAPI | v6 曾内置 `internal/auth/codebuddy` 包（[pkg.go.dev](https://pkg.go.dev/github.com/router-for-me/CLIProxyAPI/v6/internal/auth/codebuddy)）；本次 `--depth 1` 克隆的 main 为 `v8` 模块，`internal/auth/` 下**没有** codebuddy 目录，仅有测试夹具提及 `workbuddy.json` | — | 说明该 provider 在 v6→v8 之间被移除或迁出 |
| [fu827707013/dsh-codebuddy-cli](https://github.com/fu827707013/dsh-codebuddy-cli) | DSH 插件形态 | — | 未细读 |
| PyPI [`workbuddy2api`](https://pypi.org/project/workbuddy2api/) | 已上架包 | — | 与 GitHub 版本关系未证实 |

**账号池/轮换**：workbuddy2api 支持多账号隔离（「支持多个登录态隔离，方便工作/个人账号切换」），但**明确警告国内版一设备一账号**；Buddy-2API-Go 直接拒绝号池。这一族没有一个「可放心做池」的正例。

---

## 3. Trae（字节跳动）

### 3.1 登录方式
- 官方客户端（Trae CN / TRAE SOLO CN / 国际版 Trae / TRAE SOLO）内浏览器登录；网页版在 `work.trae.cn` / `www.trae.cn` 登录后由会话 Cookie 换取 JWT（**[社区逆向]**，[devilardis/auto-checkin traeweb.py](https://github.com/devilardis/auto-checkin)：`DEFAULT_WEB_BASE = "https://www.trae.cn"`，`get_user_token(cookie)` → `{token, expires_at}`，返回字段名 `Token`）。
- 未发现设备码流程。

### 3.2 凭据落盘位置与格式（Windows）
- 主路径：`%APPDATA%\<AppName>\User\globalStorage\storage.json`，`AppName ∈ {Trae CN, TRAE SOLO CN, Trae, TRAE SOLO}`（[auto-checkin extract_credentials.py](https://github.com/devilardis/auto-checkin) 的 `candidate_trae_storages()`）。
- 关键 key：`iCubeAuthInfo://icube.cloudide`（登录态）；`iCubeAuthInfo://icube-dc:<数字>`（device id）；同文件还有 `telemetry.machineId`、`iCubeLastVersion`。
- **加密格式 `tc`**（社区脚本逐字给出常量，可复现）：
  ```
  base64 解码 → header[0:6] 必须为 74 63 05 10 00 00（"tc"）
              → rb = [6:38]（32字节随机）→ enc = [38:]
  salt  = SALT_A[i] XOR SALT_B[i]   # 两个 64 字节硬编码常量
  h     = SHA512(SHA512(rb) || salt)
  plain = AES-CBC-decrypt(enc, key=h[0:16], iv=h[16:32]) 去 PKCS#7
  plain = stored[0:64]（SHA512(payload)）|| payload(JSON)
  ```
  国际版该 key 可能是**明文 JSON**（脚本按首字符是否为 `{` 分支）。
- 解密后 JSON 字段：`token`、`refreshToken`、`expiredAt`、`userId`、`host`（默认 `https://api.trae.cn`）、`account.username`。

### 3.3 token 结构与刷新
- access `token` + `refresh_token`（客户端模式）；刷新请求把 `RefreshToken` + `UserID` 发给 host，取 `data.Result.Token` 作为新 access token（[auto-checkin trae.py](https://github.com/devilardis/auto-checkin) 的 `_refresh`）。网页模式则是「Cookie → JWT」，JWT 过期后重新用 Cookie 换（Cookie 失效需重新粘贴）。**[社区逆向]**
- 社区工具把「客户端模式带 refresh_token」判定为**可自动续期、不算硬失效**，而网页模式 Cookie 失效即硬失效（[auto-checkin web.py](https://github.com/devilardis/auto-checkin) 的失效判定逻辑）。

### 3.4 调用接口
已知的（积分/签到链）请求头模式：
```
Authorization: Cloud-IDE-JWT <token>     # client 模式
X-Cloudide-Token: <token>
x-uid: <user_id>
```
网页模式则用 `Authorization: Cloud-IDE-JWT <jwt>`（[auto-checkin trae.py](https://github.com/devilardis/auto-checkin)）。参考 host：`https://api.trae.cn`（注释称其为「积分/支付 API（AIPayHost 默认值）」）。

**未证实**：Trae 的 chat/completions 类上游端点、以及它是否校验客户端指纹。本次调研**没有找到**把 Trae 会话转成 OpenAI API 的开源项目（只找到积分监控/签到类）。

### 3.5 额度与限流
- 计量单位为**积分**；社区项目围绕「每日签到/积分」构建，说明存在每日签到接口与业务码（见 §2.6 同类模式）。
- 有第三方 VS Code 扩展专门展示 Trae CN 额度（[TraeCN Quota, Open VSX](https://open-vsx.org/extension/GhostCmdr/traecn-quota)）——存在「额度可被外部读取」的旁证，但接口细节未取得。

### 3.6 开源实现
- [devilardis/auto-checkin](https://github.com/devilardis/auto-checkin)：TRAE（Trae CN / TRAE SOLO CN / 网页版）与 WorkBuddy 每日签到；含本机凭据提取、按 `user_id` 去重、多账号、Docker 常驻调度；免责声明自认「相关接口为逆向自官方客户端/网页的非公开实现，可能随版本变化失效」。
- 该 README 明确记录风控现象：**「Cookie 导入提示被风控拦截：复制的是 DevTools → Network 里请求头的完整 Cookie（包含 ttwid 等）」**——即 Cookie 复制不完整/特征不符会被风控拦。

---

## 4. Kiro（AWS）

### 4.1 登录方式（两条路）
1. **桌面 Social Auth**（Kiro IDE 内点 Google/GitHub 登录）：授权入口 `https://prod.{region}.auth.desktop.kiro.dev/login`，**本地回调端口范围 19876–19880**（AIClient-2-API `kiro-oauth.js` 常量 `callbackPortStart/End`）。**[官方仓库证据？不，是社区实现]** → 归类 **[社区逆向]**。
2. **AWS Builder ID / IAM Identity Center（OIDC）设备码**：先 `POST https://oidc.{region}.amazonaws.com/client/register` 拿 `clientId`/`clientSecret`，再 `POST /device_authorization` 拿设备码，轮询 `POST /token`；起始 URL `https://view.awsapps.com/start`；scopes 为 `codewhisperer:completions`、`codewhisperer:analysis`、`codewhisperer:conversations`（AIClient-2-API `kiro-oauth.js`；[jwadow/kiro-gateway](https://github.com/jwadow/kiro-gateway) 亦描述 `kiro-cli login` 对应 AWS SSO）。**[社区逆向]**

### 4.2 凭据落盘位置与格式（Windows）
| 路径 | 关键字段 | 来源 |
|---|---|---|
| `%USERPROFILE%\.aws\sso\cache\kiro-auth-token.json` | `accessToken`、`refreshToken`、`expiresAt`(ISO)、`authMethod`(`social`)、`provider`(`Google`)、`profileArn`、`region`，企业 SSO 另含 `clientIdHash` | [kiro-gateway](https://github.com/jwadow/kiro-gateway)、[OpenTokenUsage/kiro.md](https://github.com/poweruserz/OpenTokenUsage) |
| `%USERPROFILE%\.aws\sso\cache\<hash>.json` | AWS SSO 形态：`accessToken`、`refreshToken`、`expiresAt`、`region`、`clientId`、`clientSecret` | kiro-gateway |
| `%USERPROFILE%\.kiro\oauth_creds.json` | AIClient-2-API **自己生成**的凭据文件（`credentialsDir: '.kiro'`） | AIClient-2-API |
| `%USERPROFILE%\.local\share\kiro-cli\data.sqlite3` | `auth_kv` 表：`kirocli:odic:token` / `codewhisperer:odic:token`（access/refresh/expiration）、`...:device-registration`（client id/secret） | kiro-gateway |
| `%APPDATA%\Kiro\User\globalStorage\state.vscdb` | `kiro.kiroAgent` → `kiro.resourceNotifications.usageState`（用量缓存） | OpenTokenUsage/kiro.md（原文给 macOS 路径） |
| `%APPDATA%\Kiro\User\globalStorage\kiro.kiroagent\profile.json` | `{arn, name}`（profileArn 兜底） | 同上 |

> 上表前三行的 Windows 路径由 `~` 展开推得；README/文档直接给出的是 `~/.aws/sso/cache/...`（跨平台通用写法）。第四/五行的 Windows 路径是**推断**（原文给的是 macOS 的 `~/Library/Application Support/...`）。

### 4.3 token 结构与刷新
- 刷新（Kiro Desktop Auth，**无 clientId/clientSecret 时走这条**）：
  ```
  POST https://prod.{region}.auth.desktop.kiro.dev/refreshToken
  Content-Type: application/json
  User-Agent: KiroIDE-<version>-<machineId>
  {"refreshToken": "<refresh_token>"}
  → { accessToken, refreshToken, expiresIn, profileArn }
  ```
- 刷新（AWS SSO/OIDC，**有 clientId/clientSecret 时走这条**）：`https://oidc.{region}.amazonaws.com/token`，字段 `refreshToken` + `clientId` + `clientSecret`。
- kiro-gateway 的参数名与 AIClient-2-API 的实现一致（`REFRESH_URL: https://prod.{{region}}.auth.desktop.kiro.dev/refreshToken`，body `{refreshToken}`，返回 `accessToken/refreshToken/profileArn`，`expiresAt = now + expiresIn*1000`）。**[社区逆向]，两份独立实现互证**

### 4.4 调用接口
- 运行时服务是 **AWS CodeWhisperer / Amazon Q Developer Runtime**：`https://q.{region}.amazonaws.com`（kiro-gateway 直接称自己为 "Proxy gateway for Kiro API (Amazon Q Developer / AWS CodeWhisperer)"）。
- 已知用法示例（**[社区逆向]**）：
  ```
  GET https://q.<region>.amazonaws.com/getUsageLimits?origin=AI_EDITOR&profileArn=<arn>&resourceType=AGENTIC_REQUEST
  Authorization: Bearer <accessToken>
  ```
  额外头：`TokenType: EXTERNAL_IDP`（外部 IdP 账号）、`redirect-for-internal: true`（AWS 内部账号）。
- 推理协议的具体 service/operation 名（`AmazonCodeWhispererStreamingService` / `generateAssistantResponse` / `SendMessageStreaming`）在本次调研中**未取得一手证据** → **[未证实]**。AIClient-2-API 的 `claude-kiro.js` 有 17.8 万字节实现，是现成的可用参考，但本次未逐行核对协议名。
- 指纹：UA 里带 `KiroIDE-<version>-<machineId>`；未见更强的设备校验证据。

### 4.5 额度与限流
- `getUsageLimits` 返回：`usageBreakdownList[*]`（`type: CREDIT`、`currentUsage`、`usageLimit`、`resetDate`、`freeTrialUsage{currentUsage,usageLimit,expiryDate,daysRemaining}`）、`subscriptionInfo.subscriptionTitle/type`、`overageConfiguration.overageStatus`（`Enabled`/`Disabled`）、`nextDateReset`、`userInfo.userId`（[OpenTokenUsage/kiro.md](https://github.com/poweruserz/OpenTokenUsage)）。
- 超额表现：`429` 限流、`402` 额度耗尽（kiro-gateway 的账号故障转移正是监听这两个码）。
- 也可从本地 `state.vscdb` 的 `usageBreakdowns` 读离线用量；官方客户端把完整 `GetUsageLimitsCommand` 请求/响应写进 `logs/*/window*/exthost/kiro.kiroAgent/q-client.log`。

### 4.6 开源实现与账号池（这一族最成熟）
[jwadow/kiro-gateway](https://github.com/jwadow/kiro-gateway)（AGPL-3.0）是本次调研中**账号池设计最完整**的样本：
- 凭据类型三种：`{"type":"json","path":...}`（IDE/企业）、`{"type":"sqlite","path":...}`（kiro-cli 的 `data.sqlite3`）、`{"type":"refresh_token","refresh_token":...,"profile_arn":...}`（**直接粘贴 refresh token**）；甚至支持给一个**目录**让它扫描目录下所有文件作为独立账号。
- 故障转移：某账号返回 429/402 时自动换下一个；连续失败若干次后**临时停用**并定期探测是否恢复。单账号时不做转移（直接把原始错误暴露）。
- 首次启动会把 `.env` 里的凭据**一次性迁移**到 `credentials.json`，之后只认 `credentials.json`。
- 自动刷新：「Automatic refresh before expiration」。
- 输出：`/v1/chat/completions`（OpenAI）+ `/v1/messages`（Anthropic 原生），可接 Claude Code / Codex / Cursor / Cline 等。
- 免责声明：与 AWS / Anthropic / Kiro 无关联，「Use at your own risk and in compliance with the terms of service of the underlying APIs」。

AIClient-2-API 侧另有 `batchImportKiroRefreshTokens(refreshTokens[], region)` 与 `batchImportKiroRefreshTokensStream`：**批量粘贴 refreshToken → 逐个调刷新端点 → 生成凭据文件 → 自动关联到 Provider Pool**，并按 `refreshToken` 去重。这就是「手动粘贴 token」批量化入池的现成实现。

---

## 5. Cursor（Anysphere）

### 5.1 登录方式
- 浏览器 OAuth（Auth0 代理）；CLI 侧 `agent login` 会把 token 写进 keychain（条目名 `cursor-access-token`、`cursor-refresh-token`）[OpenTokenUsage/cursor.md](https://github.com/poweruserz/OpenTokenUsage)。**[社区逆向]**
- 未发现设备码流程。

### 5.2 凭据落盘位置与格式（Windows）
- SQLite：`%APPDATA%\Cursor\User\globalStorage\state.vscdb`，表 `ItemTable`：
  | key | 含义 |
  |---|---|
  | `cursorAuth/accessToken` | JWT bearer |
  | `cursorAuth/refreshToken` | 刷新凭据 |
  | `cursorAuth/cachedEmail` | 邮箱 |
  | `cursorAuth/stripeMembershipType` | 套餐档（`pro`/`ultra`） |
  | `cursorAuth/stripeSubscriptionStatus` | 订阅状态 |
  读取示例（文献中给的是 macOS 路径）：`sqlite3 .../state.vscdb "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'"`。
- **Windows 路径是推断**：`OpenTokenUsage/cursor.md` 原文只给 `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`（macOS）。VS Code 系约定为 `%APPDATA%\Cursor\User\globalStorage\state.vscdb`，但本次**未取得 Windows 一手证据** → 标记为**推断**。

### 5.3 token 结构与刷新
```
POST https://api2.cursor.sh/oauth/token
Content-Type: application/json
{"grant_type":"refresh_token","client_id":"KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB","refresh_token":"<rt>"}

成功 → {"access_token":"<jwt>","id_token":"...","shouldLogout":false}
失效 → {"access_token":"","id_token":"","shouldLogout":true}   # 必须重新登录
```
- access token 是短命 JWT；客户端在每次请求前按需刷新，并把新 access token **写回它读取的那个来源**（SQLite 或 keychain）。
- 有一类 `cursor.com` 网页端点用会话 Cookie 而非 Bearer：`WorkosCursorSessionToken=<userId>%3A%3A<access_token>`，`userId` 由 JWT `sub` 推导（`google-oauth2|user_abc` → `user_abc`）。

### 5.4 调用接口
- 已知可用的**非公开**协议：**Connect RPC v1（JSON over HTTP）**，基址 `https://api2.cursor.sh`，服务 `aiserver.v1.DashboardService`：
  | 端点 | 用途 |
  |---|---|
  | `POST /aiserver.v1.DashboardService/GetCurrentPeriodUsage` | 本计费周期花费/限额/百分比 |
  | `POST /aiserver.v1.DashboardService/GetPlanInfo` | 套餐名、含额、价格 |
  | `POST /aiserver.v1.DashboardService/GetUsageLimitPolicyStatus` | 是否进慢池、特性门、可用模型（响应未文档化） |
  | `POST /aiserver.v1.DashboardService/GetUsageLimitStatusAndActiveGrants` | 同上 + 活动 credit grants |
  | `GET /api/auth/stripe` | 订阅与 Stripe 余额（用上面的 Cookie） |
  必需头：`Authorization: Bearer <access_token>`、`Content-Type: application/json`、`Connect-Protocol-Version: 1`。
- **chat 侧未证实**：本次调研**没有**取得 Cursor chat 推理端点/头（社区常说的是 `aiserver.v1.ChatService/StreamUnifiedChatWithTools` + `x-cursor-checksum` 类指纹头）的一手或可复现证据 → **[未证实]**。仅找到 [`everestmz/cursor-rpc`](https://github.com/SimHacker/moollm/blob/main/skills/cursor-mirror/reference/assimilated/EVERESTMZ-CURSOR-RPC.yml) 这一 RPC 参考被第三方仓库收录，未逐条核对。

### 5.5 额度
- `planUsage`（单位：**美分**）：`totalSpend`/`includedSpend`/`bonusSpend`/`remaining`/`limit`、`autoPercentUsed`、`apiPercentUsed`、`totalPercentUsed`。
- `spendLimitUsage`（套餐用尽后的按需预算）：`pooledLimit/pooledUsed/pooledRemaining`（团队池）、`individualLimit/individualUsed/individualRemaining`、`limitType: user|team`。
- 企业账号走 REST `GET /api/usage`（按请求数计）。
- 团队判定：`planName == "Team"` 或 `spendLimitUsage.limitType == "team"` 或 `pooledLimit > 0`。

### 5.6 开源实现
- [OpenTokenUsage](https://github.com/poweruserz/OpenTokenUsage)：只读用量面板，按「SQLite 优先 → CLI keychain 兜底」顺序读凭据，刷新后**写回原来源** —— 这是「复用本机已登录文件」模式最干净的参考实现。
- 本次**没有找到**把 Cursor 订阅转成 OpenAI API 的活跃项目（`cursor-api`/`cursor2api` 类仓库未在检索中命中可靠目标）→ **[未证实]**。

---

## 6. Windsurf（Cognition，前 Codeium）

### 6.1 登录方式
- **Firebase 密码登录**：`POST https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword`，刷新走 `POST https://securetoken.googleapis.com/v1/token`；两者都**必须带** `Referer: https://windsurf.com/`，否则 Google 返回 403 `API_KEY_HTTP_REFERRER_BLOCKED`；建议同时带 `X-Client-Version: Chrome/JsCore/11.0.0/FirebaseCore-web`（[windsurf-account-manager-simple](https://github.com/chaogei/windsurf-account-manager-simple)）。
- **Devin 账号体系**（官方把部分老 Firebase 账号迁到 Devin）：`auth1_token`（浏览器 localStorage，格式 `auth1_<52字符>`）+ `session_token`，5-header 认证；`WindsurfPostAuth` 接口自某个版本起额外要求 `X-Devin-Auth1-Token` 头（同上 README 的版本历史 v1.7.8）。
- **`windsurf://` 深链回跳**：一键换号工具通过 `windsurf://` 协议触发官方客户端完成登录，并需要给 `extension.js` 打补丁**移除 180 秒 OAuth 超时限制**（同上）。
- 未发现设备码流程。

### 6.2 凭据落盘位置与格式（Windows）
| 层 | 位置 | 内容 |
|---|---|---|
| 加密层 | `secret://windsurf_auth.sessions`（存在 `state.vscdb` 内） | Windows DPAPI 加密的 Buffer（会话数据 + API Key） |
| 状态层 | `windsurfAuthStatus`（同上） | **明文 JSON**：API Key（`sk-ws-01-` 前缀）、用户信息、团队配置 |
| 明文层 | `codeium.windsurf-windsurf_auth`、`codeium.windsurf` | 用户名、基础配置 |
- 数据库文件：`%APPDATA%\Windsurf\User\globalStorage\state.vscdb`（SQLite，表 `ItemTable`）。
- 配置文件：`%APPDATA%\Windsurf\User\globalStorage\storage.json`（机器 ID、设备标识）。
- 项目直言：**「API Key 在 windsurfAuthStatus 中明文存储，保护数据库文件至关重要」**。
- 全部来自 [windsurf-account-manager-simple README](https://github.com/chaogei/windsurf-account-manager-simple) 的「Windsurf 数据存储架构」**[社区逆向]**。

### 6.3 token 结构与刷新
- Firebase：ID Token（经 `identitytoolkit` 登录）+ `refresh_token`（经 `securetoken` 刷新）；工具称「Token 过期前 5 分钟自动刷新」。
- **换 API Key**：`POST https://register.windsurf.com/exa.seat_management_pb.SeatManagementService/RegisterUser`（**也支持** `web-backend.windsurf.com`），用 Firebase ID Token 换 `api_key` + `name` + `api_server_url`。
- API Key 格式：`sk-ws-01-<88~94 字符 Base64>`，总长 103 字符，Base64 解码后为 32 字节（README 给了字符分布统计）。
- Devin：`session_token` **没有显式过期时间**，用「32 天占位机制」，**基于 401 触发强制刷新**。

### 6.4 调用接口
- Connect-Web（gRPC-Web + Protobuf），基址 `https://web-backend.windsurf.com`，服务 `exa.seat_management_pb.SeatManagementService`：
  `GetPlanStatus`、`GetCurrentUser`、`GetUsers`、`UpdateSeats`、`GetTeamBilling`、`GetTeamCreditEntries`、`ProcessReferralCode`；另有 `exa.user_analytics_pb.UserAnalyticsService/GetAnalytics`。
- 请求体是**手写 protobuf**：固定前缀 `0x0a, 0xa1, 0x07`（README 强调「是固定的协议标识，不要修改」）+ token + 字段（如座位数 `0x10, seat_count`）。
- 必需头：`connect-protocol-version: "1"`、`x-debug-email: ""`、`x-debug-team-name: ""`、`Referer: https://windsurf.com/`、`User-Agent`、`sec-ch-ua`/`sec-ch-ua-mobile`/`sec-ch-ua-platform`。
- **Cascade/chat 推理端点未证实** → 本次未取得证据。

### 6.5 额度
`GetPlanStatus` 响应字段：`available_flex_credits`、`available_prompt_credits`、`used_flex_credits`、`used_prompt_credits`、`billing_strategy`、`daily_quota`、`weekly_quota`。
`GetTeamBilling`：`plan_name`、`base_quota`、`extra_credits`、`total_quota`、`used_quota`、**`cache_limit`**（README 特别提醒：**缓存使用率达到 cache_limit 时，即使总额度还有剩余也无法继续使用**）、`payment_method`、`next_billing_date`、`invoice_url`。

### 6.6 开源实现（并注意其中的滥用成分）
[chaogei/windsurf-account-manager-simple](https://github.com/chaogei/windsurf-account-manager-simple)（AGPL-3.0，Tauri + Rust + Vue，Windows）：
- 账号池：SQLite 存 10 万+ 账号记录，AES-256-GCM 加密密码/token，密钥放 Windows Credential Manager；批量导入/刷新/导出（导出字段含 Refresh Token、Access/Session Token、Windsurf API Key、Devin Auth1 Token、Devin Session Token 等）。
- 刷新与轮换：「自动 Token 刷新」+「一键换号」（refresh_token → access_token → RegisterUser 取 api_key → 加密写回 `state.vscdb` → 重置机器 ID → 重启客户端）。
- **需要明确标注的滥用项**：README 的功能列表包含「积分重置 —— 通过座位数更新 API 实现，智能在 3/4/5 座位数之间轮换」「虚拟卡生成 / 自定义 BIN 号用于支付测试」「无感换号补丁（改 `extension.js`）」。这些属于**主动规避平台计费与风控**的行为，风险等级远高于「只是把登录态转成 API」。
- 免责声明：「本工具仅供学习和个人使用，请遵守 Windsurf 服务条款。」

---

## 7. GitHub Copilot

### 7.1 登录方式
- **GitHub OAuth 设备流**：`ericc-ch/copilot-api` 提供独立 `auth` 子命令「Run GitHub authentication flow without starting the server」，也可 `--github-token` 直接给 token 适配 CI；交互式首次 `start` 会顺带完成认证（[README](https://github.com/ericc-ch/copilot-api)）。
- 或复用 `gh auth login` 的 token：OpenTokenUsage 按「自身 keychain → `gh:github.com` keychain → `auth.json` 文件」顺序找 GitHub token（[copilot.md](https://github.com/poweruserz/OpenTokenUsage)）。
- AIClient-2-API 走的是**第三条路：细粒度 PAT**——`GITHUB_COPILOT_API_KEY`，要求带 `Copilot Requests` 权限（源码报错原文：`Add one as GITHUB_COPILOT_API_KEY with the Copilot Requests permission.`）。**[官方源码？不，是第三方仓库源码]** → **[社区实现]**

### 7.2 凭据落盘位置与格式
- **AIClient-2-API：不落盘**，凭据只在配置/环境变量里（`GITHUB_COPILOT_API_KEY`）。
- copilot-api：把 GitHub token 与相关数据存在自己的数据目录（Docker 文档写 `/root/.local/share/copilot-api`，并建议 bind mount 持久化）→ Windows 对应位置**未证实**（可能是 `%USERPROFILE%\.local\share\copilot-api`）。
- **`%LOCALAPPDATA%\github-copilot\`（`hosts.json` / `apps.json`）这一常见说法，本次调研未取得任何证据 → [未证实]**。

### 7.3 token 结构
```
GET https://api.github.com/copilot_internal/v2/token
Headers: (githubHeaders) Authorization: token <github_token>
→ { token, expires_at, refresh_in }        # token 是短命 Copilot token
```
`[...copilot-api/src/services/github/get-copilot-token.ts](https://github.com/ericc-ch/copilot-api)` 的接口定义就是这三个字段；`expires_at` 是 epoch 秒、`refresh_in` 是建议刷新间隔（秒）。**[社区逆向/源码级]**

### 7.4 调用接口
- 基址 `https://api.githubcopilot.com`（OpenAI 兼容）：`/chat/completions`，另有 `/v1/messages`（`/v1/embeddings` 亦有实现）。AIClient-2-API 的 `GITHUB_COPILOT_BASE_URL` 默认值即 `https://api.githubcopilot.com`。
- 额度查询头（[OpenTokenUsage/copilot.md](https://github.com/poweruserz/OpenTokenUsage)）：
  ```
  Authorization: token <token>
  Accept: application/json
  Editor-Version: vscode/1.96.2
  Editor-Plugin-Version: copilot-chat/0.26.7
  User-Agent: GitHubCopilotChat/0.26.7
  X-Github-Api-Version: 2025-04-01
  ```
  → **这些编辑器身份头是被要求的**（该文档只列了这些；`Copilot-Integration-Id` / `openai-intent` / `x-initiator` / `machineid` 等头在本次调研中**未取得证据** → **[未证实]**）。
- 推理请求是否强校验同一批头：copilot-api 的实现会在每次请求前刷新 Copilot token（`refresh_in` 驱动），说明 token 短命是主要约束；指纹校验强度未取得对照实验证据 → 部分**未证实**。

### 7.5 额度与限流
- `GET https://api.github.com/copilot_internal/user` →
  - 付费：`copilot_plan`、`quota_reset_date`、`quota_snapshots.{premium_interactions,chat,completions}.{percent_remaining, entitlement, remaining, quota_id}`
  - 免费：`copilot_plan: "individual"`、`access_type_sku: "free_limited_copilot"`、`limited_user_quotas.{chat,completions}`、`monthly_quotas`、`limited_user_reset_date`
- AIClient-2-API 的 `getQuota()` 用另一种口径：从 Copilot SDK `rpc.account.getQuota` 取 `quotaSnapshots`，按 `isUnlimitedEntitlement`、`entitlementRequests`、`remainingPercentage`、`usedRequests`、`resetDate` 归一化，并设 `quotaTypes = ['premium_interactions','chat','completions']`，缓存 60 秒。
- 超额/风控表现：copilot-api README 自带的警告最具体（见 §13.4）：自动化批量请求可能触发 GitHub 滥用检测，收到 GitHub Security 警告，进一步异常可能导致 **Copilot 访问被临时封禁**；可用 `--rate-limit <秒>` + `--wait` + `--manual` 降速。

### 7.6 开源实现（星标最高的一族）
| 项目 | 星标 | 凭据获取 | 池/轮换 |
|---|---|---|---|
| [ericc-ch/copilot-api](https://github.com/ericc-ch/copilot-api) | 4147 | 设备流 OAuth / `--github-token` | 无（单账号 + 限速/人工审批） |
| [caozhiyuan/copilot-api](https://github.com/caozhiyuan/copilot-api) | 1081 | 同上（另支持 Codex/OpenCode Go 等上游） | 未见证据 |
| [StarryKira/copilot2api-go](https://github.com/StarryKira/copilot2api-go) | 141 | Go 重写 | 未见证据 |
| [suhaibbinyounis/github-copilot-api-vscode](https://github.com/suhaibbinyounis/github-copilot-api-vscode) | 129 | 复用 VS Code 内的 Copilot | — |
| [QLHazyCoder/copilot-api](https://github.com/QLHazyCoder/copilot-api) | 71 | 已从上游 fork 脱离 | — |
| AIClient-2-API | — | **细粒度 PAT** | 走通用 Provider Pool |

---

## 8. Gemini CLI / Gemini Code Assist（含 Antigravity）

### 8.1 登录方式
- **官方一级路径：`gemini` → 「Sign in with Google」**，浏览器完成登录，凭据本地缓存；个人账号（含 Google AI Pro/Ultra 订阅）**不需要** Google Cloud 项目，公司/学校/Workspace 或 Code Assist 许可证才需要设置 `GOOGLE_CLOUD_PROJECT`（[官方文档 docs/get-started/authentication.mdx](https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx)）。
- 其他官方路径：`GEMINI_API_KEY`（AI Studio）、Vertex AI（ADC / service account JSON / API key）。
- **未指定固定回调端口**：本次调研没有取得 gemini-cli 的 loopback 端口号证据 → **[未证实]**。AIClient-2-API 的实现是 `redirectUri = "http://" + host + ":" + port`（动态端口）。
- **OAuth client id（两个，均来自客户端源码/社区实现常量）**：
  - Gemini CLI：`681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com`
  - Antigravity：`1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`
  - scopes：`https://www.googleapis.com/auth/cloud-platform` + `https://www.googleapis.com/auth/userinfo.email`
  （AIClient-2-API `src/auth/gemini-oauth.js`；仓库为 GPLv3 第三方实现，同样属于 **[社区逆向]**）

### 8.2 凭据落盘位置（Windows）
| 产品 | 路径 |
|---|---|
| Gemini CLI | `%USERPROFILE%\.gemini\oauth_creds.json` |
| Antigravity | `%USERPROFILE%\.antigravity\oauth_creds.json` |
（AIClient-2-API `credentialsDir: '.gemini'` / `'.antigravity'`，`credentialsFile: 'oauth_creds.json'`；README「Authorization File Storage Paths」亦列 `~/.antigravity/oauth_creds.json`）
- 相关但不是凭据：`%USERPROFILE%\.gemini\.env`（官方文档明确支持，用于持久化 `GOOGLE_CLOUD_PROJECT` / `GEMINI_API_KEY` 等）。

### 8.3 token 结构与刷新
- Gemini CLI / Antigravity 的 `oauth_creds.json` 是标准 Google OAuth 结构（`access_token`/`refresh_token`/`scope`/`token_type`/`id_token`/`expiry_date`）。
- 生命周期：AIClient-2-API FAQ #14 原文 —— **「OAuth tokens (e.g., Gemini, Antigravity, Codex) typically have a limited lifespan (e.g., 1 hour)」**；并说明「不开自动刷新就会 401/403」。
- 刷新端点：Google 标准 `https://oauth2.googleapis.com/token`（`grant_type=refresh_token`）——本次由 AIClient-2-API 的刷新实现支撑 **[社区逆向]**，未引用 Google 一手文档。

### 8.4 调用接口（Code Assist / Antigravity）
来自 AIClient-2-API `src/providers/gemini/gemini-core.js` 与 `antigravity-core.js`：
- Gemini CLI（Code Assist）基址：`https://cloudcode-pa.googleapis.com`，API 版本 `v1internal`；
  操作：`loadCodeAssist`（发现 project ID）、`onboardUser`（长任务轮询）、`generateContent`、`streamGenerateContent`、`:retrieveUserQuota`。
- Antigravity 基址候选（按序故障切换）：`https://daily-cloudcode-pa.googleapis.com` → `https://cloudcode-pa.googleapis.com`，同 `v1internal`。
- 必需头：
  ```
  User-Agent: GeminiCLI/<version>/<modelName> (<platform>; <arch>)
  X-Goog-Api-Client: <GEMINI_CLI_API_CLIENT_HEADER>
  ```
  即 **UA 里带 CLI 版本 + 模型名 + 平台/架构**，属于客户端指纹式校验；Antigravity 另有自己的 `userAgent` 常量。
- 因此：**「上游会看客户端身份」在这一族是确定的**（至少 UA/Api-Client 头被显式构造），但是否**拒绝**不匹配的 UA，本次未取得对照实验 → 部分未证实。

### 8.5 额度
- CLI 侧：`v1internal:retrieveUserQuota`（AIClient-2-API 会调用它并解析；调用失败时报 `No data received from retrieveUserQuota`）。
- 官方文档侧只给出套餐对比入口（`geminicli.com/plans/`）与「配额与定价」页；Free tier / AI Pro / Ultra 的额度差异未在本次抓取范围内 → 部分**未证实**。

### 8.6 开源实现
- [AIClient-2-API](https://github.com/justlovemaki/AIClient-2-API)：`gemini-oauth.js`（本地回调 OAuth + 写凭据文件）、`gemini-core.js`、`antigravity-core.js`（含按 tier 判定模型是否退役的逻辑）。
- 官方 [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli)：认证方式的权威说明，也是 `oauth_creds.json` 读写逻辑的原始出处（本次通过其 docs 与第三方实现的常量互证）。

---

## 9. Qwen Code（阿里 / Qwen）

### 9.1 登录方式：**标准 RFC 8628 设备码 + PKCE S256**（本次证据最硬的一家，因为源码公开）
来自 Qwen 官方开源仓库 `QwenLM/qwen-code` 的 `packages/core/src/qwen/qwenOAuth2.ts`（**[官方源码]**）：
```ts
const QWEN_OAUTH_BASE_URL = 'https://chat.qwen.ai';
const QWEN_OAUTH_DEVICE_CODE_ENDPOINT = `${QWEN_OAUTH_BASE_URL}/api/v1/oauth2/device/code`;
const QWEN_OAUTH_TOKEN_ENDPOINT       = `${QWEN_OAUTH_BASE_URL}/api/v1/oauth2/token`;
const QWEN_OAUTH_CLIENT_ID = 'f0304373b74a44d2b584a3fb70ca9e56';
const QWEN_OAUTH_SCOPE = 'openid profile email model.completion';
const QWEN_OAUTH_GRANT_TYPE = 'urn:ietf:params:oauth:grant-type:device_code';
const QWEN_CREDENTIAL_FILENAME = 'oauth_creds.json';
```
- 设备码请求带 `code_challenge` + `code_challenge_method: 'S256'`（PKCE），轮询时提交 `code_verifier`；`expires_in` 决定轮询上限，`slow_down` 时轮询间隔从 2s 逐步增长到最多 10s。
- 错误语义：`400 + authorization_pending` → 继续轮询；`429 + slow_down` → 降频；其他 400（`access_denied`/`expired_token`）→ 抛 `QwenOAuthPollError`。
- 有浏览器时自动打开 `verification_uri_complete`，无浏览器/非交互时打印 URL（支持 OSC 8 超链接）。
- 同一 client_id/端点/scope 在 [AIClient-2-API `src/auth/qwen-oauth.js`](https://github.com/justlovemaki/AIClient-2-API) 里**逐字一致**（两份独立实现互证）。
- 另注：源码注释显示 CLI 具备 `qwen serve` 守护进程 + 「device-flow registry」的 HTTP 路由来持久化凭据（对应 PR #4255 / #4175），也就是说**官方自己在做「让别人也能走设备流拿 token」的通道**。

### 9.2 凭据落盘位置（Windows）
- `%USERPROFILE%\.qwen\oauth_creds.json`（`path.join(Storage.getGlobalQwenDir(), 'oauth_creds.json')`）。
- 写入特性（源码级，值得抄）：
  - 权限 `0o600`（`QWEN_CREDENTIAL_FILE_MODE = 0o600`），注释说明「与 opencode 的 `auth.json` 一致，避免共享主机上被其他用户读取」；
  - **先写临时文件（创建时即 0o600）→ 再原子 `rename` 覆盖**，避免「已存在但权限更宽的旧文件」出现明文可读窗口；Windows 上 `chmod` 无效则记 WARN 并依赖 NTFS ACL；
  - 写完清 `SharedTokenManager` 内存缓存，避免同进程继续用旧（常为空）缓存而重复触发设备授权。

### 9.3 token 结构
```ts
interface QwenCredentials {
  access_token?, refresh_token?, id_token?,
  expiry_date?,       // = now + expires_in*1000
  token_type?, resource_url?
}
```
- **`resource_url` 是关键**：token 响应里返回它，作为该账号的 API 基址（说明「用哪个域名/区域」是随账号下发的）。
- 刷新：`grant_type=refresh_token` + `refresh_token` + `client_id`；30s 超时；**400/401 即认为 refresh_token 失效 → 删除凭据文件**并抛 `CredentialsClearRequiredError`（要求用户重新 `/auth`）；可能返回新的 refresh_token（旋转）。

### 9.4 调用接口
- 具体 chat 端点本次**未证实**；但 `resource_url` 字段 + scope `model.completion` 表明调用基址由 token 响应下发（**强证据表明是「按账号下发的 API 基址」而非固定域名**）。
- AIClient-2-API 的 `src/providers/openai/qwen-core.js`（51KB）是现成实现，本次未逐行核对端点名 → **[未证实]**。

### 9.5 额度
- 官方文档层面：Qwen Code 有免费额度/每日额度概念，具体接口未在本次证据范围内 → **[未证实]**。

### 9.6 开源实现
- [AIClient-2-API](https://github.com/justlovemaki/AIClient-2-API)（`openai-qwen-oauth` provider + 设备流 + 凭据文件 + 自动入池）。
- 官方 `qwen-code`：设备流与凭据文件格式的权威来源。

---

## 10. Claude Code（Anthropic）

### 10.1 登录方式
- OAuth 登录（`claude login` 类流程；官方文档指向「Logging in to your Claude account」与 Claude Code 的 Authentication 页）。
- OAuth **client id**：`9d1c250a-e61b-44d9-88ed-5944d1962f5e`（[OpenTokenUsage/claude.md](https://github.com/poweruserz/OpenTokenUsage) **[社区逆向]**）。
- refresh scope 串（同一来源）：`user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload`。

### 10.2 凭据落盘位置（Windows）
- 文件：`%USERPROFILE%\.claude\.credentials.json`
  ```jsonc
  {
    "claudeAiOauth": {
      "accessToken": "<jwt>",
      "refreshToken": "<token>",
      "expiresAt": 1738300000000,       // unix ms
      "scopes": ["..."],
      "subscriptionType": "pro",
      "rateLimitTier": "..."
    }
  }
  ```
- macOS 上优先用 **Keychain**，服务名 `Claude Code-credentials`；设置 `CLAUDE_CONFIG_DIR` 时会用派生服务名 `Claude Code-credentials-<sha256(dir) 前8位>`；文件是**兜底**（「可能由旧版本 Claude Code 留下」）。
- **Windows 上是否走凭据管理器、还是只用 `.credentials.json`，本次未证实** → 标记不确定。相关证据是第三方 issue 讨论「OAuth refresh 会重写 `~/.claude/.credentials.json` 并丢掉 `subscriptionType`，导致 Claude Code 降级为 API-key 模式」（[NousResearch/hermes-agent#83338](https://github.com/NousResearch/hermes-agent/issues/83338)）——至少说明**该文件在刷新时会被重写，且必须保留 `subscriptionType`**，这是实现时必须小心的坑。

### 10.3 token 结构与刷新
```
POST https://platform.claude.com/v1/oauth/token
Content-Type: application/json
{
  "grant_type": "refresh_token",
  "refresh_token": "<rt>",
  "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
  "scope": "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
}
→ { "access_token": "...", "refresh_token": "...", "expires_in": 3600 }
```
- access token 是短命 JWT，**到期前 5 分钟主动刷新**，或在 401/403 时被动刷新。

### 10.4 调用接口
- `https://api.anthropic.com/v1/messages`，`Authorization: Bearer <oauth access token>`，并且**必须**带 `anthropic-beta: oauth-2025-04-20`（[OpenTokenUsage/claude.md](https://github.com/poweruserz/OpenTokenUsage)）。
- 用量端点：`GET https://api.anthropic.com/api/oauth/usage`，头同上。
- 客户端指纹：Claude Code 会带 `user-agent: claude-cli/<version>`、`x-app: cli`、`X-Stainless-*`、`metadata.user_id`（含哈希账号 uuid + `session_id`）等 —— **本次未取得一份逐项列出这些头的可复现来源** → **[未证实]**（AIClient-2-API 的 claude provider 走的是 API key 而非 OAuth，亦未提供该证据）。

### 10.5 额度
`GET /api/oauth/usage` 返回（**[社区逆向]**，字段含注释）：
```jsonc
{
  "five_hour": { "utilization": 25, "resets_at": "..." },      // 5 小时滚动窗口 %
  "seven_day": { "utilization": 40, "resets_at": "..." },      // 7 天窗口 %
  "seven_day_opus": { ... },        // 可选，按套餐
  "seven_day_omelette": { ... },    // 可选
  "extra_usage": { "is_enabled": true, "used_credits": 500, "monthly_limit": 10000, "currency": "USD" }
}
```
所有窗口**同时生效**，命中任一即限流。

### 10.6 开源实现
- [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router)（本地已克隆）：把 Claude Code 的请求路由到任意上游；**它是「用别的上游喂 Claude Code」，不是「把 Claude 订阅喂给别人」**。
- [cc-switch](https://github.com/farion1231/cc-switch)（本地已克隆）：配置切换器。
- [decolua/9router](https://github.com/decolua/9router)：OAuth 上游网关，其 issue #291 是本次最重要的**风险讨论**来源（见 §13.5）。
- [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)（本次克隆，`--depth 1`，模块 `v8`）：`internal/auth/` 下有 `anthropic_auth.go`、`anthropic.go`、`oauth_server.go`、`pkce.go`、`refresh_patch.go` 等，即**把 Anthropic OAuth 作为一等凭据类型**实现。
- [AIClient-2-API](https://github.com/justlovemaki/AIClient-2-API)：`src/providers/claude/` 走 `claude-kiro-oauth`（**Kiro 上游冒充 Claude 协议出口**）与 `claude-core`（直连 Anthropic，用 API key）。

---

## 11. `justlovemaki/AIClient-2-API` 深读（必读参考）

- 克隆：`.scratch/model-gateway/vendor/AIClient-2-API`（`git clone --depth 1`，HEAD `03d90e9`，2026-10-11，版本号 3.5.8，GPLv3）
- 代码规模：`src/auth/*`（OAuth） + `src/providers/*`（上游实现） + `src/providers/provider-pool-manager.js`（**约 12 万字节的账号池实现**） + `src/utils/rate-tracker.js`。

### 11.1 它支持哪些「订阅制上游」（provider 一览）
| provider | 凭据类型 | OAuth/设备流实现 |
|---|---|---|
| `claude-kiro-oauth` | Kiro OAuth | `src/auth/kiro-oauth.js`（48.7KB） |
| Codex | OpenAI OAuth | `src/auth/codex-oauth.js`（44.3KB，本地回调端口 **1455**，client id `app_EMoamEEZ73f0CkXaXp7hrann`，`auth.openai.com/oauth/authorize`，scopes `openid email profile offline_access`；账号 id 取自 JWT 声明 `https://api.openai.com/auth`.chatgpt_account_id） |
| Gemini / Antigravity | Google OAuth | `src/auth/gemini-oauth.js` |
| Qwen | 设备流 + PKCE | `src/auth/qwen-oauth.js` |
| Grok CLI | xAI OIDC | `src/auth/grok-cli-oauth.js`（discovery `https://auth.x.ai/.well-known/openid-configuration`，client id `b1a00492-073a-47ea-816f-4c329264a828`，回调 `http://127.0.0.1:56121/callback`，scope `openid profile email offline_access grok-cli:access api:access`） |
| Grok Web | **Cookie/SSO** | `src/auth/grok-auth.js`（`GROK_COOKIE_TOKEN` + `GROK_USER_AGENT`，README 提醒 UA 必须与取 cookie 时的浏览器一致，否则被拦） |
| iFlow | OAuth + Basic 认证 | `src/auth/iflow-oauth.js`（client id `10009311001` + secret） |
| GitHub Copilot | **细粒度 PAT**（非 OAuth） | `src/providers/github/github-copilot-core.js` |
| Forward | 自定义转发 | `src/providers/forward/` |

另外 `src/utils/tls-sidecar.js` + `tls-sidecar/` 目录：README FAQ #13 说明 Grok 类上游 **403 常因 TLS 指纹被挡**，需要编译并启用 TLS sidecar 来「bypass 403/Cloudflare」。这是「指纹校验确实存在」的又一个旁证（[README](https://github.com/justlovemaki/AIClient-2-API)）。

### 11.2 它怎么存账号
- 一个账号 = **一个凭据文件**，放在 `configs/<providerDir>/<timestamp>_oauth_creds.json`（OAuth 生成的默认路径），或用户/其它工具生成的既有路径：
  | 服务 | 默认路径（README 表） |
  |---|---|
  | Kiro | `~/.aws/sso/cache/kiro-auth-token.json` |
  | Antigravity | `~/.antigravity/oauth_creds.json` |
  | Codex | `~/.codex/oauth_creds.json` |
  | Grok CLI | `configs/grok-cli/..._xai-..._oauth_creds.json` |
  并注明 `~` 在 Windows 即 `C:\Users\<用户名>`。
- 池配置：`configs/provider_pools.json`（`PROVIDER_POOLS_FILE_PATH`，默认 `configs/provider_pools.json`，可用 `--provider-pools-file` 覆盖）。每个节点的字段（[provider_pools.json.example](https://github.com/justlovemaki/AIClient-2-API)）：
  ```
  customName, <该 provider 的凭据字段，如 OPENAI_API_KEY/OPENAI_BASE_URL>,
  checkModelName, checkHealth, notSupportedModels,
  uuid, isHealthy, isDisabled,
  lastUsed, usageCount, errorCount, lastErrorTime
  ```
  池是「按 provider type 分组的节点数组」，还支持 `concurrencyLimit`、`queueLimit`。
- 凭据来源有三种（README + UI 模块）：Web UI 点「Generate Authorization」走内置 OAuth；上传/放置现成凭据文件后「Quick Associate」一键绑定；`autoLinkProviderConfigs()` 在 OAuth 成功后自动把新凭据挂进对应 Pool（`onlyCurrentCred` 模式）。

### 11.3 它怎么刷新
- `ProviderPoolManager` 里刷新相关的完整机制：`checkAndRefreshExpiringNodes()`（周期性检查并刷新临近过期节点）、`warmupNodes()`（预热）、`refreshNode(providerType, uuid, force)`、`_refreshNodeToken()`、`_enqueueRefresh()` + `_flushRefreshBuffer()`（**刷新请求合并/缓冲**，避免同一节点被并发刷新）、`_awaitRefreshWithTimeout()`、`_calculateNodeScore()`/`_doSelectProvider()` 里的 `needsRefresh` 过滤、`resetProviderRefreshStatus()`。
- 只有**被载入活动池**的 provider 才会被后台刷新（FAQ #15）：没勾选「Preload Model Providers」的 provider 不会初始化，后台刷新任务也不会处理它的 token → **长期不用会静默过期，直到某次请求失败**。
- 刷新失败/过期后的恢复靠健康检查：`performInitialHealthChecks()` / `performHealthChecks()` / `_checkProviderHealth()` / `_buildHealthCheckRequests()` + `_checkAndRecoverScheduledProviders()`（到点自动把节点从不可用恢复为候选）。

### 11.4 它怎么轮换（账号池选择算法，源码级）
`_doSelectProvider()` → `_calculateNodeScore()`（`src/providers/provider-pool-manager.js`）：
1. 过滤：`isHealthy && !isDisabled && !needsRefresh`；若指定了模型，再用 `notSupportedModels`/`supportedModels` 过滤（Antigravity 还会按账号 tier 判断该模型是否对该账号可用）。
2. 打分（越小越优先）：
   - 不健康/禁用 → `1e18`（垫底）；
   - 并发满且队列满 → `1e17`；
   - 并发满但可排队 → `1e15 + waitingCount*1e10`；
   - 否则 `baseScore + usageScore + sequenceScore + loadScore + freshBonus`：
     - `baseScore` = 刚做过健康检查（<60s）的「新鲜」节点用固定负偏移 `-1e14`，否则用 `lastUsed` 时间戳（**LRU**）；
     - `usageScore = usageCount * 10000`（用过越多越靠后）；
     - `sequenceScore = min(相对序列号,100) * 1000`（打破平局，保证轮询）；
     - `loadScore = activeCount * 5000`；
     - 新鲜节点的 `freshBonus`。
3. 选中后写入 `lastUsed`，递增 `_lastSelectionSeq`（**毫秒级并发下也不会两次选到同一节点**），`usageCount++`，`_debouncedSave()` 防抖落盘。
4. 失败处理：`markProviderUnhealthy/Immediately/WithRecoveryTime`（带恢复时间的临时下线）、`markProviderNeedRefresh`、`markProviderHealthy`、`enableProvider/disableProvider`、`resetAllHealthInType`、`refreshProviderUuid`。
5. 跨类型降级：`providerFallbackChain` / `getFallbackChain()` / `selectProviderWithFallback()` / `acquireSlotWithFallback()`（FAQ #12、#5 都推荐用它 + 池解决 429）。
6. 429 冷却：`RATE_LIMIT_COOLDOWN_ENABLED` + `RATE_LIMIT_COOLDOWN_MS` —— 被限流的账号**暂时离开池并自动恢复**（FAQ #5，这是解决「免费额度/频控」的官方建议做法）。

### 11.5 它的免责声明（原文）
> 「This project (AIClient2API) is for learning and research purposes only. Users assume all risks…」「…is an API proxy tool and does not provide any AI model services…Users should comply with the terms of service and policies of each third-party service…」「This project runs locally and does not collect or upload any user data…」「Users should comply with the laws and regulations of their country/region…strictly prohibited to use this project for any illegal purposes.」

---

## 12. 其它「把订阅/CLI 转成 API」的开源实现（横向）

| 项目 | 上游 | 凭据获取方式 | 账号池 / 轮换 | 备注 |
|---|---|---|---|---|
| [jwadow/kiro-gateway](https://github.com/jwadow/kiro-gateway) | Kiro | **三种**：凭据文件路径 / kiro-cli SQLite / **直接粘贴 refresh_token**；还支持「给一个目录，扫描成多个账号」 | **有**，429/402 自动故障转移 + 连续失败临时停用 + 定期探测恢复 | 本次调研中账号池设计最完整；AGPL-3.0 |
| [Zhengyuuuui/workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api) | CodeBuddy(国内) / WorkBuddy(海外) | 内置浏览器 SSO 登录（`cli-external-link`）；`--device-token` 注入；session 文件 | 多账号隔离（未做自动轮换证据） | 指纹模拟、脱敏、压缩、熔断、限速 |
| [EchoPing07/Buddy-2API-Go](https://github.com/EchoPing07/Buddy-2API-Go) | CodeBuddy / WorkBuddy | 管理后台 OAuth 设备流 + 手动导入 JWT | **明确拒绝号池**（单账号） | Go 单二进制 + Web 面板 |
| [shuishuipingan/qoder2api-hub](https://github.com/shuishuipingan/qoder2api-hub) | Qoder 双区 | 设备流 / PAT / **只读扫描本机 auth 文件** | **有**：多账号、稳定派生设备指纹、账号级冷却换号、定时保活 | 见 §1.6 |
| [Liki4/qodercli2api](https://github.com/Liki4/qodercli2api) | Qoder | OAuth | 未见证据 | 被 qoder2api-hub 列为协议逆向参考 |
| [devilardis/auto-checkin](https://github.com/devilardis/auto-checkin) | Trae / WorkBuddy | **本机客户端凭据提取**（含解密）+ Cookie 导入 | 多账号（按 user_id 去重） | 只做签到，不是 API 网关 |
| [chaogei/windsurf-account-manager-simple](https://github.com/chaogei/windsurf-account-manager-simple) | Windsurf / Devin | 邮箱密码登录 + refresh_token 导入 | **有**（10 万级）+ 一键换号 | 含积分重置/虚拟卡等滥用功能 |
| [ericc-ch/copilot-api](https://github.com/ericc-ch/copilot-api) | GitHub Copilot | 设备流 OAuth / `--github-token` | 无（限速 + 人工审批） | 星标最高（4147） |
| [AIClient-2-API](https://github.com/justlovemaki/AIClient-2-API) | 9 类（Kiro/Codex/Gemini/Antigravity/Qwen/Grok/iFlow/Copilot/Forward） | 内置 OAuth + 凭据文件 + 自动挂池 | **有**（通用 Provider Pool，见 §11） | 覆盖最广 |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) | Anthropic / Codex / Gemini-Antigravity / xAI / Kimi / Vertex / Devin / Meta | 内置 OAuth（`internal/auth/*_auth.go` + `oauth_server.go` + `pkce.go`） | 未见证据（本次未细读） | `v8` 模块；v6 曾有 `auth/codebuddy` 包 |
| [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) | 反向：把 Claude Code 接到任意上游 | 配置 api key | — | 与「订阅转 API」方向相反的参考 |
| [9router](https://github.com/decolua/9router) | 多家 OAuth 上游 | OAuth | — | 其 issue #291 是风险证据来源 |
| U-Pool（[U-C4N/U-Pool](https://github.com/U-C4N/U-Pool)） | 通用账号池 | — | 有池设计文档（本次仅见设计文档命中，未细读） | 建议后续单列调研 |
| new-api / one-api（本地已克隆） | 通用渠道池 | API key/渠道 | **有**（渠道轮换、权重、禁用、重试） | 本次**未细读**；作为「池的通用工程范式」参考 |

---

## 13. 风险：服务条款性质与现实风控

### 13.1 结论先行
这类做法在**几乎所有**目标服务的条款下都属于**被明确禁止或至少高风险的「未授权访问 / 转售 / 凭据中间人」行为**：区别只在于「条款有没有写死」和「厂商有没有在查」。
- **条款写得最死、且明确说了会主动执法**：Anthropic、Google。
- **靠设备指纹/UA 强校验间接执法**：腾讯 CodeBuddy/WorkBuddy（`x-device-token` 连坐）、Qoder（机器身份）、Windsurf（DPAPI + 机器 ID + 座位数）。
- **条款有相关禁止项但未点名**：Cursor、GitHub Copilot。
- **未取得条款原文**：Trae、Kiro/AWS、Windsurf（见 13.6）。

### 13.2 Anthropic（Claude Code / Claude 订阅）—— 最明确的一条
**官方 Claude Code 法律页**「Authentication and credential use」原文（[code.claude.com/docs/en/legal-and-compliance](https://code.claude.com/docs/en/legal-and-compliance)）：

> "**OAuth authentication** is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications."
>
> "**Developers** building products or services that interact with Claude's capabilities, including those using the **Agent SDK**, should use API key authentication through Claude Console or a supported cloud provider. **Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens — sign-in to a Claude account must complete through Anthropic's own flow.**"
>
> "Anthropic reserves the right to take measures to enforce these restrictions and **may do so without prior notice**."

同一页「Can customers offer Claude Code in their products?」：

> "**The Claude Code binary must not be modified.** … customers may not remove, disable, or restrict any authentication method built into it"
> "**Customers may not pay for, resell, or intermediate Claude usage on their end users' behalf.**"

**消费者条款**（[anthropic.com/legal/consumer-terms](https://www.anthropic.com/legal/consumer-terms)，2025-10-08 生效）§2 与 §3 的操作用语：
> §2 "**You may not share your Account login information, Anthropic API key, or Account credentials with anyone else. You also may not make your Account available to anyone else.**"
> §3(2) "To develop any products or services that compete with our Services, including … **or resell the Services**."
> §3(3) "To **decompile, reverse engineer**, disassemble…"
> §3(7) "**Except when you are accessing our Services via an Anthropic API Key or where we otherwise explicitly permit it, to access the Services through automated or non-human means, whether through a bot, script, or otherwise.**"
> §12 "We may suspend or terminate your access to the Services (**including any Subscriptions**) at any time without notice to you if we believe that you have breached these Terms… **you will not be entitled to any refund.**"

**解读（我的判断，非条款原文）**：把一个 Pro/Max 订阅的 OAuth token 放进自建网关、供本机或他人以「非 Claude Code 客户端」形态调用，正落在 §3(7) + 「may not … intermediate Claude.ai credentials or session tokens」上。若这个网关只服务**本人**且**仍走 Claude Code 客户端**（例如 claude-code-router 那种「Claude Code → 别的上游」方向），风险性质完全不同——那是把 Claude Code 当开放 agent runtime 用，Anthropic 没有禁止。

**2026-02 的措辞收紧**：社区在 9router issue #291 里反复引用一段更新的措辞：
> "Using OAuth tokens obtained through Claude Free, Pro, or Max accounts in any other product, tool, or service — including the Agent SDK — is not permitted and constitutes a violation of the Consumer Terms of Service."

该段在我抓取的 `legal-and-compliance` 页中**没有逐字出现**（该页的现行表述见上），因此我把它归为 **[社区转述，未能在一手页面逐字定位]**。其**实质内容与一手页面一致**。

### 13.3 Google / Gemini
[Gemini API Additional Terms of Service](https://ai.google.dev/gemini-api/terms)（2026-03-23 生效）：
> 「**Use Restrictions**」：You may not use the Services to develop models that compete with the Services… You also may not attempt to **reverse engineer, extract or replicate any component of the Services**, including the underlying data or models.
> 「In addition to the "API Prohibitions" section in the [Google APIs Terms of Service], you must comply with our Prohibited Use Policy…」
> 「**Unpaid Services**… Google uses the content you submit… including **human reviewers may read, annotate, and process your API input and output**… Do not submit sensitive, confidential, or personal information to the Unpaid Services.」
> 「Use of Google AI Studio and Gemini API is for developers building with Google AI models for professional or business purposes, **not for consumer use**.」

**解读**：把补贴额度/订阅额度经第三方网关转出去，与「API Prohibitions」中的**转售/未授权访问**类禁止直接冲突；同时**免费额度下的数据会被用于训练并可能被人工审阅**——把 DeepSeek/其他私人代码经这条链路走，是实打实的数据外泄风险。另外，用免费额度做「商业转售」在条款里是明确不可的。

### 13.4 GitHub Copilot
- 官方条款路径：[GitHub Terms for Additional Products and Features](https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features#github-copilot) 明确 **Copilot 的适用条款分流**：
  > 「For GitHub Copilot Business and Copilot Enterprise license holders who purchase directly from GitHub, your use of GitHub Copilot is governed by the **GitHub Generative AI Services Terms**… For all other GitHub Copilot users, your use of GitHub Copilot is governed by **Section J (AI Features) of the GitHub Terms of Service**.」
  即：**没有一条 Copilot 专项条款写「禁止代理」**，禁令来自母条款 + 可接受使用政策。
- 可接受使用政策相关条：`github-acceptable-use-policies#4-spam-and-inauthentic-activity-on-github`（**禁止把服务器用于过度的自动化批量活动或给基础设施带来不当负担**）——copilot-api README 自己引的就是这一条。
- **最具体的一手风险提示来自反代项目自己的 README**（[ericc-ch/copilot-api](https://github.com/ericc-ch/copilot-api) 顶部 Security Notice）：
  > "**Excessive automated or scripted use of Copilot (including rapid or bulk requests, such as via automated tools) may trigger GitHub's abuse-detection systems.** You may receive a warning from GitHub Security, and further anomalous activity could result in **temporary suspension of your Copilot access.**"
  证据等级：**[社区实践共识 / 项目作者自述]**，不是 GitHub 官方公告。

### 13.5 Cursor
[Cursor Terms of Service](https://cursor.com/terms-of-service)（2026-09-03 版）§1.5 **Use Restrictions** 原文（节选）：
> "you may not: (i) **reverse engineer, disassemble, decompile, decode, or otherwise attempt to derive or gain access to the source code, object code or underlying structure of the Service**; … (iii) **rent, lease, lend, or sell the Service**; … (v) use the Service or any Suggestions to develop or train a model that is competitive with the Service, or engage in model extraction or theft attacks; (vi) probe, scan or attempt to penetrate the Service; (viii) harvest, scrape, or extract data from the Service; … (xi) **knowingly permit any third party to do any of the foregoing.**"
> §9 Termination「We reserve the right to modify, suspend, or discontinue the Services or your access to the Services… **If we terminate your access to the Service due to a violation of these Terms, you will not be entitled to any refund.**」

**解读**：把订阅当成上游 API 转出去，最直接的命中点是 §1.5(iii)「rent, lease, lend, or sell the Service」与 (xi) 的「允许第三方为之」。**反向（Cursor 自己的 token 被读出来做用量面板，如 OpenTokenUsage）**则主要落在 (i)/(viii) 的灰色区。

### 13.6 其它厂商（未取得条款原文）
| 厂商 | 状态 |
|---|---|
| **Windsurf / Cognition** | **未证实**：`windsurf.com/terms-of-service` 已 404，页脚指向 `/terms-of-service-individual`，而该 URL 发生**跨域跳转**到 `devin.ai`，本次未能跟随取回原文。Behavioral 证据是反代/换号工具自己写了免责声明。 |
| **Kiro / AWS** | **未证实**：未取得 AWS Service Terms 中与「凭据共享/代理」相关的原文段落；kiro-gateway 只写了「use at your own risk and in compliance with the terms of service of the underlying APIs」。AIClient-2-API README 另有一句提示「Kiro service usage policy has been updated, please visit the official website」但不含条文。 |
| **Trae / 字节跳动** | **未证实**：未找到 ToS 原文；社区项目免责声明自认「接口为逆向自官方客户端/网页的非公开实现」。 |
| **Qoder / 阿里** | **未证实**（ToS 原文）：但官方文档层面有**行为性约束的旁证** —— 新人权益「每个用户限一次，额外注册的试用账号会被冻结」、每日活动「按人去重」、新人 300 Credits「虚拟机不参与」，均来自社区对官方活动规则的转述与实测（[qoder2api-hub](https://github.com/shuishuipingan/qoder2api-hub)）；对应的一手入口是 Qoder 的官方活动/定价文档，本次未逐条抓取。 |
| **CodeBuddy / WorkBuddy 腾讯** | **未证实**（ToS 原文）：未取得腾讯条款条文。**但拿到了社区一手风控描述**：国内版 `x-device-token` 与账号是「设备 + 账号」绑定，「多个账号共用同一个 token 会被平台做**设备关联**，触发**连坐封号**——每台设备只能用自己的 token」「请勿分享自己的国内 token，也请勿使用他人分享的 token——被关联的是一整台设备，换号也救不回来」（[workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api)）。 |

### 13.7 现实风控与封号证据（逐条列证据等级）
| 日期 | 服务 | 事件 | 来源 | 等级 |
|---|---|---|---|---|
| 2026-03-12 → 2026-07-22 | Anthropic | 仓库 9router 用户问「用 Claude Code 订阅做代理被封概率」；维护者答：**「So far, the only reports I've heard are from users getting banned by **Antigravity** — I haven't seen similar reports from other providers yet.」**；另有贡献者指出 Anthropic 2026-02 收紧措辞，且**技术上可检测**（厂商同时控制客户端与推理端点）；建议「**低风险**：Claude Code 当客户端、路由到别家模型；**高风险**：把 Claude OAuth 插进别的 agent runtime」 | [9router#291](https://github.com/decolua/9router/issues/291) | **[社区一手讨论]**（含维护者与贡献者发言） |
| 2026-02-20 | Anthropic | 媒体标题级报道「Anthropic officially bans third-party subscription authentication」「Anthropic Bans OAuth Tokens from Consumer Plans in Third-Party Tools」 | [gigazine](https://gigazine.net/gsc_news/en/20260220-anthropic-third-party-block/)、[thedroidguy](https://thedroidguy.com/anthropic-bans-oauth-tokens-from-consumer-plans-in-third-party-tools-1271021) | **[第三方报道]**（但我已用一手 legal 页核实其实质内容） |
| 持续 | Trae | 「**Cookie 导入提示被风控拦截**：复制的是 DevTools → Network 里请求头的完整 Cookie（包含 ttwid 等），且网页版需处于登录状态」 | [auto-checkin](https://github.com/devilardis/auto-checkin) 常见问题 | **[社区实践]** |
| 持续 | Tencent CodeBuddy(国内) | 共用 `x-device-token` → 设备关联 → **连坐封号** | [workbuddy2api](https://github.com/Zhengyuuuui/workbuddy2api) | **[社区实践]** |
| 持续 | Qoder | 发全套**派生**机器头 → 活动列表被**静默过滤**（逐头隔离实测）；同机多账号按人/设备去重；「额外注册的试用账号会被冻结」 | [qoder2api-hub](https://github.com/shuishuipingan/qoder2api-hub)、[auto-checkin](https://github.com/devilardis/auto-checkin)（WorkBuddy 部分） | **[社区实践，有对照实验]** |
| 持续 | GitHub Copilot | 自动化批量请求 → GitHub Security 警告 → 进一步异常 → **Copilot 访问临时封禁** | [copilot-api README](https://github.com/ericc-ch/copilot-api) | **[项目作者自述]** |
| 持续 | Claude Code（配置层面） | OAuth refresh 重写 `.credentials.json` 时**丢掉 `subscriptionType`**，导致客户端降级为 API-key 模式（账号功能受损，非封号） | [hermes-agent#83338](https://github.com/NousResearch/hermes-agent/issues/83338) + [#83366](https://github.com/NousResearch/hermes-agent/pull/83366)（修复 PR） | **[社区 bug 报告]** |

**注意「未证实」的部分**：我**没有**找到 Clint/Claude 官方发布的「因代理封号」公告，也没有找到可核实的时间-账号对应案例；9router 维护者的说法是「只听说 Antigravity 有封号报告」（**Antigravity = Google 的新 IDE，且其凭据来自 Google OAuth**，与 §13.3 的 Google 侧执法吻合，而非 Anthropic 侧）。

---

## 14. 结论：自建网关支持这类上游的三种最小可行凭据获取方式

先把「一个上游适配器」拆成四件事，三种方式只是**第 1 件事的不同实现**：
1. **拿凭据**（三种方式之别）
2. **刷新**（各服务不同：refresh_token / 设备流重登 / 无官方续期）
3. **指纹与协议**（UA、x-* 头、签名、protobuf/自定义编码）
4. **池化**（选择、冷却、健康检查、轮换）

### 方式 A：复用本机已登录文件（读文件/读 SQLite/读 keychain）
- **适用且成本低**（多为明文 JSON）：Kiro（`~/.aws/sso/cache/kiro-auth-token.json`）、Qwen（`~/.qwen/oauth_creds.json`）、Gemini/Antigravity（`~/.gemini|.antigravity/oauth_creds.json`）、Claude Code（`~/.claude/.credentials.json`）、Cursor（`state.vscdb` 两个 key，需 SQLite 读）、Windsurf（`state.vscdb` 的 `windsurfAuthStatus` 明文 API Key）。
- **成本中高、脆弱**（加密/进程内存）：Trae（自定义 `tc` 格式 + 硬编码 salt）、Qoder CLI（AES-128-CBC，key 取 `machine_id` 前 16 字符）、Qoder 桌面端（Chromium os_crypt + DPAPI）、WorkBuddy 新版（AES-256-GCM，密钥**只在运行中的客户端进程内存**里 → 必须扫内存或要求客户端处于运行态）、Windsurf 的加密会话层（DPAPI）。
- **稳定性**：
  - 明文文件类：**高**（只要上游不改路径/字段）；第三方项目已证明可长期跑（AIClient-2-API / kiro-gateway 都主推这条）。
  - 加密/内存类：**低到中**，强耦合客户端版本，客户端一升级就可能全线失效（auto-checkin 明说「新版客户端会把 token 加密存储」，WorkBuddy「官方无续期接口」）。
  - 共同风险：**刷新时会被官方客户端与网关互相覆盖**（Claude 的 `.credentials.json` 丢字段问题就是实例），需要「以官方客户端写入的为准」或「写回时保留所有字段」。
- **实现成本**：单服务 1–3 天（明文）／3–10 天（加密，含调试）。
- **合规味道最轻**（只读本机自己的凭据、不改客户端），但**若把它接到「非官方客户端」上调用，13 节的条款问题依旧存在**。

### 方式 B：内嵌登录流程（网关自己跑 OAuth / 设备流 / 浏览器 SSO）
- **适用**（都有公开或可复原的流程）：
  | 服务 | 流程 | 已知回调/端点 |
  |---|---|---|
  | Qwen | RFC 8628 设备码 + PKCE | `chat.qwen.ai/api/v1/oauth2/device/code`、`/token`，client_id 已在官方源码 |
  | Qoder | 设备流 PKCE S256（+ 官方 PAT 另一条路） | 双区 URL 参数不同；PAT 在 `qoder.com/account/integrations` |
  | CodeBuddy / WorkBuddy | 浏览器 SSO `cli-external-link` → 轮询 token | `copilot.tencent.com/login?platform=VSCode&state=`、`www.workbuddy.ai/login?platform=workbuddy-ai&state=` |
  | Gemini / Antigravity | Google OAuth（本地回调，动态端口） | 两个 client_id 已知 |
  | Codex | 授权码 + 本地回调 | 端口 **1455**，client id `app_EMoamEEZ73f0CkXaXp7hrann` |
  | Grok CLI | OIDC discovery + 本地回调 | 端口 **56121**，issuer `auth.x.ai` |
  | Kiro | Social Auth 本地回调 **19876–19880** / Builder ID 设备码 | `oidc.{region}.amazonaws.com` |
  | Copilot | GitHub OAuth 设备流 / `gh auth login` | `copilot_internal/v2/token` |
  | iFlow | OAuth + Basic | `iflow.cn/oauth`，client id `10009311001` |
- **稳定性**：
  - **拿到的凭据寿命最长**（refresh_token 常为 30 天~1 年，如 Qoder `drt-` ≈ 1 年、Kiro refreshToken 长期），且**不依赖本机是否装了客户端**——这是它相对方式 A 的最大优势。
  - 但对**客户端指纹强的服务**，光有 token 不够：Copilot 要编辑器身份头、Qoder 要 COSY 签名 + 机器头、腾讯要 UA + `x-device-token`、Gemini 要 CLI UA —— 这些是**协议实现成本**，与凭据获取难度无关。
  - 脆弱点：client_id/scope/端点随时会被官方改；设备流可能被上游以「非官方客户端」为由拒绝（Qoder 用**派生**机器头时活动列表被静默过滤，就是这类）。
- **实现成本**：单服务 2–5 天（纯 OAuth）+ 协议实现成本另计（Qoder/Tencent/Codex 属高）。
- **UX 最好**（用户在网关里点一下就走完），也最接近官方行为。

### 方式 C：手动粘贴 refresh token / PAT / 会话 JWT
- **适用**：Kiro（kiro-gateway 的 `{"type":"refresh_token"}` 是现成范式；AIClient-2-API 有批量导入）、Qoder（官方 PAT `pt-`，**这是唯一被官方文档承认的可编程凭据**）、CodeBuddy/WorkBuddy（JWT 导入）、Trae（`refreshToken` + `user_id`）、Cursor（`refreshToken`）、Qwen/Gemini（`refresh_token`）。
- **不适用/最差**：WorkBuddy 新版（token 只存客户端进程内存，且**官方无续期接口**）、Trae（tc 加密）、Windsurf 加密层。
- **稳定性**：
  - **手填一次即可长期用**的前提是「能自动刷新」——凡是 refresh_token 可轮换的服务（Kiro/Qoder/Qwen/Cursor/Claude），稳定性≈方式 A/B；凡是**不能自动刷新**的（WorkBuddy），稳定性**低**（token 过期即断，必须人工重来）。
  - 必须处理 **refresh token 轮换**（新 refresh_token 会替换旧值，必须落盘，否则下次刷新失败）。
- **实现成本**：**最低**（一次粘贴 + 一次刷新调用 + 落盘），1 天内可跑通。
- **运维成本**：账号越多越痛；建议提供「批量粘贴」入口（Kiro 的 `batchImportKiroRefreshTokens` 就是现成设计）。
- **额外优势**：**这才是官方愿意给你用的路**——Qoder 的 PAT 是官方文档写明、可在账户页自助生成的；把 PAT 作为首选，把「复用本机文件/内嵌登录」作为 PAT 不存在时的兜底，能让网关里「最容易解释其合法性」的那条路占主导。

### 选型建议（按「先做哪个」排序）
1. **先做方式 C（PAT/refresh token 粘贴）+ 通用池**：实现成本最低，覆盖 Qoder(PAT)、Kiro、Codex、Qwen、Gemini、Copilot(细粒度 PAT) 一大片；接口上把它设计成「凭据提供者接口」的第一实现，后面两种方式只是同一个接口的另一个实现。
2. **再做方式 B（内嵌设备流）**：优先给「PAT 不存在」的服务（Kiro / Codex / Qwen / Gemini / Copilot / Qoder 设备流）；回调端口需要可配置（1455、56121、19876–19880 都已被占用风险）。
3. **方式 A 作为「一键导入本机登录态」的加分项**，且**只读、需用户二次确认**（qoder2api-hub 的「只读探测 + 两步确认，永不静默采用」是正确姿态）；**不要依赖加密类**（Trae/WorkBuddy 新版/桌面端 Qoder）作为生产路径。
4. **池化设计**：直接照 `AIClient-2-API` 的 Provider Pool（节点健康/禁用/`lastUsed` LRU/`usageCount`/`errorCount`/序列号防并发重选/`concurrencyLimit`/429 冷却/跨类型 fallback）+ `kiro-gateway` 的故障转移（429/402 换号、连续失败临时停用、定期探测恢复）。**每账号独立派生设备身份**（qoder2api-hub 的 `derive_id`），**绝不跨账号共用 device token**（腾讯的连坐封号就是反例）。
5. **合规边界（必须写进产品）**：Anthropic 与 Google 明文禁止把订阅凭据用于第三方产品/中间人；因此**默认不内置 Claude/Google 订阅凭据通道**，或把它做成显式、默认关闭、带原文提示的选项；反过来，「Claude Code 客户端 + 非 Anthropic 上游」是安全的（§13.2 解读）。
6. **数据面风险**（常被忽略）：Google **免费额度**下的输入输出会被用于训练且可能被人工审阅；Windsurf 的 API Key 在客户端明文存储。网关如果做日志/留存，必须默认不记正文（`Buddy-2API-Go` 就明确「只存元信息，不存对话内容」）。

---

## 15. 未证实清单

| 条目 | 状态 |
|---|---|
| Qoder CLI 官方 OAuth 的具体端点、client_id、PKCE 参数、token 名称（`dt-`/`drt-`/`pt-`） | **未证实（官方层面）**：终端用户文档只说浏览器登录 + 后台自动刷新；上述细节全部来自社区逆向 |
| Qoder 桌面端 `auth.v1.dat` 的 Windows 精确路径与 `com.qoder.app.stable` / `com.qoder.cn.app.stable` 命名 | **未证实**：来自 [qoder2api-hub] 单一来源的社区描述 |
| WorkBuddy 是否为「腾讯海外版 CodeBuddy」的官方定性 | **未证实**：官方站点标题（codebuddy.cn 托管 WorkBuddy 文档）+ 同一后端域名/协议/客户端目录名是**强旁证**，但没有一句官方声明 |
| CodeBuddy / WorkBuddy 的中继接口（`cli-external-link`、`/v2/plugin/`、`/v3/config`、`x-device-token` 规则） | **未证实（官方层面）**：全部来自社区逆向项目，且各项目互相引用 |
| 腾讯 CodeBuddy / WorkBuddy 的服务条款原文（是否明文禁止转售/代理） | **未证实**：未取回条款文本 |
| Trae 的 chat/completions 类上游端点、是否校验客户端指纹 | **未证实**：未找到 Trae 的 API 网关类开源项目；只有签到/额度类 |
| Trae 网页版 Cookie→JWT 的具体端点路径 | **未证实**：社区脚本只给出 base URL 与响应字段名 |
| Kiro 推理协议的精确 service/operation 名（`AmazonCodeWhispererStreamingService` / `generateAssistantResponse` / `SendMessageStreaming`） | **未证实**：只有「运行时是 AWS CodeWhisperer/Amazon Q（`q.{region}.amazonaws.com`）」这层得到印证 |
| Kiro 在 Windows 上 `state.vscdb` / `profile.json` 的精确路径 | **推断**：文档原文给的是 macOS 的 `~/Library/Application Support/Kiro/...` |
| Cursor 的 chat 推理端点、`x-cursor-checksum` 等指纹头的算法与被校验事实 | **未证实**：只证实了 `aiserver.v1.DashboardService` 的用量/套餐 Connect-RPC 端点 |
| Cursor 在 Windows 的 `state.vscdb` 精确路径与是否也用 keychain | **推断/未证实**：唯一来源给的是 macOS 路径 |
| Windsurf 的 Cascade/chat 推理端点；Windsurf 现行 ToS 原文（`/terms-of-service` 已 404，`/terms-of-service-individual` 跨域跳到 devin.ai） | **未证实** |
| GitHub Copilot 的 `Copilot-Integration-Id` / `openai-intent` / `x-initiator` / `machineid` 头与 `%LOCALAPPDATA%\github-copilot\hosts.json` | **未证实**：本次未取得证据；已证实的是 Editor-Version / Editor-Plugin-Version / User-Agent / X-Github-Api-Version 四条 |
| Copilot 的 API 调用请求**是否**强制校验同一批编辑器头（有对照实验的证据） | **未证实**：只有「额度查询接口要求这些头」的一手记录 |
| Gemini CLI 的 loopback 回调固定端口 | **未证实**：官方文档只描述「打开浏览器登录」，未给端口；社区实现用动态端口 |
| Gemini/Antigravity 的刷新端点是否为 `oauth2.googleapis.com/token` | **未证实**：由第三方实现推断，未引用 Google 一手文档 |
| Gemini/Antigravity 的 `oauth_creds.json` 字段全集与权限加固方式 | **未证实**：AIClient-2-API 只给了目录/文件名；`0o600 + 临时文件 + 原子 rename` 是 **Qwen** 源码的做法（同一命名约定，可能同源，但未证实 gemini-cli 照做） |
| Qwen Code 的 chat/completions 端点与额度接口 | **未证实**：只证实 `resource_url` 由 token 响应下发 |
| Claude Code 在 Windows 上是否用凭据管理器（还是只用 `.credentials.json`） | **未证实**：文件路径与字段来自社区，macOS 走 Keychain 有明确来源 |
| Claude Code 请求是否强校验 `X-Stainless-*` / `user-agent: claude-cli/<ver>` / `metadata.user_id` | **未证实**：本次未取得逐项列出这些头的可复现来源；已证实的是 `anthropic-beta: oauth-2025-04-20` |
| 「Anthropic 2026-02 更新措辞为『Using OAuth tokens obtained through Claude Free, Pro, or Max accounts in any other product, tool, or service — including the Agent SDK — is not permitted』」 | **未逐字定位到一手页面**：社区多处引用；现行一手页面用词为「Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens」（实质一致） |
| AWS / 字节 / 阿里的服务条款中与「凭据转售/代理」相关的具体条文 | **未证实**：未取回条款原文 |
| CLIProxyAPI 的 `internal/auth/codebuddy` 包：v6 存在（pkg.go.dev 链接），但 v8 源码中已无该目录，**移除原因与时间未证实** |
| new-api / one-api / U-Pool 的池实现细节 | **未细读**：本地有 new-api / one-api 克隆，但本次只作为「通用渠道池」提及 |
| 「Trae 积分/额度可被外部读取」的接口细节 | **未证实**：只有第三方 VS Code 扩展（TraeCN Quota）存在这一旁证 |

---

## 16. 来源清单

**官方（厂商自己发布）**
- Qoder CLI 登录与认证：<https://docs.qoder.com/cli/authentication.md>
- Qoder CLI 用量与额度：<https://docs.qoder.com/cli/usage.md>
- Qoder 文档总索引：<https://docs.qoder.com/llms.txt>
- Claude Code 法律与合规（含 Authentication and credential use）：<https://code.claude.com/docs/en/legal-and-compliance>
- Anthropic 消费者条款：<https://www.anthropic.com/legal/consumer-terms>
- Anthropic 商业条款：<https://www.anthropic.com/legal/commercial-terms>
- Anthropic 使用政策：<https://www.anthropic.com/legal/aup>
- Google Gemini API 附加条款：<https://ai.google.dev/gemini-api/terms>
- Google APIs Terms of Service：<https://developers.google.com/terms>
- GitHub 附加产品条款（Copilot 分流）：<https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features>
- GitHub 可接受使用政策 §4：<https://docs.github.com/en/site-policy/acceptable-use-policies/github-acceptable-use-policies>
- Cursor 服务条款：<https://cursor.com/terms-of-service>
- Cursor 可接受使用政策：<https://cursor.com/acceptable-use-policy>
- Gemini CLI 认证文档（源码）：<https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx>
- Qwen Code OAuth 源码：<https://github.com/QwenLM/qwen-code/blob/main/packages/core/src/qwen/qwenOAuth2.ts>
- Kiro 官网：<https://kiro.dev/>
- Tencent CodeBuddy / WorkBuddy 文档入口：<https://www.codebuddy.cn/docs/enterprise/>

**社区逆向 / 第三方实现（存在性证据与机制细节）**
- justlovemaki/AIClient-2-API：<https://github.com/justlovemaki/AIClient-2-API>（本地克隆 `.scratch/model-gateway/vendor/AIClient-2-API`，HEAD `03d90e9`，v3.5.8）
- jwadow/kiro-gateway：<https://github.com/jwadow/kiro-gateway>
- Zhengyuuuui/workbuddy2api：<https://github.com/Zhengyuuuui/workbuddy2api>
- EchoPing07/Buddy-2API-Go：<https://github.com/EchoPing07/Buddy-2API-Go>
- Sliverkiss/CodeBuddy2api：<https://github.com/Sliverkiss/CodeBuddy2api>；wicm84266964/Buddy2api：<https://github.com/wicm84266964/Buddy2api>；cyl2361341082-alt/Buddy2api：<https://github.com/cyl2361341082-alt/Buddy2api>；ShouZhuo0413/codebuddy2api：<https://github.com/ShouZhuo0413/codebuddy2api>
- shuishuipingan/qoder2api-hub：<https://github.com/shuishuipingan/qoder2api-hub>
- Liki4/qodercli2api：<https://github.com/Liki4/qodercli2api>；mmqz/cpa-multi-plugins：<https://github.com/mmqz/cpa-multi-plugins>
- zxcrf/opencode-qoder-auth（npm）：<https://registry.npmjs.org/opencode-qoder-auth>
- devilardis/auto-checkin：<https://github.com/devilardis/auto-checkin>
- chaogei/windsurf-account-manager-simple：<https://github.com/chaogei/windsurf-account-manager-simple>
- poweruserz/OpenTokenUsage（各 provider 凭据/端点参考）：<https://github.com/poweruserz/OpenTokenUsage>（`docs/providers/{cursor,claude,copilot,kiro,codex,antigravity,grok,...}.md`）
- ericc-ch/copilot-api：<https://github.com/ericc-ch/copilot-api>；caozhiyuan/copilot-api：<https://github.com/caozhiyuan/copilot-api>；StarryKira/copilot2api-go：<https://github.com/StarryKira/copilot2api-go>；QLHazyCoder/copilot-api：<https://github.com/QLHazyCoder/copilot-api>；suhaibbinyounis/github-copilot-api-vscode：<https://github.com/suhaibbinyounis/github-copilot-api-vscode>
- router-for-me/CLIProxyAPI：<https://github.com/router-for-me/CLIProxyAPI>（本地克隆，`v8` 模块；v6 的 codebuddy 包见 <https://pkg.go.dev/github.com/router-for-me/CLIProxyAPI/v6/internal/auth/codebuddy>）
- decolua/9router issue #291（封号风险讨论）：<https://github.com/decolua/9router/issues/291>
- NousResearch/hermes-agent#83338（`.credentials.json` 刷新丢 `subscriptionType`）：<https://github.com/NousResearch/hermes-agent/issues/83338>
- kuops/opencode-codebuddy-auth：<https://github.com/kuops/opencode-codebuddy-auth>；fu827707013/dsh-codebuddy-cli：<https://github.com/fu827707013/dsh-codebuddy-cli>
- PyPI workbuddy2api：<https://pypi.org/project/workbuddy2api/>
- U-C4N/U-Pool（账号池设计文档）：<https://github.com/U-C4N/U-Pool>

**第三方报道**
- gigazine（2026-02-20）：<https://gigazine.net/gsc_news/en/20260220-anthropic-third-party-block/>
- The Droid Guy：<https://thedroidguy.com/anthropic-bans-oauth-tokens-from-consumer-plans-in-third-party-tools-1271021>
- 腾讯云开发者社区（WorkBuddy 产品介绍）：<https://cloud.tencent.com.cn/developer/article/2760498>

**本次新增克隆（供后续细读）**
- `.scratch/model-gateway/vendor/CLIProxyAPI`（`git clone --depth 1`）
- `.scratch/model-gateway/vendor/auto-checkin`（`git clone --depth 1`）
