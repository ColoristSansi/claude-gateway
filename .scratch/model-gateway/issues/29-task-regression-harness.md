# 建回归基建：录制 / 回放 / 断言 / 入口

Type: task
Status: resolved
Blocked by: —

## Question

把票 19 定的形态做成能跑的东西：录制工具、可编程回放器、结构级比对、`check.ps1` 入口。

## 为什么是 task

票 19 已定完所有取舍（fixture 形态、切块方案、断言粒度、掩码、入口开关、`verify.ps1` 退役）。本票只是把它们实现出来，没有新的决策要定；实现过程中若发现与票 19 冲突，回去改票 19 而不是在这里自定。

## 要做的

1. **录制工具**（单独小二进制或脚本，不进网关生产路径）：跑真实上游一次，写出 `.sse` + `.meta.json`（含 `scenario` / `outbound_wire` / `upstream` / `model` / `recorded_at` / `request` / `expect` / `quirks`），并同时写出实际收到的出站请求供人工比对头注入。
2. **mock 加请求回显**：收到的请求头 / 路径 / body 进 `/api/logs`（或可读回执）；同时修掉 `mock.rs:36` 字面量 `split("\n\n")` 对 CRLF 的误判。
3. **可编程回放器**：按 `meta.json` 的 `chunks` 字节偏移切块发送（默认一段一帧），支持整帧 / 跨 chunk 半帧 / 单字节流 / 行尾变体；两处复用同一回放器——单元层直接喂字节数组，集成层起 mock 走真实 HTTP。
4. **结构级黄金比对**：解析两侧 SSE 逐帧逐字段比对（`type` 序列、`index` 连续性、`text`、`tool_use.input`、`thinking.signature`、`usage` 数值）+ HTTP 状态码与错误信封 + `meta.json` 里声明的字段级掩码；**禁止运行时自动写 golden**。
5. **`quirk::all()` 与双向守卫**（枚举值 ⇄ fixture `quirks` 标签）。
6. **`check.ps1`**：`-Quick` / `-Replay` / `-Live` / `-Smoke`，不带参数 = 全跑（不含 Live 与 Smoke 的人工部分），自行起网关与 mock、聚合出口码；**删除 `verify.ps1`**。
7. `prototype/tests/` 集成测试骨架能跑起来（哪怕先只有 1 格真断言）。

## 判据

- `cargo fmt --check`、`clippy -D warnings`、`cargo test` 全绿。
- `check.ps1` 与 `check.ps1 -Replay` 都能自行把网关与 mock 拉起来。
- 头类 quirk 至少有一条集成断言真读过回显的请求头。

## 相关

- [票 19](../issues/19-grilling-test-strategy.md)：本票唯一的形态来源。
- [票 30](../issues/30-task-build-regression-suite.md)：本票产出的基建由它填充 fixture 与断言。

## Answer

2026-10-15 落地完成（用户验收通过），判据三条全过。落点表与怎么跑写在 `prototype/README.md` §7。

### 落了什么

| 件 | 落地形态 |
|---|---|
| 录制 `record.ps1` | curl 打真实上游一次，`.sse` 字节原样落盘 + 写 `meta.json`（含 `request_sent` 供人工比对头注入）；`-DryRun` 只打印；重录必须人工加 `-Force`；支持 `-Header` 补网关会注入的头 |
| 回放 `src/replay.rs` | `meta.json` 的 `chunks`（字节偏移）优先，否则内建 `frame` / `half` / `byte` / `whole` / `crlf`；单元层直接喂字节，集成层由 mock 复用同一份代码 |
| mock 切块 + 回显 | 按当前切法逐块发（>64 块时不再逐块 sleep，否则单字节流要跑几十秒）；收到的路径 / 头 / body 进 `GET /api/echo`；顺带修掉旧的字面量 `split("\n\n")`（它对 CRLF 误判，现在走 `sse::find_sep`） |
| 结构级比对 `tests/common/golden.rs` | 两侧 SSE 解析成帧后逐帧逐字段比，差异给人可读字段路径；`message.id` 一类字段按 `masks` 掩码；**没有任何写黄金文件的代码路径** |
| quirk 守卫 `src/provider.rs` | `Quirk::all()` ⇄ fixture `quirks` 标签双向守卫；`Quirk::acts_on()` 用 `match` 声明作用对象，新增枚举值编译器逼着补行；豁免只许写进 `fixtures/quirk-exempt.json` 且必须带理由 |
| 入口 `check.ps1` | `-Quick` / `-Replay` / `-Live` / `-Smoke`，不带参数 = Quick + Replay，聚合出口码并列失败项；`verify.ps1` 已删除 |
| 集成骨架 `tests/replay.rs` | 测试自己 spawn 网关（18787/18788、临时 DB、`GATEWAY_ENV` 指到不存在的文件 → 绝不打真实上游），十格走真实 HTTP 断言 |

### 实测（2026-10-15）

- `pwsh -File .\check.ps1` **全绿**：`cargo fmt --check`、`clippy --all-targets -D warnings` 无警告、单元测试 **23** 个、回放集成测试 1 格（内含十格断言 + 五种切法不变性 + 头类 quirk + 认证头 + 路径模板）全过；约 20 秒。
- 四个开关各自实跑：`-Quick` 23/23、`-Replay` 15/15、`-Live` 0 个 `#[ignore]`（本票没有），`-Smoke` 因 `smoke.md` 还没写而按「跳过 + 警告」处理。
- **黄金断言反验**：往 `fixtures/expected/text.anthropic.sse` 里塞一帧 → `text/anthropic: 与黄金不一致：帧数不同：实际 8 帧，黄金 9 帧`，随即恢复。
- **守卫反验**：抽掉 `fixtures/text/anthropic.sse` → `provider::tests::every_tagged_fixture_has_bytes` 报「没有对应的字节文件」，随即恢复；单测 23 个全绿。
- **录制工具真跑过**：opencode Go 的 Chat 面**要求 `x-opencode-session`**，不补该头直接 400 `MissingSessionID`；补上头后 200、15774 字节真 SSE、末帧 `DONE`。这既是 recorder 的验收，也正面印证了 `SessionIdHeader` 这条 quirk 存在的理由。录到 `%TEMP%` 未入库（矩阵归票 30）。

### 与票 19 的四处偏差（都在票 19 的意图内）

1. **切法怎么拨**：票 19 只说「可编程回放器」，没说 HTTP 层怎么切。实现是 mock 侧控制面 `POST <mock>/__chunks {"scheme":"half"}`。否决了「借 `anthropic-beta` 传切法」——那会把测试开关塞进协议头，还会让别的测试跟着虚假失败。
2. **黄金文件只冻结 1 份**（`fixtures/expected/text.anthropic.sse`）：这一份是人工逐帧核对后冻结的（8 帧：start / block_start / ping / 两个 text_delta / block_stop / message_delta / message_stop），不是运行时自动写的；其余 9 格归票 30。
3. **quirk 豁免**：票 19 §6 只留了「fixture 标签」一条路。`ClaudeModelsOnAnthropicOnly` 是守卫型 quirk（wire 错配早拒），不产生 fixture 回放，落在 `src/upstream.rs::guard`；按票 23 它本就该被删除（删除属实现阶段），所以这一条写成带理由的豁免，而不是硬塞一个假标签。
4. **顺手修掉一个真 bug**：`sse::find_sep` 原来认 `\n\r\n` 为帧终止符，但正文结束位置取在 LF 上，于是最后一行的 `data:` 拖一个 `\r` 尾巴 → JSON 解析失败 → 整帧被当 ping 丢掉。CRLF 切法一加就暴露（真实上游若发 CRLF，流式内容会整段丢失）。已改成四种行尾组合都取在行尾之前。

### 留给票 30 的

- 其余 9 格黄金文件与 `expect` 的字段级断言、`tool_result` 多轮与增量 JSON、错误信封、并发会话、日志脱敏、台账（README §2 七条 ⇄ fixture）、`smoke.md`。
- 第 8 格（配置写入引擎）仍挂起（票 28 §10、地图 Out of scope）。
- 反向验证（第 12 条）现在**有了抓手**：抽掉 `fixtures/text/anthropic.sse`（或它的 meta），`provider::tests::every_tagged_fixture_has_bytes` 会红；抽掉一份 meta 的 `quirks` 标签会让 `fixture_quirk_tags_are_real_enum_values` / 双向守卫变红。本票已按此把「标签指着的 fixture 必须真有字节」也纳入守卫。
