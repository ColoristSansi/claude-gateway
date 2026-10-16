# 三个 quirk 的执法点（temperatureMax2 / emptyThinkingSignature / silentTruncate）

Type: task
Status: open
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
