# 协议转换的回归门禁用黄金 fixture 回放 + 结构级断言

协议转换的 bug 面在字节方言上，而现有套件（`verify.ps1`）只做事件序列级子串匹配：`tool_use` / `thinking` / `usage` 的内容改坏仍 10/10，`curl -s` 无 `-w`，HTTP 状态码从未被检查。所以门禁改为**黄金 fixture 回放**，断言落到**结构级**，真实上游验证降级为手动。

决定：

1. **门禁 = 黄金 fixture 回放**，不在每次提交时打真实上游。fixture 是「字节进、字节出」的端到端转换：`.scratch/model-gateway/prototype/fixtures/<scenario>/<outbound-wire>.sse` + 同名 `.meta.json`，黄金文件 `.scratch/model-gateway/prototype/fixtures/expected/`。
2. **断言必须是结构级**：解析两侧 SSE 后逐帧逐字段比对（`type` 序列、`index` 连续性、`text`、`tool_use.input`、`thinking.signature`、`usage` 数值），**同时断言 HTTP 状态码与错误信封**。易变字段（`message_id`、时间戳、请求 ID）按 `meta.json` 声明的字段级掩码归一化。禁止子串与事件序列级断言。
3. **真实上游验证降级为 `#[ignore]` 的手动测试**，只在 `check.ps1 -Live` 或 `cargo test -- --ignored` 下跑，读 `.scratch/model-gateway/prototype-upstream.env` 的凭据；它验「上游还能用」与「头透传真发出去了」，**不属于提交门禁**。
4. **fixture 与黄金文件可改，但必须留理由**：改动同步更新 `meta.json` 的 `recorded_at` / `upstream` / `model` / `changed_why`，提交说明写清原因；**回放测试不得在运行时自动写黄金**（金标准是录制值，不是网关自己的输出）。
5. **单一入口** `check.ps1`（`-Quick` / `-Replay` / `-Live` / `-Smoke`，不带参数 = 全跑，聚合出口码），`verify.ps1` 退役删除。

理由：这类回归只有 fixture 回放能零成本重复，而字节级方言（CRLF / keep-alive 注释帧 / 半帧切块）与头类 quirk **只在真实 HTTP 组装路径上被执行**——`quirks` 的唯一执法点是 `.scratch/model-gateway/prototype/src/upstream.rs` 组装请求那一步（`ClientUserAgent` / `SessionIdHeader` 在那里注入请求头），只喂函数等于放弃票 09 的硬约束。断言必须到结构级，因为子串断言对内容改坏不敏感（票 19 的现状基线实测）。

**Status**: accepted（2026-10-15，由 [票 27](../../.scratch/model-gateway/issues/27-task-regression-strategy-adr.md) 归档，决策本身由 [票 19](../../.scratch/model-gateway/issues/19-grilling-test-strategy.md) 定案；落地形态见 [票 29](../../.scratch/model-gateway/issues/29-task-regression-harness.md) 与 [票 30](../../.scratch/model-gateway/issues/30-task-build-regression-suite.md)）

**Considered Options**:

- **只喂函数**（单元层直接进 codec，不经 HTTP 网）：失去头注入、`quirks` 执法点、错误信封转换的覆盖。否决。
- **归一化 JSON 后比对**：归一化会在入库那步洗掉本项目最贵的字节级方言 bug。否决。
- **全部自动化**（每次提交都跑真实上游）：慢、要凭据、上游抖动会红门禁。否决，降级为 `#[ignore]` 手动。
- **首次跑自动写 golden**：金标准会变成网关自己的输出，回归退化为自证。否决。
- **吞吐 / 延迟基线**：机器相关，是噪声。只保留两条不变式（首帧延迟、上游卡死时的行为声明）。否决。
- **保留 `verify.ps1`**：能力被 `check.ps1` 完全覆盖，留着就是两套真相。否决，删除。
- **上 CI**：仓库还不是 git 仓库、单机单用户。暂不做。

**Consequences**: fixture 与黄金文件只能人工重录，没有自动更新路径——改了转换行为就必须回去重录并写 `changed_why`。`.scratch/model-gateway/prototype/README.md` §2 那 7 条「必须写进代码的上游差异」要维护一张 ⇄ fixture 台账，回放测试断言台账无空缺（让「哪条差异没网」一眼可见）。`count_tokens` 与非流式已实现（票 26）但排在铺套件之前，其断言对着已定行为写，不需要先写「预期失败」。调研产物实际在 `.scratch/model-gateway/research/`（不是仓库根 `research/`）。套件自己长在 `.scratch/model-gateway/prototype/`（单测在 `src/**`、集成测试在 `prototype/tests/`），正式建仓时整目录随代码搬。
