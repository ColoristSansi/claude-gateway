# 核心编解码搬迁

Type: task
Status: open
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
