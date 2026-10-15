# `@the-next-ai/ai-gateway`（ccr 的转换层）有没有源码

调研日期：2026-10-14
触发：用户要求核实 claude-code-router 的协议转换层是否真的是黑盒。

## 结论

**有源码，公开、MIT、版本与 npm 发布对齐。** [build-vs-wrap](build-vs-wrap.md) 里「转换主体在外部 npm 包 → 黑盒」这条判断**作废**。

## 起点澄清

被说成「黑盒」的是 **claude-code-router** 的协议转换主体，不是 `@the-next-ai/ai-gateway` 这个包：

- `claude-code-router`（[musistudio/claude-code-router](https://github.com/musistudio/claude-code-router)，MIT）已克隆到 `vendor/claude-code-router`（remote 已核对，HEAD `057abaa`）。它的仓库里有客户端与编排，**但没有协议编解码**。
- 转换由它启动的外部包承担：`packages/core/src/gateway/internal/shared.ts:309` 的 `gatewayPackageCandidates = ["@the-next-ai/ai-gateway", "gateway"]`，由 `packages/core/src/gateway/core-runtime/supervisor.ts`（250/312/319 行）解析加载。
- 那个外部包有公开 MIT 源码：[The-NeXT-AI/ai-gateway](https://github.com/The-NeXT-AI/ai-gateway)，已克隆到 `vendor/ai-gateway`（remote 已核对，HEAD `14a0c09`）。

所以准确的说法是：**转换代码不在 ccr 仓库里（这一点属实），但它在公开的 MIT 仓库里（「黑盒」不成立）**。

## 证据链

| 环节 | 事实 |
| --- | --- |
| npm 元数据 | `@the-next-ai/ai-gateway`，latest `1.0.21`（发布 2026-09-08），**license MIT**，维护者 `musistudio <me@jinhui.li>`（与 claude-code-router 同一人），描述 "TypeScript Fastify AI protocol gateway for OpenAI, Anthropic, Gemini, MCP and agent workflows" |
| 关键线索 | npm 元数据里的 `gitHead = 3fd93f8cf519f208a361876a061429b319b11017` |
| **`repository` 字段为空** | npm 元数据没有 `repository`／`homepage`——这正是上一轮误判成「黑盒」的根源 |
| 源码仓库 | <https://github.com/The-NeXT-AI/ai-gateway>，已克隆到 `vendor/ai-gateway`；`LICENSE` = MIT License, Copyright (c) 2026 musistudio |
| 版本对齐 | 克隆历史里存在提交 `3fd93f8 chore(release): bump version to 1.0.21`，与 npm `gitHead` 完全一致；本地 `package.json` 的 `name`/`version`/`license` = `@the-next-ai/ai-gateway` / `1.0.21` / `MIT` |
| 发布包内容 | tarball 238 条：**0 个 `.ts` 源文件**、110 个 `.d.ts`、3 个 `.js`（打包产物，`main: dist/index.js`）、112 个 `.map`。源码只在 GitHub，不在 npm 包里 |

## 规模与结构（175 个 TS 文件 / 118,371 行）

| 目录 | 文件 | 行数 | 内容 |
| --- | --- | --- | --- |
| `src/gateway` | 54 | 57,457 | 主处理 `handler.ts`（319KB）、`openai-json.ts`（116KB）、`idempotency.ts`、`precheck.ts`、`provider-health*.ts`、`health-routing.ts`、`codex-websocket-conversion.ts`、`routes.ts`、`auth.ts` |
| `src/adapters` | 34 | 16,783 | 协议编解码，见下 |
| `src/agent` | 31 | 15,952 | agent 编排 |
| `src/provider` | 7 | 4,202 | `deepseek-thinking.ts`、`external.ts`、`plugins.ts`、`webhook.ts` |
| `src/plugins` | 8 | 3,834 | 插件系统 |
| `src/mcp-gateway` | 8 | 2,068 | MCP 网关 |
| `src/billing` | 5 | 1,577 | 计费 |
| `src/upstream` | 2 | 1,796 | `client.ts` |
| `src/manager` | 3 | 1,947 | 管理面 |

### 协议编解码（我们最关心的部分）

- 入站方向 `src/adapters/builtins/source/`：`anthropic-messages.ts`、`openai-chat-completions.ts`、`openai-responses.ts`、`gemini-generate-content.ts`、`gemini-stream-generate-content.ts`、`parsers.ts`、`formatters.ts`
- 出站方向 `src/adapters/builtins/target/`：`anthropic-messages.ts`、`openai-responses.ts`、`gemini-generate-content.ts`、**`openai-chat-compat/`（`deepseek.ts`、`zhipu.ts`、`minimax.ts`、`xiaomi-mimo.ts`）**、`tools.ts`、`shared.ts`
- 横切：`adapters/builtins/reasoning-envelope.ts`（思考内容跨协议封装——正是我们「thinking signature 版本化信封」那个问题）、`image-input.ts`、`common.ts`、`adapters/registry.ts`

## 能力缺口（对照 [spec 第 4、9 节](../spec.md) 与票据 [11](../issues/11-design-stack-storage-and-observability.md) 的六项保真清单）

- **`count_tokens` 全仓 0 处**（`src` 递归 grep，含路径字符串）→ Claude Code 会退化为字符估算，我们要自己实现。
- **`cache_control` 只有 1 处**（`src/adapters/builtins/target/anthropic-messages.ts:624`，疑似透传白名单）→ 六项保真里的 **prompt caching 基本没有**。
- 无 Ollama 入站。
- 无「四个家族槽位」概念（那在 ccr 客户端侧：`packages/core/src/agents/claude-code/environment.ts` 导出 `ANTHROPIC_DEFAULT_FABLE/OPUS/SONNET/HAIKU_MODEL`；注意 ccr 用的是完整 ID 如 `claude-fable-5`、`claude-opus-4-8`，不是我们要求的裸家族名）。
- 形态是 Node/Fastify 服务器：Redis 共享状态、Docker Compose、计费、MCP 网关、插件系统、多实例部署——**不是桌面单进程**。

## 对决策的影响

- [ADR-0001](../../../docs/adr/0001-self-built-thin-conversion-layer.md) 里针对 ccr 的「黑盒」理由已修订。
- **结论保持不变**，但理由换了：(1) 形态——[ADR-0003](../../../docs/adr/0003-tauri-rust-react-desktop.md) 已定 Rust 核心，用它就要在 Tauri 里塞一个 Node sidecar；(2) 保真缺口——`count_tokens` 与 prompt caching 它没有；(3) 118K 行 TS 的长期同步成本。
- **新角色：协议转换层的一手移植参照**（MIT，可抄代码，保留版权声明与许可文本）。建议的移植清单：`source/openai-responses.ts` 与 `target/openai-responses.ts`（Codex 那条最复杂的入站）、`target/openai-chat-compat/deepseek.ts` 与 `provider/deepseek-thinking.ts`（DeepSeek 的 `reasoning_content` 回传）、`reasoning-envelope.ts`（思考内容封装）、`image-input.ts`、`gateway/codex-websocket-conversion.ts`、`gateway/precheck.ts` 与 `provider-health*.ts`（可用性预检与健康路由）。

## 教训（写进流程）

判断一个 npm 包是不是黑盒，**不能只看 `repository` 字段**：这里它是空的，但 `gitHead` 指向的提交在公开仓库里存在，且版本完全对齐。核实的顺序应是 `gitHead` → 搜索同名 GitHub 组织/仓库 → 克隆比对 `package.json`。
