# 出站转发与回归基建搬入

Type: task
Status: open
Blocked by: 40

## Question

搬 `provider.rs`（声明式供应商表 + `quirks`）与 `upstream.rs`（出站转发、错误信封、ping、探针 deadline、日志落点），以及**整套回归基建**：`mock.rs` / `replay.rs` / `fixtures/` / `tests/` / `record.ps1` / `check.ps1` 的回放开关。

同时把 mock 与网关**分进程隔离**：产品路径里不许出现 `/__chunks`、`/__bodylog`、`/api/echo` 与 `App` 上的 `echo` / `chunk_scheme` 字段（[ADR-0007](../../../docs/adr/0007-skeleton-and-implementation-order.md) 的第一笔原型债）。

依据：[ADR-0005](../../../docs/adr/0005-protocol-conversion-regression-strategy.md)、[票 19](../issues/19-grilling-test-strategy.md)、[票 29](../issues/29-task-regression-harness.md)、[票 30](../issues/30-task-build-regression-suite.md)、[prototype/README.md](../prototype/README.md) §7–§9。

## 交付

- `provider.rs` / `upstream.rs` 进 core（`egress` + `binding` 侧的 `provider`）。
- `mock.rs` 进**测试专用**位置（独立 bin 或 test-only 模块），由测试与 `check.ps1` 起，不参与产品构建。
- `fixtures/`（46 文件 / 15 份结构级黄金）、`tests/`（12 格）、`replay.rs`、`record.ps1` 原样搬入。
- quirk 双向守卫与 `fixtures/quirk-exempt.json` 的豁免机制随行。

## 收口验收

- `check.ps1 -Replay` 全绿：12 格 24 用例 + 五种切法不变性 + 头类 quirk + 错误信封 + `ledger_has_no_gaps`。
- 产品构建（`cargo build -p gateway-core -p <tauri bin>`）的产物里不含 mock 的路由字符串（`strings` 或源码断言任一）。
