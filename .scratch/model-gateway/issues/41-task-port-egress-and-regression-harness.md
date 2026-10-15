# 出站转发与回归基建搬入

Type: task
Status: resolved
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

## Answer

**做完了**（2026-10-15）。出站转发、mock 隔离、46 份 fixture / 15 份黄金、12 格回放套件与台账全部落在产品侧，
四条验收逐条真跑，证据见下。集成分支 tip（票 42 存储 + 票 43 外壳与生命周期）已并入并就地解决冲突。

### 落地形态

| 原型 | 产品侧落点 |
|---|---|
| `src/provider.rs` | `crates/gateway-core/src/egress/provider.rs`（一字未改，只有守卫测试的 fixtures 目录与豁免理由里的路径改到产品侧） |
| `src/upstream.rs` | `crates/gateway-core/src/egress/upstream.rs`（转发链 / 错误信封 / ping 保活 / 探针 deadline 原样；日志落点换成票 42 的 `Store`） |
| `src/main.rs` 的转发链 | `crates/gateway-core/src/ingress/proxy.rs`（`pub fn router()` + `pub struct Gateway` 运行态） |
| `src/store.rs` 的 `Route` | `crates/gateway-core/src/binding/mod.rs`（最小绑定形状 `Route`，票 49 / 51 在它上面长） |
| `src/mock.rs` | `crates/gateway-core/src/bin/mock-upstream.rs`（**测试专用独立 bin**） |
| — | `crates/gateway-core/src/bin/gateway-harness.rs`（回放用的网关进程：挂 `proxy::router()` + `/healthz` + `/__bodylog`） |
| `src/replay.rs` | `crates/gateway-core/src/replay.rs`（**不写进 `lib.rs`**，mock 与回放测试各 `#[path]` 引一份） |
| `fixtures/` | `crates/gateway-core/tests/fixtures/`（46 文件 / 15 份黄金，字节原样） |
| `tests/replay.rs` + `tests/common/` | 同名搬到 `crates/gateway-core/tests/`（断言一字未减） |
| `record.ps1` | `crates/gateway-core/tests/record.ps1`（凭据与落点都改到产品侧，**不覆盖**冻结原型） |
| `prototype/README.md` §8 台账 | `crates/gateway-core/tests/LEDGER.md`（产品侧那份，`ledger_has_no_gaps` 读它） |

`check.ps1` 改了四步：回放步骤换成 `cargo test -p gateway-core --features mock --test replay`、
补一步 `--features mock --bins`（回放器切法单测）、clippy 补一遍带 `mock` 特性的（否则两个测试 bin
与回放测试根本不参与 lint）、并把「回放测试还没搬入」那段占位逻辑删掉。

### mock 隔离方案与理由

方案就是票面建议的那条，产品侧命名的开关：

- `[features] mock = []`；`[[bin]] mock-upstream`、`[[bin]] gateway-harness` 与 `[[test]] replay`
  三条都挂 `required-features = ["mock"]`。
- 产品构建（`cargo build -p gateway-core -p claude-gateway`）不带这个特性，所以**产不出**两个 bin，
  内核里也没有 mock 的任何字符串或状态格（见验收 2 的 `grep` 结果）。
- **分进程**：mock 是独立 bin，网关侧只按 `base_url` 打 HTTP；回放测试同时拉起两个进程
  （`gateway-harness` + `mock-upstream`）。票 43 的 `service.rs` 与票 41 的 harness 挂的是
  **同一个** `ingress::proxy::router()`，不存在两套 `/v1/*`。
- 理由：mock 的路由（`/__chunks`、`/api/echo`）与回显缓冲是**测试能力**，不是协议的一部分；
  放进产品 router 会让它们随包发出（ADR-0007 的第一笔原型债）。用 cargo 特性而不是「约定不入库」，
  是因为这条约束得由构建系统执行，不能靠自觉。

### 台账新位置

`crates/gateway-core/tests/LEDGER.md`：含「差异清单」（票 19 §5 的 7 条，逐条）+「台账」（7 行，
目标写成 `文件` 或 `文件::函数`）+「明确不覆盖的两处」+ quirk 守卫与豁免说明。
`tests/replay.rs::ledger_has_no_gaps` 读它、校验 7 行且每个目标真实存在（函数名要在文件里找得到）。
原型 `README.md` §8 保持原样不动。

### 验收（逐条真跑）

| # | 命令 | 结果 |
|---|---|---|
| 1 | `pwsh -File .\check.ps1 -Replay` | `全部通过。` 出口码 0：replay `20 passed`（12 格转发/回归断言 + 回放器 5 + golden 4）+ replay-unit `5 passed` |
| 2 | `cargo build -p gateway-core -p claude-gateway` 后 `grep -a -c` | `claude-gateway.exe` 与 `libgateway_core.rlib` 里 `/__chunks`、`/api/echo`、`chunk_scheme`、`echo_push`、`mock-upstream` **各 0 次**；对照：`target/debug/deps/mock_upstream.exe` 里 `/__chunks` 命中 1 次（证明 grep 不是假阴性）；且产品构建**不再产出** `mock-upstream.exe` / `gateway-harness.exe`（先删干净再构建，构建后仍不存在） |
| 3 | `pwsh -File .\check.ps1 -Quick -Replay` | `全部通过。` 出口码 0：fmt / clippy(workspace) / clippy(mock) 无输出；unit = lib `37` + `gateway_service` `4`；replay `20`；replay-unit `5` |
| 4 | 抽掉 `crates/gateway-core/tests/fixtures/usage/anthropic.sse` 后 `cargo test --workspace egress::provider` | 红：`every_tagged_fixture_has_bytes ... FAILED` + `…\fixtures\usage\anthropic.sse 没有对应的字节文件`（`test result: FAILED. 3 passed; 1 failed`）。另抽掉两份带 quirk 标签的 `text/*.meta.json` → `every_quirk_is_covered_by_a_fixture_or_explicitly_exempt ... FAILED`。两份都 `mv` 出去、验 md5（`f25d27c…`）后还原，再跑 4 个守卫全 ok |

补充证据：`fixtures/` 46 个文件、`expected/` 15 份；`.scratch/` 无任何改动（冻结原型只当参照）。

### 与票 43 的合并（冲突口径）

- 保留 43 的 `service.rs` 与 `ingress/api.rs`；43 在 `ingress/proxy.rs` 留的空挂载点换成真转发链，
  **模块名与 `router()` 入口名不变**。
- `ingress/mod.rs` 两侧都保留：`pub mod api;` + `pub mod proxy;`，组装点仍是 `ingress::router`，
  签名扩成 `router(api_state, forwarding: Option<Arc<proxy::Gateway>>)`——`None` 时只挂管理面
  （与 43 的行为完全一致，43 的 `/v1/status` 404 断言照样过），`Some` 时 `/api/*` 与 `/v1/*` 分开挂、互不重叠。
- 因此 `GatewayConfig` 多一个 `forwarding` 字段，`src-tauri/src/shell.rs` 与
  `tests/gateway_service.rs` 的 `config()` 各补一处 `None`。这是合并的接线，不是重写 43 的服务。
- `proxy::Gateway` 的 `Debug` 是**手写**的（只报条数与旋钮）：供应商表里带 API key，
  `derive(Debug)` 等于把密钥打进日志。

### 遗留与坑

1. **路由表还是测试脚手架**：`gateway-harness` 里的表（mock 场景 × wire + 家族槽位 + 两个真实上游条目）
   是照着原型 `main.rs` 抄的。真绑定表（store 驱动 + 客户端档案 + 家族槽位展开）归票 49 / 51；
   它们接手后 `Gateway::routes` 换成 store 查询，「harness 只负责把测试要的绑定种进库」。
2. **Tauri 外壳此刻只服务管理面**：`shell.rs` 传 `forwarding: None`，因为供应商表与绑定表还没落地。
   票 51 接了真绑定表之后要在这里挂上 `Some(Arc<Gateway>)`，否则 `/v1/*` 在真实外壳里是 404。
   这条是 M1 收口（44）前必须接的最后一根线，已在 `service.rs` 的字段注释里写明。
3. **测试读日志不再经 `/api/logs`**：`/api/*` 归 43 / 53，所以「默认不写 body / 开关开了能捞到 /
   2 秒到点自关 / 密钥反查」四条改成**直接开票 42 的 `Store` 读 `requests` 表**（`obs::recent`）。
   断言的实质没变（哪一行、body 写没写），只是不再依赖一个还没实现的管理端点。
   同理密钥反查那条把 `/api/routes` 换成了 `/v1/models` + 库文件字节。
4. **`fixtures/quirk-exempt.json` 的豁免理由改了文件路径**（`src/upstream.rs` → `src/egress/upstream.rs`）。
   这是搬入时对 fixture 目录唯一一处文本改动，SSE 字节与其余 meta 一字未动。
5. **`ClaudeModelsOnAnthropicOnly` 仍在豁免表里**：它是守卫型 quirk（wire 错配当场拒），不产生回放；
   票 23 已决定删它，删除随实现阶段。
6. **两处照旧不覆盖**：`: keep-alive` 注释帧（两个真实上游都不发，按票 19 §2 不手编）、
   客户端档案覆盖（原型没这个概念，归实现阶段）。理由都写在 `LEDGER.md`。
7. **共享 `CARGO_TARGET_DIR` 的坑**（票 43 提醒）：跑需要真实进程的验收前先把 exe 拷到私有路径，
   否则并行票重编会换掉正在跑的二进制。本票验收时两个 exe 都已拷到 `%TEMP%\41-*.exe` 后再 grep。
8. **`cargo test --workspace` 不跑回放套件**（要 `--features mock`），所以回放步骤是 `check.ps1`
   单独的、带特性的两步；`-Quick` 单独跑不含回放，这是有意的（秒级门禁）。

## Comments

