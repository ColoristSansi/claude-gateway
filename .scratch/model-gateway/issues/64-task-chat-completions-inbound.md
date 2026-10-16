# OpenAI Chat 入站与入站错误信封收口

Type: task
Status: open
Blocked by: —

## Question

实现期收口后的代码评审（`cg-notes/code-review.md`）指出两条**阻断**之一：spec §1 目标 3 / §3 / §4.3 要求的 `POST /v1/chat/completions` **整条不存在**（全路由枚举里没有它，`openai.rs` 只有出站编解码，`lib.rs` 自述「OpenAI Chat 入站未做」；票 40 与票 57 的 Answer 都自认缺，但 39–63 的 M1–M4 里没有哪张票排过它）。打到该路径的请求只会落到空体 404。

本票补齐它，并把评审列出的**入站面错误信封**问题一并收口（它们同属一条链路）。

依据：spec §1 目标 3、§3、**§4.3**、§5、§10「错误表达」；`docs/adr/0001-self-built-thin-conversion-layer.md`；`cg-notes/code-review.md` 的「阻断 1」与三条「中」。

## 交付

- **`POST /v1/chat/completions` 入站**：解码 OpenAI Chat 请求 → IR（`messages` / `tools` / `tool_choice` / 单串 system → IR 的 system 块 / `max_tokens` / `temperature` / `stream` / `response_format` 走票 57 已就位的 `ResponseFormat::from_chat_request`），复用既有 `ingress::proxy` 的 `forward_ir` 链路，不另起一条转发路径。
- 流式 `data:` 帧 + `[DONE]`；非流式把同一批帧缓冲到终态再组装（与 §4.1 同范式）。
- **DeepSeek 的 usage 附在最后一个内容块、不单独发 chunk**（spec §4.3）。
- **错误信封按入站协议给形状**（spec §4.2 / §4.4）：OpenAI Chat 要 `{"error":{"message","type","code"}}`。
- **入站解析失败也要给信封**：语法错的 JSON / 缺 `content-type` 现在落到 axum 默认的 text/plain 400/415/422（评审「中」第一条）——按入站家族分流成上一条那三种形状。Ollama 面已用 `Bytes` 手解且是对的，照它的做法。
- **`/v1/*` 未匹配路径**也按入站家族给信封（评审「中」第三条）；空体 404 只留给真正无协议的路径。
- **`ingress::router` 的 fallback 面**：管理面以后谁加 fallback 会让 `Router::merge` 在启动时 panic（评审「中」第五条）——把两张脸的兜底合成一个，或加一条单测钉死「管理面无 fallback」。
- 顺手两条低：`lib.rs` 文档里关于 `mock` 特性的描述与实际不符，改准确；`proxy.rs` 每请求 `println!` 打 UA / `anthropic-version` / `anthropic-beta` 降到 debug 或只在 body 日志窗口开着时打。

## 收口验收

- 单测 / 回放：Chat 入站**流式与非流式各一条**（黄金帧）、**工具往返一条**、**错误信封三条**（三家入站各一）、**usage 附在最后内容块一条**。
- `pwsh -File ./check.ps1 -Quick -Replay` 全绿（含前端类型检查步）；票 41 / 51 / 56 的回放格**一格不许红**。
- 可选：`-Live` 打一条真实上游的 Chat 入站。
- 前端「模型与映射」页的协议列**不需要**新增取值（入站协议与出站 wire 是两回事）——若你认为界面要能看出「这个虚拟模型能用哪几种入站」，写进 Answer 的建议里，不要在本票顺手改界面。

## 不做

- **上游 `openai-responses` 出站**（评审的另一条阻断）：那是出站面，本票不碰（已在 `Wire::implemented()` 里标为「本版未实现」并挡住误配）。
- **不改 spec / ADR**：spec 与实现的口径差（6 个 Desktop 接管键、`request_frames` 表、`cache_write` 缺价口径、quirks 示例表）由编排者带用户决定，本票不写进 spec。
