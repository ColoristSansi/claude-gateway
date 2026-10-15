# 协议转换的测试与回归策略

Type: grilling
Status: resolved
Blocked by: —

## Question

协议转换的 bug 面很大（四个参考项目都在这上面踩坑），用什么方式保证改了不出事？

## 需要定下来的

1. **录制帧 fixture 的存放与格式**：怎么录（真实上游跑一次存原始 SSE）、怎么回放、怎么表达 LF/CRLF 混合与 keep-alive 空行这类坑。
2. 断言粒度：只断事件序列与终态，还是也断中间块内容；工具调用的 `tool_use`/`tool_result` 往返怎么断言。
3. 覆盖清单：五类场景（纯文本 / 工具调用 / thinking / usage / 异常中断）× 每对「入站 × 出站」协议，先做哪几对。
4. 出站方言的回归：`quirks` 每加一个枚举值就要加 fixture，这条怎么在流程上守住。
5. 端到端冒烟：mock 上游 + 真实客户端的自动/半自动检查清单（Claude Desktop 无法自动化到什么程度）。

## Answer

（本票只定策略与不变量，不实现套件；实现在原型里按本策略推进。）

### 0. 现状基线（本次只读盘点实测，作为改动前的地板）

- `verify.ps1` 是**纯事件序列级**：正则抽 `event:` 名 + 两个子串（`upstream_abort`、非零 token），**不解析 data JSON**；`tool_use` / `thinking` / `usage` 的 fixture 内容改坏仍 10/10；`curl -s` 无 `-w`，HTTP 状态码从未被检查。
- fixture 10 份**全 LF、CR 字节 0、无 `: keep-alive` 注释帧**；mock 每帧固定 `sleep(25ms)` 且逐帧整体写出，`sse.rs` 的半帧缓存分支**零覆盖**。
- Rust 测试 **0 个**，无 `tests/`、无 CI、无任务 runner。
- 「新增 quirk ⇒ 新增 fixture」只活在 `prototype/src/provider.rs:49` 注释里；头类 quirk 因 mock 不回显请求头而**无法断言**。
- 覆盖错位：两 wire 都是**出站**，入站只有 Anthropic Messages 一种，所以 5×2 无空格；真空格是格内空洞（`tool_result` 多轮、CRLF/keep-alive/半帧、非流式、`count_tokens`、错误信封）。

### 1. 测试分层（三层网）

| 层 | 守什么 | 跑法 |
|---|---|---|
| Rust 单元测试 | 纯函数：两套 codec、`sse.rs` 分帧与半帧缓存、`quirks` 决策 | `cargo test`，无网络 |
| 黄金 fixture 回放 | 「字节进、字节出」的端到端转换 | `cargo test`（集成测试自带 mock，自己拉网关） |
| 真实客户端半自动冒烟 | 配置写入、模型列表、四槽位端到端 | `check.ps1 -Smoke`，脚本能断言的自动、其余等人签字 |

理由：票 14 已证「上游差异 7 条必须进代码」，这类回归只有 fixture 回放能零成本重复；而 Desktop 配置写入与菜单显示只能实机验（票 15 / 16）。

### 2. fixture 形态

- **字节**：原始 SSE 逐字存 `.sse` 文本（连 `\r\n` 与空行原样保留），不美化、不重排、不归一化。归一化会在入库那步洗掉本项目最贵的字节级方言 bug。
- **传输**：**fixture 字节只存真正录制到的那一份**；「切块」不单独占一个 fixture。`meta.json` 里用**按字节偏移的 chunks 数组**声明切块方案（默认 = 一段一帧），一个 fixture 可配多套方案（整帧 / 跨 chunk 半帧 / 单字节流 / CRLF 行尾）。keep-alive 靠真录到 `: keep-alive` 注释帧的那份 fixture，不手编。
- **目录**：`prototype/fixtures/<scenario>/<outbound-wire>.sse` + 同名 `.meta.json`；黄金文件 `prototype/fixtures/expected/<scenario>.<outbound-wire>.sse`。
- **`meta.json` 字段**：`scenario` / `outbound_wire` / `upstream` / `model` / `recorded_at` / `request`（入站请求体，含 `tool_result` 多轮时的完整 messages）/ `chunks`（切块方案数组）/ `expect`（HTTP 状态码、错误类型、末帧类型）/ `quirks`（本 fixture 覆盖的 quirk 标签）/ `changed_why`。
- **录制**：写一个**单独的录制工具**（`--record` 小二进制或脚本），跑真实上游一次写入 `.sse` 与 `.meta.json`；录制时**同时写出实际收到的出站请求**，供人工比对头注入。重录是人工触发的动作，绝不在回归里自动发生。

### 3. 回放路径与断言

- **两种回放、同一个回放器**：单元层直接喂字节数组进 `sse` / codec；集成层起 mock 上游走真实 HTTP，让 `upstream.rs` 的头注入、`quirks` 执法点、错误信封转换都在网里被覆盖（`quirks` 的唯一执法点就在 HTTP 组装路径上，只喂函数等于放弃票 09 的硬约束）。
- **mock 加回显**：把收到的请求头 / 路径 / body 写入 `/api/logs`（或返回可读回执），让头类 quirk、路径模板、auth 头注入可被断言。**测试能力放在 mock 里，不放在网关的生产路径上**（否决「网关干跑端点」）。
- **断言 = 结构级**：解析两侧 SSE 后逐帧逐字段比对——`type` 序列、`index` 连续性、`text`、`tool_use.input`、`thinking.signature`、`usage` 各字段数值。**同时断言 HTTP 状态码与错误信封**（现有套件这条是空的）。
- **掩码**：`message_id`、时间戳、请求 ID 这类易变字段做字段级掩码归一化后比对；不掩就只能退回子串断言，等于 q5 那条被否决的路。掩码规则写在 `meta.json`，黄金内容写进 `expected/`。
- **金标准是录制值，不是网关自己的输出**：禁止「首次跑自动写 golden」。流式帧顺序属契约，黄金文件里的顺序不作为无所谓排序的对象。
- **fixture / 黄金文件可改，但必须留理由**：改动时同步更新 `meta.json` 的 `recorded_at` / `upstream` / `model` / `changed_why`，并在提交说明里写清原因；回放测试自身不得在运行时重写黄金文件。

### 4. 工具调用往返

两者都覆盖：
1. 上游流式回 **partial 的 `tool_use.input` 分片** → 断言网关拼完的 JSON 完整且合法（增量拼接的 bug 只在流式分片路径出现）；
2. 客户端带 `tool_result` 的**多轮请求**入站 → 断言它被正确编到出站 wire（Anthropic 挂在 user 轮 content block，OpenAI 必须是独立 `role:"tool"` 行且排在同轮文本之前，README §2 第 4 条）。

### 5. 覆盖矩阵次序

只做「**Anthropic 入站 × {Anthropic, OpenAI Chat} 出站**」这一行，按序推进：
1. 先把现有五场景（纯文本 / 工具调用 / thinking / usage / 异常中断）从子串断言换成**结构级黄金断言**；
2. 补 `tool_result` 多轮 + 增量 JSON；
3. 补字节级怪癖（CRLF / keep-alive / 半帧）；
4. 补上游错误信封（401 / 403 / 429）与未知模型 404。

**OpenAI Chat 入站列不在本票范围**：三个目标客户端都走 Anthropic 入站，它是另一张票（Ollama 入站同理，仍在 Not yet specified）。

### 6. quirk 守卫（双向）

1. `quirk::all()` 返回全部枚举值，静态断言「每个值都至少被一份 fixture 的 `meta.json` 的 `quirks` 标签覆盖」；
2. 新增枚举值时必须同时声明它作用的**出站 wire 或请求头**，否则静态断言失败；
3. 头类 quirk（`ClientUserAgent` / `SessionIdHeader`）靠 §3 的 mock 回显在集成层真断言，不靠静态检查充数。

### 7. 门禁与入口

- **单一入口** `pwsh -File .\check.ps1`，四个开关：`-Quick`（fmt + clippy + 单元测试，秒级）、`-Replay`（起网关与 mock，跑全部 fixture 回放集成测试）、`-Live`（跑带 `#[ignore]` 的真实上游验证）、`-Smoke`（客户端冒烟清单并逐项等确认）。**不带参数 = 全跑（不含 Live 与 Smoke 的人工部分）**，聚合出口码，最后打印失败行与出口码。
- **流程门禁**：改动编解码 / SSE / quirks / 转换时，agent 必须先跑它并把结果贴给用户。
- **`verify.ps1` 退役**：其能力被 `check.ps1` 完全覆盖，**删除**；「网关是否活着」由测试自带的健康检查报错，不保留同能力的旧脚本。
- **真实上游验证**：带 `#[ignore]` 的手动测试，只在 `-Live` 或 `cargo test -- --ignored` 下跑，读 `prototype-upstream.env` 的凭据；验「真实上游还能用」与「头透传真发出去了」，**不属于每次提交必跑的门禁**。
- 暂不上 CI（仓库还不是 git 仓库、单机单用户）。

### 8. 客户端冒烟（两层）

- **脚本能断言的那半自动**：起网关、四个槽位各跑一次流式、断言分落两家上游与终态、写 / 还原 Desktop 配置、写后重读文件比对（含票 17 的「四态且每次进页面实读文件比对」）。
- **剩下那半打印逐条清单等人确认**：Desktop 菜单真的显示四个家族名、状态文案、切回 1P 后官方登录态完好。
- 清单落在 `.scratch/model-gateway/smoke.md`（只列 **agent 做不到、必须人做** 的动作），`check.ps1 -Smoke` 打印它并逐项等确认；不是复制票 17 / 18 定过的行为。

### 9. 决策 ⇄ fixture 台账

在 `prototype/README.md` 维护一张台账：README §2 那 7 条「必须写进代码的差异」逐条对到覆盖它的 fixture 路径（或写「未覆盖 + 原因」）。回放集成测试断言台账里每条都已存在，缺失就红——让「哪条差异没网」一眼可见。

### 10. 补充纳入本票的守卫（用户勾选）

- **性能与背压 / 超时**：只钉两条不变式——① 首帧延迟（上游发第一帧后客户端应在有限时间内拿到 `message_start`，证明无整包缓冲）；② 上游卡住不发帧超过 `N` 秒时网关行为必须被声明且有断言（向前发 `ping` 或断流转 Anthropic 错误信封，二者都可）。**不做吞吐 / 延迟基线**（机器相关，是噪声）。
- **并发与会话隔离**：两个并发 fixture——① 两段不同对话同时发，断言 `SessionIdHeader` 各自稳定且互不串；② 同一段对话连发两次，断言会话头一致。附带：两客户端同时打同一槽位。
- **日志与脱敏**：① 默认不写 body 且能断言未写；② 开关开启后能捞到 body、到点（1 小时）自动关；③ 诊断包里不含密钥（拿一个已知密钥字符串反查）。实现跟着一起做。
- **配置写入引擎自身**：把 `desktop-3p.ps1` 的路径**参数化**（接受根目录），测试在临时目录里造出真的 `Claude-3p/configLibrary` 形状，断言：① 幂等（跑两次字节相同）；② 首写备份存在且可还原；③ 用户手改过的字段不被重写抹掉；④ 还原后尽量接近原状。**真实 `%LOCALAPPDATA%` 绝不在自动测试里被动过。**（**2026-10-15 由[票 28](28-grilling-config-writer-transactionality.md) 更新**：载体不再是参数化的 `desktop-3p.ps1`——那个脚本的三处行为已被票 28 推翻，重做归实现阶段、不开票；断言③的「手改字段」按非接管键理解，见 §15。这条断言何时能写，取决于引擎落地。）
- **模型解析与路由边界**四类：① 未知模型（含 `claude-opus-6` 这类将来名）的状态码与 Anthropic 错误信封；② `model` 大小写与首尾空白处理规则；③ 客户端档案覆盖生效且不影响其他客户端；④ 入站鉴权缺失的行为必须显式声明（本机单用户可明确「不鉴权」，但要写下来并断言）。

### 11. `count_tokens` 与非流式

写进本票的**待覆盖清单**——网关尚未实现它们（票 16：Desktop 推理探针是非流式 `max_tokens:1`，连接测试走 `/v1/messages/count_tokens`，不实现就常驻「模型不可用」横幅；票 14 原型是非流式直接 400）。**实现另开票**（[26](26-grilling-count-tokens-and-non-streaming.md)），且排在铺套件（[30](30-task-build-regression-suite.md)）之前，所以那两格的断言是对着已定行为写的，不需要先写「预期失败」。

### 12. 套件自己的验收标准（写在本票，作为它的尺）

1. `check.ps1` 不带参数能自行起网关与 mock 并聚合出口码；
2. `cargo test` 从 0 变非零且全绿；
3. `cargo fmt --check` 与 `clippy -D warnings` 无警告；
4. 10 份现有 fixture 全部过**结构级**黄金断言（不再是子串）；
5. 至少新增两类 fixture（字节级怪癖 + `tool_result` 多轮）；
6. 台账里每条已覆盖、无「未覆盖」空缺；
7. quirk 双向守卫能因「少一份 fixture」而变红（反向验证一次）。

### 13. 落地位置

就在 `prototype/` 里长：Rust 单测放 `src/**` 子模块，回放集成测试放 `prototype/tests/`，fixture 与黄金文件留在 `prototype/fixtures/`。正式建仓时整目录随代码搬，**本票不做迁移**。

### 14. 由本票新开的票据

- [建回归基建：录制 / 回放 / 断言 / 入口](29-task-regression-harness.md)（task，无阻塞）
- [铺满回归套件：fixture 矩阵与台账](30-task-build-regression-suite.md)（task，阻塞于 29 / 26 / 28）
- [实现 `count_tokens` 与非流式入站](26-grilling-count-tokens-and-non-streaming.md)（grilling，无阻塞）
- [配置写入引擎的事务性](28-grilling-config-writer-transactionality.md)（grilling，无阻塞）——本票第 10 条只定了「断言什么」，它定「什么行为能让那些断言成立」
- [测试与回归策略 ADR](27-task-regression-strategy-adr.md)（task，无阻塞）

### 15. 由票 28 明确的措辞（2026-10-15）

第 10 条断言③「用户手改过的字段不被重写抹掉」里的「用户手改过的字段」**指非接管键**：每个「客户端 × 文件」有一份显式接管键清单，清单内的键以网关为准（用户手改会被覆盖），清单外的一切原样保留。票 30 写这条断言时**只改非接管键**。完整行为见[票 28](28-grilling-config-writer-transactionality.md)。
