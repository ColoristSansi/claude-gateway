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

