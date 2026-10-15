# 前端四页（概览 / 上游 / 模型与映射 / 客户端）

Type: task
Status: resolved
Blocked by: 45, 49, 51

## Question

按 spec §10 第 1–4 节做 M2 需要的四页。前端只做外观与交互，不参与转发、不直接读写客户端配置文件。

依据：spec §10（含故障态与错误表达两段）、[票 17](../issues/17-grilling-ui-information-architecture.md)。

## 交付

- **概览**：三步清单（加上游 → 绑槽位 → 切客户端；未就绪高亮、点进对应页；就绪判据 = 上游至少一条已保存且自检通过 / 四行槽位都绑定且探针通过 / 三个客户端至少一个「已生效」）、网关状态与端口、今日用量、最近错误（折叠）；两个一键动作（批量切客户端、三步全绿后的 haiku 槽位端到端验收）。
- **上游**：列表（名称 / baseUrl / 协议 / 上次自检结果 / 上次拉取时间与警告角标 / 被引用数）+ 四步向导（选供应商可跳过 → baseUrl 与 key（第二种 wire 面按需展开第二个 key 输入框）→ 拉模型可跳过、带可用 wire 徽章、手改锁定 → **强制自检并保存**）。
- **模型与映射**：客户端标签（全局 / Desktop / Claude Code / Codex）；四行槽位表（槽位 | 上游 | 真实模型 | 协议 | 测试）+ 普通虚拟模型列表；协议列可手改但候选取值只限声明的值；每行一颗「价格」按钮（四项可填可留空）。
- **客户端**：三张卡 + 四态徽章（未配置 / 已生效 / 值不匹配 / 已回官方），**每次进页面实读文件比对**、不缓存；可折叠「当前值 vs 期望值」对照表；一键切换 / 一键还原 /（Desktop）1P↔3P 开关；写文件与删数据二次确认并列出绝对路径与备份路径；**「未登录」不作阻断性错误**；Codex 保持一张卡。
- 网关未运行时顶部常驻横幅 +「启动网关」按钮，配置类页面仍可编辑；错误统一模板 + 脱敏诊断包。

## 收口验收

- 三步主路径从零走一遍（新库起）：加上游 → 绑槽位 → 切客户端。
- 未运行态横幅、端口被占时的「换端口」确认流程（含自动同步已生效客户端地址）。
- 客户端页四态在真实文件被手改后能如实显示「值不匹配」。
## Answer

（2026-10-15，task/52-frontend-four-pages，收工前已并 `integration/model-gateway` 两次：`2dfe335`（票 53）与
`a42b7a7`（票 56），冲突就地解决、门禁在合并后的树上重跑全绿）

### 做了什么

**前端（`app/`，壳层归本票）**：七页侧栏（顺序逐字：概览 / 上游 / 模型与映射 / 客户端 / 日志与调试 /
用量与成本 / 设置），日志页把票 53 的 `pages/LogsPage.tsx` **原样折进**第 5 项（逻辑一字节没动），
用量与成本 / 设置是「待实现」占位。四页本体：

- `pages/OverviewPage.tsx`：三步清单（① 上游：有上游且没未通过自检的；② 槽位：`/api/bindings` 四行都绑了；
  ③ 客户端：`clientsNotEffective < 3`）——未就绪高亮 + 点进对应页；网关状态与端口、今日用量（走票 53 的
  `/api/usage/summary`，人民币为主、无价与 token 未知单独计数）、最近错误（折叠，只收请求错误）；两个一键动作
  （批量切客户端；三步全绿才可点的 haiku 端到端验收，结果卡列上游 / 状态码 / 耗时 / 首字延迟 + 原始响应）。
- `pages/UpstreamsPage.tsx`：六列列表（名称 / baseUrl / 协议 / 上次自检 / 上次拉取+警告角标 / 被引用数可展开）
  + 四步向导（① 供应商预填可跳过 → ② baseUrl 与 key，第二种 wire 面按需展开第二个 key 输入框 → ③ 拉模型可跳过、
  wire 徽章、手改锁定、三结局与按钮全用 `PullReport` 的文案 → ④ 强制自检并保存，没通过要显式确认才存）。
  删被引用的上游走二次确认并列出引用处。
- `pages/ModelsPage.tsx`：客户端标签（全局 / Claude Desktop / Claude Code / Codex）+ 四行槽位表
  （槽位 | 上游 | 真实模型 | 协议 | 测试）+ 普通虚拟模型列表（每客户端可覆盖）；协议列的候选取值只限该模型
  `supported_wires`（空集合时给三值并标「未声明，靠自检」）；每条一颗「价格」按钮（四项可填可留空 + 逐项来源）。
- `pages/ClientsPage.tsx`：三张卡 + 四态徽章（逐字）、**每次进页面重新实读**、可折叠「当前值 vs 期望值」
  （不匹配行标黄）、一键切换 / 一键还原 /（Desktop）1P↔3P、Codex 保持一张卡；写文件与还原都走二次确认并列出
  **绝对路径与备份预告路径**；「未登录」只作为说明文字（`notes`）。
- `App.tsx`：未运行横幅（`/api/*` 连不上时）+「启动网关」+「换端口」（探测 → 用户确认 → 才写入 → 换完
  `setApiBase` 并提示已同步 N 个客户端）；侧栏两个徽标；错误模板与二次确认是全页共用的两件；
  跨源收紧到白名单（见下）。

**后端（`/api/*`）**：`ingress/api/` 五个子模块（`upstreams` / `bindings` / `clients` / `overview` / `gateway`）
+ `crates/gateway-core/src/clients/mod.rs`（本票的装配层：三客户端配置根、四态实读比对、切换 / 还原 / 1P↔3P /
  备份清单）。业务全部复用票 45–51 的公开 API，没有第二套实现。两处必要的底层改动：

1. **写入引擎加了「实读比对」**（`writer::Writer::inspect / remember / recorded / preview_backup / list_backups`
   + `OwnedOp::expected/display_key`）：四态判据是「四行接管键全中 / 都没有且上次动作是 restore / 其余」，
   `client_config_state` 仍是唯一真相。
2. **转发面三张表改成可热重载**（`proxy::Gateway` 的 `upstreams/catalog/bindings` 从裸字段改成 `RwLock`，
   新增 `reload()` 与读访问器；票 56 的 `forward_ir`/Ollama 面同改）：界面上完配置**立刻生效**——
   否则「绑完槽位再问客户端」要重启网关才通，M2 主路径会当场断。

### 端点表最终形态（新增 / 改名逐条）

对齐 `frontend.md` §③ 那张表；下表「票面」= 该行的用途，括号里是本票落的路径。**只有第 1 行是原有的。**

| # | 方法 + 路径 | 说明 |
| --- | --- | --- |
| 2 | `GET /api/sidebar/badges` | 新增。`upstreamsFailedSelfcheck` / `clientsNotEffective` / `ready`（拉取与发现不计入） |
| 3 | `POST /api/gateway/start` | 新增（网关在跑时的路径；没在跑时前端走 `gateway_start` IPC） |
| 4 | `POST /api/gateway/port/probe` | 新增。只预填：`{currentPort, currentPortFree, suggestedPort}` |
| 5 | `POST /api/gateway/port` | 新增。确认后写入 + 同步已生效客户端，回 `{port, previousPort, synced[], failed[], message}` |
| 6 | `GET /api/errors/recent?limit=` | 新增。只收请求错误，每条带 `template`（发生了什么 / 可能原因 / 一个主按钮） |
| 7 | `POST /api/diagnostics/copy` | 新增。回 `{text}`；**端点自己再查一遍明文**，命中就 500 不返回 |
| 8 | `POST /api/clients/switch-all` | 新增。已生效的跳过，回 `{results[], skipped[], failed[], message}` |
| 9 | `POST /api/acceptance/run` | 新增。haiku 槽位非流式最短请求（**打自己的 `/v1/messages`**），回上游 / 状态码 / 耗时 / 首字延迟 / logId |
| 11 | `GET /api/providers` | 新增。随包预填表（新文件 `egress/providers.json`，7 条，来自 research/providers.md）+ models.dev 快照的 provider 清单 |
| 12–15 | `GET/POST /api/upstreams`、`PUT/DELETE /api/upstreams/{id}` | 新增。DELETE 被引用时 409（引用处走 `/{id}/references`），`?force=true` 连绑定一起删 |
| 16 | `GET /api/upstreams/{id}/references` | 新增 |
| 17 | `PUT /api/upstreams/{id}/credentials` | 新增（回掩码，无明文） |
| 18 | `POST /api/upstreams/{id}/discovery` | 新增。原样透传 `PullReport`：disposition / message / actions / reuseAt / warnings / chooseFrom / models |
| 19 | `GET /api/upstreams/{id}/models` | 新增（含逐项价与来源、未声明标注） |
| 20 | `POST /api/upstreams/{id}/models` | 新增（手工条目 + 可选四项价） |
| 21 | `POST /api/upstreams/{id}/selftest` | 新增（`{wire?, model?}`，缺省取目录里第一条有声明 wire 的） |
| 22 | `GET /api/bindings?client=` | 新增。四行槽位 + 普通虚拟模型 + `profiles[]` + `wires[]` |
| 23 | `PUT /api/bindings/{slot}?client=` | 新增 |
| 24 | `POST /api/bindings/{slot}/probe?client=` | 新增（非流式 `max_tokens:1`，✓/✗ + 耗时 + 原始提示） |
| 25 | `CRUD /api/virtual-models` | 新增（`GET/POST ?client=`、`DELETE /{name}?client=`） |
| 26 | `PUT /api/pricing/{upstream}/{model}` | 新增（另有 `GET` 同一路径给「现在的取值 + 来源」） |
| 27 | `GET /api/clients` | 新增。三张卡 + 版本 + 绝对路径 + 当前值 vs 期望值 + 备份预告 + notes |
| 28–30 | `POST /api/clients/{id}/apply` / `/restore` / `/api/clients/desktop/mode` | 新增（apply 带 `{claim}` 时走 Codex 认领覆盖） |
| 31 | `POST /api/clients/{id}/keep-mine` | 新增。把库记录对齐磁盘现值（不写文件），下次切换不再判冲突 |
| 32 | `POST /api/clients/{id}/claim-overwrite` | 新增。只有 Codex 有定义 |
| 33 | `GET /api/clients/{id}/backups` | 新增（含 `backupRoot` 与还原口径说明） |

**改名 / 取舍 3 条**：① 票面写 `PUT /api/pricing/:upstreamId/:modelId`，本票落成
`PUT /api/upstreams/{id}/models/{model}/prices` 并另给 `GET /api/pricing/{upstream}/{model}` 读现值——
写价与目录条目同源，读价两边都方便；② 验收结果的请求标识：`requests.request_id` 目前**转发面从不写**
（列为空），所以回执里 `logId`（行 id）一定在、`requestId` 可能为 null，前端跳日志详情用 `logId`（见遗留）；
③ 概览用量走票 53 的 `/api/usage/summary`（本票最初的重复实现已删），字段是它那套 snake_case。

### 验收（逐条真跑）

1) **构建**：`pnpm --dir app build` → `✓ 23 modules transformed / ✓ built`（tsc --noEmit 也过）；
`cargo build --workspace` → `Finished dev profile`（46s）。
2) **三步主路径从零（真实上游）**：
```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/52'
cargo test -p gateway-core --test m52_acceptance -- --ignored --nocapture three_step
```
→ `test result: ok. 1 passed`。逐段观测（2026-10-15，凭据现读 `prototype-upstream.env`，全程不打印）：
① 起手徽标 `upstreamsTotal 0 / clientsNotEffective 3 / ready false`；
② `POST /api/upstreams` → 201，回执里 **key 0 次**；
③ `POST /discovery` → `fetched`，**拉到 37 条**，反查警告 0；
④⑤ 逐候选自检 → `cat-jam-free / openai-chat` **200**（elapsed 992ms）；
⑥ 四个槽位全绑（200×4）；⑦ haiku 探针 **200 / 941ms**；
⑧ `POST /api/clients/claude-code/apply` → `state = effective`，写的是临时根
`…\cg52-real-upstream\clients\claude-code\settings.json`；
⑩ `POST /api/acceptance/run` → `{"ok":true,"upstream":"opencode","status":200,"elapsedMs":1571,
"firstByteMs":1569,"logId":1}`，走的是真实入站 `/v1/messages`（日志里有 `[in] /v1/messages …model=claude-haiku`）；
⑪ 用量汇总 1 次请求；⑫ 诊断包 1168 字节、**密钥明文命中 0 次**；⑬ 还原 → `official`。
另有 `admin_api.rs`（假上游、5 条）覆盖同一条链路，含「目录里没有的模型绑不上 → 409」。
3) **未运行态横幅 + 换端口确认流程**：`admin_api.rs::port_probe_and_change_syncs_only_effective_clients`
（`test result: ok. 5 passed`）逐条断言——探测**不写端口**（`/api/status` 端口不变）、确认后
`已同步 1 个客户端`（只同步换之前已生效的 Claude Code，未生效的 codex 一个文件都没建）、
`config.toml` 不存在、外壳收到 `restart:<新端口>`、已生效客户端文件里的地址真的换成了新端口；
端口被占时 → **409**。前端侧：`/api/*` 全连不上时顶部横幅「网关未运行」+「启动网关」+「换端口」，
换端口走「探测 → 二次确认（写库 / 同步生效客户端 / 按新端口重启三条都写明）→ 写入」，
`/api` 也发不出去时退到 `gateway_start` / `gateway_probe_port` / `gateway_change_port` 三条 Tauri IPC
（`src-tauri/src/shell.rs`，与 HTTP 端点共用 `apply_port_change` 同一份判据）。
4) **客户端四态（真实文件被手改）**：
```
cargo test -p gateway-core --test m52_acceptance -- --ignored --nocapture real_client_file
```
→ `test result: ok. 1 passed`。真实 `C:\Users\ZHANQUAN\.claude\settings.json`（原内容只有
`{"autoUpdatesChannel":"latest"}`）上：① 起手 `未配置` → ② 切换 `已生效`（首次接管，备份
`…\com.claudegateway.desktop\backups\claude-code\settings.json.20261015-230843`，**非接管键原样保住**）→
③ 手工把 `ANTHROPIC_BASE_URL` 改成 `http://127.0.0.1:1/hand-edited` → 状态 `值不匹配`、对照表那行
`当前 "http://127.0.0.1:1/hand-edited" vs 期望 "http://127.0.0.1:8787"` → ④ 一键还原 → `已回官方`，
文件逐字回到 `{"autoUpdatesChannel":"latest"}`（断言与原内容 JSON 相等）。**已还原。**
5) **门禁**（合并票 53 / 56 之后重跑）：
```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/52'
pwsh -File ./check.ps1 -Quick -Replay
```
→ `全部通过。`（exit 0）：fmt + clippy（workspace + mock 特性）+ 单元 137 + **5（admin_api，本票）** +
14 + 8 + 4（gateway_service）+ 4（logs_api）+ **12（ollama_api，票 56）** + 14（provider_table）+ 19 + 1 +
回放 **22 passed** + 回放器切法 5 条。

### 留给票 55 的 UI 步骤（M2 收口现场走一遍）

1. 概览三步清单：点①进「上游」→ 加上游向导（选供应商或跳过 → baseUrl+key → 拉模型 → **强制自检**）→
   回来①变绿；点②进「模型与映射」→ 四行槽位各选上游 / 真实模型 / 协议 → 每行「测试」出 ✓ → ②变绿；
   点③进「客户端」→ 一键切换（Desktop 卡片会提示完全退出重开）→ ③变绿。
2. 三步全绿后点「跑一次端到端验收（haiku 槽位）」：结果卡四项 + 失败时的错误模板与跳日志详情。
3. 「客户端」页：手改任一配置文件后**重新进页面**（不是点刷新页签）应显示「值不匹配」+ 标黄对照表；
   测试「以网关配置覆盖」（= 一键切换）与「保留我的修改」两条分支；再「一键还原」回官方态。
4. Desktop 的 1P↔3P 两个按钮各点一次（注意它不做热重载，必须完全退出重开 Desktop 看效果）。
5. 未运行态：托盘退出 → 重开应用前先占住 8787（或直接杀网关进程）→ 看横幅与「启动网关」；
   再占住端口点「换端口」→ 确认 → 看「已同步 N 个客户端」与客户端配置里的地址。
6. 侧栏徽标：删掉一条已自检的上游后「上游」项应出现数字；把三个客户端都还原后「客户端」项应为 3。
7. 日志页（票 53）与概览、客户端页里的「复制诊断」各点一次，粘出来确认**没有密钥明文、没有 body**。

### 遗留与坑

- **`requests.request_id` 没人写**：列在 v2 就有，转发面从没填过。所以验收回执里 `requestId` 常是 null，
  本票用 `logId` 兜底（前端跳日志详情、诊断包取请求元数据都用它）。票 53 / 59 接日志详情时**顺手填上**，
  否则「搜索框匹配请求 ID」永远搜不到东西。
- **`client_config_state` 之外没有「未配置 vs 已回官方」的第二种判据**：库被清掉后，一个从没配过的文件
  与一个刚还原过的文件长得一样（都是「没有网关键」）——本票按「有 restore 记录 = 已回官方」判，
  区别只在记录里。换机 / 删库后显示「未配置」，是事实，不做猜测。
- **客户端版本检测很薄**：只读几个已知版本文件（`%LOCALAPPDATA%\AnthropicClaude\version` 等），读不到就
  「未检测到版本」。各家安装布局不同，真要准得先有安装清单（没票，M3 之后再谈）。
- **`/api/providers` 的预填表只有 7 条**（commandcode / opencode 两面 / 千问两面 / deepseek 两面），
  全部来自 `research/providers.md`；models.dev 快照里另外 25 家没 baseUrl 的 provider 只列了 id 与模型数，
  向导里选不到预填（可以手填）。补表要有据可依，不猜域名。
- **Desktop / Codex 的一键切换没有真机验证**：本票只验了 Claude Code 的真实文件（写 + 手改 + 还原）。
  Desktop 走的是同一份 `writer::desktop`（票 46 已端到端验收过），但**经由本票的装配层**再走一遍
  （`/api/clients/desktop/apply` 真的改 `%LOCALAPPDATA%\Claude-3p`）属于票 55 的现场步骤。
- **重启网关后的前端状态**：换端口会写 localStorage，刷新即用；但网关被托盘停掉、前端还开着时的
  「启动网关」（IPC）之后要等 1.5 秒才自动重读一次状态，体验上是一拍——不做轮询/事件推送（没票要求）。
- **前端零依赖**（不引路由 / 状态库 / CSS 框架）：导航是 hash 路由 + `useState`，样式是 `app.css`。
  七页要加多语言与无障碍时可能得换，但 spec 把它们留在「Not yet specified」，本票不提前建。
- **`ApiState` 现在同时挂 `admin`（本票）与 `logs`（票 53）**：两者都没挂时（M1 那档单测）
  `/api/*` 的聚合端点回 503「这一档没有管理面」，不回 500——别把这个当成故障。
