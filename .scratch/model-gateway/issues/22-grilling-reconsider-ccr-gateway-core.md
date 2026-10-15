# 要不要因为源码公开而重新评估 ccr / ai-gateway 的角色

Type: grilling
Status: resolved
Blocked by: —

## Question

[21](21-research-ccr-gateway-package-source.md) 核实了 `@the-next-ai/ai-gateway` 有公开 MIT 源码且与 npm 1.0.21 版本对齐。这让 [ADR-0001](../../../docs/adr/0001-self-built-thin-conversion-layer.md) 里针对 ccr 的「黑盒」出局理由失效。要不要重开「自研 vs 基于它二次封装」这个决策？

## 两边的事实

**支持重开（基于它二次封装）**

- 协议编解码这一层它已经有现成的：入站四个（Anthropic Messages / OpenAI Chat / OpenAI Responses / Gemini）、出站四个，还有 `openai-chat-compat/{deepseek,zhipu,minimax,xiaomi-mimo}` 这类方言适配。
- MIT，可抄可改，版本与发布对齐。
- 附带现成的可用性预检（`precheck.ts`）、健康路由、上游客户端、幂等与并发控制。

**支持维持自研**

- 形态：它是 Node/Fastify 服务器（Redis、Docker Compose、计费、MCP、插件、多实例），装进 Tauri 应用要么带一个 Node sidecar，要么放弃 [ADR-0003](../../../docs/adr/0003-tauri-rust-react-desktop.md) 的 Rust 核心。
- 保真缺口：`count_tokens` 全仓 0 处；`cache_control` 仅 1 处 —— 六项保真里缺两项。
- 118,371 行 TS 的长期同步成本；它的部署面（多实例/Redis）远超本机单用户需要。

## 需要定的

1. 三选一：维持自研（把 ai-gateway 当移植参照）／ 改成 Tauri + Node sidecar 跑它再补缺口 ／ 折中（继续 Rust，但把它的编解码按 MIT 逐文件移植并标注出处）。
2. 若走折中，移植哪些文件、怎么记出处与许可（保留版权声明与许可文本）。
3. [ADR-0001](../../../docs/adr/0001-self-built-thin-conversion-layer.md) 与 [spec 第 2 节](../spec.md) 的最终措辞。

## Answer

**用户选择：保持自研，把 ai-gateway 当一手移植参照（选项 1）。**

- 架构不变：[ADR-0003](../../../docs/adr/0003-tauri-rust-react-desktop.md) 的 Tauri 2 + Rust 核心 + React 继续有效，**不引入 Node sidecar**。
- `@the-next-ai/ai-gateway`（MIT）作为**协议编解码的一手移植参照**：逐文件参考/移植并保留版权声明与 MIT 许可文本。优先移植清单：`adapters/builtins/source/openai-responses.ts` 与 `target/openai-responses.ts`（Codex 那条最复杂的入站）、`target/openai-chat-compat/deepseek.ts` + `provider/deepseek-thinking.ts`（DeepSeek 的 `reasoning_content` 回传）、`adapters/builtins/reasoning-envelope.ts`（思考内容封装）、`image-input.ts`、`gateway/precheck.ts` 与 `provider-health*.ts`（可用性预检）。
- 两个仓库地址已核对并留在 `vendor/`：`musistudio/claude-code-router`（HEAD `057abaa`）、`The-NeXT-AI/ai-gateway`（HEAD `14a0c09`，npm 对齐提交 `3fd93f8`）。
- 命名更正已落到各处：被称作「黑盒」的是 **claude-code-router 的转换主体不在其仓库里**，而非该外部包无源码。

## Comments

- 2026-10-14：用户回答本票据时同时指出命名问题（「黑盒」指的是 claude-code-router，不是 ai-gateway），已据此修正 [ADR-0001](../../../docs/adr/0001-self-built-thin-conversion-layer.md)、[spec 第 2 节](../spec.md)、[05](05-research-build-vs-wrap.md)、[21](21-research-ccr-gateway-package-source.md) 与两份调研报告的措辞。
- 2026-10-14（范围调整）：订阅制渠道的账号接入已整体移出本项目，本票据 Answer 里原先「第二个 effort 的账号池可借」的描述随之删除。
