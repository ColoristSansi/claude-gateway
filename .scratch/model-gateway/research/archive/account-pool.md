# 账号池 / 登录态管理：现有实现深挖

调研对象（均已克隆在 `.scratch/model-gateway/vendor/`）：

| 代号 | 仓库 | 语言 | 有没有真正的"多账号凭据池" |
|---|---|---|---|
| **codearts** | `deepseek-harness-codearts` | TypeScript（DSH 插件，13 家订阅渠道） | ✅ 最完整，`AccountPool` 2250 行 |
| **dsh-free** | `dsh-our-free-model/vendor/channel-pack` | TypeScript（同族，前身） | ✅ 同款设计的较小版本（1345 行） |
| **Astra** | `Astra-Gate` | C#/.NET | ✅ SQLite 账号池 + 三种切换模式 + 额度探针 |
| **zen-gate** | `zen-gate` | Go | ⚠️ 池里轮换的是**出口 IP 与模型**，不是账号 |
| **cc-switch** | `cc-switch` | Rust/Tauri | ✅ 每 provider 各自一个多账号 store（device code 登录） |
| **AIClient** | `AIClient-2-API` | Node.js | ⚠️ 多账号 = 扫目录下 `codex-<email>.json` |

---

## 1. 凭据从哪来

### codearts（最全）

凭据**不落插件自己的文件**，走宿主 DSH 的凭据库：`ctx.credentials.resolve(credentialRef(entry.credentialRef))`（`src/account-pool.ts:1576`、`1444`）。账号池条目只存 ref 名（`CODEARTS_ACCOUNT_A1B2C3D4` 形态，`src/types.ts:53-54`）。

登录流程（**浏览器 OAuth + 本地回调**，不是粘贴 token）：

- `client_id = 'codearts-agent'`（即其 URI scheme，取自官方 product.json），`REDIRECT_PATH = '/oauth/callback'`，token 端点是华为 STS `https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens`（`src/oauth.ts:6-16`）。
- 回调地址是**本机回环**：`http://127.0.0.1:${port}${REDIRECT_PATH}`（`src/oauth.ts:159`）。
- PKCE：verifier = `randomBytes(48).toString('base64url')`，challenge = S256（`src/oauth.ts:58-62`）。
- **还有 DPoP**：ES256(P-256) 密钥对，每次 token 请求签一个 `dpop+jwt`（htm/htu/iat/jti），放 `DPoP` 头（`src/oauth.ts:65-85`、`101-112`）。
- 两步式登录 `startLogin()` 立即返回 `loginUrl`，前端 `window.open`，后台 await 回调（`src/jet-hub-rpc.ts:1443-1480`；契约见 repo AGENTS.md「`ctx.xxxAuth` 服务统一接口」）。

同仓库里**不同 provider 的取证手段完全不同**，这是关键事实：

| 手段 | 证据 |
|---|---|
| 浏览器 OAuth + 127.0.0.1 回调 | `src/oauth.ts:159`；`src/gemini-oauth.ts`（loopback 端口，注释在 `channel-pack/src/gemini-oauth.ts:21`） |
| OAuth device flow（无回调） | `src/cline-oauth.ts`（WorkOS device code，`channel-pack/src/cline-oauth.ts:24-29,247`）；`src/minimax-oauth.ts` |
| 本地弹窗页承载二维码/表单 | `src/loomy-wechat-login.ts:11,175`（`http://127.0.0.1:${port}/wechat/qr`）；`src/raccoon-login-page.ts:11,141` |
| **cookie 登录（非 OAuth）** | 库库AI/百度 GenFlowPro：必须"自己起 chromium + CDP"才能拿回 `.baidu.com` 上的 **HttpOnly** cookie（`src/chromium-session.ts:1-8`） |
| **读本机客户端登录态文件** | OfficeAce：`~/.office-claw/session.cookie` 是 **DPAPI** 密文（`OC-DPAPI-1` 前缀），用 PowerShell `ProtectedData::Unprotect(...,"CurrentUser")` 解出 `{"value":...}`（`src/officeace-auth.ts:453-480`） |
| **复用本机客户端本地端口** | 同上，OfficeAce 本地 API `127.0.0.1:3004` + `cookie: oc_sid=<值>`（`src/officeace-auth.ts:954-970`） |
| 读 IDE 遥测文件派生设备指纹 | `~/.zcode/v2/telemetry-state.json`（`src/zcode.ts:15`） |

### Astra

账号 = SQLite 行；`source` 列明确区分四种来源：`login` / `local` / `token` / `batch`（`src/Astra.Core/Models/ProviderAccount.cs:28-38`）。

- **直接读本机 CLI 登录态**：Codex `~/.codex/auth.json`（`CODEX_HOME` 优先），有专门的"本机是否已有 codex login 登录态"探测端点（`src/Astra.Server/Api/SubscriptionEndpoints.cs:299-318`、`1086-1100`）。
- **粘贴**：按 provider 家族接受不同格式（`src/Astra.Providers/Subscription/CredentialImport.cs:50-60`）：
  - `claude-subscription` → `claude-refresh-token` / `claude-setup-token` / `claude-credentials-json`
  - `openai-subscription` → `codex-auth-json` / `refresh-token`
  - `grok-subscription` → `refresh-token` / **`grok-sso`（浏览器 SSO cookie）**
  - copilot → `github-token` / `copilot-token`
- **粘的是本机文件的内容也算一种**：`claude_credentials` = "Claude Code's `.credentials.json`（`claudeAiOauth` 对象）"（`CredentialImport.cs:23-24`），`codex_auth_json` = "Codex's `auth.json`"（`CredentialImport.cs:26-27`）。解析就认 `claudeAiOauth.accessToken / refreshToken`（`CredentialImport.cs:243-249`）。
- **cookie 不落库**：Grok SSO cookie 注释写明"converted to Grok OAuth tokens through a device-code approval, **never stored itself**"（`CredentialImport.cs:29-30`；设备码实现在 `src/Astra.Providers/Subscription/XaiSsoDeviceFlow.cs:76-190`）。

### cc-switch

- Codex/Copilot/xAI 均为 **OAuth device code**：`device_code` 请求 + `poll_for_token`（`src-tauri/src/commands/auth.rs:45-206`、`commands/copilot.rs:39-74`）。
- 同时也**读本机 CLI 登录态**：`~/.codex/auth.json` 里属于某 `account_id` 的 `refresh_token`（`src-tauri/src/codex_config.rs:626,708`），并支持 keyring 模式 `cli_auth_credentials_store = "keyring"`（`codex_config.rs:1149`）。
- 它把客户端配置**改写指向自己的本地代理**：`base_url = "http://127.0.0.1:15721/v1"`（`codex_config.rs:3063`）——即"本地端口复用"的另一种形态（抢客户端的 base_url，而不是抢它的 cookie）。

### zen-gate

- QoderCN/Lingma：**扫本机 IDE 缓存目录**导入登录态，`loadCredentialFile` / `importLingmaCacheCredential`（`internal/qoderhub/remote/credentials.go:63-94`、`192-205`），多候选目录 + auto/newest/longest 选择策略（`credentials.go:30-36`、`70-94`）。导入/导出走 AES（文件头 `crypto/aes`，`credentials.go:4`），落盘 `0600`、目录 `0700`（`credentials.go:119-129`）。
- WorkBuddy：上游 OAuth 的**纯轮询式登录**（没有回调）——`POST /v2/plugin/auth/state?platform=CLI` 拿 `{state, authUrl}`，用户在浏览器完成，然后用 state 轮询 `/v2/plugin/auth/token?state=`，`code 11217` = 尚未完成（`internal/lane/workbuddy_account.go:170-265`）。
- 它自己还会**起一个本地 sidecar 端口** `127.0.0.1:8095` 承载 Qoder 网关（`internal/qoderhub/hub.go:15,30`）。

### dsh-free

同一套家族手法：本地回调服务器 `createServer` + `127.0.0.1`（`channel-pack/src/login.ts:1-30,239`、`lobsterai-oauth.ts:256-348`）、device code（`cline-oauth.ts:24-29,247`）、本地二维码页（`loomy-wechat-login.ts:175`）。

### AIClient-2-API

本地回调端口各有硬编码：Codex `http://localhost:1455/auth/callback`（`src/auth/codex-oauth.js:20`，`createServer` 在 `283-287`）、Grok CLI `http://127.0.0.1:56121/callback`（`grok-cli-oauth.js:23`、`271-305`）、iFlow `http://localhost:${port}/oauth2callback`（`iflow-oauth.js:174`、`333-335`）、Kiro `127.0.0.1`（`kiro-oauth.js:597-706`）、Qwen 用 device code（`qwen-oauth.js:19,162`）。同时读本机 CLI 凭据文件：`~/.<dir>/oauth_creds.json`（`qwen-oauth.js:136`、`gemini-oauth.js:371`、`kiro-oauth.js:380`）。

**结论（Q1）**：六个仓库里，**没有一个只靠"用户手工粘贴 token"**。最少也提供浏览器 OAuth 登录；一半以上同时读本机 CLI/IDE 的登录态文件；两个（codearts 的 cookie 渠道、OfficeAce）必须起浏览器或解 DPAPI 才能拿到凭据。

---

## 2. 凭据格式、有效期与刷新

### codearts

持久化凭据形状（`src/oauth.ts:179-194`）：

```ts
{ access_key_id, secret_access_key, security_token,
  expires_at,            // = 响应里的 credentials.expiration（ISO 串）
  refresh_token, code_verifier, dpop_private_key_jwk }
```

即 **OAuth access（华为 STS 三元组）+ refresh_token + PKCE verifier + DPoP 私钥**——refresh 时必须原样带上后两者（`exchangeRefreshToken`，`src/oauth.ts:164-176`）。

- 续期判据用**凭据自己的 `exp`，不用池里的 `expiresAt`**：`shouldRefreshNow()` 复用 `REFRESH_LEAD_MS = 3_600_000`（距过期不足 1 小时才刷）（`src/refresh.ts:2`、`src/expiry-sync.ts:91`）。理由（写在同一处注释）：池值正是可能陈旧的那份数据，拿它当尺子会漏刷。
- **刷新失败的终态判定极窄**：只认 `error === 'invalid_grant'` 或 `error_code` 含 `ExpiredRefreshToken`（`src/oauth.ts:134-141`）。`InvalidDPoPHeader` **刻意不算终态**——它只是"这一次 proof 没过"（时钟偏差/重放/网关抖动），把它当终态会把材料完好的账号一步标死（同文件 `124-133` 的注释记录了这个真实缺陷）。
- 判终态**前先重读凭据**：并发下服务端烧掉的是旧的那份，磁盘上可能已躺着新的一份（repo AGENTS.md「判终态前先重读凭据」；实现见 `src/service.ts` 的 `CodeArtsAuth.refreshAll`）。
- **per-credentialRef 串行队列**：三条续期入口（30 分钟定时器 / 推理路径按需 / 面板刷新按钮）会在同一进程内并发消费同一份 refresh_token，必须串行 + 锁内重读。实现是 `refreshQueues = new Map<string, SerialQueue>()` + `queueFor(refName)`（`src/service.ts:143,158-162`；队列本体 `src/serial-queue.ts:67`；其他家同型：`src/gemini-auth.ts:108`、`src/accio-auth.ts:356`、`src/officeace-auth.ts:745-757`，后者还会在超过 64 条时淘汰最旧的队列）。⚠️ 该锁**只在单进程有效**——同机 web + desktop 两个 profile 会跨进程互踩，所以"重读"那条腿也必须留着（`src/service.ts:131-140`）。
- 边界：**pool 里的 `refreshable` 只是凭据材料的镜像，不能当续期门禁**（曾经把一次瞬时失败固化成单向门，账号从此永不自愈）。

### Astra

`access_token_enc` / `refresh_token_enc` / `expires_at_utc` / `last_refresh_at_utc`（`src/Astra.Data/Repositories/ProviderAccountStore.cs:13-15`）。刷新走 `OAuthClient`：`grant_type=refresh_token` + `client_id`（`src/Astra.Providers/Subscription/OAuthClient.cs:98-120`），401 时"强制刷新一次再重试"（`src/Astra.Gateway/Pipeline/UpstreamAuthResolver.cs:220-232`）。`RefreshLead` 决定提前量（`SubscriptionTokenService.cs:208`）。失败落 `status = expired`（refresh 失败）/ `revoked`（`invalid_grant`，grant 本身没了）（`src/Astra.Core/Models/ProviderAccount.cs:6-13`）。

### zen-gate

`WBAccount{AccessToken, RefreshToken, UID, Enabled, ExpiresAtMS}`（`internal/lane/workbuddy.go:78-79`）。过期时间**从 JWT 自己的 `exp` claim 解析**（`workbuddy_account.go:158-163`、`275-295`），不是靠响应字段。提前 **2 小时**主动刷（`wbRefreshEarly = 2 * time.Hour`，`workbuddy.go:62`）。刷新**单飞**：整个交换持 `wbMu`，后来者先复查 freshness（`workbuddy_account.go:104-116`）——原因写得很清楚：**上游每次刷新都换 refresh token，两次并发刷新会让第二对被作废、账号被登出**（`workbuddy_account.go:15-19`）。429 后 401 路径还给一次"刷新+重试"（`workbuddy_account.go:93-101`）。

### dsh-free

同 codearts 形状（`.credentials.yaml`，JSON 串原样携带，`channel-pack/src/buddy.ts:255`）。

---

## 3. 账号状态机

### codearts：**没有**可用/封禁枚举，状态是"一组可判定谓词 + 一张按模型的解禁时刻表"

账号条目字段（`src/types.ts:44-128`）：

| 字段 | 语义 |
|---|---|
| `enabled` | 用户开关；停用账号**绝不参与自动选号**（`account-pool.ts:1550`） |
| `expiresAt` | 展示用；注释明确警告它来自**用户可编辑的备份文件**、`sanitizeAccounts` 不过滤值域（`types.ts:57-75`） |
| `refreshable` | **只是凭据材料的镜像**，不是"曾被拒过"的案底（`types.ts:77-78`） |
| `modelRateLimits?: Record<modelId, resetAtMs>` | **唯一的状态载体**：每个模型的解禁时刻（`types.ts:79-87`） |
| `refreshError` / `source` | 展示与溯源（`types.ts:133-135`） |
| `traeCheckinDeviceGeneration` / `opencodeProxy` / `opencodeFingerprintGeneration` | 渠道专属的"轮换代次/出口代理"（`types.ts:88-127`） |

**没有** `banned` / `rate_limited` 这种状态字段；"当前是否可用"是每次现算的：

```
enabled && (modelId 为空 || !modelRateLimits[modelId] || Date.now() >= resetAt)
```
（`src/account-pool.ts:1552-1558`，与 `allAccountsRateLimitedForModel` 同判据 `69-83`）

**状态迁移** = 写/删一个 `(accountId, modelId) → resetAtMs` 标记：

- 撞限流 → 写标记：`updateModelRateLimit`（`account-pool.ts:1687-1789`）
- 到点自愈 → 删标记：`sweepExpiredRateLimits`（`account-pool.ts:1969-1984`），另有**懒过期**（选号时判 `Date.now() >= resetAt`）
- 手动"重测/重置" → `clearModelRateLimits`（`account-pool.ts:1456-1486`）

### 冷却时长：**不是固定值，是绝对时刻**，且来源分三档

| 来源 | 时长 | 证据 |
|---|---|---|
| 上游报文里能解析出真值 | 用真值（Cline 实测可达 19h39m，从人类可读文本解析） | `channel-pack/src/cline-rate-limit.ts:156-194` |
| 响应头 `Retry-After` | 用头里的值；**0 是合法值**（=立即解除），必须 `=== undefined` 判空 | `channel-pack/src/cline-rate-limit.ts:187-189`；同族 `retry-after.ts` |
| 按自然日结算的额度 | `nextUtc8DayStartMs()`（UTC+8 次日 0 点） | `zcode-adapter.ts:1610`、`qoder-adapter.ts:1037`、`account-pool.ts:1850` |
| 兜底 | provider 各自常数（如 cline 1 小时） | `channel-pack/src/llm-adapter.ts:2260-2307` |

两条**反直觉但重要**的规则：

1. **只接受更长的解禁时刻**（`current >= resetAtMs` 时不覆盖）：因为各家的"解析不出时长"兜底值通常**短于**真值，后写的兜底会覆盖真值 —— cline"倒计时结束了还是失败，又显示 60 分钟"就是这么无限循环的（`account-pool.ts:1673-1685`）。
2. **无效时刻必须直接拒绝**，不能当"很短"处理：`1e308` 这类超出 `Date` 范围的有限数写进去后，所有比较恒 false（模型被永久限流、sweep 也清不掉），并且下次会 `new Date(v).toISOString()` 抛 `RangeError`。判据统一走 `isValidResetAtMs`（`account-pool.ts:1696-1716`、`types.ts:82-86`）。

### 自适应 / 历史中位数估算

- **codearts：未找到**任何基于历史的中位数/自适应该冷却。它的自适应只有一个**排序**补偿：限额账号"沉底"（见 §4）。
- **zen-gate 有真正的中位数估算**：`RecoveryETA` = **已闭合 episode 时长的中位数**（需 ≥2 个样本）从本次 onset 起外推；样本不足时退回冷却边界（`internal/lane/throttle.go:179-205`）。episode 历史持久化（14 天 TTL、最多 12 条，`throttle.go:219-231`；落盘结构 `internal/store/store.go:191-206`）。冷却默认 **10 秒**（不是 60），理由是实测 92 条 episode 里最短 0.6 秒、且被拒请求不耗额度，"短冷却"是便宜的方向（`throttle.go:30-49`），**上限 2 小时**。
- **zen-gate 另有指数退避的 (exit, model) ban**：`banBaseLimited << min(n-1, 5)` 封顶，`region` 类**不长**（确定性判决，探也没用）（`internal/subs/ledger.go:46-80`）。
- **dsh-free 有 per-kind 自适应**：服务端给了就用（封顶 30 分钟），没给则 `auth` → 60s、`rate_limit` → 指数退避 `60s * 2^(n-1)`、`free_usage_limit` / 余额不足 → **按当日 24h**（钱不会在 60 秒内到账，短冷却会让空钱包账号被反复选中）（`channel-pack/src/opencode-adapter.ts:178-207`）。

### Astra：状态枚举 + 冷却时刻

`status ∈ {active, expired, revoked}`（`ProviderAccount.cs:6-13`），外加 `cooldown_until_utc`、`last_error`、`enabled`、`is_current`、`sort_order`（`ProviderAccount.cs:55-92`）。

可用判据：`enabled && status == Active && !(cooldownUntilUtc > now)`（`UpstreamAuthResolver.cs:82-83`）。

429 冷却时长优先读上游头：`anthropic-ratelimit-unified-reset`（unix 秒）→ 否则 `Retry-After` → 否则 **5 分钟**，并 clamp 到 **[1 分钟, 7 天]**（`src/Astra.Gateway/Pipeline/GatewayPipeline.cs:683-699`）。

---

## 4. 轮换与选择算法

### codearts：**"手动顺序优先 + 限流豁免"，不是轮询 / 不是最少使用 / 不是最快恢复**

`getAvailableAccount`（`account-pool.ts:1544-1604`）：过滤 → **取候选数组的第一个**，而候选数组顺序 = **用户在面板拖拽设定的手动顺序**。注释明确交代了历史上曾按"限流重置最早到期"重排、为什么废弃：那会让拖拽变成纯 UI 装饰（`1560-1569`）。

代价与补偿：默认情况下 **1 号账号扛全部流量**，撞限额后 2 号顶上，**1 号一解禁又因为排在前面被立刻捡回去**，后面的号额度白白过期（用户原话见 `account-pool.ts:129-141`）。补偿机制是 **"限额即沉底"**：`sinkAccountToProviderEnd` 把受限账号挪到**该 provider 的末尾并持久化**，使它在解禁后不再抢回首位（`account-pool.ts:159-211`）。开关是环境变量 `DSH_RATE_LIMIT_SINK`，**默认开启**，只有显式写 `0/false/off/no` 才关（`account-pool.ts:93-112`）。

⚠️ 沉底函数的注释记录了一个 Critical 缺陷的形状：池里**允许**存在两条同 id 的账号（手工编辑的备份文件导入即可造出），此时按 `id` 过滤会一次滤掉两条 → 长度不变式被破坏 → 回填最后一个槽位写 `undefined` → 该 provider 全部推理请求崩、并且 `undefined` 经 JSON 序列化后被重启清理 ⇒ **账号永久丢失**。所以它按**对象引用**判等（`account-pool.ts:173-208`）。

**请求级轮换**：调用方拿 `excludeAccountIds` 传"已试过"的集合，池负责排除——因为池只回"它认为最优的一个"，调用方无法枚举候选自己去重；若失败类别**不写限流标记**（5xx / 请求错误，不该留徽章），刚失败的账号仍是排序第一，不排除就原地打转（`account-pool.ts:1498-1508`）。适配器侧的循环形态见 `qoder-adapter.ts:1018-1053`（`switchAccountOnQuota`）、`zcode-adapter.ts:1601-1623`。注释里还点名 Go 侧的对应物是 `PickExcluding(tried)`（`pool.go:131`）。

**粘性绑定：codearts 未找到。** 它的换号是**每次请求重新取第一个可用账号**，只有 `excludeAccountIds` 在单次请求内提供"别拿回同一个"的保证；没有任何 session/conversation → 账号的映射表。**这一点很重要**：上游的 prompt cache 是按账号的，跨请求换号会丢缓存，而 codearts 靠"手动顺序 + 沉底"让同一账号尽量长期稳定，而不是靠显式粘性键。

### Astra：三种模式，`balanced` 才是真正的调度器

- `manual` / `failover` / `balanced` 三档（`UpstreamAuthResolver.cs:129-147`）。
- `SelectAccount`（pinned → provider 当前账号 → 第一个可用）：`UpstreamAuthResolver.cs:91-96`。
- `balanced` 的 `AccountScheduler.Pick`（`src/Astra.Gateway/Pipeline/AccountScheduler.cs:60-110`）：
  1. 先按 **sticky key** 找已有绑定，绑定账号仍可用就直接复用；
  2. 否则按 **最少 in-flight** → **最近 5 分钟内被分配次数最少** → `SortOrder` → `CreatedAt` → `Id` 依次排序取第一个；
  3. 取号时**占一个 in-flight 名额**（`Lease`，`Dispose` 释放），失败路径用 `finally` 保证不泄漏名额（`GatewayPipeline.cs:582-586`）；
  4. 绑定 **1 小时 TTL**、上限 10 000 条、按 LRU 淘汰，**状态只在内存**（重启丢绑定，"代价只是上游一次 cache miss"）（`AccountScheduler.cs:14-25`、`142-160`）。

**粘性键是什么**（`src/Astra.Gateway/Pipeline/StickyKeys.cs:17-51`）：

| 协议 | 粘性键 |
|---|---|
| OpenAI Chat / Responses | body 里的 `prompt_cache_key`（前缀 `pck:`） |
| Anthropic | 头 `x-claude-code-session-id`；否则 body `metadata.user_id` 里的 `session_id`（兼容 CLI ≥2.1.78 的 JSON 形态与 legacy 字符串）；**否则** `sha256(system + "\n\0\n" + 第一条 user 文本)` 的指纹（前缀 `fp:`），两者都空则无键 |
| Gemini | 无粘性键 |

键超过 256 字符就先哈希（`StickyKeys.cs:27,138-141`），避免绑定表膨胀。

### zen-gate：换的是**出口**和**模型**，不是账号

`Complete` 的循环（`internal/lane/client.go:714-764`）：`egressRetryable` 且**尚未吐字**且轮换激活且预算够 → **同一模型换出口重试**（`egressTries`，注释：429 是"某一个出口的答复"，配额按出口 IP 计费，有钱的出口可能两跳之外）；否则若失败可换模型且未吐字 → `nextCandidate(needs, tried)` 换模型。`tried` 是 `map[string]bool` 记账（`720`、`761`）。WorkBuddy 那条渠道是**单账号**（`wbAcct` 一个快照，`workbuddy_account.go:24-28`），根本没有多账号池。

### AIClient-2-API：**文件即账号**

多账号 = 扫目录里的 `*.json`（`kiro-oauth.js:820`）并按邮箱命名 `codex-<email>.json`（`codex-oauth.js:558,604,637`）。没有池化选择逻辑，只有"哪个文件"。

---

## 5. 失败分类

### zen-gate（这一项它最完整，因为注释把"为什么"写清了）

两个**不同问题**的两张表：

```go
// 「同一个模型值不值得换一个出口再问一遍」
func egressRetryable(code string) bool {
    switch code {
    case CodeQuota, CodeTransport, CodeServer, CodeTimeout: return true
    }
    return false
}

// 「换一个模型能不能答上」
func switchableFailure(uerr *UpstreamError) bool {
    switch uerr.Code {
    case CodeQuota, CodeRegion, CodeServer, CodeTimeout, CodeEmpty, CodeTransport: return true
    }
    return false
}
```
（`internal/lane/client.go:787-806`）

`CodeAborted`（用户取消）两张表都**不在**里面（测试明确断言，`internal/lane/egress_retry_test.go:41`）。`egressBudgetFor` 给不同失败类别**不同的重试预算**：429 几乎不需要（`return 0`），慢失败需要一整个 attempt 的时间（`779-785`）。

### `sawAny` 闸门：**已经吐字之后，既不许换出口也不许换模型**

三处独立实现，语义一致：

- 引擎层：`if egressRetryable(...) && !sawAny && ...`（`client.go:734`）；`if !failover || sawAny || !switchableFailure(uerr) { return out, uerr }`（`client.go:745`）。`sawAny = result.SawText || result.SawToolCall || result.SawReasoning`（`client.go:1006`）。
- Anthropic 出口：`if uerr != nil && outcome.Finish == "" && !st.sawAny { ... }`（`internal/gateway/anthropic.go:269`），`st.sawAny` 在**任何** chunk 到达时置位（`anthropic.go:310-311`）。若 SSE 已经开始写出（`sse.flushed`），错误只能作为**流内 error 事件**下发，HTTP 200 已经提交（`anthropic.go:293-300`）。
- 通用 relay：同款判据（`internal/gateway/relay.go:299-307`）。

注释的理由：「No switch after content reached the caller (it would duplicate output)」（`client.go:742-744`）。

### codearts / dsh-free：按**错误码链 + 状态码**分类，且明确区分"该等"与"该换"

- 429 是 HTTP 语义上**唯一**的限流信号，判据**状态码优先**，文案/业务码只作兜底；且"非 429 绝不能因为有状态码就判为限流"（`channel-pack/src/llm-adapter.ts:2223-2242`）。空体 429 必须靠状态码判真（`2227-2233`）——否则"空体 429 在 CodeArts 上依旧不自愈"。
- **换账号 vs 换模型 vs 直接透传**在 Cline 那家被显式列表化（`channel-pack/src/cline-rate-limit.ts:213-221`）：

  | 形态 | 正确动作 |
  |---|---|
  | 402 `Insufficient credits` | 去充值（**等没用**） |
  | 429 `Daily free limit reached` | 等没用（按天结算）→ 改用**同一账号的另一个免费模型** |
  | 429 其它 | 稍后重试 / 换账号 |

- **"不写徽章 ≠ 不换号"**：402 仍然换号重试（别的账号可能有余额），但不写倒计时徽章——写徽章会让用户白等（`cline-rate-limit.ts:196-206`）。
- 业务码**必须按链判定而不是单值**：腾讯系"模型饱和" `14003` 恰好是 HTTP 429，但语义是"换模型"而不是"换账号"（`llm-adapter.ts:2108-2180`）。
- 流内错误的坑：额度耗尽码形如 `InferHub.4291.200`，历史上被 `429` 的**裸子串**匹配吃掉（`4291` 命中 `429` 前缀）→ 现在 `429` 锚定为独立数字 `(^|[^0-9])429([^0-9]|$)`（`llm-adapter.ts:454-487`）。

### Astra

只对两个信号换账号：**429**（写 cooldown + 换号，`GatewayPipeline.cs:564-573`）与**401/403 的凭证拒绝**（先强制刷新一次，刷新仍失败才换号，`549-559`）。没有"换模型"这一维（模型路由是另一套）。`FailOverAsync` 会写 cooldown，然后取"第一个未试过且可用"的账号；`balanced` 模式下**不动 provider 的 current 账号**（`UpstreamAuthResolver.cs:241-252`、`GatewayPipeline.cs:588-595`）。

---

## 6. 并发与限流

- **codearts：账号级并发上限 —— 未找到。** `AccountPool` 里没有任何 in-flight 计数或信号量；RPM/TPM 也不记账。它只做两件并发防护：(a) 状态文档落盘的**串行链** `storeChain`（`account-pool.ts:326-332`，因为 `store.save()` 异步、两个写者的 save 可以乱序完成），(b) 续期的 per-ref 串行队列。
- **Astra：有 in-flight 记账，无 RPM/TPM。** `_inflight` 字典 + `Lease`（`AccountScheduler.cs:31,43-53,104-108`）；`_recent` 是 5 分钟滑动窗口的分配次数（`134-140`）。这两个正是 `balanced` 的排序键。RPM/TPM 级的限制在别处：`TokenLimiter` 管的是**入站 key** 的配额并回 `Retry-After`（`GatewayPipeline.cs:386-387`、`Pipeline/TokenLimiter.cs:67-77`）。
- **zen-gate：没有账号级上限；"并发"的代理指标是出口/队列。** Qoder 侧有排队等待（`waitForQueue`，`qoder-adapter` 同族实现见 codearts `src/qoder-adapter.ts:1060`）。dsh 侧 ZCode 有**服务端指定的排队时长** `10605 retryAfterSeconds` 与延迟优先序（`channel-pack/src/model-queue.ts:18,242-251`）。
- **一句话**：所有被调研的池都是**"限流驱动"**的（撞了才退），没有一个是**"配额预算驱动"**的（提前按 RPM/TPM 分配）。

---

## 7. 存储与加密

### codearts / dsh-free：账号索引在 state.json，凭据在宿主的 .credentials.yaml —— **凭据不加密（明文 YAML）**

| 东西 | 位置 | 证据 |
|---|---|---|
| 账号索引（含 `modelRateLimits`） | `$DSH_HOME/jet-hub/state.json`（0.1.7+；更早是 `settings.yaml` 的 `jet-hub:` 段） | `src/jet-hub-store.ts:8,22,678,827` |
| 凭据本身 | `$DSH_HOME/.credentials.yaml`（DSH 凭据库，按 `refs:` 索引） | `src/jet-hub-store.ts:632,746`；`src/account-pool.ts:1576` |
| 原子写 | tmp + `rename`（同步 `writeFileSync` + `renameSync`） | `src/jet-hub-store.ts`；`channel-pack-store.ts:480-481` |
| 账号恢复 | state.json 丢失时**从 `.credentials.yaml` 的 `refs:` 段重建索引**（只取键名不读值）；需要一张**每 provider 手动登记的 ref 前缀表**，漏加 = 该 provider 账号静默消失 | `src/jet-hub-store.ts:537-545,581-594,632` |
| 独立文档 | `permanent-locks.json` / `ui-preferences.json` / `dead-models.json` / `token-ledger.json` / `gemini-sigs.json` 全部**单独一份** | `src/permanent-lock-store.ts:42-43`；`badge-preferences.ts:34`；`dead-model-store.ts:67`；`token-ledger-store.ts:58` |

**为什么必须拆文档（这是本仓库反复踩出来的最硬的一条约束）**：`state.json` 是 **dsh home 级、同机多 profile 共享**的文档，而它是**整体替换**语义 —— 另一条工作区里跑着**旧版本代码**时，它全量重写只会带上自己认识的键，于是它不认识的键**被静默抹掉**。`permanent-locks` 的后果是"真把永久积分烧掉"（`permanent-lock-store.ts:4-27`）。

**多进程/多实例共享：这是被明确承认的弱点**——同机 dsh web 与 desktop 两个 profile 各持自己的锁，**跨进程互踩只能靠"判终态前重读凭据"兜**（`src/service.ts:131-140`）。另有一条实测观察：账号池条目可能与凭据"不同命"（state.json 里条目消失而 `.credentials.yaml` 凭据完好，根因当时**未定位**，见 repo AGENTS.md）。

### Astra：SQLite + DataProtection 加密

`provider_accounts` 表列：`access_token_enc, refresh_token_enc, expires_at_utc, status, last_refresh_at_utc, extra_json, enabled, is_current, sort_order, cooldown_until_utc, last_error, kind, source, http_proxy`（`src/Astra.Data/Repositories/ProviderAccountStore.cs:13-15`）。加密走 `ISecretProtector`（DataProtection），**DTO 只暴露掩码**（`ProviderAccount.cs:43` 的注释：`Tokens are stored DataProtection-encrypted and never returned by the admin API`）。日志里凭据只留 scheme + 末 4 位（`GatewayPipeline.cs:597-621`）。SQLite WAL ⇒ 多实例共享是数据库层面的。`http_proxy` 是**账号级**的（`ProviderAccount.cs:88-92`）。

### zen-gate

配置 JSON 一份（`internal/store/store.go:92-166`），WorkBuddy 的两个 token 在导出时被 `MaskedValue` 掩掉（`store.go:641-645`）。导入的外部凭据文件写出时 `0600`、目录 `0700`、tmp+rename（`qoderhub/remote/credentials.go:119-129`）。节流台账持久化（`store.go:197-206`）。

### cc-switch

每 provider 一个账号 store（`CodexAccountData`，`codex_oauth_auth.rs:264-287`）；**只持久化 `refresh_token` / `id_token`**，access token 按需换取；另有 `token_updated_at_ms` 做"拒绝采纳更旧的一代 refresh token"的代次守卫（同处 `283-286`）。

---

## 8. 额度探测

**Astra 是唯一一家把"主动探测额度"做成一等服务的**（`src/Astra.Providers/Subscription/SubscriptionQuotaService.cs`）：

| 渠道 | 端点 | 证据 |
|---|---|---|
| Claude | `https://api.anthropic.com/api/oauth/usage` + `anthropic-beta: oauth-2025-04-20` | `SubscriptionQuotaService.cs:16,311` |
| Codex | `https://chatgpt.com/backend-api/codex/usage`，**失败换** `.../wham/usage` | `:18,326,339-340` |
| Grok | `cli-chat-proxy.grok.com/v1/billing?format=credits` / `/v1/user?include=subscription` | `:41-42` |
| Kimi | `https://api.kimi.com/coding/v1/usages` | `:378` |
| ZCode/Z.AI | `{host}/api/monitor/usage/quota/limit` | `:350-362` |
| Copilot | copilot-language-server 的 `quota_snapshots.{chat,premium_models,premium_interactions}` | `:196-256` |

- 归一化成 `{fetchedAtUtc, session:{usedPercent,resetsAtUtc}, weekly:{...}, planLabel, ...}` 存进 `ProviderAccount.Extra["quota"]`（`SubscriptionQuotaService.cs:12-19,90-91`）。
- **新鲜度**：`IsFresh()` 比 `FreshFor`（`59-63`）——UI 用旧快照 + 手动刷新，没有强制轮询定时器。
- **探针自身有恢复腿**：401/403 触发一次强制 token 刷新再重试（`:19-20,293-300`）。
- 明确**不探**的情形：Claude `setup-token` 账号（scope 不覆盖 usage 端点，探测会把 403 误读成"grant 已死"，`:53-57,73`）；没有探针的家族返回 null 让 UI 显示"暂不支持"，而不是猜字段名（`:84-85`）。
- 已知坑（注释里记的）：上游有些字段**会自相矛盾**——Kimi 的 `usages.limit_5h/limit_7d.used_ratio` 实测与权威 `usage` 冲突（周额度已耗尽却报 0），所以**刻意不读它**（`:589-593`）。

**codearts**：折算层会主动查余额并按模型写"额度已尽"标记（`markModelsExhaustedForExpiry`，`account-pool.ts:1835-1867`；zcode 的 `exhaustedModelsOf` 一次查询得出全部已尽模型）。这里有一个**必须记住的教训**：`modelRateLimits` 一张表承载**两种语义**——"撞墙"（适配器，线上事实）与"观测"（余额接口，可能滞后），两者写的键值完全相同，于是折算的清除逻辑会把适配器刚写的标记**无条件擦掉**，造成"同一个号被反复选中、反复撞墙、每次真烧一发请求"的死循环。修法是引入纯内存的 `expiryMarkedKeys` **来源登记**：折算只删自己登记的键，适配器一旦写过就"认领"它（`account-pool.ts:284-323`）。—— 这一条对任何"主动探测 + 被动撞墙"并存的池都成立。

**dsh-free**：有手动"重测"入口（`channel-pack/src/account-probe.ts`，注释直言"账号卡片上的限额重置只是一次 429 事件的快照"），以及 `scripts/verify-zcode-fight-single.mjs` 这类验证脚本。

**zen-gate**：免费通道**没有余额 API**，明确写着"quota is accounted per session upstream and only signals itself by refusing"，所以只有被动 429 + 探针轮次（`internal/lane/throttle.go:9-14`，`ProbeIntervalMinutes` 在 `internal/store/store.go:102`）。

---

## 9. 健康检查与恢复

三条**独立**的恢复腿（codearts 三样都有，这也是它最值得抄的部分）：

1. **懒过期 + 定时清扫**：选号时判 `Date.now() >= resetAt` 即视为可用（`account-pool.ts:1552-1558`），另有 `sweepExpiredRateLimits` 清已过期标记（`1969-1984`）。
2. **续期调度器**：`setInterval` **并且启动时先立刻跑一轮**——只有 `setInterval` 会让短寿命令牌（cline 1h / codearts ~2h / raccoon 3h）在宿主关闭期间到期、重启后最长 30 分钟显示过期（`src/refresh-scheduler.ts:93-102`；`src/index.ts:2238-2290`）。判据用**凭据自己的 exp**（`expiry-sync.ts:91`）。续期的"不要刷"分支**必须仍做一次有效期对账**，否则存量账号的池值永远无人更正。
3. **成功流量即证据**：zen-gate 把它写成了语义——"A served request is proof: clear this exit's cooldown and the ban booked against this very (exit, model) pair"（`internal/lane/client.go:1035-1040` → `internal/subs/ledger.go:82-110`）。且**只清那一个 pair**：节点级冷却（传输状态，与模型无关）可以整个清掉，但 (exit, model) 的 ban 不能连带清——"model-B 成功不能给 model-A 的墙重新上膛"（`ledger.go:86-91`）。429 的 throttle 由 `MarkOK` 关闭并闭合 episode（`throttle.go:113-140`）。

**判定"恢复可用"的方式**：没有一家做主动健康探测来解除冷却（那会白花额度）。统一是"**时间到 + 下次真实请求成功**"双重证据；zen-gate 额外用探针轮次 + `RecoveryETA` 中位数仅作**展示/排序**。

**刷新失败的终态处理**（codearts 的教训最贵）：
- 终态范围必须**窄**（只 `invalid_grant` / `ExpiredRefreshToken`），`InvalidDPoPHeader` 这类"一次请求层面"的拒绝不能标死（`src/oauth.ts:124-141`）。
- 判终态前**重读凭据**（并发下别人可能已经续成功）。
- 真终态时日志带**服务端原文**，且同一 ref 同一原因**只告警一次**（`terminalWarned`）——否则每 30 分钟重复一行会把有用信息埋掉。
- 已知未修的残留：其余八个 provider 的 `refreshAll` 第一行仍是 `if (!entry.refreshable) continue`，同一个单向门还在（repo AGENTS.md 明说"本次只修了用户报障的 codearts"）。

---

## 10. 可抄的最小实现（接口草图 + 关键伪代码）

以下是把上述事实压缩后的**最小可用**抽象。刻意只保留有证据支撑的机制，砍掉每家渠道专属的东西。

```ts
// ---------- 1. 数据 ----------
type AccountId = string

interface Account {
  id: AccountId
  provider: string          // 'claude-sub' | 'codex-sub' | ...
  enabled: boolean          // 用户开关：停用绝不参与自动选号
  credentialRef: string     // 凭据本体不在池里，只在秘密存储里
  sortOrder: number         // 用户拖拽顺序 = 选号优先级（唯一顺序来源）

  // 状态：不设 status 枚举，用"可判定谓词 + 按模型的解禁时刻表"
  cooldown: Record<string /*modelId*/, number /*resetAtMs*/>
  lastError?: string
}

// 可用判据（唯一实现，选号/探测/展示三处共用）
const isUsable = (a: Account, modelId: string, now: number) =>
  a.enabled &&
  (modelId === '' || !a.cooldown[modelId] || now >= a.cooldown[modelId])  // 懒过期

// ---------- 2. 失败分类：两张表，别合并 ----------
type FailClass = 'quota' | 'auth' | 'region' | 'transport' | 'server' | 'aborted' | 'bad-request'

// 换出口：请求没有拿到答复，换条路白试一次
const retryOtherEgress = (c: FailClass) =>
  c === 'quota' || c === 'transport' || c === 'server' || c === 'timeout'

// 换账号：这个账号答不了，别的账号可能有余额
const switchAccount   = (c: FailClass) => c === 'quota' || c === 'auth'
// 注意：'aborted'（用户取消）永不重试；'quota' 里还要再分“当日额度”与“瞬时拥挤”

// ---------- 3. 冷却时长：绝对时刻，来源分档，只接受更长 ----------
function cooldownUntil(f: Failure, model: string, now: number): number {
  // ① 上游真值优先（响应头 / 报文里的绝对或相对时刻；0 是合法值=立即解除）
  if (f.retryAfterMs !== undefined) return now + min(f.retryAfterMs, CAP)
  // ② 按自然日结算的额度 → 次日 0 点
  if (f.dailyQuota) return nextUtc8DayStart(now)
  // ③ 余额耗尽 → 当日（不是 60 秒：钱不会在 60 秒内到账）
  if (f.outOfCredit) return now + 24h
  // ④ 兜底退避（指数 + 封顶）
  return now + min(BASE * 2 ** (f.attempt - 1), CAP)
}

async function markLimited(acc: AccountId, model: string, until: number) {
  if (!isValidInstant(until)) return           // 拒绝 1e308 之类：否则永不清除 + Date 抛 RangeError
  const cur = pool[acc].cooldown[model]
  if (isValidInstant(cur) && cur >= until) {/* 保留更长的，但仍要沉底 */} else {
    pool[acc].cooldown[model] = until
    claimSource(acc, model)                     // ★ 见下：标记来源登记
  }
  if (until > now) sinkToProviderEnd(acc)       // 解禁后不再抢回首位（唯一补偿“手动顺序”的手段）
  await persist()
}

// ---------- 4. 选号 ----------
async function pick(provider: string, model: string, tried: Set<AccountId>) {
  const cands = accounts
    .filter(a => a.provider === provider && isUsable(a, model, now))
    .filter(a => !tried.has(a.id))
    .sort((x, y) => x.sortOrder - y.sortOrder)   // 手动顺序即优先级（不要按“最早解禁”重排）
  // 若需吞吐而非“手动顺序”，改成 Astra 那三键：最少 in-flight → 5 分钟内最少分配 → sortOrder
  for (const a of cands) {
    const cred = await secrets.resolve(a.credentialRef)
    if (!cred) { log.warn('credential missing, skipping'); continue }   // 坏账号只跳过它
    return { account: a, credential: cred }
  }
  return null
}
```

**必须一起带的四条**（每条都对应一个真实缺陷）：

1. **标记来源登记**（codearts 的 `expiryMarkedKeys`）：如果池里既有"撞墙写标记"又有"主动探测写标记"，两张证据强弱不同而键值相同 → 必须登记来源，**探测路径只能删自己写的键**，撞墙路径一旦写过就"认领"它。否则会出现"同一个号被反复选中、反复撞墙、每次真烧一发请求"的死循环（`src/account-pool.ts:284-323`）。
2. **同一 (账号, 模型) 的标记只接受更长者**：所有解析路径的兜底值都偏短，后写的会把真值覆盖掉（`account-pool.ts:1673-1685`）。
3. **`sawAny` 闸门**：任何字节已经发给客户端之后，**不许换账号也不许换模型**（会重复输出）；此时错误只能作为流内 error 事件下发，HTTP 200 已经提交（zen-gate `client.go:734,745`，`anthropic.go:269,293-300`）。
4. **续期三件套**：per-ref 串行（上游常轮换 refresh token，并发刷新会互相作废）+ 判终态前重读凭据 + 启动立刻跑一轮（而不是只挂 `setInterval`）。**并且 `refreshable` 这类"材料镜像"字段绝不能当续期门禁**——一次瞬时失败会固化成单向门，账号从此永不自愈。

**存储最小形态**：两张表分开——账号索引（含 `cooldown`、`sortOrder`）与凭据本体（密文，带 `access+refresh+expiresAt`）。若多实例共享同一份文档，**必须**考虑"另一份旧代码整体重写该文档"的后果：不认识的键会被静默抹掉 ⇒ 需要独立文档或真正的数据库（这是 codearts 用五个独立 JSON 文件、Astra 用 SQLite 的原因）。

**粘性绑定**（只在需要蹭上游 prompt cache 时才加）：键取 `prompt_cache_key` / `x-claude-code-session-id` / `metadata.user_id.session_id`，都没有则 `sha256(system + 首条 user 文本)` 指纹；绑定 1h TTL + LRU 上限 + **内存态**（重启丢绑定，只损失一次 cache miss）；账号不可用时**重新绑定**而不是报错（Astra `StickyKeys.cs` + `AccountScheduler.cs`）。

---

## 简评：只靠"手动粘贴 token"能覆盖订阅制服务吗？

**不能覆盖全部，但在多数渠道上够用——而且没有任何一家被调研的实现真的只做粘贴。**

我看到的真实做法，按"必须多脏"分三档：

**第一档：粘贴就够（token 是自足的）**

- `claude setup-token`（`sk-ant-oat01-…`，长期有效的推理令牌，没有 refresh grant）——Astra 把它列为一等格式（`CredentialImport.cs:11-12,168-169`）。
- Coding Plan 的静态 API Key（Kimi `sk-kimi-…`、Z.AI `{id}.{secret}`）——Astra 的 `ApiKey` kind（`CredentialImport.cs:14-15,195-208`）。
- refresh token 本身（`sk-ant-ort01-…`，导入时**交换一次**再按登录态刷新，`CredentialImport.cs:8-9,166-167`）。

**第二档：粘贴"登录态文件的内容"才够（用户得先在本机 CLI 登录一次）**

- Claude Code 的 `~/.claude/.credentials.json`（`claudeAiOauth` 对象）——Astra 直接接受把它**粘贴进来**，也接受只粘里面那一层（`CredentialImport.cs:23-24,243-249`）。
- Codex 的 `~/.codex/auth.json` —— 粘贴（`CredentialImport.cs:26-27,225-230`），或**由网关自己去读该文件**（`SubscriptionEndpoints.cs:1086-1100`）。
- 这一档的本质是：**token 能用，但用户拿不到一个"可以复制的字符串"**，除非他先在本机客户端登录、然后去翻它的凭据文件。cc-switch、AIClient-2-API、zen-gate（Lingma 缓存目录）都选择了"我来替你读"（`codex_config.rs:626,708`；`codex-oauth.js:637`；`qoderhub/remote/credentials.go:63-94`）。

**第三档：粘贴根本不可能，必须浏览器 / 本地端口 / cookie / DPAPI**

1. **浏览器 OAuth + 本机回调**：codearts（PKCE + **DPoP**，`oauth.ts:58-85,159`）、gemini（loopback，`channel-pack/src/gemini-oauth.ts:21`）、lobsterai（`lobsterai-oauth.ts:95,348`）、AIClient 一堆（`codex-oauth.js:20` 的 `localhost:1455`、`grok-cli-oauth.js:23` 的 `127.0.0.1:56121`）。回调是 loopback 地址，**没有可粘贴的等价物**。
2. **device flow**：cline（WorkOS）、minimax、Astra 的 Grok、cc-switch 的 Codex/Copilot。好处是**不需要监听端口**，但用户仍必须在浏览器里点一次（`channel-pack/src/cline-oauth.ts:24-29,247`）。
3. **纯 cookie 登录（无 OAuth 可做）**：库库AI/百度 GenFlowPro —— 要拿 `.baidu.com` 上的 **HttpOnly** cookie，**只能自己起 chromium + CDP**（`src/chromium-session.ts:1-8`）。这类渠道粘贴 token 无从谈起。
4. **本机客户端会话文件 + 本地端口**：OfficeAce 的 `~/.office-claw/session.cookie` 是 **DPAPI** 密文（`officeace-auth.ts:453-480`），且只用于**只读的倍率查询**；它的数据还要走 `127.0.0.1:3004` 本地 API（`:970`）。更关键的是注释里那句：**本地 cookie 无法区分多账号**（它只反映"当前登录在果办里的那一个账号"）⇒ 多账号折算必须走云侧直读，否则两个账号会显示成同一份余额（`officeace-auth.ts:912-930`、`aggregate-expiry.ts:343,858`）。**这是"本地端口复用"与"多账号池"的正面冲突**：擦本机状态天然是单账号的。
5. **浏览器 SSO cookie → 换成 token**：Astra 的 Grok SSO 接受粘贴 `sso=…` cookie（或 `email----password----…----sso` 组合行），但**只是拿来跑一次 device-code 审批**，cookie 本身**从不入库**（`CredentialImport.cs:29-30,99-101,184-190`）。这是 cookie 与 token 之间最干净的一座桥。

**给自建网关的建议（依据我看到的做法）**：

- **登录优先，粘贴兜底**。六家里没有任何一家把粘贴做成唯一路径；Astra 的格式表（`FormatsOf`）是按 provider 家族分别声明"能不能粘贴 / 支持哪些粘贴格式"，这是最省事的设计：能不能粘贴是**渠道属性**，不是全局决策。
- **必须实现"读本机 CLI 登录态"**，至少对 Claude Code / Codex 这两家。它们的凭据文件路径稳定（`~/.claude/.credentials.json`、`~/.codex/auth.json` / `CODEX_HOME`），解析成本极低，且**已经有一套成熟的兼容层**（Astra 把两种形态都认了）。对用户来说这是最省事的一条路，且不涉及任何伪造客户端指纹。
- **多账号必须用"登录一次 → 存一份凭据 + 独立 credentialRef"的模型**，而不是"擦同一份本机状态"。所有能真正多账号的实现（codearts/Astra/cc-switch）都是这个形状；反过来，任何依赖"本机全局单 cookie / 单一 keyring 条目"的路径**天然只能表达一个账号**。
- **订阅渠道的"多账号"含义是"多个人/多份订阅"，不是"同一 token 多开"**。所以别再想"把 token 切成几份"——池的粒度天然是一个账号一份完整凭据。
- 若目标渠道里存在**纯 cookie 登录**的（百度系等），要么接受"起浏览器抓 cookie"这条重资产路线，要么**这个渠道就别进池**。两者的成本差一个数量级。

---

## 附：本次调研中的"未找到"清单（不猜）

| 问题 | 结论 |
|---|---|
| codearts 账号级并发上限 / 信号量 | **未找到** |
| codearts 的 RPM / TPM 记账 | **未找到** |
| codearts 的会话粘性绑定（session/conversation → 账号） | **未找到**（换号是每请求重选第一个可用账号；稳定性靠手动顺序 + 沉底） |
| codearts 基于历史中位数的自适应冷却 | **未找到**（自适应只体现在"更长者胜"与"沉底"） |
| codearts 中名为 `switchableFailure` / `egressRetryable` / `sawAny` 的符号 | **未找到**（这三个属于 zen-gate；codearts 的等价物是按错误码链分类 + 请求级 `excludeAccountIds`） |
| codearts 凭据的加密 | **未找到**（凭据由宿主 DSH 凭据库存为 `.credentials.yaml`，插件侧未见加密；Astra 才是加密的） |
| zen-gate 订阅渠道的多账号池 | **未找到**（WorkBuddy/QoderHub 都是单账号；池化的是**出口 IP** 与**模型**） |
| zen-gate 免费通道的余额 / usage API | **未找到**（注释明说该通道不提供余额接口，只能靠 429） |
| dsh-free 的额度探针独立定时器 | **未找到**（有手动「重测」入口与折算时读取，未见独立轮询定时器） |
