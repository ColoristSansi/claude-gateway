# Ollama 兼容面

Type: task
Status: open
Blocked by: 51

## Question

按 spec §4.4 落 Ollama 兼容面：`/api/chat`、`/api/tags`、`/api/version`、`/api/ps`、`/api/show`，外加「装载 / 卸载」特权语义。与 Anthropic / OpenAI 三个入站**共用同一条 IR 与出站链路**，只有解码器与错误封装不同。

依据：spec §4.4、[票 34](../issues/34-research-ollama-inbound.md)、[票 36](../issues/36-grilling-ollama-endpoint-scope.md)、`research/ollama-aux-endpoints.md`。

## 交付

- `/api/chat`：**NDJSON 一行一帧**（不是 SSE），末帧 `done:true` 才是终态；`message.content` 是字符串、`tool_calls[].arguments` 是**对象** → 工具调用缓冲到 block stop；回传只有 `tool_name` → 按名反查补 `tool_use_id`（否则 Anthropic 出站 400）；错误体 `{"error":"…"}`。
- `/api/tags`：与 `/v1/models` 同一集合，`name` / `model` 都填**裸虚拟模型名**、`digest` 用名字的 sha256、不谎报 gguf；入站解析接受并剥掉 `:latest`。
- `/api/version`：回真实版本串 + 顺带实现 `HEAD`；`/api/ps` 回 `{"models":[]}`（空数组非 `null`）。
- `/api/show`：只填能诚实填的键——必给 `details` 六子键 + `model_info:{}` + `modified_at`；`modelfile` / `parameters` / `template` / `license` / `thinking` 整键省略。
- 装载 / 卸载（空 `messages`；`keep_alive:0`）**本地回假终态不打上游**；`messages` 非空时 `keep_alive` 忽略。
- 未知名统一 404 `model '<name>' not found`；其余 `/api/*`（`pull`/`create`/`copy`/`delete`/`blobs/*`/`generate`/`embed`/`embeddings`）统一 404。

## 收口验收

- 回放 / 单测：五个端点 + 两处怪癖（NDJSON 终态、按名反查补 id）+ 装载卸载 + 错误信封。
- `ollama run` 或 curl 实测一条问答走通（本机没有 Ollama 客户端时用 curl 记录）。
