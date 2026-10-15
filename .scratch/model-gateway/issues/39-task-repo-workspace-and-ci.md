# 建仓、workspace 骨架与 CI

Type: task
Status: open
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
