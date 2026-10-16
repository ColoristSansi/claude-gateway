# 三个 quirk 的执法点（temperatureMax2 / emptyThinkingSignature / silentTruncate）

Type: task
Status: resolved
Blocked by: —

## Question

spec §6:211 定「`quirks` 只允许枚举值，**新增一个 quirk 必须同时新增一条回归 fixture**」，并且在示例表里列了 `noModelsEndpoint` / `temperatureMax2` / `emptyThinkingSignature` / `silentTruncate`。前一个（`noModelsEndpoint`）有执法点（票 50 收窄成「不必尝试」），**后三个至今没有任何执法点**：`crates/gateway-core/tests/fixtures/quirk-exempt.json` 逐字自认「转换代码里还没有执法点；实现后必须同批补一条 fixture」。

评审（`cg-notes/code-review.md` 的「高」第三条）点名这三条，其中还有一处真 bug：`crates/gateway-core/src/anthropic.rs` 用 `signature.is_some()` 丢弃无签名 thinking，**覆盖不到 `Some("")`**——空串签名会被原样回给客户端，而 spec §4.1 要的是「没有 signature 的 thinking 块丢弃」（带回去下一轮会被官方校验拒）。

依据：spec §4.1（无签名 thinking 丢弃）、**§5:155**（`temperature` 各家取值域不同，**出站时按目标模型夹取**）、§5 第 3 条硬规则（**上游缺 `finish_reason` 时必须自己补一个终态**）、§6:211 / §6 示例表；`crates/gateway-core/tests/fixtures/quirk-exempt.json` 的现有豁免理由。

## 交付

三个 quirk 各补**执法点 + 回归 fixture**，并把它们从 `quirk-exempt.json` 的「无执法点」豁免里**移出**（豁免理由若变成「有执法点」就不该再挂在那里；移出后 quirk 双向守卫必须仍绿）：

1. **`temperatureMax2`** —— 按目标模型/上游把出站 `temperature` 夹到合法域（该 quirk = 上游上限 2）。注意 spec §5 说「各家取值域不同」，所以夹取要挂在**声明式供应商表/quirk** 上，**不许在转换代码里写供应商名判断**（§6 的硬规矩）。补一条 fixture：同一个 IR 请求打到带该 quirk 的上游，出站体里的 `temperature` 是夹取后的值；不带该 quirk 的上游原样。
2. **`emptyThinkingSignature`** —— 把「无签名 thinking 块丢弃」修成**覆盖空串**（`signature` 为 `None` **或空串**都丢）。补 fixture：上游帧里带一个 `signature: ""` 的 thinking 块 → 客户端面不出现该块；带真签名的照常保留（这条是回归护栏，别把正常的也丢了）。
3. **`silentTruncate`** —— 上游可能**静默截断**（缺 `finish_reason` / 终态帧），网关必须**自己补一个终态**，否则最后一个块不关、内容丢失（§5 第 3 条硬规则，原型踩过）。补 fixture：上游流在内容块中途断掉且**没有**终态 → 客户端面必须收到一个合成的终态（`stop_reason` 归一化、块关闭），且**不**把半截内容丢掉。

## 收口验收

- 三个 quirk **各一条**能变红的 fixture / 用例（说明它真在链路上生效，不是只加了个枚举值）；
- `quirk-exempt.json` 里这三条豁免被移出或改写成「有执法点」，且 quirk 双向守卫（`every_quirk_is_covered_by_a_fixture_or_explicitly_exempt` + `every_tagged_fixture_has_bytes`）仍绿；
- **反向验证至少两条**：把执法点改坏 → 对应用例变红 → 改回变绿（贴前后）；
- `pwsh -File ./check.ps1 -Quick -Replay` 全绿（含前端类型检查步）；**票 41/51/56/64/65 的回放格一格不许红**（这三条都动在共用链路上）；
- 若某条 quirk 的真实语义无法从现有材料确定（spec 只给了名字与一句话），**不许猜**：把该条留成显式欠账（写进 Answer 的遗留 + 豁免理由改成「语义待定，为什么」），并说明你需要什么证据才能定。

## 不做

- 不改 spec / ADR（`quirks` 示例表已列这三个值，本票只是把「说了的」做实）。
- 不动票 65 的 Responses 出站与票 64 的 Chat 入站。

## Answer

三条全部做实，没有一条留成欠账。每条的执法点都读**声明式 quirk**（`up.has_quirk(...)`），转换
代码里没有任何供应商名（spec §6 硬规矩）。

### 1. `temperatureMax2` — 出站温度夹取

- **执法点**：`crates/gateway-core/src/egress/upstream.rs` 的 `clamp_temperature(req, up)`，
  在 `forward` 与 `forward_buffered` 编 body **之前**各调一次（三条出站 wire 共用同一处；
  夹在 IR 上，`anthropic` / `openai-chat` / `responses` 三条编码器都从 `req.temperature` 取值）。
- **fixture**：`tests/fixtures/tempmax2/anthropic.meta.json` + `anthropic.sse`
  （`quirks: ["TemperatureMax2"]`），同一份字节挂在两条上游上：
  `mock-anthropic-tempmax2`（上游 `mockq`，声明 quirk）与 `mock-anthropic-tempnone`（上游 `mock`，
  不声明）。用例 `tests/replay.rs::quirk_enforcement_points` ①：出站回显 `temperature` 一个 2.0、
  一个原样 3.0。
- **判据来源**：spec §5:155「`temperature` 各家取值域不同，出站时按目标模型夹取」+
  §6:211「新增一个 quirk 必须同时新增一条回归 fixture」+ §6「不许在转换代码里写供应商名」。
- **形状选择（没有加带载荷的变体）**：`Quirk::TemperatureMax2` 保持**无载荷**枚举值。上限 2 已经
  逐字写在变体名里（票 23 只定了认证与可用 wire 的粒度，没给 quirk 载荷更细的粒度；spec §6 的
  `quirks` 样例就是字符串数组）。现有枚举确有带载荷的形式（`ClientUserAgent(String)` /
  `SessionIdHeader(String)`，库里按 `name` + `arg` 存），但那是「载荷不是固定的」才需要的；
  给一个名字即上限的 quirk 加 `arg` 属于没人要的灵活性。将来要表达「上限 N」时，照
  `SessionIdHeader` 的形状加 `TemperatureMaxN(String)` 并同批补 fixture 即可。

### 2. `emptyThinkingSignature` — 空串签名与 `None` 一样丢

- **执法点**：`crates/gateway-core/src/anthropic.rs` 的 `AnthropicAssembler::message()`：
  丢弃条件从 `signature.is_some()` 改成
  `signature.as_deref().is_some_and(|s| !s.is_empty())`（`None` 或空串都丢）。
- **fixture / 用例**：`tests/fixtures/emptysig/anthropic.meta.json` + `anthropic.sse`
  （`quirks: ["EmptyThinkingSignature"]`，帧里带 `signature_delta` 的 `signature: ""`）；
  单元护栏 `anthropic::tests::thinking_without_signature_or_with_an_empty_one_is_dropped`
  （`None` / 空串 / 真签名 `sig-1` 三种一起断言）；链路用例在
  `tests/replay.rs::quirk_enforcement_points` ③：非流式打到 `mock-anthropic-emptysig`，
  `content` 里没有 thinking 块、文本块照旧；同时用 `mock-anthropic-thinking` 断言真签名
  （`bW9jay1zaWc=`）照常保留。
- **判据来源**：spec §4.1:96「没有 signature 的 thinking 块丢弃」（该句就写在**非流式组装**那一段，
  所以执法点也只在组装器里；流式是上游帧直转，不在这一句的射程内）。

### 3. `silentTruncate` — 静默截断处自己补终态

- **执法点**：`egress/upstream.rs`。新增 `OpenBlocks`（记还开着的 content block）在两条出口上都
  observe；收流结束时若「没有传输错误、没有 error 事件、没有终态、且上游声明了
  `Quirk::SilentTruncate`」，就把它 `close_all()`（逐个 `BlockStop` + 一个 `Eos`）喂回**现有的**
  编码器/组装器：
  - 流式：`Pump::finish` 的 synthesize 分支（`synthesize_terminal` = `up.has_quirk(&Quirk::SilentTruncate)`）；
  - 非流式：`forward_buffered` 收流后、算 `failure` 之前。
  补的终态走各 encoder 既有的 Eos 路径（`message_delta` + `message_stop`，`stop_reason` 缺省归一化
  成 `end_turn`），**没有另造一条收尾路径**。
- **fixture / 用例**：`tests/fixtures/trunced/anthropic.meta.json` + `anthropic.sse`
  （`quirks: ["SilentTruncate"]`，流在 `content_block_delta` 中途干净结束，无 `content_block_stop` /
  `message_delta` / `message_stop`）。用例 `tests/replay.rs::quirk_enforcement_points` ②：
  `mock-anthropic-trunced`（上游 `mockq`）末帧 `message_stop`、无 `error` 帧、有
  `content_block_stop index 0`、拼出的文本仍是「说到一半就断了」。
- **判据来源**：spec §5 第 3 条硬规则「上游缺 `finish_reason` 时必须自己补一个终态，否则最后一个块
  不关、内容丢失」（cc-switch 线上 issue）。
- **闸门怎么落**：**只在声明了该 quirk 的上游上补**。spec §4.1:96 明写非流式「上游断流 / 无终态 →
  502 JSON 错误信封」，票 30 的 `abort` / `hardcut` 又把这个「无终态 = 报错」做成了判据；两条硬
  要求只有在「quirk 声明过的上游才补」时才同时成立——quirk 的 `acts_on()` 本身就写着「上游**可能**
  静默截断（不给终态）」。护栏用例即 `mock-anthropic-trunced-plain`（同一份字节、上游不声明 quirk）
  仍给出 `upstream_abort` 且无 `message_stop`。传输层真错误（`hardcut` 那种 TCP 硬切）不进补终态
  分支，仍是 error 帧——「静默」的定义就是干净收流、连错误都不给。

### 验收 1：三条各有一条能变红的用例

`cargo test -p gateway-core --features mock --test replay quirk_enforcement_points`
实现前（红）：`三个 quirk 的执法点 4 处不合格：带 quirk 的上游出站 temperature 应夹到 2，实际 3.0 /
静默截断必须补到 message_stop 收尾，实际末帧 Some("error") / 补了终态就不该再发 error 帧 /
断掉的那个块必须被关掉`；
`cargo test -p gateway-core --lib anthropic::tests::thinking` 实现前（红）：
`left: 2, right: 1`。实现后两条都 `ok`。

### 验收 2：豁免移出 + 双向守卫

`tests/fixtures/quirk-exempt.json` 的三条（`TemperatureMax2` / `EmptyThinkingSignature` /
`SilentTruncate`）**整条删掉**（不是改写成「有执法点」——豁免表的语义是「还没有 fixture 标签」，
留着只会让守卫少一层网）。`cargo test -p gateway-core --lib egress::provider::tests`：
`4 passed`（`every_quirk_is_covered_by_a_fixture_or_explicitly_exempt`、
`every_tagged_fixture_has_bytes`、`fixture_quirk_tags_are_real_enum_values`、
`every_quirk_declares_what_it_acts_on`）。豁免表里剩两条上游级事实
（`NoModelsEndpoint` / `ModelsListNeedsCredential`）。

### 验收 3：反向验证（三条各做一遍，改坏 → 红 → 改回 → 绿）

1. `clamp_temperature` 里 `t.min(2.0)` → `t`：
   `三个 quirk 的执法点 1 处不合格：带 quirk 的上游出站 temperature 应夹到 2，实际 3.0`（红）；
   改回后 `test quirk_enforcement_points ... ok`。
2. `Pump::finish` 的 `if self.synthesize_terminal` → `if self.synthesize_terminal && false`：
   `3 处不合格：静默截断必须补到 message_stop 收尾，实际末帧 Some("error")… / 断掉的那个块必须被关掉`（红）；
   改回后 `ok`。
3. `anthropic.rs` 的丢弃条件改回 `b.signature.is_some()`：
   `thinking_without_signature_or_with_an_empty_one_is_dropped ... FAILED`，`left: 2, right: 1`（红）；
   改回后 `ok`。

### 验收 4：门禁

`pwsh -File ./check.ps1 -Quick -Replay` → 末行 `全部通过。`，exit 0。
含前端类型检查步（`tsc --noEmit && vite build`，这次先在 worktree 里 `pnpm --dir app install`：
上一次跑这一步是因为本 worktree 没有 `app/node_modules` 才红的，不是代码问题）。
回放 23 格全绿，票 41/51/56/64/65 的格子（`fixtures_replay_through_the_gateway` 的台账矩阵、
`chunking_invariance_and_header_injection`、`responses_inbound_contract`、
`client_profile_override_and_models_exposure`、`body_recording_switch_drives_the_proxy_window` 等）
一格没红。

### 留下的坑

- **回放脚手架多了一条上游 `mockq`**（`gateway-harness.rs`）：同一个 mock 进程的第二条上游，
  只为造出「带 quirk / 不带 quirk」两种形态。它是**测试专用 bin**，产品构建看不到（`required-features = ["mock"]`）。
- `silentTruncate` 只覆盖「干净收流、无终态」；TCP 硬切（`hardcut`）仍走 error 帧。若将来 spec 要把
  §5 第 3 条改成**所有**上游的普适行为，那要连 §4.1:96 的 502 与票 30 的 `abort` / `hardcut` 判据一起
  重新定案——本票按「quirk 声明才补」落地，两条硬要求同时成立。
- `emptyThinkingSignature` 的执法面只在**非流式组装**（spec §4.1 那句的射程）。流式路径是上游帧直转，
  带空签名的 thinking 块仍会原样出现在流里；要覆盖它得让编码器缓冲整个 thinking 块再决定发不发，
  不在本票范围。

