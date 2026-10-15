# 打包：NSIS per-user + 免安装 zip

Type: task
Status: resolved
Blocked by: 43

## Question

把应用打成能装的东西：NSIS **per-user** 安装包（装进 `%LOCALAPPDATA%`、不需要管理员）+ 手工打包的免安装 zip。不签名（[票 18](../issues/18-grilling-packaging-and-updates.md)：官方无 portable bundle，zip 不参与自动更新）。

依据：spec §12 M2、[票 18](../issues/18-grilling-packaging-and-updates.md)。

## 交付

- `tauri.conf.json` 的 bundle 段：NSIS target、per-user 安装模式、图标、`productName = Claude Gateway`、identifier `com.claudegateway.desktop`（一经发布不得改）。
- 免安装 zip 的打包脚本（手工，进 `check.ps1 -Smoke` 的产物清单或单独脚本）。
- 数据目录 `%APPDATA%\com.claudegateway.desktop\`：安装版与 zip 版共用；设置页只读路径 + 打开目录，不提供搬家。
- CI 打 tag 出包：NSIS + zip + `latest.json` 占位（真签名与升级在 [票 61](../issues/61-task-auto-update.md)）。

## 收口验收

- 本机装 NSIS（不弹 UAC）→ 启动 → 能用网关（此时功能按已完成的票）。
- zip 解压即跑，且与安装版读到同一份数据目录。
- 卸载**保留数据**（原生「Delete app data」勾选，默认不删）；还原客户端配置的动作归 [票 62](../issues/62-task-autostart-uninstall-export.md)。

## Answer

2026-10-15 完成。打包骨架在票 39/43 已立好（productName / identifier / NSIS target / installMode 本来就在），本票补 NSIS 语言、免安装 zip 打包脚本、CI tag 出包，并把四条收口验收在合入集成分支 tip（`ac40394`，含票 41/44 前半）之后真跑一遍。

### 做了什么

1. `src-tauri/tauri.conf.json`：`bundle.windows.nsis` 加 `"languages": ["SimpChinese", "English"]`（票 18 §1：跟随系统；**不**开 `displayLanguageSelector`，所以没有语言选择页）。其余字面值一字未动，identifier 更是没碰。
2. 新增 `pack.ps1`（仓库根）：手工打包入口，也是 CI 唯一的打包路径（`cargo tauri build` + 打 zip + 写 `latest.json` 占位）。
3. `.github/workflows/ci.yml` 的 `bundle` job 从占位换成真出包（只在 `refs/tags/*` 上跑）。
4. 数据目录**没改代码**：`crates/gateway-core/src/store/mod.rs::default_data_dir` 早就按 identifier 定位 `%APPDATA%\com.claudegateway.desktop\`；票 44 又把 `Store::open` 接进外壳，所以这份目录现在真的有 `gateway.db`。本票做的是「安装版与 zip 版共用同一份」的实测。

### bundle 配置字面值（`src-tauri/tauri.conf.json`）

```json
"productName": "Claude Gateway",
"version": "0.1.0",
"identifier": "com.claudegateway.desktop",
"build": { "beforeBuildCommand": "pnpm --dir ../app build", "frontendDist": "../app/dist" },
"bundle": {
  "active": true,
  "targets": ["nsis"],
  "publisher": "claudegateway",
  "icon": ["icons/32x32.png", "icons/icon.ico"],
  "windows": { "nsis": { "installMode": "currentUser", "languages": ["SimpChinese", "English"] } }
}
```

- `beforeBuildCommand` 确实把 `app/` 现编进 `frontendDist`：构建日志有 `Running beforeBuildCommand pnpm --dir ../app build` → `dist/index.html 0.34 kB` + `dist/assets/index-BbY27J1q.js 219.64 kB`；随后在产出的 exe 里 `findstr /m index-BbY27J1q.js claude-gateway.exe` 命中。
- per-user 的机制（tauri-bundler 2.10.2 模板 `installer.nsi`）：`INSTALLMODE == currentUser` → `RequestExecutionLevel user`（不请求提权）+ `StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"`。
- `languages` 生效的依据：语言名解析不出来时 bundler 会 `log::warn!("Custom tauri messages for {lang} are not translated…")`（`nsis/mod.rs`），本次打包日志里**没有**任何 warn / unknown language。

### 打包脚本用法

```powershell
# 仓库根；CARGO_TARGET_DIR 已设则用它，否则取 cargo metadata 的 target_directory
pwsh -File .\pack.ps1
```

本次产物（`CARGO_TARGET_DIR=C:/Users/ZHANQUAN/AIGC/Project/Claude Gateway/target`）：

```
target\release\bundle\nsis\Claude Gateway_0.1.0_x64-setup.exe   (3861949 B)
target\release\bundle\zip\Claude-Gateway_0.1.0_x64-portable.zip (5458047 B，内含 claude-gateway.exe 14765056 B)
target\release\bundle\zip\latest.json                           (占位，票 61 才写真签名)
```

`latest.json` 实际内容：

```json
{ "version": "0.1.0", "notes": "占位（票 54）：真签名与升级流在票 61 落地，本文件不可用于自动更新。",
  "pub_date": "2026-10-15T13:12:24Z", "platforms": {} }
```

CI（tag 触发）：checkout → rust stable + pnpm 12.10.1 + node 24 → `cargo install tauri-cli --version 2.12.2 --locked` → `./pack.ps1` → `upload-artifact`（nsis\*.exe + zip 目录）。本地无法验证 runner 那一半（见遗留）。

### 收口验收（逐条真跑，输出为原文摘录）

**0. 预备**：共享 `target` 会被并行票覆盖，先把产物拷到私有路径再验：
`Copy-Item ...\bundle\nsis\*.exe, ...\bundle\zip\* $env:TEMP\cg54`。

**1. NSIS 本机安装（不弹 UAC）→ 启动 → `/api/status` 可用**

```
IsInRole(Administrator) = False                      # 当前会话未提权
Start-Process "$env:TEMP\cg54\Claude Gateway_0.1.0_x64-setup.exe" -ArgumentList '/S' -Wait → 退出码 0
%LOCALAPPDATA%\Claude Gateway\ = claude-gateway.exe (14765056 B) + uninstall.exe
Test-Path "$env:ProgramFiles\Claude Gateway" = False                 # 没进 Program Files
reg query HKCU\...\Uninstall\Claude Gateway →
    DisplayName=Claude Gateway  DisplayVersion=0.1.0  Publisher=claudegateway
    InstallLocation="C:\Users\ZHANQUAN\AppData\Local\Claude Gateway"
    UninstallString="C:\Users\ZHANQUAN\AppData\Local\Claude Gateway\uninstall.exe"
启动后 GET http://127.0.0.1:8787/api/status →
    {"version":"0.1.0","port":8787,"running":true,"startedAt":"2026-10-15T13:16:30Z",
     "dbPath":"C:\\Users\\ZHANQUAN\\AppData\\Roaming\\com.claudegateway.desktop\\gateway.db"}
    监听 8787 的 pid 的 exe = C:\Users\ZHANQUAN\AppData\Local\Claude Gateway\claude-gateway.exe（与启动的 exe 一致）
```

未提权会话里 `/S` 装成功、落在 `%LOCALAPPDATA%` = per-user、无 UAC。

**2. zip 解压即跑，且与安装版同一份数据目录**

```
Expand-Archive ...Claude-Gateway_0.1.0_x64-portable.zip -DestinationPath %TEMP%\cg54\portable
  → 解压后只有一个 claude-gateway.exe（无任何安装步骤）
Start-Process %TEMP%\cg54\portable\claude-gateway.exe
GET /api/status →
    {"version":"0.1.0","port":8787,"running":true,"startedAt":"2026-10-15T13:16:32Z",
     "dbPath":"C:\\Users\\ZHANQUAN\\AppData\\Roaming\\com.claudegateway.desktop\\gateway.db"}
    监听 8787 的 exe = C:\Users\ZHANQUAN\AppData\Local\Temp\cg54\portable\claude-gateway.exe
```

两边 `dbPath` **逐字相同**；`logs\gateway.log` 末尾两条启动行分别来自安装版（13:16:30Z）与 zip 版（13:16:32Z）——同一份数据目录、同一个日志文件、同一个库。数据目录此时有 `gateway.db`(4096) / `gateway.db-shm`(32768) / `gateway.db-wal`(131872) / `logs\gateway.log`。

**3. 卸载保留数据**

```
卸载前: 数据目录 = logs\, gateway.db, gateway.db-shm, gateway.db-wal, logs\gateway.log (2426 B)
        logs\gateway.log SHA256 = 026E34E587E0763B5B648B789B067CE8BC82FD9E4DC7D934F9C0890F664B0D96
Start-Process "$env:LOCALAPPDATA\Claude Gateway\uninstall.exe" -ArgumentList '/S' -Wait → 退出码 0
卸载后: Test-Path "$env:LOCALAPPDATA\Claude Gateway" = False    # 程序删了
        Test-Path "$env:APPDATA\com.claudegateway.desktop" = True   # 数据留着
        四个文件大小一字未变（4096 / 32768 / 131872 / 2426）
        logs\gateway.log SHA256 = 026E34E5…0D96（与卸载前逐字一致）
        HKCU\...\Uninstall\Claude Gateway 已消失；开始菜单快捷方式已消失
        Program Files\Claude Gateway = False（本来就没有）
```

`.db/-shm/-wal` 三个文件当时被并行票 44 的实例握着（SQLite 锁），所以只对 `gateway.log` 取了哈希；三个库文件用「存在 + 字节数不变」作证。

静默卸载不显示确认页，正对应「未勾选 → 不删」。原生复选框默认**未勾选**的机制依据：`installer.nsi:454` 用 `${__NSD_CheckBox_STYLE}`，该样式在 NSIS 的 `nsDialogs.nsh` 里是 `DEFAULT_STYLES|WS_TABSTOP|BS_TEXT|BS_VCENTER|BS_AUTOCHECKBOX|BS_MULTILINE`（`BS_AUTOCHECKBOX=0x3`，**不含** `BS_CHECKED=0x1`）→ 初始未勾选；只有 `$DeleteAppDataCheckboxState = 1` 才 `RmDir /r "$APPDATA\${BUNDLEID}"`（`installer.nsi:877-890`）。复选框**肉眼确认**留票 55。

**4. 门禁**（合并 tip 之后重跑）

`pwsh -File ./check.ps1 -Quick -Replay` → `全部通过。`（exit 0）：fmt 干净；clippy 无警告；单测 5（shell）+ 37（core）+ 4（gateway_service）；回放 20 用例 + 回放器 5 单测全过。

### 必须人眼确认（票 55 现场签字）

1. 交互式安装向导：中文/英文文案、目录选择页、**全程无 UAC 弹窗**、开始菜单/桌面快捷方式。
2. 未签名导致的 SmartScreen「Windows 已保护你的电脑」提示（票 18 §1 已定不签名；README 要写「更多信息 → 仍要运行」）。
3. 交互式卸载确认页上「Delete app data」**默认未勾选**（本票只跑了静默卸载），以及勾选后确实连 `%APPDATA%\com.claudegateway.desktop` 一起删。
4. 窗口与托盘图标外观（安装版与 zip 版各一次）。

### 遗留与坑

- **并行票会互相抢 8787 与单实例互斥量**：验收期间票 44 的实例（`%TEMP%\cg44\cg44.exe`）正占着 8787。`tauri-plugin-single-instance` 的互斥量按 identifier 命名（票 18 §2 明确不开 semver feature），**所有构建的 Claude Gateway 共用一个互斥量**，所以两个票的实例不能同时跑。本次做法：等它退出后再跑，并且每次都核对「监听 8787 的进程 exe 路径 == 刚启动的 exe」，防止别的实例抢答。中途为绕开占用曾临时把端口写进共享数据目录的 `config.json`（跑完已删除，`Test-Path config.json = False` 已核）；最终证据全部是 8787 上的。
- **卸载后保留一个注册表键**：`HKCU\Software\claudegateway\Claude Gateway` 的默认值（安装位置）在默认路径下不删（`installer.nsi:880` 在「勾选删数据」分支里），下次安装会用它复用目录（`RestorePreviousInstallLocation`）。无副作用，但要知道。
- **CI 的 bundle job 本地验不了**（要 GitHub runner + tag）：pnpm 钉 12.10.1、tauri-cli 钉 2.12.2（与本机一致）；`latest.json` 只是占位，真签名/升级在票 61。
- **静默装/卸不写 `Installer Language`**（没有语言选择页，跟随系统）→ 界面语言只能靠人眼确认。
- 卸载时的「还原客户端配置」（`--restore-clients --silent`）**本票没做**，归票 62；现在的卸载器不碰任何客户端配置。
- 验收用的安装已卸载干净，本机没留下新安装；数据目录沿用原有那份，只多了几行日志。
