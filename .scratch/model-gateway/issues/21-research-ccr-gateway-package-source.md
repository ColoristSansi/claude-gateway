# ccr 的转换层到底是不是黑盒

Type: research
Status: resolved
Blocked by: —

## Question

claude-code-router 的协议转换主体在外部 npm 包 `@the-next-ai/ai-gateway` 里。这个包有没有公开源码？（用户要求补做这项核实）

## Answer

**有源码，公开、MIT、版本与 npm 发布对齐。**「黑盒」这条判断作废。

**起点澄清（用户 2026-10-14 指出）**：被说成「黑盒」的是 **claude-code-router** 的协议转换主体——它不在 ccr 自己的仓库里（[musistudio/claude-code-router](https://github.com/musistudio/claude-code-router)，已克隆到 `vendor/claude-code-router`，remote 已核对），而由 ccr 启动的外部包 `@the-next-ai/ai-gateway` 承担：`packages/core/src/gateway/internal/shared.ts:309` 写着 `gatewayPackageCandidates = ["@the-next-ai/ai-gateway", "gateway"]`，由 `packages/core/src/gateway/core-runtime/supervisor.ts` 解析加载。所以「黑盒」说的是「转换代码不在 ccr 仓库里」，而不是「ai-gateway 这个包没有源码」——后者恰恰相反。

- 仓库：<https://github.com/The-NeXT-AI/ai-gateway>（已克隆到 `vendor/ai-gateway`），`LICENSE` = MIT, Copyright (c) 2026 musistudio。
- 对齐证据：npm 的 `gitHead = 3fd93f8cf519f208a361876a061429b319b11017`，克隆历史里存在提交 `3fd93f8 chore(release): bump version to 1.0.21`；本地 `package.json` 为 `@the-next-ai/ai-gateway` / `1.0.21` / MIT。
- 误判根源：该包 `package.json` **没有 `repository` 字段**，npm 元数据里也没有 homepage——上一轮据此判成黑盒。发布包本身（238 条：110 个 `.d.ts`、3 个 `.js`、112 个 `.map`、**0 个 `.ts`**）确实只有编译产物，源码只在 GitHub。
- 规模：175 个 TS 文件 / 118,371 行，`src/gateway` 一个目录就 57,457 行。
- 能力缺口：`count_tokens` 全仓 0 处；`cache_control` 仅 1 处（`target/anthropic-messages.ts:624`）→ 六项保真里的 prompt caching 基本没有；无 Ollama 入站；无四个家族槽位（那在 ccr 客户端侧，且用的是完整 ID 而非裸家族名）。
- 形态：Node/Fastify 服务器（Redis、Docker Compose、计费、MCP 网关、插件系统、多实例），不是桌面单进程。

细节与移植清单：[ccr-gateway-package.md](../research/ccr-gateway-package.md)。

**决策影响**：[ADR-0001](../../../docs/adr/0001-self-built-thin-conversion-layer.md) 已修订出局理由（黑盒 → 形态与保真缺口），结论不变；该包升级为协议转换层的**一手移植参照**（MIT，可抄代码并保留版权声明）。是否重新评估「直接基于它二次封装」另开票据 [22](22-grilling-reconsider-ccr-gateway-core.md)。

**流程教训**：判断 npm 包是否黑盒，先看 `gitHead` 再去搜同名 GitHub 仓库，别只看 `repository` 字段。

## Comments

- 2026-10-14：本条为补做核实，纠正 [05](05-research-build-vs-wrap.md) 答案里的「黑盒」判断。
