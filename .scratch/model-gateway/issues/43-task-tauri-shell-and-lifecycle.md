# Tauri 外壳、管理 API 与网关生命周期

Type: task
Status: resolved
Blocked by: 39

## Question

起正牌应用外壳：Tauri 2 + 托盘 + 单实例 + 端口检测 + 关窗隐藏，并把「网关由谁起」定案（同进程起 axum，不是 sidecar）。管理 API 骨架先给 `/api/status`。

依据：spec §3 / §10（托盘段、故障态）、[票 18](../issues/18-grilling-packaging-and-updates.md)。

## 交付

- `src-tauri`：窗口、托盘菜单四项（显示主窗口 / Claude Desktop 1P↔3P 快切占位 / 网关状态（运行中 · 端口）/ 打开日志目录 / 退出）、左键单击开窗口、图标随状态变色。
- 网关服务在 `setup` 里起在 `127.0.0.1:8787`（端口可配，默认 8787）；占用时**明确报错不静默换端口**。
- 单实例：第二次启动被挡住并把已有窗口提到前面。
- 关窗 = 隐藏到托盘；只有托盘「退出」才真正停止（连网关一起停）。
- `/api/status`：版本、端口、运行状态、启动时间、库路径。

## 收口验收

- 启动后 `curl http://127.0.0.1:8787/api/status` 返回真实数据。
- 二次启动被单实例挡住；关窗后托盘图标仍在、`/api/status` 仍可用。
- 先把 8787 占住再启动，给出明确报错（含端口号与占用提示）。

## Answer

2026-10-15，worktree `cg-wt/43`，分支 `task/43-tauri-shell-and-lifecycle`（基于 `integration/model-gateway`，收口前把集成 tip 并了进来）。

### 做了什么

**「网关由谁起」落成同进程**（票 18 §2 定案，不用 sidecar）：网关本体在 `gateway-core`，外壳只决定起停时机。

- `crates/gateway-core/src/service.rs`（新）：`GatewayConfig { port, db_path, version }` + `start()` / `Gateway::{port, local_addr, status, stop}`。绑 `127.0.0.1:{port}`（默认 `DEFAULT_PORT = 8787`），`AddrInUse` → `StartError::PortInUse`，**不换端口**；`stop()` 优雅关监听、等 `axum::serve` 收尾，返回 `running == false` 的快照。
- `crates/gateway-core/src/ingress/api.rs`（新）：`GET /api/status` → `{ version, port, running, startedAt, uptimeSeconds, dbPath }`（camelCase）。`startedAt` 是 RFC3339 UTC（手写 civil_from_days，不为一个时间戳引日期库；本地化交给前端）。
- `crates/gateway-core/src/ingress/proxy.rs`（新）+ `ingress::router()`：**管理面 `/api/*` 与转发面 `/v1/*` 分开挂**，两个 `Router` 各自带状态后 merge；转发面本票只留挂载点（端点在票 40/41/51），`/v1/status` 不存在、`/api/messages` 不存在——两边不互相别名（有断言）。
- `crates/gateway-core/src/store/mod.rs`：`DB_FILE_NAME` `pub(crate)` → `pub`（外壳拼 `/api/status` 的库路径，不想再抄一遍字面量）；数据目录仍只有一处定义 `store::default_data_dir()`。
- `src-tauri`：`shell.rs`（新）+ `main.rs` 接线。单实例插件**第一个注册**（插件要求），窗口/托盘/关窗/退出在 `shell.rs`。

**托盘与关闭语义怎么落的**

- 托盘由代码建（`TrayIconBuilder::with_id("main")`），**五项**按下单顺序：`显示主窗口` / `Claude Desktop 1P↔3P 快切（待配置写入引擎）`（**禁用占位**：要动客户端配置，归票 45/46）/ `网关状态：运行中 · 127.0.0.1:8787`（禁用只读项，起不来时写「未运行（127.0.0.1:8787 未监听）」）/ `打开日志目录` / `退出`。票面写「四项」但列了五项，按 spec §10 的五项做。
- 左键单击开窗口：`show_menu_on_left_click(false)` + `TrayIconEvent::Click{Left, Up}` → `show_main_window()`（`show` + `unminimize` + `set_focus`）；菜单留给右键。
- 图标两态：`icons/tray-running.png`（绿）/ `icons/tray-stopped.png`（灰），`tray_icon_bytes(bool)` 选图，起不来时用灰的；工具提示同状态文案。
- **关窗 = 隐藏到托盘**：`on_window_event` 收 `WindowEvent::CloseRequested` → `api.prevent_close()` + `window.hide()`，路由进程与网关都不动。
- **只有「退出」才真停**：菜单 `quit` → `stop_gateway()`（`block_on(gateway.stop())` 优雅关监听）→ `app.exit(0)`；`RunEvent::Exit` 再兜一次（重复调用是空操作）。
- 起不来也**继续建托盘**：`setup` 里绑端口失败只打日志（控制台 + `<数据目录>\logs\gateway.log`），`Shell` 里 `gateway = None`、托盘显示未运行，窗口照常开——故障态（spec §10）留给票 52 的横幅与「换端口」。
- `tauri.conf.json` 删掉 `app.trayIcon`：Tauri 2 会按该配置**自动再建一个**托盘（`tauri-2.12.4/src/app.rs:2589`），与代码里这个带菜单的会变成两个；图标/菜单/事件因此统一在 `shell.rs`。

**测试（TDD，先红后绿）**：新 `crates/gateway-core/tests/gateway_service.rs` 4 格（状态字段真值 / 端口被占报错 / 两面不别名 / stop 后监听关闭且 running=false），`ingress/api.rs` 1 格（RFC3339 对已知时刻，含闰日 2000-02-29 与 2100 非闰年），`src-tauri/src/shell.rs` 5 格（端口配置默认与坏值回退 / 状态文案 / 两态图标是不同 PNG / 日志追加 / 图标能被 tauri 解码）。全部是公开接口上的行为断言，不碰真实 `%APPDATA%` 已有数据（临时目录 + 临时端口）。

### 收口验收（逐条真跑，输出照抄）

跑验收前把 exe 拷到私有路径（见「坑」）：`%TEMP%\cg43\final.exe` = `cargo build -p claude-gateway` 的产物。

**1. 启动后 `curl http://127.0.0.1:8787/api/status` 返回真实数据**

```
$ curl -s http://127.0.0.1:8787/api/status
{"version":"0.1.0","port":8787,"running":true,"startedAt":"2026-10-15T12:29:27Z","uptimeSeconds":8,"dbPath":"C:\\Users\\ZHANQUAN\\AppData\\Roaming\\com.claudegateway.desktop\\gateway.db"}

$ curl -s -D - -o /dev/null http://127.0.0.1:8787/api/status | head -3
HTTP/1.1 200 OK
content-type: application/json
content-length: 187
```
库路径 = 票 42 / 票 18 定的那份（`store::default_data_dir()` = `%APPDATA%\com.claudegateway.desktop\gateway.db`），版本取自 `CARGO_PKG_VERSION`，`uptimeSeconds` 随请求增长。

**2. 二次启动被单实例挡住；关窗后托盘在、`/api/status` 仍可用**

```
# 二次启动（第二个进程 15s 内自己退出）
二次启动：exit=0 elapsed=369ms 15s内退出=True
仍在跑的进程数：1（就是先起的那一个）

# 关窗 = 向主窗口发 WM_CLOSE（等价用户点 X）
关窗前：pid=2944 hwnd=1048970 可见=True
CloseMainWindow() 发出 WM_CLOSE = True
关窗后：进程还在=True 窗口句柄还在=True 可见=False
关窗后 /api/status：{"version":"0.1.0","port":8787,"running":true,"startedAt":"2026-10-15T12:29:27Z","uptimeSeconds":29,"dbPath":"C:\\Users\\ZHANQUAN\\AppData\\Roaming\\com.claudegateway.desktop\\gateway.db"}
```
窗口是被**隐藏**（句柄还在、不再可见），不是销毁；进程与网关都没停。托盘图标「还在」这一眼属人验（见下）。

**3. 先把 8787 占住再启动（占位进程 `pid 12012` 在 `127.0.0.1:8787` LISTENING），报错原文**

```
网关启动失败：端口 8787 已被占用（127.0.0.1:8787 绑定失败：Address already in use）。该地址已写进客户端配置，网关不会自动换端口；请先关掉占用 8787 的程序，或在设置页改用空闲端口后重启。
```
同一行也进了日志（`%APPDATA%\com.claudegateway.desktop\logs\gateway.log`）。此时外壳**照常活着**（`final.exe` 仍在进程表里，托盘写「未运行」），8787 上仍只有占位进程——没有被静默顶掉、也没有换端口。

**4. `pwsh -File ./check.ps1 -Quick -Replay` 绿**

```
$ pwsh -File ./check.ps1 -Quick -Replay   # exit 0
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
  claude-gateway（bin 单测）  5 passed; 0 failed
  gateway-core（单测）       33 passed; 0 failed
  gateway_service（集成）     4 passed; 0 failed
=== replay (集成：自起网关 + mock，回放 fixtures) ===
  还没有回放测试目标（票 41 搬入 crates/gateway-core/tests/replay.rs）；这一步先跳过。
全部通过。
```
`-Replay` 仍是占位（回放目标随票 41 进集成分支，本票分支上还没有）。

### 必须人眼确认（票 44 现场签字）

1. 托盘图标真的出现在通知区、右键五项文案与顺序、**左键单击开出主窗口**（脚本只证明了事件接线，点不了托盘）。
2. 图标两态换色：正常运行是绿、端口被占/未运行是灰（脚本只证明了选了不同 PNG）。
3. 关窗后**托盘图标仍在**（脚本证明了窗口被隐藏、进程与网关都在，看不见托盘本体）。
4. 二次启动时**已有窗口被提到前面**（脚本只证明了第二进程 369ms 退出、只剩一个进程）。
5. 托盘「退出」确实连网关一起停、进程消失（托盘菜单点不到；代码路径是 `quit` → `stop_gateway()` → `app.exit(0)`，`RunEvent::Exit` 兜底）。
6. 端口被占时窗口与托盘的表现（我证明了报错文本、外壳存活、8787 没被顶掉；窗口里此刻是空页面，横幅属票 52）。

### 遗留与坑

- **共享 `CARGO_TARGET_DIR` 会被并行票的构建覆盖**：我第一次起的 `target/debug/claude-gateway.exe` 已被别的 worktree 的构建换掉（窗口起来了但没网关没托盘没日志，正是票 39 那版 `main.rs`）。跑验收前先 `cp target/debug/claude-gateway.exe %TEMP%\...\final.exe` 再跑，并确认拷出来那份含新串（`grep -a show-window`）。
- **外壳还没开库**：`/api/status` 只报库路径，`Store::open_default()`（首次自建目录与库）留给票 44 接进生命周期；数据目录已由外壳建 `logs\`。
- **端口配置是本票的 shortcut**：读 `<数据目录>\config.json` 的 `{"port": N}`，缺省/非法/越界一律回 8787（有断言），目标改读票 42 库里的 `settings.port`（`settings_keys::PORT`）——`store` 还没有设置项读写接口、设置页（票 52）也还没写它，代码里留了 `shortcut:` 注释。
- `app.trayIcon` 从 `tauri.conf.json` 删了（理由见上）；托盘不再由配置驱动。
- Windows 上 tokio 不设 `SO_REUSEADDR`：实测刚退出后立刻重启时端口上还有上一轮 curl 连接的 TIME_WAIT 残留（未证实会挡住重启，我两次重启都绑上了）。若将来有「刚退出就重启说端口被占」的报障，先看这里。
- `/v1/*` 是空 Router，只有挂载点；转发路由与端点随票 40/41/51。
- 窗口内容仍是空壳（`app/dist` 只在 `pnpm --dir app build` 后有产物，不入库），页面本体属票 52。
