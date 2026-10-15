# 决策 ⇄ fixture 台账（票 41 从原型 `README.md` §8 抄入）

> 原型（`.scratch/model-gateway/prototype/`）已冻结，那份台账一个字节都不许改；
> 这张表是**产品侧**的那一份，由 `tests/replay.rs::ledger_has_no_gaps` 读。
> 目标必须真实存在（文件，或 `文件::函数`）；空目标或写「未覆盖」都让门禁变红。

## 差异清单

票 19 §5 列的 7 条「声明式表吸收不了、必须写进代码的差异」：

1. model 与 wire 的绑定是逐模型的（同一上游的 Claude 模型可能只在某个端点）。
2. usage 字段名与到达时机不同（Anthropic 在 `message_delta`，Chat 在末尾 chunk 且字段名不同）。
3. OpenAI wire 没有 `message_start`，必须补发。
4. `tool_result` 的载体不同（Anthropic 挂在 user 轮的 content block，Chat 提升成 `role:tool`）。
5. thinking 签名不可跨协议搬运（Anthropic 直通保住，Chat wire 换不回来）。
6. 入站的 Anthropic 协议头（`anthropic-version` / `anthropic-beta`）必须透传，且只在 Anthropic wire 上。
7. 上游错误信封要就地转成**入站协议**的信封，且保持状态码。

## 台账

| # | 差异 | 覆盖它的 fixture / 测试 |
| --- | --- | --- |
| 1 | model 与 wire 的绑定是逐模型的 | `src/binding/mod.rs` + `src/egress/upstream.rs::guard` |
| 2 | usage 字段名与到达时机不同 | `tests/fixtures/usage/anthropic.sse` + `tests/fixtures/usage/openai.sse` |
| 3 | OpenAI wire 没有 message_start，必须补发 | `tests/fixtures/text/openai.sse` + `tests/replay.rs::fixtures_replay_through_the_gateway` |
| 4 | tool_result 的载体不同 | `tests/fixtures/toolresult/anthropic.sse` + `tests/fixtures/toolresult/openai.sse` + `tests/replay.rs::tool_roundtrip_multiturn_and_partial_input` |
| 5 | thinking 签名不可跨协议搬运 | `tests/fixtures/thinking/anthropic.sse` + `tests/fixtures/thinking/openai.sse` |
| 6 | 入站的 Anthropic 协议头必须透传 | `tests/replay.rs::chunking_invariance_and_header_injection` |
| 7 | 上游错误信封要就地转成入站信封 | `tests/replay.rs::error_envelopes_and_route_boundaries` + `src/bin/mock-upstream.rs::error_status` |

## 明确不覆盖的两处

（写了原因，不是漏掉。）

- **`: keep-alive` 注释帧**：2026-10-15 真录两次（普通生成与长生成，分别打 opencode 的 Chat 面与
  commandcode 的 Anthropic 面）都**没录到**注释帧——两个真实上游都不发它。按票 19 §2
  「字节只存真录到的那一份」不手编；解析路径由 `src/sse.rs` 的单元测试
  `comment_frames_do_not_swallow_the_next_frame` 覆盖。真录到的那一个 `event: ping` 帧已作为
  `tests/fixtures/keepalive/anthropic.sse` 收下（40KB / 888 行，字节原样）。
- **客户端档案覆盖**（同一槽位按客户端走不同上游）：原型没有客户端档案概念（票 10 的设计），
  按用户 2026-10-15 的决定不在原型里造，归实现阶段（票 51 / 49）；`binding::Route` 已经带着
  `is_family`，档案那一维加在它上面。

## quirk 双向守卫与豁免

- 枚举值 ⇄ fixture 的 `quirks` 标签双向守卫在 `src/egress/provider.rs` 的四份单元测试里
  （`cargo test --workspace` 就跑，不需要 `mock` 特性）。
- 真有理由不覆盖的 quirk 只许写进 `tests/fixtures/quirk-exempt.json` 并带理由。当前唯一一条：
  `ClaudeModelsOnAnthropicOnly` 是**守卫型** quirk（wire 错配当场拒，见
  `src/egress/upstream.rs::guard`），不产生 fixture 回放；票 23 已决定删掉它，删除随实现阶段。
  （搬入时只改了这条理由里的文件路径——原型已冻结，那份豁免文本不许动。）
