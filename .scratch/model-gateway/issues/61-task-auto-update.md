# 自动升级

Type: task
Status: resolved
Blocked by: 54

## Question

把升级闭环做起来：公开 GitHub Releases + `latest.json` + 应用内检查 + 签名校验 + passive 静默安装 + 自动重启。**升级不改端口、不改数据、不改客户端配置。**

依据：spec §12 M4、[票 18](../issues/18-grilling-packaging-and-updates.md)。

## 交付

- tauri updater 配置：https 强制、签名校验不可关、`installMode` passive/quiet 可静默装、装完 `exit(0)` 由 updater 拉起。
- 签名密钥对：公钥进应用配置；**私钥进 GitHub Secret，另留一份交付用户保管**（说明它丢了以后再也发不了升级）。
- 应用内「检查更新」与提示；用户确认后静默安装并自动重启。
- CI 打 tag 产出签名产物与 `latest.json`。
- 数据目录另留一份上一版包 + 回退指引（写在设置页「关于」或帮助里）。

## 收口验收

- 本机装 v1 → 发一个 v2 release → 应用内升级成功、重启后版本为 v2、端口与数据与客户端配置都没变。
- 签名校验故意用错公钥时拒绝安装并给出可读错误。

## Answer

2026-10-16 完成。范围按用户拍过的：**先只做本地闭环**，GitHub 那半步（建公开仓库、配 Secret、发 Release）留给票 63 现场（第 6 节逐条列清，含界面步骤与逐条命令）。

分支 `task/61-auto-update`。本票的提交：

| commit | 内容 |
| --- | --- |
| `4b0514a` | 票 61 实现（内核留包/回退指引 + 管理 API + Tauri updater + 配置 + 打包脚本 + 本地源 + 前端横幅 + 判据测试） |
| `b051b2d` | merge 集成分支（票 52 前端四页 + 票 58），冲突就地解决 |
| `6283f5e` | 检查更新 / 安装改成幂等（界面重复点不是错误） |
| `282a8a5` | merge 集成分支到 `3321afa`（票 55 收口 / 票 59 设置页与用量页 / 票 60 收口 / 票 60b 白屏修复）；顺带把票 59「关于与版本」那格的占位接到本票的数据源，并接上设置页的「检查更新」按钮 |

收工 tip = `282a8a5`：门禁 `pwsh -File ./check.ps1 -Quick -Replay` → **`全部通过。`**（exit 0；unit 177 + 各集成格 + 回放 22 + 回放器 5），`pnpm --dir app build` 绿。三条收口验收都跑在它上面（第 5 节）。

### 1. 做了什么

| 交付 | 落点 |
| --- | --- |
| `updates\` 留包 + 升级回执 + **可执行**回退指引 + 更新状态形状 | **`crates/gateway-core/src/dist/update.rs`**（不依赖 Tauri，裸跑可测） |
| `GET /api/update/status`、`POST /api/update/check`、`POST /api/update/install` | `crates/gateway-core/src/ingress/api.rs`（外壳没递 updater 进来就 **503**） |
| 检查 / 下载 / 验签 / passive 静默安装 / 装完自动重启 | **`src-tauri/src/updater.rs`**（`ShellUpdater` 实现 `UpdateSource`）+ `src-tauri/src/main.rs` 注册插件 + `shell.rs` 接线与「启动后静默检查一次」 |
| updater 配置 | `src-tauri/tauri.conf.json`（生产，第 2 节逐字）；`src-tauri/tauri.conf.test.json`（**只给本地验收**） |
| 签名产物 + 真 `latest.json` | `pack.ps1`（新增 `-Sign` / `-BaseUrl` / `-PrivateKeyPath` / `-PrivateKeyPassword` / `-Config`） |
| 本地更新源 | **`serve-updates.ps1`**（新：`127.0.0.1:8791`，只读、只认 GET/HEAD、路径出不去） |
| 应用内「检查更新」与提示 | `app/src/UpdateBanner.tsx`（新，全局横幅）+ `app/src/pages/SettingsPage.tsx`「关于与版本」那格的按钮（票 59 留的禁用占位，本票接上） |
| 设置页「关于」要的回退指引 | `ingress/api/settings.rs::about` 改从 `dist::update` 取（指引原文 + `updates\installers\` 真路径 + `update.supported: true`） |
| CI 打 tag 出签名产物与清单 | `.github/workflows/ci.yml` 的 bundle job（`TAURI_SIGNING_PRIVATE_KEY*` 缺省时退化成占位清单 + 警告，不红） |
| 判据钉死 | `crates/gateway-core/tests/update_guard.rs`（安全开关 + 升级不动端口/数据/客户端配置）、`crates/gateway-core/tests/update_api.rs`（接口面与错误面） |

依赖只加了一个：`tauri-plugin-updater = "2"`（解析到 2.14.0，要求 tauri ≥ 2.12，本仓 2.12.4）。它带来的 `reqwest 0.13` 与内核的 `reqwest 0.12` 在树里并存，互不影响。

**升级不改端口、不改数据、不改客户端配置**——机制，不是靠自觉：

- 装的是同一目录：NSIS `installMode: currentUser` + `RestorePreviousInstallLocation`（复用注册表里的安装位置），升级不换目录、不提权；
- 升级走 NSIS `/UPDATE`：`installer.nsi` 在 update 模式下跳过数据删除、快捷方式与 Run 键清理；`src-tauri/nsis/hooks.nsh` 的 PREUNINSTALL 还原段整段被 `${If} $UpdateMode <> 1` 挡住，所以升级**不重写**三个客户端的配置；
- 端口来自 `settings` 表（票 52 起唯一真相，`config.json` 只当迁移兜底），升级路径一个字节都不碰它；
- 应用侧升级只写 `updates\` 一个目录；包先落 `updates\installers\`，再交给插件跑安装器。

`update_guard.rs` 把上面每一条都写成断言（含 `DEFAULT_PORT == 8787`、生产端点必须 https、三个 `dangerous*` 一个都不许开、`requireSignedVersion == true`、`installMode == passive`、hooks 的守卫必须在、`latest.json` 两把平台键都在），谁改配置都会被门禁挡住。

### 2. updater 配置（逐字）

生产 `src-tauri/tauri.conf.json`：

```json
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/OWNER/REPO/releases/latest/download/latest.json"
      ],
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE1NEU1ODZGNUM3M0VDNDIKUldSQzdITmNiMWhPcGNpS2tLc3daQitKaWlUamEvU1B0NXVSaThRRXhvSnpMRmMvVUxva0pTeGgK",
      "requireSignedVersion": true,
      "windows": {
        "installMode": "passive"
      }
    }
  }
```

- `OWNER/REPO` 是**占位**：公开仓库由票 63 现场建，建完把这一处换成真仓库（第 6 节第 1 条）。
- **一个危险开关都没开**（没有 `dangerousInsecureTransportProtocol` / `dangerousAcceptInvalidCerts` / `dangerousAcceptInvalidHostnames`）。release 构建里 http 端点会直接报 `InsecureTransportProtocol`——这是判据，**不许**为了让本地 HTTP 跑通去关它。
- `requireSignedVersion: true`：签名里必须带它签的那个版本（`signer sign --app-version` 写进 trusted comment），挡住「拿旧包的合法签名配一个新版本号」的降级攻击。实测 `.sig` 解出来是 `trusted comment: timestamp:… file:Claude Gateway_0.2.0_x64-setup.exe version:0.2.0`。
- 签名校验**不可关**：插件 `download()` 先 `verify` 再给字节，验不过就没有字节；我们的错误映射再把原因翻成人话（第 5.2 节贴原文）。

**本地验收专用覆盖配置** `src-tauri/tauri.conf.test.json`（`cargo tauri build --config` 深合并到生产配置上）：

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "plugins": {
    "updater": {
      "endpoints": ["http://127.0.0.1:8791/latest.json"],
      "dangerousInsecureTransportProtocol": true
    }
  }
}
```

哪部分只在 dev/测试用、生产是什么：

| 项 | 生产（`tauri.conf.json`） | 本地验收（`tauri.conf.test.json` 覆盖） |
| --- | --- | --- |
| 端点 | `https://github.com/OWNER/REPO/releases/latest/download/latest.json` | `http://127.0.0.1:8791/latest.json` |
| 协议强制 | 强制 https（不开危险开关） | 显式开 `dangerousInsecureTransportProtocol`，**只在这一份文件里** |
| 公钥 / `requireSignedVersion` / `installMode` | 生效 | 不覆盖，仍来自生产配置 |
| 装的包 | 同一份 NSIS，只有内嵌端点不同 | 同上 |

实测证据：用 `--config` 打的 release exe 里能搜到 `127.0.0.1:8791/latest.json` 与公钥字符串、**搜不到** `github.com/OWNER/REPO`；反过来，收尾装回机器的**正式**包（不带 `--config`）里能搜到 `github.com/OWNER/REPO`、搜不到本地端点。

### 3. 签名密钥：路径与纪律

| 项 | 值 |
| --- | --- |
| 私钥 | `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key` |
| 私钥口令 | `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key.password.txt` |
| 公钥 | `C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key.pub`（内容已进 `tauri.conf.json`） |
| 权限 | 三个文件都 `icacls /inheritance:r` + 只留当前用户读 |
| 生成命令 | `cargo tauri signer generate --ci -p <口令> -w C:\...\cg-notes\keys\updater.key` |

**丢了以后再也发不了升级**：公钥已经编进所有用户手里的程序，只有这把私钥能签出被接受的新版本；私钥或口令丢**一个**，就只能换密钥对 + 让用户手动重装一次新版。请把 `cg-notes\keys\` 整个目录当用户资产，离线备份（U 盘 / 密码管理器），别只留在这台机器上。

纪律：私钥与口令**只在仓库外**；`git grep "rsign encrypted secret key"` 在仓库里搜不到任何密钥内容（`update_guard.rs` 里那条同名字符串是断言用的**检查**，不是密钥）；票据、日志、测试夹具里都没有明文；`pack.ps1` 只打印私钥**路径**，口令走环境变量（不进进程命令行）。

### 4. 本地闭环怎么起（三条验收用的命令）

```powershell
# 一次性：仓库根、本票 target
$env:CARGO_TARGET_DIR = "C:/Users/ZHANQUAN/AIGC/Project/cg-target/61"

# ① 打 v1（0.1.0，本地验收覆盖配置）并装上
pwsh -File .\pack.ps1 -Config src-tauri/tauri.conf.test.json
Start-Process "<target>\release\bundle\nsis\Claude Gateway_0.1.0_x64-setup.exe" -ArgumentList /S -Wait
Start-Process "$env:LOCALAPPDATA\Claude Gateway\claude-gateway.exe"

# ② 抬版本到 0.2.0（tauri.conf.json + src-tauri/Cargo.toml 两处，打完还原）、打 v2 并签名
pwsh -File .\pack.ps1 -Config src-tauri/tauri.conf.test.json -Sign `
  -BaseUrl http://127.0.0.1:8791 `
  -PrivateKeyPath C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key `
  -PrivateKeyPassword (Get-Content C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key.password.txt -Raw)

# ③ 起本地更新源（把 <target>\release\bundle\updates\ 挂到 127.0.0.1:8791）
pwsh -File .\serve-updates.ps1 -Port 8791 -Root "<target>\release\bundle\updates"

# ④ 应用内升级（界面上的「下载并安装」按钮打的就是这两个请求）
Invoke-RestMethod -Method Post http://127.0.0.1:8787/api/update/check
Invoke-RestMethod -Method Post http://127.0.0.1:8787/api/update/install
Invoke-RestMethod            http://127.0.0.1:8787/api/update/status
```

`latest.json`（`pack.ps1 -Sign` 产出的真清单，节选）：

```json
{
  "version": "0.2.0",
  "notes": "Claude Gateway 0.2.0（票 61：签名产物与 latest.json 由 pack.ps1 -Sign 产出）",
  "pub_date": "2026-10-15T17:56:30Z",
  "platforms": {
    "windows-x86_64-nsis": { "signature": "dW50cnVzdGVk…", "url": "http://127.0.0.1:8791/Claude%20Gateway_0.2.0_x64-setup.exe" },
    "windows-x86_64":      { "signature": "dW50cnVzdGVk…", "url": "http://127.0.0.1:8791/Claude%20Gateway_0.2.0_x64-setup.exe" }
  }
}
```

两把平台键都写：装机版（`bundle_type = nsis`）插件先查带安装器名的那把，免安装 zip 版只查不带的那把。

### 5. 收口验收（逐条真跑，输出为原文摘录）

三条都在收工 tip `282a8a5` 上跑；进程收尾一律**精确 PID + 精确 exe 路径**，不按进程名模糊匹配。

#### 5.0 基线与环境

```
8787 上的进程：pid=31772 exe=C:\Users\ZHANQUAN\AppData\Local\Claude Gateway\claude-gateway.exe
现状：version=0.1.0 port=8787 dbPath=…\com.claudegateway.desktop\gateway.db
当前会话是管理员: False
（三客户端状态见 /api/clients；槽位绑定见 /api/bindings，验收前后逐键相同——见 5.4）
```

#### 5.1 本地闭环：装 v1 → 本地源发 v2 → 应用内升级成功（端口 / 数据 / 客户端配置都没变）

```
静默安装退出码 = 0
v1 答话：version=0.1.0 port=8787 dbPath=…\com.claudegateway.desktop\gateway.db
升级端点（status.endpoints）= http://127.0.0.1:8791/latest.json     ← 本地覆盖配置确实生效
=== 快照 A（升级前）：version=0.1.0 port=8787，数据目录 36 个文件，五份客户端文件 sha256 见下 ===

本地更新源已起：pid=24892 → …\cg-target\61\release\bundle\updates
check → current=0.1.0 available=0.2.0
        url=http://127.0.0.1:8791/Claude%20Gateway_0.2.0_x64-setup.exe
install（立刻应答）→ installing=True
等安装器把新版拉起来（passive 静默装 + 装完自动重启）……
重启后：version=0.2.0 port=8787 dbPath=…\com.claudegateway.desktop\gateway.db
装出来的 exe 版本：0.2.0  大小=23912960

—— updates\ 里留了什么 ——
  \updates\last-update.json  121 字节
  \updates\installers\Claude Gateway_0.2.0_x64-setup.exe  6111637 字节
last-update.json：
{ "from": "0.1.0", "to": "0.2.0", "at": "2026-10-15T18:10:02Z", "package": "Claude Gateway_0.2.0_x64-setup.exe" }

=== 快照 B（升级后）：version=0.2.0 port=8787，数据目录 36 个文件 ===
```

对比（`acc4b-compare.ps1 -A A -B B`，判据全部为真）：

```
[① 端口 / 库路径]
    A: port=8787 dbPath=…\com.claudegateway.desktop\gateway.db
    B: port=8787 dbPath=…\com.claudegateway.desktop\gateway.db
    端口与库路径完全一致 = True
[② 数据]
    A 文件清单 36 项；B 文件清单 36 项；清单一致 = True
    大小有变化的文件（1 个）：logs\gateway.log: 4843 → 6038 字节      ← 只有网关自己的运行日志（就是这次验收写的）
    配置数据（--export，抹掉 exportedAt/appVersion 后）sha256：
      A = 90B12DC535961FB80EF10BCDB1376304C10DAAB601E7C6D038B4A839F6DB2FEE
      B = 90B12DC535961FB80EF10BCDB1376304C10DAAB601E7C6D038B4A839F6DB2FEE   一致 = True
      settings.port：8787 vs 8787
[③ 客户端配置]（五份文件逐个 SHA256，A → B 全 True）
    9f1c2b3a-…json  0EE2EE7B7FF373BC18CE4E5870CF70D54A21564661AB1EF79AF968B30D3EEA73
    _meta.json      A26A134435727E72B35C8907ECAF0F65DD11AA0D824D066AB250E028BEE5D595
    .claude\settings.json            9FD4F341EA2E736B0543E1DBE06E37F1DE1ADC0B5E0AC17BAD09A8086F795127
    .codex\config.toml               D0948B1DFDC664F3D19219A7DD93A41CC142076E61A7C51A21AE341BEB74DF3B
    .codex\claudegateway-model-catalog.json  2309BB41E360301A8EBF1C0370CE56B2363DA3AD0DEE2F7BC7184A786AB1A0D9

=== 结论 ===
  端口/库路径未变            True
  数据目录清单未变            True
  配置数据未变(归一化 export sha256)  True
  除自身日志外文件字节未变      True
  五份客户端配置未变          True
全部通过。
```

补充说明「配置数据未变」这条为什么不直接比 `--export` 文件的字节：导出文件里带 `exportedAt`（导出时刻）与 `appVersion`（导出的程序版本，0.1.0 → 0.2.0），这两处**本来就该变**；抹掉这两个字段后两次导出**逐字相同**（上面那对 sha256 就是这么算的）。

#### 5.2 签名校验故意用错公钥时拒绝安装，并给出可读错误

做法：用一把**一次性**密钥（只在 `%TEMP%\cg61\wrongkey` 里活，跑完删掉，不进仓库/票据）把 v2 的包签成 0.3.0，本地源照常发出去：

```
check → current=0.1.0 available=0.3.0 url=http://127.0.0.1:8791/Claude%20Gateway_0.3.0_x64-setup.exe
install（立刻应答）→ installing=True

—— 应用给出的错误原文 ——
安装更新失败：签名校验不过，已拒绝安装：这个包不是用「应用配置里那个公钥」对应的私钥签的
（The signature was created with a different key than the one provided）。请检查更新源是不是用错了密钥——不要绕过校验。

拒绝之后：installing=False available=0.3.0
updates\installers 里有什么（被拒的包不该落盘）：
  （目录还不存在）
版本没变（还是 0.1.0）：0.1.0
```

三件事同时成立：**拒绝安装**（版本没动）、**给出可读错误**（上面那句，含插件的原文）、**被拒的包一个字节都没落盘**（验签在 `save_package` 之前）。

#### 5.3 回退指引可执行（按写出来的步骤真退回 v1）

先看没有旧包时的那一支（指引自己给了可执行的做法）：

```
当前版本：0.2.0
本地留着的包：1 个   Claude Gateway_0.2.0_x64-setup.exe  版本=0.2.0
建议回退目标（previous）= <null>
指引第 2 步：本次是**首次安装**（或本地没留更旧的包），回退要用你当初下载的那一份安装包；
     也可以把旧版安装包拷进 …\com.claudegateway.desktop\updates\installers 再按下面的步骤做。
```

照指引把 v1 的包放进 `updates\installers\`（指引第 4 条那句话），再取一次指引——它现在**指名道姓**给出该双击哪个文件：

```
2. 双击这一个安装包：
     C:\Users\ZHANQUAN\AppData\Roaming\com.claudegateway.desktop\updates\installers\Claude Gateway_0.1.0_x64-setup.exe
4. 数据、端口、客户端配置**都不动**：…（升级时 NSIS 走 /UPDATE，跳过数据与客户端的清理）
5. 想再回到新版：装 …\updates\installers 里版本最高的那一个。
```

然后按第 1/2/3 步真做（第 1 步＝托盘「退出」，脚本里按精确 PID 停）：

```
第 1 步：退出 Claude Gateway —— 已按精确 PID 停掉安装版 gateway：21040；8787 空闲
第 2 步：装指引指定的那个包 —— 安装退出码 = 0
第 3 步：打开，核对版本 —— 回退后：version=0.1.0 port=8787 dbPath=…\gateway.db
=== 快照 C：version=0.1.0，数据目录 36 个文件 ===
```

回退前后对比（`-A A -B C`）同样全绿：

```
端口与库路径完全一致 = True；文件清单 36 = 36 一致 = True
大小有变化的文件（1 个）：logs\gateway.log: 4843 → 6183 字节
配置数据（归一化）sha256 A = C = 90B12DC535961FB80EF10BCDB1376304C10DAAB601E7C6D038B4A839F6DB2FEE
五份客户端配置 A → C 全 True
结论：五项判据全部 True，全部通过。
```

#### 5.4 门禁、前端构建，与跑完之后的机器状态

```
pwsh -File ./check.ps1 -Quick -Replay   →   全部通过。   (exit 0)
  fmt 干净；clippy 0 警告（含 mock 特性那一遍）；unit 177（含本票 dist::update 10 条 + update_api 4 条 + update_guard 4 条）；
  回放 22 用例；回放器 5 单测
pnpm --dir app build                    →   tsc --noEmit && vite build，✓ built（303 kB）
```

跑完把机器恢复成「应用在跑 + 数据目录与客户端/槽位原样」：

```
静默卸载退出码 = 0；安装目录还在吗: False
数据目录已按备份恢复（36 个文件）
客户端文件被改过（卸载器还原），已放回：…（四份）
客户端文件被卸载器删了，已从备份放回：C:\Users\ZHANQUAN\.codex\claudegateway-model-catalog.json
装回正式包：%TEMP%\cg61\formal\Claude Gateway_0.1.0_x64-setup.exe（生产配置）→ 安装退出码 = 0；卸载项 DisplayVersion = 0.1.0
起来后：version=0.1.0 port=8787 dbPath=…\gateway.db（8787 上 pid=…，exe=%LOCALAPPDATA%\Claude Gateway\claude-gateway.exe）
三个客户端状态（验收前 → 现在）：Claude Desktop 已生效 → 已生效；Claude Code 已生效 → 已生效；Codex 已生效 → 已生效
五份客户端文件 sha256 与验收前逐字一致：True × 5
四个槽位绑定逐键一致（claude-fable / claude-opus / claude-sonnet / claude-haiku → commandcode / deepseek/deepseek-v4.1-flash / openai-chat；虚拟模型 command/deepseek 不变）
一次性密钥目录已删
```

**注意**：「卸载器会按默认（还原客户端配置）把三个客户端切回官方」这条在验收里真的触发了（票 62 的 `hooks.nsh`，默认答案＝还原）——这是产品行为、不是意外；我用验收前的备份把那 5 份文件逐字节放了回去（上面 sha256 全 True）。**另外**：`/api/clients` 里 claude-code 的 `backupPreview` 指向的备份文件名时间戳与验收前不同（一次卸载-还原循环留下的记账值，`backups\claude-code\` 里现在只有 6 份、跟备份完全一致），客户端配置本身逐字节相同。

### 6. 留给票 63 的 GitHub 步骤（逐条；本票按用户拍的范围没碰 GitHub）

1. **建公开仓库**：`gh repo create <owner>/<repo> --public`（或网页 New repository → Public → Create）。建完把 `src-tauri/tauri.conf.json` 里 `endpoints` 的 `OWNER/REPO` 换成真值（**一处**），提交。
2. **配两个 Secret**（私钥是用户资产，只从 `cg-notes\keys\` 取，别从聊天记录抄）：
   - 界面：仓库 → Settings → Secrets and variables → Actions → **New repository secret** →
     Name `TAURI_SIGNING_PRIVATE_KEY` → Secret 填 `updater.key` 的**文件内容**（不是路径）→ Add secret；
     再 New repository secret → Name `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` → 填 `updater.key.password.txt` 的内容 → Add secret。
   - 命令行等价：`gh secret set TAURI_SIGNING_PRIVATE_KEY < C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\updater.key`（口令那条同理，`gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD`）。
3. **抬版本并打 tag**：`src-tauri/tauri.conf.json` 与 `src-tauri/Cargo.toml` 两处**一起**抬（`/api/status` 报的是后者、updater 比对的是前者，必须一致），然后 `git tag v0.2.0 && git push origin v0.2.0`。
4. **等 CI 出包**：tag 触发 `bundle` job，跑 `pack.ps1 -Sign -BaseUrl https://github.com/<owner>/<repo>/releases/download/<tag>`，产出 NSIS 安装包、免安装 zip、`bundle\updates\latest.json`（带签名）与 `.sig` 并上传 artifact。**先看 job 日志里有没有「没有 TAURI_SIGNING_PRIVATE_KEY…」那句警告**——有就说明 Secret 没生效，别往下走。
5. **建 Release 并上传资产**：至少要有 `Claude Gateway_<版本>_x64-setup.exe` 与 `latest.json`；`latest.json` 里 `platforms.*.url` 指向的就是这个 Release 的资产地址（资产名里的空格 GitHub 会写成 `%20`，我们的清单里也是 `%20`，对得上）。建议：
   `gh release create v0.2.0 --draft --title "Claude Gateway 0.2.0" --notes "…" "<target>\release\bundle\updates\latest.json" "<target>\release\bundle\nsis\Claude Gateway_0.2.0_x64-setup.exe"`
   先用 `--draft` 看一眼：`curl -sI <latest.json 的 url>` 要能拿到 200，再 `gh release edit v0.2.0 --draft=false`。
6. **现场验一次真 https 闭环**：装一份**生产配置**的 0.1.0（不带 `--config`，端点指真仓库的占位已换成真值）→ 启动 → 设置页「关于与版本」点「检查更新」或顶栏横幅 → 看到 0.2.0 → 「下载并安装」→ 装完重启后 `/api/status` 的 `version` 是 0.2.0。收尾自查：`git grep 127.0.0.1:8791 src-tauri/tauri.conf.json` 必须是**空**。
7. 把「首次运行会有 SmartScreen『Windows 已保护你的电脑』→ 更多信息 → 仍要运行」写进 README（票 18 §1 已定不买证书），并把上面几步的实测结果写进票 63 的 Answer。

### 7. 遗留与坑

- **`latest.json` 换位置了**：票 54 把占位清单写在 `bundle\zip\`，本票起清单只有一份、在 `bundle\updates\`（与安装的包同源，URL 才对得上）；CI 的 artifact 路径也跟着加了 `bundle/updates/*`。
- **签名没走 `createUpdaterArtifacts`**：改用 `pack.ps1 -Sign` 显式调 `cargo tauri signer sign --app-version`。理由：开发机与 CI 的 check job 都不该因为缺密钥而构建失败，而 `createUpdaterArtifacts` 会在缺密钥时让 `tauri build` 报错。
- **启动后静默检查延后 15 秒**（`std::thread::sleep` + `block_on`，不动 Tauri 的异步时间驱动）：离线 / 端点不通只写进 `/api/update/status.error`，不弹窗；`POST /api/update/check` 与 `/api/update/install` 都已幂等（重复点不是错误）。
- **装完自动重启是安装器干的**：插件 `ShellExecute` NSIS（`/P /UPDATE /R /ARGS …`）后 `exit(0)`，重启由 `/R` 完成；所以 `/api/update/install` 是**立刻应答**，前端靠轮询 `status` 看进度，进程被换掉时那一轮 fetch 失败是正常的。
- **免安装 zip 版不参与自动更新**（票 18 §1）：它没有安装器信息，插件只查 `windows-x86_64` 那一档；清单里两把键都写了。设置页对 zip 版显示「此版本请手动替换」（`about.portable.note`）。
- **回退是「照指引装旧包」，不做一键按钮**（票 18 §3 Q11）：只有「升级上去的版本」会自动留包；手工装的那一版要用户自己留，指引里两种分支都写明了。
- **卸载器默认还原客户端配置**（票 62 的行为）：验收这次真触发了，逐字节放回无残留；但**以后谁在这台机器上卸载正式包，三个客户端都会切回官方态**——这是预期行为，票 63 现场验收时要知道。
- **`/api/about` 是票 59 的端点**，本票只把它的回退指引 / `updatesDir` / `packages` / `update.supported` 换成真的（并顺手删掉那个只为占位服务的 `list_files` 辅助函数与它的单测，改成一条针对真数据源的单测）。
- **本地验收留下的东西**：`%TEMP%\cg61\` 下有脚本与产物快照（不在仓库里）；`cg-target\61\release\bundle\updates\` 里还有一份**验收用的** 0.2.0 test 包与签名（本地源用），发布时必须用 CI 重新打。
- **收尾装回机器的是本分支打的正式 0.1.0**（生产配置、端点仍是 `https://github.com/OWNER/REPO/...` 占位）：所以在那台机器上点「检查更新」会失败（占位仓库不存在），错误会显示在界面上。这是票 63 把 `OWNER/REPO` 换成真仓库之前的**预期**状态。
