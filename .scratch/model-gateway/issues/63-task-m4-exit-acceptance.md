# M4 收口：分发全流程验收

Type: task
Status: resolved
Blocked by: 61, 62

## Question

走 spec §12 的 M4 收口验收：干净机器上装 → 升一版 → 卸载（选还原）全流程。**主会话 + 用户在场**。

依据：spec §12 M4。

## 交付

- 一份端到端记录贴进本票 Answer：安装路径、升级前后版本、卸载后的残留检查、三客户端状态。

## 收口验收

- 干净机器（或先完整卸载清干净）装 NSIS → 能跑 → 三客户端可用。
- 升级一版成功且不改端口 / 数据 / 客户端配置。
- 卸载选「还原」→ 三客户端回官方态、数据目录按选择保留。
- 用户签字。

## Answer

**M4 收口通过**（2026-10-16，主会话 + 用户在场；分支 `integration/model-gateway`）。这是 25 张实现期票据的最后一张。

现场步骤卡：`C:\Users\ZHANQUAN\AIGC\Project\cg-notes\m4-live.md`（344 行）；证据 `…\cg-notes\63-*.log`；产物收在仓库外 `…\cg-notes\63-packages\`；改前快照 `…\cg-notes\63-prewrite-backup\`。

### 用户拍的范围（票 61 时定的）

用户选：**M4 用本地更新源把全流程跑通并签字，走公开 GitHub Releases 那半步留作待办**。所以本票验收的是「干净装 → 升一版 → 卸载（选还原）」的完整链路（升级源是本地签名源），GitHub 那部分作为待办逐条列出（见下），**不伪造**。

### 一、底子

`check.ps1 -Quick -Replay` → `全部通过。` exit 0（unit 177 + 回放 22 + 回放器 5 + ollama 12 + structured_output 19 + update_api 4 + update_guard 4 + …）；`pnpm --dir app build` exit 0（303.07 kB）。

### 二、干净装（等价「干净机器」）

先完整卸载再装，全程留了逐字节备份：

1. 卸载前备份 36 个数据文件 + 5 份客户端配置到 `63-prewrite-backup\`（清单 0 处不一致）。
2. `uninstall.exe /S` → exit 0；程序目录消失、`HKCU\…\Uninstall\Claude Gateway` 消失、Run 值查不到；**数据目录按默认保留（36 文件）**；三个客户端回官方态。
3. `/S` 静默装 0.1.0 → exit 0、**本会话非管理员、没有弹 UAC**；`/api/status` = `0.1.0 / 8787 /` 同一 dbPath；数据目录还是同一份。
4. 把 36 + 5 份逐字节放回并核对 sha256（0 处不一致）→ 三张卡 `effective`、徽标 `ready: true`、四槽位探针 4/4 200。
5. **免安装 zip 与安装版共用同一份数据目录**：解压即跑（先退安装版以免抢单实例互斥量），两边 `/api/status` 的 `dbPath` **逐字相同**（`%APPDATA%\com.claudegateway.desktop\gateway.db`）。

### 三、升一版（本地签名源，应用内）

`0.1.0 → 0.2.0`（跑了**两遍**，第二遍用最终产物）。三条不变性逐条绿：

| 判据 | 结果 |
| --- | --- |
| 不改端口 | `8787` 与 dbPath **逐字未变** |
| 不改数据 | 停应用后数据目录逐文件 SHA256 **0 处不一致**（`gateway.db` 与 `-wal` 在内；比对排除 `updates\`、`logs\` 与 SQLite `-shm`）；归一化导出 sha 相同 |
| 不改客户端配置 | 五份客户端配置 sha256 **全等** |

留包与回执：`updates\installers\Claude Gateway_0.2.0_x64-setup.exe` + `last-update.json{from:0.1.0,to:0.2.0}`；回退指引可执行（按指引真退回 0.1.0，前后五项判据全绿——票 61 已单独跑过一轮）。

### 四、卸载（选还原）

`/S` 静默（= 默认「是（还原）」）：exit 0；程序目录 / 卸载项 / Run 值清干净；**数据目录保留（38 文件）**；三个客户端回官方态（`.claude/settings.json` 36 B `DA00E2B1…`、`codex/config.toml` 1167 B `3F867024…`、Desktop `_meta.json` 175 B `492A09C2…`、profile 4 B `E3566B3A…`、网关建的 `claudegateway-model-catalog.json` 被删）。

**交互式卸载页由用户当场确认**：确认页 **「Delete app data」默认未勾**、弹出的「还原客户端配置 / 保持现状」**默认是「是（还原）」** → 用户确认第①步正常。

### 五、用户签字

用户在机器上逐项走过并签字（① 交互式卸载页正常；② zip 解压即跑且与安装版 dbPath 逐字相同、没有网页版界面；③ 应用内「检查更新」升到 0.2.0 且端口/数据/客户端配置未变；④ 一键还原后三张卡「已回官方」、数据目录保留）。

### 六、GitHub 待办（用户选择留到以后，逐条可执行）

抄自票 61 的 Answer 第 6 节，未改一字：

1. `gh repo create` 建**公开**仓库，把 `src-tauri/tauri.conf.json` 里的 `OWNER/REPO` 占位换成真值。
2. 配两个 Secret：`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（值只从 `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\` 取，**私钥不许入库**）。
3. 两处版本一起抬 + 打 tag。
4. 看 CI 日志里有没有「没有 TAURI_SIGNING_PRIVATE_KEY」——没有才算真签上。
5. 建 Release 上传 `setup.exe` + `latest.json`（先 draft 验 200）。
6. 现场跑一次真 https 闭环；`git grep 127.0.0.1:8791 src-tauri/tauri.conf.json` 必须为空。

**签名私钥是用户资产**：`…\cg-notes\keys\updater.key`（+ `.password.txt` + `.pub`，ACL 只留当前用户）。**丢了以后再也发不了升级**——请离线备份整个 `keys\` 目录。

### 遗留

1. ~~本机现在装的是 **0.2.0（本地验收源那一版）**，端点指 `127.0.0.1:8791`~~ → **已被下面「收尾第二轮补」第 3 条取代**：本机现在是公开仓库发的**生产版 0.2.0**，端点指真仓库。
2. 卸载后 `HKCU\Software\claudegateway` 会留一个键（只有勾「Delete app data」才清）。
3. 收尾数据目录 38 个文件而非 36（多的两个是升级回执与留包），属预期。
4. `commandcode` 的 chat 面在 02:34–02:41 抖了 7 分钟（四探针连续 3 轮全 504，同一刻上游自检与 `/v1/messages` 真流量都 200，02:43 自己恢复）——**现场遇到重问一次即可**，原文 `63-upstream-flap.log`。
5. 免安装 zip 不参与自动更新（spec 已定）。
6. ~~**上游 Responses 出站 wire 仍未实现**~~ → 该缺口在收尾期由[票 65](65-task-responses-outbound.md) 补上（本票写这句话时 65 还没开）。

## Answer（收尾第二轮补：GitHub 那半步 + 真 https 闭环）

**执行人**：收尾第二轮（2026-10-16）。本条只补本票「六、GitHub 待办」那 6 条的落地与第 6 条现场闭环，不改前面已签字的四段验收。

### 1. 仓库与 Release（对应待办 1–5）

- 公开仓库 **<https://github.com/ColoristSansi/claude-gateway>**（README + MIT LICENSE 一并补上，README 里写了 SmartScreen 那条）。
- `src-tauri/tauri.conf.json` 与 `src-tauri/Cargo.toml`（+ `Cargo.lock`）**一起**抬到 `0.2.0`，`endpoints` 的 `OWNER/REPO` 换成真仓库（唯一一处）；`main` 快进到与 tag 同一提交，tag **`v0.2.0`**。
- CI：`check`（main 与 tag 两次都绿）与 `bundle` 都 success；pack 步的环境里能看到 `TAURI_SIGNING_PRIVATE_KEY: ***`，**没有**「没有 TAURI_SIGNING_PRIVATE_KEY」那句警告 → 真签上了。
- Release `v0.2.0`（公开）三个资产：`Claude.Gateway_0.2.0_x64-setup.exe`（6083879 B）、`latest.json`（1449 B）、`Claude-Gateway_0.2.0_x64-portable.zip`（9019545 B）。清单里的 `signature` 与产物 `.sig`（444 B）逐字相同，两个平台键（`windows-x86_64-nsis` / `windows-x86_64`）写同一份签名与地址。
- 匿名自检：`https://github.com/ColoristSansi/claude-gateway/releases/latest/download/latest.json` → **200**；清单里那条安装包地址 → **206**（range 取到 1024 B）。

### 2. 两个顺带修掉的真洞（都不在票 61 的清单里）

1. **CI 两个 job 都缺「装前端依赖」**：`check.ps1` 的 web 步与 tauri 的 `beforeBuildCommand` 都要 `app/node_modules`，而 workflow 只装了 pnpm CLI。第一次跑就会两个 job 全红。commit `60aace9`。
2. **票 61 那句「资产名里的空格 GitHub 会写成 `%20`，对得上」是错的**：GitHub 会把资产名里除 `[A-Za-z0-9._-]` 以外的字符**换成点**（实测 `Claude Gateway_0.2.0_x64-setup.exe` → `Claude.Gateway_0.2.0_x64-setup.exe`，顺手传的 `space test.txt` → `space.test.txt`）。按原样发出去的清单点「检查更新」会在**下载那步 404**。`pack.ps1` 现在只在 `-BaseUrl` 是 `https://github.com/...` 时做这个替换（本地源仍按磁盘原名取文件），commit `3ebfb5f`；这也是第一次打 tag 后把 Release 与 tag 撤掉重发的原因（当时还没有任何消费者）。

### 3. 真 https 闭环（对应待办 6）——现场实测

1. 旧版：用**生产配置 0.1.0**（端点已是真仓库、无 `127.0.0.1:8791`；包 `cg-notes\67-packages\v-0.1.0-production-realendpoint-setup.exe`，SHA256 `3A22437E…C49`）`/S` 装到本机，`/api/status` → `version=0.1.0 port=8787`。
2. `POST /api/update/check` → `currentVersion=0.1.0`、`available.version=0.2.0`、
   `available.downloadUrl=https://github.com/ColoristSansi/claude-gateway/releases/download/v0.2.0/Claude.Gateway_0.2.0_x64-setup.exe`、`error=null`。
3. `POST /api/update/install` → 立刻应答 `installing=true`；约 25 秒后 `/api/status` → **`version=0.2.0`、`port=8787`、`dbPath` 逐字不变**。
4. 回执 `%APPDATA%\com.claudegateway.desktop\updates\last-update.json` = `{"from":"0.1.0","to":"0.2.0","at":"2026-10-16T11:33:42Z","package":"Claude.Gateway_0.2.0_x64-setup.exe"}`。
5. 收尾自查：`git grep 127.0.0.1:8791 src-tauri/tauri.conf.json` → **空**。
6. 数据目录文件数 39 → 39（升降级都不动数据）。原文 `cg-notes\67-https-loop.log`。

### 4. 现场两处与交班单不符（如实记下）

1. **闭环前本机并没有「已安装的 0.2.0」**：`%LOCALAPPDATA%\Claude Gateway` 当时**不存在**，占着 8787 的是一个从 `target\release\bundle\zip\Claude-Gateway_0.1.0_x64-portable\` 跑起来的**免安装 0.1.0 残留进程**（08:56 起；exe 里既没有 `8791` 也没有真仓库字符串，是更早的构建）。按「精确 PID + 精确 exe 路径」停掉它之后才知道。→ 本票遗留 1 与 START-HERE 的「本机现状」都按事实更正。
2. **三个客户端现在是 `official`（不是「已生效」）**：残留进程那段时间没有接管，或中途被还原过。要回到收尾态得再点一次「一键切换」（三个客户端都要完全退出重开）：

   ```powershell
   $b='http://127.0.0.1:8787'
   foreach ($id in 'claude-desktop','claude-code','codex') {
     Invoke-RestMethod -Method Post -Uri "$b/api/clients/$id/apply" -ContentType 'application/json' -Body '{}'
   }
   ```

   上游 2 条、数据与库都在（`/api/upstreams` 2 条）。

## Answer（用户签字 · 收尾第三轮：界面检查更新 / 三客户端真问一句 / 交互式卸载）

**执行人**：收尾第三轮（2026-10-16 22:19–22:58，主会话 + 用户在场；分支 `integration/model-gateway`，走完 `git status` 空）。
**依据**：现场卡 `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\accept-live.md`，按其 §6 的顺序走（§0 基线 → §1 界面 → §2 三客户端 → §3 交互式卸载）。
**本轮机器的门禁没重跑**（未改任何代码）；**判据一条未改**、**tag `v0.2.0` 未动**（仍指 `3ebfb5f`）、**四个槽位绑定未改**。

### 0. 基线（卡 §0）——与卡记的抖动形状完全一致

| 项 | 实测 |
| --- | --- |
| `/api/status` | `version=0.2.0 port=8787 dbPath=C:\Users\ZHANQUAN\AppData\Roaming\com.claudegateway.desktop\gateway.db` |
| `/api/clients` | `claude-desktop=effective claude-code=effective codex=effective`（`clientsNotEffective=0`、`ready:true`） |
| `/api/sidebar/badges` | `upstreamsFailedSelfcheck=1`（`commandcode` 上次自检留的 `ok:false`，`at=1792159873`） |
| 两条上游自检（重跑） | `commandcode` **200 / 2850 ms**（`MiniMaxAI/MiniMax-M2.5`，openai-chat）；`opencode-go` **200 / 1823 ms**（`cat-jam-free`） |
| 四槽位探针 · 第①轮 | 4×**504**，每次恰好 **20.0 s**（`probe-18df0876…/…87ad0/…f78/…8842`） |
| 同刻真形状判别 | `/v1/messages` `claude-haiku` `max_tokens=512` → **200 `end_turn`，文本 `你好`，3414 ms**（`max_tokens=128` 时 `stop_reason=max_tokens`、文本空，正是卡里「答不出内容≠坏」那条） |
| 端到端验收按钮 · 第①次 | **504**，`elapsedMs=20008 firstByteMs=0`，**`logId=76`**，`req-65df5eef7061e0011` |
| 60 s 后 · 四槽位 | **4/4 200**（2652 / 3309 / 2018 / 2652 ms） |
| 端到端验收按钮 · 第②次 | **`ok:true`**，`elapsedMs=2336 firstByteMs=2266`，`upstream=commandcode`，**`logId=77`**，`req-65df5f3f7456a0012` |

判别法（自检 200 + 真客户端形状 200 + 探针成片 504 → 重试）**当场成立**，结论仍是「上游那一刻不答」，**不是装错**。还原前五份客户端文件哈希与卡 §3 逐字相同（275 B `A26A1344…` / 440 B `0EE2EE7B…` / 366 B `9FD4F341…` / 3136 B `6D7D5EC7…` / 6367 B `2309BB41…`）。

### 1. 界面点一次「检查更新」（卡 §1）

设置 → 关于与版本 → 「检查更新」，用户当场报：

| 判据 | 结果 |
| --- | --- |
| ① 当前版本 `0.2.0` | **过**（界面 `0.2.0`；`/api/about` → `version=0.2.0`） |
| ② 提示已是最新版本 | **过**（界面「已经是最新版本（当前 0.2.0）」；`/api/update/check` → `current=0.2.0 available=空 error=空`） |
| ③ 「更新端点」显示真仓库地址 | **界面没有这一项**（见下） |
| ④ 回退指引指到 `…\com.claudegateway.desktop\updates\installers\` | **过**（用户确认指到；`/api/about.updatesDir` 同值） |

**第 ③ 条的实测事实**：应用界面里**不存在「更新端点」这一行**。「关于与版本」卡片只有 `版本 / 数据目录 / 更新包目录` + 两段说明 + 回退指引原文 + 更新包清单 + 「检查更新」按钮（`app/src/pages/SettingsPage.tsx:571-637`）；全前端搜 `端点`/`endpoint` 只命中一条注释，没有任何面展示更新端点 URL。**不是用户没找到，是界面没这个面。**

端点本身按机器侧核过，是真的、无本地源残留：

- `src-tauri/tauri.conf.json:40` → `https://github.com/ColoristSansi/claude-gateway/releases/latest/download/latest.json`
- `git grep -n '127.0.0.1:8791' -- src-tauri/tauri.conf.json` → **空**
- **已安装的 exe**（`%LOCALAPPDATA%\Claude Gateway\claude-gateway.exe`）里 `ColoristSansi` 出现 **1 次**、`127.0.0.1:8791` 出现 **0 次**

**没改判据、没顺手开新票**：这条按「卡要求人眼看的项、当前界面无此面」如实记下。**后续（2026-10-17，用户当场决定）**：改现场卡的措辞——`accept-live.md` §1 第 ③ 条已改成「机器侧核实端点三处（`tauri.conf.json` 的 `plugins.updater.endpoints` / 已安装 exe 含该字面量且不含 `127.0.0.1:8791` / `GET /api/update/status` 的 `endpoints[0]`）」，卡头也记了修订日期与原因。**界面仍不加这一行**：后端 `/api/update/status` 本来就回 `endpoints`，是前端从没读它；要恢复「界面看得见」按卡里那句开新票，前端约 6 行、后端零改动。

### 2. 三个客户端各「真问一句」（卡 §2）

三处都拿到真实回答、都没有连接 / 鉴权错：

| 客户端 | 问 / 答（用户与终端原文） | 网关侧同一刻日志（`GET /api/logs`） |
| --- | --- | --- |
| Claude Code（真 CLI `C:\Users\ZHANQUAN\.local\bin\claude.exe` 2.1.299，读 `~/.claude/settings.json`，5.4 s） | 问「只回答一句：你实际是哪个模型？」→ 答 `I'm Claude, running as claude-opus[1m] in this session — …` | `id=78 req-65df61e6af18c0013 vm=claude-opus client=claude-code upstream=commandcode real=deepseek/deepseek-v4.1-flash wire=openai-chat 200 fb=3614 total=4045 out=63 cost_usd=0.006818` |
| Claude Desktop（3P，开新对话） | 问「你是什么模型？」→ 答「我是 Claude，由 Anthropic 开发，本次会话运行在 claude-fable 模型上。」 | `id=79 22:43:53 vm=claude-haiku client=claude-desktop 200 fb=4688 out=102 req-65df6317aa7cd0014` |
| Codex（选 `claude-*` 槽位模型） | 问「你是什么模型？」→ 答「我是 OpenAI 的 Codex，一个基于 GPT 系列模型驱动的编程助手。」 | `id=81 22:44:53 vm=claude-fable 200 fb=1434 out=193 req-65df63505192a0016`；`id=82 22:44:54 vm=claude-fable 200 fb=2987 out=108 req-65df63517a2ec0017` |

用户当场另取一份端到端验收证据：第③次又撞抖动 **504 `logId=83`**，重试 **`ok:true` / 200 / `logId=84` / `firstByteMs=16410`**（慢但过）。

**两处如实记下（都不构成不过）**：

1. **Desktop 的请求落在 `claude-haiku` 槽位**，不是它嘴上说的 `claude-fable`——`client=claude-desktop` 的历史 5 条日志**全部**是 `vm=claude-haiku`，那是 Desktop 自己选的模型；模型自报的名字不可信。判据要的「有真实回答、不报连接/鉴权错」→ **过**。
2. **`.codex\config.toml` 被 Codex 自己改写了**：用户在 22:44 问 Codex 那句时，Codex 应用往文件头插了一行 `notify = [...]`，mtime `22:44:41`，哈希从 `3136 B 6D7D5EC7…` 变成 `3136 B CEBEABF1…`（同尺寸、不同内容）；两个 `claudegateway managed` 块原封未动。**所以卡 §3 的「还原前哈希」在这份文件上会被客户端自己动**——判据仍按卡 §3 的说明（不以哈希相等为判据）。
3. Claude Code 自己还吐了两条客户端提示，原文留档：`[claude-code:unrecognized_model] {"model":"claude-opus[1m]","query_source":"sdk"}`，以及一条「auto mode 分类器请求不计费的新版不适用于走经 `127.0.0.1:8787` 的会话」的公告（明确说不影响可用，只是那项新计费不适用）。

### 3. 交互式卸载走查（卡 §3，选默认还原那条）

**卸载前快照**：程序目录 2 文件；卸载项 `DisplayVersion=0.2.0`；Run 值**本来就没有** Claude Gateway 行；`HKCU\Software\claudegateway` 在；数据目录 **40 文件**；网关进程 **PID 26432**。

用户双击 `%LOCALAPPDATA%\Claude Gateway\uninstall.exe` 当场报的四处，与 `src-tauri/nsis/hooks.nsh:13-31` 的实现逐条对得上：

| 判据 | 用户看到 | 源码 / 机器侧旁证 |
| --- | --- | --- |
| ① 「Delete app data」默认未勾 | **默认不勾选** | 确认页是 Tauri 原生页（`hooks.nsh:6-7` 注释「默认不勾 = 保留数据」）→ 实测**数据目录 40 文件一个没少** |
| ② 只弹一次询问框、默认「是」，无第二勾选项 / 无「下次不再问」 | **是** | `hooks.nsh:15-17`：只有**一个** `MessageBox MB_YESNO\|MB_ICONQUESTION`，`/SD IDYES` = 默认「是」，`IDNO` 跳 `cg62_keep_clients` |
| ③ 一条信息框「已把三个客户端的配置还原成官方状态…」 | **是** | `hooks.nsh:25-27`，与卡 §3 第 3 条一字不差 |
| ④ 失败分支「没能自动还原（退出码 N）」 | **没弹出** | 两个框是 if/else（`hooks.nsh:19-28`）→ `--restore-clients` **退出码 0** |

**卸载后实测**：程序目录**已删除**、卸载项**已清**、Run 值**查不到**、网关进程**已退出**、数据目录 **40 文件保留**、`HKCU\Software\claudegateway` **仍在**（与本票遗留 2 一致）。

**还原后五份客户端文件**（mtime 全 `22:52:10`）：

| 文件 | 还原后 | 与 M4 官方态对比 |
| --- | --- | --- |
| Desktop `_meta.json` | 175 B `492A09C2AF9F4773C0A2B52A5669DABD5E8C4FC894DE4122A43E872B9E0692EE`，`appliedId` 回 `ac278ce7-…`（Default） | **与本票「四、卸载」记的 `492A09C2…` 逐字相同** |
| Desktop profile `9f1c2b3a-….json` | 4 B `E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F`，内容 `{}` | **与本票记的 `E3566B3A…` 逐字相同** |
| `.claude\settings.json` | 36 B `DA00E2B1460E5B04DEDB64D39BC0C96CDBFF8B03E0C3489406D410BE2753E6EB`，只剩 `autoUpdatesChannel` | **与本票记的 `DA00E2B1…` 逐字相同** |
| `.codex\config.toml` | 2660 B `8AA266D6…`，`claudegateway` **零残留**，Codex 自己的 `notify` 行**保留** | 大小与 M4 的 1167 B 不同（Codex 后来自己加了很多内容），按卡 §3 说明**不比哈希** |
| `.codex\claudegateway-model-catalog.json` | **已消失** | 卡 §3 判据「网关建的那份消失」→ 过 |

**摘键还原语义正确**：网关插的键全摘掉，Codex 自己写进同一文件的 `notify` 一行没被顺手删——「还原 ≠ 整份覆盖」当场验证。

### 4. 走完恢复现场（卡 §3 末尾两行）

用户选择：**装回生产 0.2.0 + 三张卡点回「已生效」**。

1. 从 Release 下载 `Claude.Gateway_0.2.0_x64-setup.exe`：**6083879 B，SHA256 `A1B3DF0F9B1ED42312AA24C931B405F1312FD9EA9417AEE3541BA995E8DCFA85`**；与本地 `cg-notes\67-release2\claude-gateway-v0.2.0\nsis\` 那份**逐字节相同**。
2. `/S` 装回：**exit 0（569 ms，非管理员、无 UAC）**；启动后 `/api/status` → `version=0.2.0 port=8787 dbPath 逐字不变`；数据目录 **40 文件**。
3. **apply 之前三张卡是 `official`（已回官方）** —— 卡 §3 那句判据（三张卡变「已回官方」）在网关重新跑起来后现场拿到。
4. 三个客户端各 apply 一次 → **三张卡 `effective`**；徽标 `upstreamsFailedSelfcheck=0 clientsNotEffective=0 ready:true`（因第 0 节重跑过自检）。
5. **五份文件回到卸载前逐字节相同**：`275 B A26A1344…` / `440 B 0EE2EE7B…` / `366 B 9FD4F341…` / `3136 B CEBEABF1…` / `6367 B 2309BB41…`（`config.toml` 与卸载前那份含 `notify` 的完全相同 → 摘键还原 + 重写是**逐字节往返**的）。
6. 四个槽位仍是 `commandcode / deepseek/deepseek-v4.1-flash / openai-chat`，**未改**。

### 5. 未做 / 异常 / 交给用户

1. **卡 §3 第 5 条（重装后再卸一次、选「否」验五个文件不动）未做**——用户当场选择不做；「否」分支（`hooks.nsh:17` 的 `IDNO` 跳转）仍只有源码证据，**没有现场实测**。
2. **抖动本轮共 3 次成片 504**（四槽位探针两轮 + 端到端验收两次，`logId` 76 / 83 为 504）；每次判别法都成立、重试即过，未改任何绑定。
3. **界面无「更新端点」面**（卡 §1 第 3 条无对应 UI）：未改判据、未开新票，等用户定夺。
4. 三个客户端要**完全退出重开**才读到刚写回的配置（Desktop 不热重载；Codex 模型目录只在启动时读）。
5. `HKCU\Software\claudegateway` 仍在（只有勾 Delete app data 才清）——与本票遗留 2 一致，属预期。
6. 本机 `%TEMP%` 下留了一份本次安装用的 Release 安装包（6083879 B）。
7. 收工检查：`git status` 除本 Answer 外**无其它改动**（`M .scratch/model-gateway/issues/63-task-m4-exit-acceptance.md`，+111 行）；`main` 与 `origin/main` 同为 `eb3ab54`；tag `v0.2.0` 仍指 `3ebfb5f`，**未重发、未移动**。

**用户签字（收尾第三轮）**：三件人办项均当场走过——① 设置页「检查更新」界面文案为 `0.2.0` + 「已是最新版本」，回退指引指到 `updates\installers\`（界面无「更新端点」一项）；② 三个客户端各真问一句，全部拿到真实回答；③ 交互式卸载三处默认值（Delete app data 未勾 / 只一个询问框且默认「是」/ 一条还原成功信息框）与实测清理、还原结果均正常，随后按卡装回并点回「已生效」。

### 6. 补记（2026-10-16，同一轮）：签名私钥离线备份

顺带办掉 START-HERE「待办」的最后一条（离线备份 `cg-notes\keys\`）：

- 把 `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\` 整目录（`updater.key` 348 B / `updater.key.password.txt` 32 B / `updater.key.pub` 152 B）拷到外置介质的一个专用目录；**副本是直接读介质侧文件算的 SHA256**（证明介质真读得出来，不是写缓存），三份与源**逐字节相同**：私钥 `C354501D2C9EA2EE1CB86DCDDF28E6FFEEE2C350721B4BF626CC9C6168AFE1CD`、公钥 `DDA518E0C5AE7420D1CBCE312C90E5D1A53FCE638DB2B4036C709B5CCDE438DF`（口令那份**不打印哈希**）。用户当场弹出并拔下介质 → 副本已离线。
- 备份前核过两件事：① `keys\updater.key.pub` 的内容与 `src-tauri/tauri.conf.json:42` 的 `pubkey` **逐字相同**（152 字符），且这串也存在于已安装的 `claude-gateway.exe` 里 → 备份的正是出厂 app 信任的那把私钥；② `gh secret list -R ColoristSansi/claude-gateway` 两个 Secret 仍在（`TAURI_SIGNING_PRIVATE_KEY` 2026-10-16T10:59:58Z / `..._PASSWORD` 11:00:00Z），与 `.github/workflows/ci.yml:57-58` 引用同名。
- **存放位置不写进本票、不写进仓库**，由用户自己记在密码管理器里。介质是 exFAT（**无 ACL**），保护只靠物理保管；已当场提醒「`updater.key.password.txt` 与私钥同盘 = 谁拿到盘谁就能签名」，可选加固是把口令另存进密码管理器。
- 本机源目录保持原位未动（`pack.ps1 -Sign` 要给 `-PrivateKeyPath`，CI Secrets 的值也来自它）。
- 未做功能验证（用副本私钥真签一个包）：源与副本 SHA256 逐字节相同，副本可签的前提就是源可签，故未重跑；签名链路本轮未改一行。

### 7. 后续（2026-10-17）：现场卡 §1 ③ 改判据 + 两处过时注释

- **现场卡 §1 第 ③ 条已修订**：`cg-notes\accept-live.md` 由「界面显示更新端点」改成「机器侧核实端点三处」（`tauri.conf.json` 的 `plugins.updater.endpoints` / 已安装 exe 含该字面量且不含 `127.0.0.1:8791` / `GET /api/update/status` 的 `endpoints[0]`），卡头记了修订日期与原因。**界面仍不加这一行**——后端 `/api/update/status` 本来就回 `endpoints`（`src-tauri/src/updater.rs:52` 的注释写明是「给界面显示」用），是**前端从没读它**；要恢复「界面看得见」按卡里那句开新票，前端约 6 行、后端零改动。
- **两处过时注释改掉**（票 65 落地后没同步，**纯注释、无行为变化**）：`crates/gateway-core/src/egress/mod.rs:18-19` 原写「上游 Responses wire 未做」、`crates/gateway-core/src/lib.rs:30` 表格里同一句。事实是 `responses.rs:318 encode_request` / `:965 ResponsesDecoder` 已被 `egress/upstream.rs` 的四个调用点在用。
- **门禁**（**全新 `CARGO_TARGET_DIR` = `cg-target\round3`**）：`pwsh -File .\check.ps1 -Quick -Replay` → `全部通过。` **exit 0，358 条通过 / 15 条 `#[ignore]`**（与 START-HERE 记的数字一致），含前端 `tsc --noEmit && vite build`、回放 23 格；日志 `cg-notes\accept3-check.log`。

