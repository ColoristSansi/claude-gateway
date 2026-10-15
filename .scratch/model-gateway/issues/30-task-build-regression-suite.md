# 铺满回归套件：fixture 矩阵与台账

Type: task
Status: resolved
Blocked by: 29

## Question

在已建好的基建上，把票 19 的 fixture 矩阵铺满、台账补齐、7 条验收标准逐条证明。

## 为什么是 task

形态由票 19 定死，基建由票 29 提供，本票只做填充与证明。三条前置：`count_tokens` 与非流式的行为（26）决定那两格的期望怎么写；配置写入引擎的行为（28）决定幂等 / 备份 / 用户手改保留那几条断言怎么成立。

## 要做的

1. **老 10 格换结构级黄金**：text / tools / thinking / usage / abort × {anthropic, openai} 出站，全部从子串断言升级为逐帧逐字段比对。
2. **工具往返**：① 上游 partial `tool_use.input` 分片 → 断言拼完的 JSON 完整合法；② 客户端带 `tool_result` 的多轮请求入站 → 断言编到出站 wire 的位置正确。
3. **字节级怪癖**：CRLF、`: keep-alive` 注释帧、跨 chunk 半帧、TCP 硬断（`upstream.rs` 的传输错误分支）。
4. **错误信封与路由边界**：401 / 403 / 429、未知模型（含 `claude-opus-6` 这类将来名）、`model` 大小写与首尾空白、客户端档案覆盖生效且不影响其他客户端、入站鉴权缺失的显式声明。
5. **并发与会话隔离**：两段不同对话并发不串 `SessionIdHeader`；同一段对话会话头一致；两客户端同打一槽位。
6. **性能不变式**：首帧延迟（证明无整包缓冲）+ 上游卡住时的声明行为（`ping` 或断流成错误信封）。**不做吞吐基线**。
7. **日志与脱敏**：默认不写 body、开关开启能捞到、到点自动关、诊断包不含密钥（已知密钥字符串反查）。
8. **配置写入引擎**：临时目录 + 参数化根目录下的幂等、首写备份、还原、用户手改字段保留。真实 `%LOCALAPPDATA%` 不碰。**（2026-10-15 票 28 结后注：引擎按票 28 的行为重做，但重做归实现阶段、不开票（票 28 §10、地图 Out of scope）——引擎落地前本格挂起；断言③只改非接管键。）**
9. **`count_tokens` 与非流式**：按票 26 定下的行为补断言。
10. **决策 ⇄ fixture 台账**：写进 `prototype/README.md`，README §2 的 7 条逐条对到 fixture；集成测试断言台账无缺口。
11. **`smoke.md`**：写给人的冒烟清单（agent 做不到的那半），`check.ps1 -Smoke` 打印并逐项等确认。
12. **反验守卫**：故意抽掉一份 fixture，确认 quirk 双向守卫变红。

## 判据（票 19 第 12 条的 7 条验收标准）

逐条跑一遍并把结果贴回本票：`check.ps1` 能自行起环境、`cargo test` 从 0 变全绿、fmt / clippy 无警告、10 格过结构级断言、新增两类 fixture、台账无缺口、守卫反验变红。

## 相关

- [票 19](../issues/19-grilling-test-strategy.md)：形态与验收标准。
- [票 29](../issues/29-task-regression-harness.md)：基建。
- [票 26](../issues/26-grilling-count-tokens-and-non-streaming.md)、[票 28](../issues/28-grilling-config-writer-transactionality.md)：两条前置行为。

## Answer

**套件铺满了**（2026-10-15）。12 条要做的里 11 条落地，第 8 条按地图 Out of scope 挂起；7 条验收标准逐条跑过并贴了证据。

### 落地形态

- `fixtures/` 从 10 份长到 **15 份**（6 场景 × 2 wire + `hardcut` × 2 + 真录的 `keepalive`），`fixtures/expected/` **15 格全有黄金**（票 29 时只有 1 份）。
- `tests/replay.rs` 从 1 格铺成 **11 格 24 个用例**，每格各起一份网关（端口按进程 id + 序号现算，能并行）：台账矩阵、切法不变性 + 头注入、工具往返、错误信封与路由边界、并发与会话隔离、首帧延迟 + 静默保活、探针档 504、`count_tokens` 与非流式、日志与脱敏、台账无缺口。
- 新增 mock 能力（只在 mock 侧，不进生产路径）：`err401/403/429/500`（按 model 名直接回状态码）、`hang`（先回头再静默）、`hang-nohdr`（连响应头都不回）、`hardcut`（字节发完产一个 Err，**真 TCP 级硬切**）。
- 新增两个旋钮与一个控制面：`GATEWAY_PING_SECS`、`GATEWAY_PROBE_SECS`、`POST /__bodylog`（body 日志窗口）。
- 台账写进 `prototype/README.md` §8，由 `ledger_has_no_gaps` 读表校验（7 行、目标必须真实存在、不许写「未覆盖」）。

### 本次用户决策（六个弹窗）

1. **`: keep-alive` 注释帧不造**：真录两次（普通 + 长生成，opencode Chat 面与 commandcode Anthropic 面）都**没录到**注释帧，按票 19 §2「字节只存真录到的」不手编；解析路径由 `sse.rs` 的单元测试覆盖，README §8 写明未覆盖原因。
2. **`event: ping` 原样收下**：commandcode 的 Anthropic 面真录到一个上游 ping 帧（40058 字节 / 888 行），作为 `fixtures/keepalive/anthropic.sse`（`origin: recorded`，只改 meta 里的客户端模型名并写进 `changed_why`）。
3. **上游静默时向前发 `ping`**（不给上游加超时，不违反票 26 决定 5）。
4. **body 日志最小实现**：`GATEWAY_LOG_BODY=1` / `POST /__bodylog` + 1 小时自动关。
5. **客户端档案覆盖不在原型里造**：原型没有客户端档案概念，按用户决定归实现阶段，README §8 记「未覆盖 + 原因」。
6. **`model` 规则 = 去首尾空白 + 大小写敏感**；未命中 → 404 `not_found_error`。
7. **探针档断言加 `GATEWAY_PROBE_SECS` 旋钮**，测试拨到 1 秒，门禁时长基本不变。

### 7 条验收标准（逐条证据）

| # | 判据 | 结果 |
| --- | --- | --- |
| 1 | `check.ps1` 能自行起环境并聚合出口码 | 通过：不带参数就起 10 份网关（每个 `#[tokio::test]` 一份，端口按进程 id + 序号现算）跑完 24 个用例，最后聚合出口码；不再依赖「先手工启动网关」 |
| 2 | `cargo test` 从 0 变非零且全绿 | 通过：单元 **23**、回放 **24**（票 29 是 23 个单测 + 1 格集成） |
| 3 | `cargo fmt --check` 与 `clippy -D warnings` 无警告 | 通过（`check.ps1` 前两步无输出） |
| 4 | 10 份现有 fixture 全过结构级黄金 | 通过，且扩到 **15 格**；`text.anthropic.sse` 与票 29 冻结的那份**逐字节相同**（说明这轮没改既有行为） |
| 5 | 至少新增两类 fixture | 通过两类：字节级怪癖（`hardcut` 真硬切 + `keepalive` 真录 ping）、`tool_result` 多轮（`toolresult`） |
| 6 | 台账里每条已覆盖、无「未覆盖」空缺 | 通过：`ledger_has_no_gaps` 读 README §8，7 行目标都存在；另有两处**明确不覆盖**（注释帧 / 客户端档案）写清原因且不在表内 |
| 7 | quirk 双向守卫能因「少一份 fixture」而变红 | 通过（真跑）：抽掉两份带标签的 meta → `every_quirk_is_covered_by_a_fixture_or_explicitly_exempt` 红；抽掉 `.sse` → `every_tagged_fixture_has_bytes` 红 |

反验另跑了一条：往 `fixtures/expected/text.anthropic.sse` 追加一帧 → 回放测试报「帧数不同：实际 8 帧，黄金 9 帧」。三条反验后文件都已还原（长度校验过）。

### 实测命令与输出

```
pwsh -File .\check.ps1
=== fmt    (cargo fmt --check) ===
=== clippy (cargo clippy -D warnings) ===
=== unit   (cargo test --bins) ===       23 passed
=== replay (集成：自起网关 + mock，回放 fixtures) ===  24 passed (finished in 15.21s)
全部通过。
```

### 第 8 条（配置写入引擎）仍挂起

按地图 Out of scope：`desktop-3p.ps1` 的三处行为已被票 28 推翻，重做归实现阶段、本图不开票；引擎落地后「参数化根目录下的幂等 / 首写备份 / 还原 / 非接管键手改保留」几条断言才写得出来。本票的 `[in]` / 台账 / 黄金都没依赖它。

### 顺带得到的两条新事实

- **两个真实上游都不发 `: keep-alive` 注释帧**（两次真录、长短两种生成都验过）。
- **commandcode 的 Anthropic 面会发 `event: ping`**（opencode 的 Chat 面不发）——网关按 `Ping` 事件原样转给客户端，现有 fixture 与黄金把这个行为钉住了。

本票**不新开票据**。冒烟清单交给人的那半写在 [smoke.md](../smoke.md)（`check.ps1 -Smoke` 打印并逐项等确认，已实跑过一次）。

## Comments

- 2026-10-15：**票 26 已结，本票的 `Blocked by` 去掉 26**（仍等 29 与 28）。第 9 条要断言的两种行为已在原型里实现，期望值直接对着[票 26 的 Answer](../issues/26-grilling-count-tokens-and-non-streaming.md) 写；三个现成锚点：mock 的 count_tokens 固定返回 **4242**（用来分辨「转发」与「本地估算」）、`PROBE_MAX_TOKENS=16`、`PROBE_DEADLINE=20s`（探针档超时 → 504）。另有两条新增负例可直接用：`cc-sonnet` 的 count_tokens 走降级路径（commandcode 无该路由）、`mock-*-abort` 的非流式必须 502。
