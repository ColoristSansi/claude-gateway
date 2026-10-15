# 打包、自启动、升级与首启引导

Type: grilling
Status: resolved
Blocked by: —

## Question

桌面应用怎么交付与长期维护？

## 需要定下来的

1. 打包产物形态（NSIS / MSI / 免安装），是否需要代码签名（没有签名会有 SmartScreen 警告，用户是单机自用）。
2. 是否注册开机自启与托盘常驻的默认值；退出行为（关窗是最小化还是退出，退出后客户端会不会立刻报错）。
3. 升级方式：手动下载替换 / 内置更新检查 / 静默自动更新。注意网关进程被客户端配置指向，升级时端口与配置文件必须保持稳定。
4. 首启引导：检测三个客户端是否安装、指导装 Claude Desktop 的 Developer Mode、引导填第一个上游。
5. 数据目录约定（SQLite、备份、日志各自的路径）与卸载时是否保留数据。

## Answer

2026-10-14 与用户三轮弹窗对话定案（全程中文，Q1–Q11）。支撑这些结论的 Tauri 2 / Windows 一手事实见下方「事实依据」。

### 1. 交付产物形态

- **主产物：NSIS 安装包**，per-user 模式（Tauri 默认 `installMode: currentUser`）：装到 `%LOCALAPPDATA%\Claude Gateway`，**不需要管理员权限**，带正规卸载项（`HKCU\...\Uninstall\Claude Gateway`）。不做 MSI（WiX 固定 per-machine、必须提权，对单机自用没好处）。
- **副产物：免安装 zip**。Tauri 官方 bundle 目标里**没有 portable**，所以 zip 由构建脚本手工打：`target/release/claude-gateway.exe` + 资源目录。zip 版没有卸载项、没有 Run 键自启、**不参与自动更新**（updater 只接受 msi/nsis 包），设置页对它显示「此版本请手动替换」。
- **不做代码签名**（Q2）。首次运行会有 SmartScreen「Windows 已保护你的电脑」，README 写明点「更多信息 → 仍要运行」。已核实自签名证书**没有意义**（微软原话：行为与无签名相同）；EV 证书自 2024 年起也不再豁免 SmartScreen。
- WebView2 保持默认（Windows 10 2018-04 之后与 Windows 11 随系统自带，默认的 downloadBootstrapper 只是兜底）；不内置离线运行时（+127MB 不值）。
- 应用名与标识（Q7）：`productName` / 显示名 **Claude Gateway**（不用中文显示名）；identifier **`com.claudegateway.desktop`**。已核查本机 `%APPDATA%` 无同名目录（本机另有 `dev.provide-gateway.desktop`、`com.providegateway.desktop`、`provide-gateway`、`model-gateway` 等无关目录，不冲突）。identifier 与 productName 一经发布不得再改：改 identifier 等于用户数据与卸载目标一起失联，改 productName 而未固定 WiX `upgrade_code` 会让两套版本并存。
- 安装界面语言：简体中文 + 英文（跟随系统）。安装目录允许用户改（Tauri 交互式安装固定带目录选择页，无法关闭）。

### 2. 常驻、自启与退出

- **关窗 = 隐藏到托盘，网关继续跑**：`WindowEvent::CloseRequested` → `api.prevent_close()` + `window.hide()`；只有托盘右键「退出」或设置页「退出应用」才 `app.exit(0)`。理由：网关一停，三个客户端立刻报连不上。
- **开机自启默认关闭**（Q3，用户改选），开关在设置页（票 17 §7 已有这一项）。开启时**显式写 HKCU Run 键**：Tauri autostart 插件默认 `WindowsEnableMode::Dynamic`（先试 HKLM），以管理员身份运行时会把值写到 HKLM，而卸载器只清 HKCU 同名值 → 强制用 currentUser 模式避免残留。
- **单实例**：`tauri-plugin-single-instance`，**不开 semver feature**（否则升级后互斥名变化，新旧版本会同时跑）；第二个实例把参数转交第一个并唤起主窗口，同时满足 Q8「zip 版与安装版共用数据、同时只跑一个」。
- 开机后网关没起来（用户关掉了自启）→ 走票 17 §9 的常驻横幅，不另做逻辑。
- 托盘常驻需要 Tauri `tray-icon` feature；托盘菜单沿用票 17 §8 已定项。

### 3. 升级

- **发布通道（Q10）：公开 GitHub 仓库 + Releases**（安装包 + 更新清单 `latest.json`）。已核实 Tauri updater **强制 https 端点**（非 https 直接报 `InsecureTransportProtocol`，`file://` 走不通，reqwest 不支持该 scheme），且**签名校验不可关闭**。仓库里不会有密钥（`.gitignore` 已就绪）。
- **升级行为**：启动后静默检查一次 + 设置页「检查更新」；发现新版在顶栏提示，**用户确认后**才下载并安装；Windows 安装模式 `passive`（只显示进度条，不弹交互），装完应用自动重启。
- **稳定性验收判据（本票最硬的一条）**：升级前后 ① 端口仍是 8787；② 不重写任何客户端配置；③ 数据目录与数据不变。三条都有机制支撑：NSIS `/UPDATE` 模式下卸载段跳过 app data 删除、快捷方式删除与 Run 键清理；安装路径从 `HKCU\Software\<manufacturer>\<product>` 读回，不会换目录。
- **回退（Q11）**：数据目录下留 `updates\`，存「本次下载的新包」与「上一版安装包」各一份；设置页有「回到上一个版本」指引页（告诉用户装哪个文件，数据不动）。**不做一键回退按钮**（那一步本身就会写坏东西）。
- 残留风险已知即可：升级不主动清理旧文件（仅新旧主程序同名覆盖），zip 解压目录换位置时旧目录要用户自己删。
- 实施前置（本次不做，属实现阶段）：建公开仓库、配 CI 产出 NSIS + zip + `latest.json`、生成 updater 签名密钥对（公钥进配置，私钥交给用户保管）。

### 4. 首启引导

- **不另做向导**——票 17 已定：概览页顶部三步清单就是引导。本票只补检测落点（Q6）：**客户端页三张卡片做安装检测**，概览清单第③步指向它。
- 检测判据：Claude Desktop 查 MSIX 包（本机实测 `Claude 2.37337.1.0`）；Claude Code 与 Codex 查 PATH 上的可执行文件并读版本。未检测到 → 卡片显示「未检测到安装」+ 官方下载链接；Desktop 另给开发者模式步骤（Help → Troubleshooting → Enable Developer Mode，再 Developer → Configure Third-Party Inference）。设置页不另加「环境体检」页。
- **「未登录」不是阻断错误**（票 24 已解，见[未登录态下能否只用网关凭据](24-research-client-login-state.md)）：概览三步清单的就绪判据不受登录态影响；卡片另按票 24 的两条例外升级为错误。

### 5. 数据目录、共用与卸载

- **数据目录：`%APPDATA%\com.claudegateway.desktop\`**（Tauri `app_data_dir` 按 identifier 命名，与安装位置无关 → 安装版与 zip 版天然共用同一份数据，Q8 得解）。目录由网关首次启动自建（Tauri 的 path resolver 只算路径、不创建）。
  - `gateway.db`：SQLite（配置 + 请求日志，票 11 已定）。
  - `backups\`：客户端配置首写备份（票 10/15 的「首写备份」落点）+ 数据库备份。
  - `logs\`：网关自身运行日志（与 SQLite 里的请求日志是两回事）。
  - `updates\`：本次新包 + 上一版安装包（见 §3）。
  - 密钥仍走 DPAPI，密文进 SQLite；导出/导入默认不含明文密钥（DPAPI 密文换机无效，跨机迁移要用自设口令重新加密）。
- **不提供数据目录搬家**（Q9）：设置页只显示路径 + 「打开目录」。想换盘就按文档手工搬：退出应用、拷目录、改配置。
- **卸载**：
  - 默认**保留数据**，卸载确认页用 NSIS **原生**的「Delete app data」复选框（勾选才删 `$APPDATA\<identifier>` 与 `$LOCALAPPDATA\<identifier>`；静默卸载不显示该页 → 保留数据）。
  - 卸载时弹一次选择「还原客户端配置 / 保持现状」，**默认还原**（Q5）：用 NSIS `installerHooks` 在删除 `$INSTDIR` 之前 `ExecWait` 调用 `Claude Gateway.exe --restore-clients --silent`（复用配置写入引擎的还原路径，读 `backups\`），失败则转为「请手动还原」提示；完成后提示 Claude Desktop 需完全退出重启。
  - 卸载器**只**删自己那两个 identifier 目录，`%APPDATA%\Claude`、`~/.claude`、`~/.codex` 一律不动。

### 事实依据（Tauri 2 / Windows，本次只读调研）

| 结论依赖的事实 | 来源 |
|---|---|
| NSIS 默认 per-user、装 `%LOCALAPPDATA%`、不需管理员；`installMode` 三值；交互式安装固定带目录选择页；`/S` `/P` `/UPDATE` `/ARGS` 开关 | [windows-installer](https://v2.tauri.app/distribute/windows-installer/#install-modes)、[installer.nsi](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi) |
| MSI 固定 per-machine、必须提权 | [main.wxs](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/msi/main.wxs) |
| 无 portable bundle 目标；updater 只接受 msi/nsis | `PackageType::all()`，[settings.rs](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/settings.rs) |
| 自签名 = 无签名（微软原话）；EV 不再豁免 | [SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) |
| autostart = HKCU/HKLM Run 键；插件默认 Dynamic；卸载器只清 HKCU 同名值 | [auto-launch](https://github.com/zzzgydi/auto-launch/blob/main/src/windows.rs)、[autostart 插件](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/autostart/src/lib.rs) |
| 关窗不退出与托盘退出的官方做法 | [CloseRequestApi](https://docs.rs/tauri/latest/tauri/struct.CloseRequestApi.html)、[system-tray](https://v2.tauri.app/learn/system-tray/) |
| updater 强制 https、签名不可关、passive/quiet 可静默装、装完 `exit(0)` | [updater config.rs](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/config.rs)、[signing-updates](https://v2.tauri.app/plugin/updater/#signing-updates)、[installMode](https://v2.tauri.app/plugin/updater/#installmode-on-windows) |
| `/UPDATE` 跳过数据删除；安装路径从注册表复用 | installer.nsi |
| `app_data_dir` = `%APPDATA%\<identifier>`；只算路径不创建 | [desktop.rs](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri/src/path/desktop.rs) |
| 卸载删程序与那两个 identifier 目录、保留数据；「Delete app data」是原生复选框 | installer.nsi |
| DPAPI 默认 CurrentUser 范围、换机/换用户解不开 | [CryptProtectData](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata) |

### 未定 / 明确不做

- 不做 MSI、不买证书、不内置离线 WebView2、不提供数据目录搬家、不做一键回退、不另做首启向导。
- 本票结论尚未并入 `.scratch/model-gateway/spec.md`（该文件目前没有交付/打包一节）；实施前需要补。
