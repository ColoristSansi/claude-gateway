# 核心编解码搬迁

Type: task
Status: resolved
Blocked by: 39

## Question

把 `prototype/src/` 里的五个文件搬进 `crates/gateway-core`，按 spec §3 的模块边界归位，**不改行为**：

| 原型文件 | 落点 |
| --- | --- |
| `ir.rs` | `ir` |
| `sse.rs` | `ingress` + `egress` 共用的 `sse` |
| `anthropic.rs` | `ingress::anthropic` + `egress::anthropic`（同一文件，入站出站都在这） |
| `openai.rs` | `egress::openai_chat` |
| `responses.rs` | `ingress::responses` + `egress` 侧编码器 |

三个编解码文件**各自保持完整**，不硬拆成两半（[ADR-0007](../../../docs/adr/0007-skeleton-and-implementation-order.md)）。

依据：spec §3 / §5、[prototype/README.md](../prototype/README.md) §1。

## 交付

- 五个源文件与其单测随行（原型：anthropic 5 / ir 4 / responses 6 / sse 5 个 `#[test]`）。
- core 的 `lib.rs` 写清模块划分与「core 不依赖 Tauri」这条约束。
- 不改任何一处的编解码逻辑；有必要的 `pub` 可见性调整除外。

## 收口验收

- `check.ps1 -Quick` 绿，且这几个文件的单测数与原型持平。
- 与原型逐字节行为一致：拿原型 `fixtures/` 里任意一份 SSE，两边解码出的 IR 事件序列相同（本票可用临时对照脚本，脚本不入库）。

## Answer

### 做了什么

五个文件从 `.scratch/model-gateway/prototype/src/` **逐字节**搬进 `crates/gateway-core/src/`（`ir.rs` / `sse.rs` / `anthropic.rs` / `openai.rs` / `responses.rs`，含各自 `#[cfg(test)]`）；占位目录 `src/ir/mod.rs` 删掉，换成 `src/ir.rs`。搬完 `cmp` 逐文件核对，5/5 与原型字节相同；再比 `git rev-parse :<path>` 的 index blob（`core.autocrlf=true`，行尾归一化之后仍相同），五个文件与冻结原型**同 blob SHA**——`ir d0ec8b8f…` / `sse 784a763c…` / `anthropic 7765d6cd…` / `openai 0d3c1a8f…` / `responses 4fed7b0e…`。**编解码逻辑一个字符都没改**，只动了 `lib.rs` 的模块声明/文档与两个门面文件。

- `lib.rs`：写清模块划分、每个编解码文件的两侧归属表，以及「core 不依赖 Tauri」（ADR-0007 第 1 条）这条约束；`pub mod` 收 `anthropic` / `ir` / `openai` / `responses` / `sse`。
- `ingress/mod.rs` / `egress/mod.rs`：**只是门面**（按符号归类 re-export），不装逻辑，不复制代码。
- 未改任何 Cargo 依赖，`Cargo.lock` 无变化。

### 落点取舍与理由

1. **三个编解码文件各自保持完整**（ADR-0007 第 2 条优先于票面表格的字面行）：每个文件同时含入站与出站，硬塞进单侧会让另一侧反向依赖。
2. **门面按「客户端面 / 上游面」分组**，与 spec §3 对两层的定义一致：`ingress` = 客户端请求解码 + 回给客户端的帧编码；`egress` = IR → 上游请求 + 上游 SSE → IR。已写进两个文件的模块文档与 `lib.rs` 的表。
3. **与票面表格的一处出入**：票面写 `responses.rs → ingress::responses + egress 侧编码器`，但原型 `responses.rs` 里**没有**上游 Responses 请求编码器——那里的「出站编码」是把帧写回 Codex 客户端（`ResponsesEncoder` / `ResponsesAssembler`），按 spec §3 属 ingress。所以 `egress` 不从 `responses` re-export 任何符号；上游 Responses wire 未做（spec §3 的 `OpenAiResponsesCodec` 是 M2/M3 的事）。
4. 两个 `decode_request`（anthropic / responses）同名，门面里按协议加后缀区分；**各编解码文件里的原型名字一个没动**（`anthropic::decode_request` 仍是原名）。

### `ingress` / `egress` re-export 清单（票 41 的接口）

`gateway_core::ingress`：

| 门面符号 | 来自 |
| --- | --- |
| `decode_anthropic_request` | `crate::anthropic::decode_request` |
| `AnthropicEncoder` / `AnthropicAssembler` | `crate::anthropic`（IR 事件 → 客户端 wire，流式 / 非流式） |
| `decode_responses_request` | `crate::responses::decode_request` |
| `ResponsesEncoder` / `ResponsesAssembler` | `crate::responses`（同上，Responses 形状） |

`gateway_core::egress`：

| 门面符号 | 来自 |
| --- | --- |
| `encode_anthropic_request` | `crate::anthropic::encode_request` |
| `encode_count_request` | `crate::anthropic`（count_tokens 降级路径的上游请求体） |
| `AnthropicDecoder` | `crate::anthropic`（上游 Anthropic SSE → IR 事件） |
| `encode_openai_chat_request` | `crate::openai::encode_request` |
| `OpenAiDecoder` | `crate::openai`（上游 Chat SSE → IR 事件） |
| （无） | `crate::responses` 无出站面，见上 |

两个同名 `encode_request` / 两个同名 `decode_request` 只在门面里改名，原型符号在各编解码文件里保持原名，票 41 也可直接走 `gateway_core::anthropic::encode_request` 这类全路径。

### 验收 1：`check.ps1 -Quick` 绿，且单测数与原型持平

```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/Claude Gateway/target'
pwsh -File ./check.ps1 -Quick
=== fmt    (cargo fmt --all --check) ===                无输出 → 通过
=== clippy (cargo clippy --workspace -D warnings) ===   无警告 → 通过
=== unit   (cargo test --workspace) ===                 test result: ok. 20 passed; 0 failed
全部通过。
```

逐文件 `#[test]` 数对照（原型 vs 搬后实跑，来自上面 20 个用例的逐条列表）：

| 文件 | 原型 | 搬后 | 用例 |
| --- | --- | --- | --- |
| `ir.rs` | 4 | 4 | ascii_is_four_chars_per_token / cjk_is_one_token_per_char / system_and_tools_count_but_base64_does_not / never_zero |
| `sse.rs` | 5 | 5 | splits_frames_and_keeps_order / single_byte_chunks_are_buffered_until_the_frame_ends / crlf_terminator_is_understood / comment_frames_do_not_swallow_the_next_frame / unfinished_tail_is_still_parsed_on_finish |
| `anthropic.rs` | 5 | 5 | assembles_text_and_usage / thinking_without_signature_is_dropped / tool_input_is_parsed_from_partial_json / upstream_error_and_missing_eos_are_visible / count_request_drops_max_tokens_and_stream |
| `responses.rs` | 6 | 6 | decodes_codex_request_into_ir / stream_ends_with_response_completed / freeform_tool_call_comes_back_as_custom_tool_call / plain_tool_call_stays_function_call / assembler_matches_non_streaming_contract / assembler_reports_incomplete_stream |
| `openai.rs` | 0 | 0 | 原型就没有 `#[test]`（由回放集成测试覆盖） |
| **合计** | **20** | **20** | |

### 验收 2：与原型逐字节行为一致

临时对照脚本 `.parity40/`（两个 bin 共用一个 `drive.rs` / `dump.rs`）：`proto` 直接 `#[path]` 引**冻结**原型源文件（只读），`sol` 用编好的 `gateway-core`。**跑完即删，未入库**（`git status` 里没有它；也没改仓库 `.gitignore`）。

口径：`fixtures/` 下 **15 份**输入 fixture（`fixtures/<场景>/<wire>.sse`，跳过 `expected/` 黄金目录）→ 逐字节喂 `SseParser`（强制走跨 chunk 半帧缓存那条分支）→ 按 wire 选 `AnthropicDecoder` / `OpenAiDecoder` → 把「帧序列 + IR 事件序列 + 两个客户端流式编码器输出 + 两个非流式组装器输出」全打成文本，两边 diff。

```
cargo run -q --bin proto -- <fixtures> > proto.txt   # exit 0
cargo run -q --bin sol   -- <fixtures> > sol.txt     # exit 0
diff proto.txt sol.txt                                # 无输出 → IDENTICAL(full)
sha256: 7844d5e8f444d3e5f2de4335bf874cc829fcb3575c1eccfdc97e969a182f49f4（两边同值，各 3392 行）
IR 事件段单独比对：394 行 vs 394 行，cmp 逐字节相同
```

每份 fixture 的事件数（两边同值，非空转的佐证）：`abort/anthropic` 3、`abort/openai` 2、`hardcut/anthropic` 3、`hardcut/openai` 2、`keepalive/anthropic` 297、`text/anthropic` 9、`text/openai` 7、`thinking/anthropic` 12、`thinking/openai` 10、`toolresult/anthropic` 8、`toolresult/openai` 7、`tools/anthropic` 11、`tools/openai` 10、`usage/anthropic` 7、`usage/openai` 6。

唯一需要抹平的是 Responses 编码帧里的墙上时钟 `created_at`（跨进程按秒变），dump 里置 0 后再比。

**反验**（证明对照不是空转）：把克隆源 `sse.rs` 的 `parse_frame` 改坏一行（`data.push(...)` → `data.push("MUT")`）编成第三个 bin，diff 立刻非空（3392 行 vs 2664 行）。

### 遗留与坑

1. **坑（差点交出假验收）**：第一版对照脚本用 `label.ends_with(".anthropic.sse")` 挑解码器，而 label 的分隔符是 `/`，于是**每一份** fixture 都走了 OpenAI 解码器；两边输出照样「完全相同」，验收看起来是绿的但根本没碰 `AnthropicDecoder`。改 `anchor` 判定后 anthropic 侧才出现 3~297 个事件。教训写在这里：两边一致必须配上「反验会红 + 事件数非零」才可信。
2. `openai.rs` 没有入站面（OpenAI Chat 入站），`responses.rs` 没有上游出站面——原型就没有，归 M3 / 后续票。
3. 回放集成测试（`tests/replay.rs` / `fixtures/` / `mock.rs` / `replay.rs` / `provider.rs` / `upstream.rs`）是票 41 的范围，本票只搬五个编解码文件；`check.ps1 -Replay` 在票 41 之前仍是占位跳过。
4. 本票产出的是 `pub` 符号，`gateway-core` 里暂无调用方（`src-tauri` 不用它），因此 `cargo clippy --workspace -D warnings` 依赖「lib 里 `pub` 不算 dead_code」这条规则；M2 接上转发后自然有人调。

