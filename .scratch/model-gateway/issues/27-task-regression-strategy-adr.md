# 测试与回归策略 ADR

Type: task
Status: resolved
Blocked by: —

## Question

把票 19 定下的测试与回归策略写成一条 ADR 并归档。

## 为什么是 task 而不是决策

决策已经由票 19 做完（fixture 回放为门禁、结构级断言、真实上游只做手动、`quirks` 双向守卫）。本票只做「把它写进 `docs/adr/` 并接上编号与现状引用」这件手工活，没有任何新的取舍要定。

## 要做的

1. 新建 `docs/adr/0005-protocol-conversion-regression-strategy.md`（编号接在现有 0001–0004 之后；**0004 已被[票 23](23-design-provider-table-granularity.md) 的供应商表粒度占用**），按仓库现有 ADR 形状写：一句话决策 + 理由 + `**Status**:` + `**Considered Options**:`，并链回票 19。
2. 只记**决策与理由**：门禁用黄金 fixture 回放（而不是每次都打真实上游）、断言必须到结构级（而不是事件序列级）、真实上游验证降级为 `#[ignore]` 手动、fixture 允许修改但必须留 `changed_why` 与重录日期。
3. 顺带核对：仓库根 `research/` 实际在 `.scratch/model-gateway/research/`（map 的 Notes 第 19 行写的是 `research/`），ADR 里的引用用实际路径。

## 判据

三条 ADR 判据都满足：① 不可逆——套件形态决定后续所有转换改动的成本；② 没有上下文会困惑——后来人一定会问「为什么不直接跑真实上游」；③ 真取舍——票 19 已否决过「只喂函数」「归一化 JSON」「全部自动化」等备选。

## 产物

- `docs/adr/0005-protocol-conversion-regression-strategy.md`

## Answer

（2026-10-15 结，task，AFK 自解。）

写了 [`docs/adr/0005-protocol-conversion-regression-strategy.md`](../../../docs/adr/0005-protocol-conversion-regression-strategy.md)，编号接在 0001–0004 之后（0004 已被票 23 的供应商表粒度占用），形状与 0004 一致：标题即一句话决策 + 理由段 + 五条决定 + `**Status**:` + `**Considered Options**:` + `**Consequences**:`。

只记决策与理由，不搬实现细节：

1. 门禁 = 黄金 fixture 回放，不在每次提交时打真实上游；
2. 断言必须结构级（逐帧逐字段 + HTTP 状态码与错误信封 + `meta.json` 声明的字段级掩码），禁止子串与事件序列级；
3. 真实上游验证降级为 `#[ignore]` 手动（`check.ps1 -Live` / `cargo test -- --ignored`），不属于提交门禁；
4. fixture 与黄金文件可改但必须留 `changed_why` 与重录信息，回放测试不得在运行时自动写 golden；
5. 单一入口 `check.ps1`（四开关，不带参数全跑），`verify.ps1` 退役删除。

备选列了 7 条否决：只喂函数、归一化 JSON 比对、全部自动化、首次跑自动写 golden、吞吐/延迟基线、保留 `verify.ps1`、上 CI。

**顺带核对（票内第 3 条）**：调研产物实际在 `.scratch/model-gateway/research/`（map 的 Notes 第 19 行写的是 `research/` 这个简写），ADR 里的引用已按实际路径写；同样把原型路径写成 `.scratch/model-gateway/prototype/…`（`prototype/`、`prototype-upstream.env` 都在 `.scratch/model-gateway/` 下，票 19 正文里是相对 `.scratch/model-gateway/` 的简写，照抄会在 ADR 里指错位置）。ADR 里「`quirks` 唯一执法点」的说法对着代码核过：`.scratch/model-gateway/prototype/src/upstream.rs:78-79` 注入 `ClientUserAgent` / `SessionIdHeader` 请求头。

无新增票据，无阻塞变化。附带发现（不属本票，未动）：原型里 `Quirk::ClaudeModelsOnAnthropicOnly` 仍在（`prototype/src/main.rs:117`、`store.rs:45`、`upstream.rs:42`），票 23 定的是删除它——那属于票 29/30 的改动面。
