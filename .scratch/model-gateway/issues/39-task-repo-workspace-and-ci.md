# 建仓、workspace 骨架与 CI

Type: task
Status: resolved
Blocked by: —

## Question

仓库根还没有 `.git`。建仓并立起正牌骨架：cargo workspace（`crates/gateway-core` lib **不依赖 Tauri** + `src-tauri` bin + `app/` React/Vite），把门禁 `check.ps1` 从原型抄进新骨架并能在新骨架跑，配 GitHub Actions。

本票在**主工作区**直接做——此刻还没有可分 worktree 的分支。

依据：[ADR-0007](../../../docs/adr/0007-skeleton-and-implementation-order.md)、spec §12 骨架段、[ADR-0003](../../../docs/adr/0003-tauri-rust-react-desktop.md)、[票 18](../issues/18-grilling-packaging-and-updates.md)（`productName` / identifier）。

## 交付

- `git init` + 根级 `.gitignore`（补 `target/`、`node_modules/`、`dist/`）+ 首次提交。
- 根 `Cargo.toml`（workspace 成员 `crates/gateway-core`、`src-tauri`）；core 内按 spec §3 建空模块目录：`ingress` / `ir` / `binding` / `egress` / `obs` / `store` / `writer`。
- `crates/gateway-core/Cargo.toml`：依赖 = 原型那 9 条里与转换/存储相关的部分，**不加 tauri**。
- `src-tauri/`：Tauri 2 初始化，`tauri.conf.json` 里 `productName = "Claude Gateway"`、identifier = `com.claudegateway.desktop`、窗口与托盘占位。
- `app/`：Vite + React 空壳，能被 `src-tauri` 构建时打包。
- `check.ps1` 落到仓库根（沿用 `-Quick` / `-Replay` / `-Live` / `-Smoke` 四开关语义），此时无测试也必须能绿。
- `.github/workflows/ci.yml`：push / PR 跑 `check.ps1 -Quick -Replay`（windows runner）；打 tag 出包的 job 先留占位。

## 收口验收

- `git log` 有一次提交，`git status` 干净。
- `cargo build --workspace` 通过。
- `pwsh -File .\check.ps1 -Quick` 在新骨架跑通。
- `crates/gateway-core` 的依赖树里没有 `tauri`（`cargo tree -p gateway-core` 自查）。

## Answer

（2026-10-15，主工作区直接做）

**做了什么**

- 建仓：`git init -b main`；根 `.gitignore` 在原三行基础上补 `target/`、`node_modules/`、`dist/`，另放行 `app/dist/.gitkeep`（`!app/dist/` + `app/dist/*` + `!app/dist/.gitkeep`）、忽略 `src-tauri/gen/schemas/`（tauri-build 每次重写）。骨架提交 `198353c`，189 个文件。
- 根 `Cargo.toml`：workspace，`resolver = "2"`，成员 `crates/gateway-core` + `src-tauri`。
- `crates/gateway-core`（lib）：`ingress` / `ir` / `binding` / `egress` / `obs` / `store` / `writer` 七个空模块，每个 `mod.rs` 只写一句职责；`lib.rs` 写明模块划分与「不依赖 Tauri」（ADR-0007 第 1 条）。依赖取原型那 9 条里与转换/存储相关的全部（tokio / axum / serde / serde_json / reqwest / futures-util / bytes / rusqlite bundled / anyhow），没有 tauri。
- `src-tauri`（唯一 bin）：Tauri 2 + `tray-icon` feature；`build.rs` 调 `tauri_build::build()`；`tauri.conf.json` 里 `productName = "Claude Gateway"`、`identifier = "com.claudegateway.desktop"`、主窗口 1100×760、`app.trayIcon` 占位、`bundle.targets = ["nsis"]` + `installMode = "currentUser"`（票 18）。图标用 Pillow 生成（`32x32.png` / `128x128.png` / `128x128@2x.png` / `icon.ico`）——**`icons/icon.ico` 是 Windows 上 tauri-build 的硬要求，缺了直接编译失败**。
- `app/`：Vite + React + TS 空壳（`package.json` / `vite.config.ts` / `tsconfig.json` / `index.html` / `src/main.tsx`），`pnpm build`（`tsc --noEmit && vite build`）通过；`tauri.conf.json` 的 `build.frontendDist = "../app/dist"`、`devUrl = http://localhost:1420`。
- `check.ps1` 落仓库根，四开关语义照旧（不带参数 = Quick + Replay）：workspace 化后 unit 步改 `cargo test --workspace`、fmt 改 `--all`、clippy 加 `--workspace`。新增「每步重置 `$LASTEXITCODE`」——占位步骤不跑原生命令，不重置会继承上一步的码（实测 `-Replay` 单独跑会误报失败）。
- `.github/workflows/ci.yml`：push / PR 在 `windows-latest` 跑 `check.ps1 -Quick -Replay`（rustfmt + clippy 组件 + rust-cache）；tag 出包 job 用 `startsWith(github.ref, 'refs/tags/')` 占位，不产产物。
- `prototype/` 只读参照，一字未改；未提交任何密钥（`prototype-upstream.env` 仍在忽略里）。

**收口验收（真跑）**

1. `git log` 一次提交、`git status` 干净
```
$ git log --oneline
198353c 建仓：cargo workspace 骨架、门禁 check.ps1 与 CI（票 39）
$ git status --porcelain | wc -l
0
```
   骨架提交是 `198353c`；本 Answer + `Status: resolved` 按收工要求另作一条提交，故最终 `git log` 是两条。
2. `cargo build --workspace`
```
   Compiling claude-gateway v0.1.0 (...\src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.29s
```
   干净 clone 的可重现性另验：clone 到临时目录后 `app/dist` 只有 `.gitkeep`，`cargo check --workspace` 通过（`Finished ... in 31.20s`）——`tauri::generate_context!` 只要求 `frontendDist` 目录存在，不要求里面有 index.html。
3. `pwsh -File .\check.ps1 -Quick`
```
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.51s
=== unit   (cargo test --workspace) ===
running 0 tests … test result: ok. …
全部通过。
```
   `exit 0`。`-Replay` / `-Live` 单独跑也 `exit 0`（占位：打印「还没有回放测试目标（票 41 搬入）」后跳过）；`-Smoke` 读 `.scratch/model-gateway/smoke.md` 等人签字，管道喂 `y` 实测 `exit 0`。
4. `cargo tree -p gateway-core` 自查
```
$ cargo tree -p gateway-core | grep -ci tauri
0          # 依赖树共 302 行，0 处命中 tauri
```

**遗留与坑**

- `-Replay` / `-Live` 是占位：真正的 `crates/gateway-core/tests/replay.rs` 与 fixtures 随票 41 搬入；`check.ps1` 用目标探测（`Get-ChildItem -Filter replay.rs`）自动切换，票 41 落地后不用改脚本。
- `app/dist` 里只有 `.gitkeep`（vite `emptyOutDir: false` 保住它）——代价是构建产物不入库、`dist` 不清空；正式出包走 `beforeBuildCommand`（`pnpm --dir ../app build`）现编。
- `src-tauri` 已依赖 `gateway-core` 但尚未调用（外壳与网关生命周期属 M1 后续票）；`trayIcon` 只是配置占位，托盘菜单与「关窗隐藏」在票 43。
- tauri 解析到 2.12.4（本机缓存是 2.12.2，`version = "2"` + 网络可达），已用提交的 `Cargo.lock` 钉住。
- 没跑 GUI：窗口与托盘的真实起停要在票 43 / 44 收口时人工看。
