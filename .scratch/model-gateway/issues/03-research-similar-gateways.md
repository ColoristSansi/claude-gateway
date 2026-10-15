# 同类网关（cc-switch / CodexPlusPlus）怎么做的

Type: research
Status: resolved
Blocked by: —

## Question

cc-switch 与 CodexPlusPlus 如何管理客户端配置、如何处理协议转换、如何绕开 Claude Desktop 的模型 ID 限制？哪些做法可以直接借鉴？

## Answer

调研报告：[similar-gateways.md](../research/similar-gateways.md)；仓库已克隆到 `vendor/cc-switch`、`vendor/CodexPlusPlus`。

- **cc-switch**：React + TS / Tauri 2 / Rust，**MIT，可抄代码**（保留版权声明）。覆盖 10 个客户端。v4 起不再整文件重建：`src-tauri/src/live/` 只改「关键字段」（地址/凭据/模型名/协议），首写备份一次，乐观并发（读-比对-rename 有窗口）。
- cc-switch 的 Claude Desktop 做法：写 `Claude-3p/configLibrary/<uuid>.json` + `_meta.json`（entries/appliedId），用 `inferenceProvider:"gateway"`。
- **「只能填 Anthropic 模型 ID」的两种对策**：直连模式只允许 `claude-{sonnet,opus,haiku,fable}-*`；第三方模型走「模型映射」——Desktop 只看到安全 route id，真实模型名永不写进 profile，显示名由 `labelOverride` 负责。
- 可抄细节：非安全 route id 自动改写成合法角色名且不撞车；`[1M]` 后缀转成 `supports1m`（Desktop schema 不收后缀）；网关 token 独立 Bearer 校验 + 独立 provider namespace；切回官方只清字段不删文件。
- cc-switch 协议转换全自研：按方向成对 transform + streaming，覆盖 anthropic 透传 / openai_chat / openai_responses / gemini_native。踩坑：混合 LF/CRLF 的 SSE 会丢工具调用；Chat→Anthropic 缺 `finish_reason` 时最后一块不关导致丢内容。
- 状态型信息传递：Anthropic 没有 reasoning 字段，cc-switch 用带版本前缀的 thinking signature 载荷（`ccswitch-openai-reasoning-v1:`）携带，还原侧白名单永久保留。
- **CodexPlusPlus**：Rust workspace + Tauri，**AGPL-3.0-only**，只能借思路不能抄代码。改 `~/.codex/config.toml`（`toml_edit` 保序）+ `auth.json`；`wire_api` 硬编码恒为 `responses`；切换是事务（设置字节 + live 文件双快照，失败双恢复）。
- CodexPlusPlus 的可借思路：工具调用的 `extra_content`（如 Gemini 的 `thought_signature`）**不硬编码字段名**，按 `call_id` 缓存整个 `extra_content`（LRU 256）下轮回传；`model_catalog_json` + `slug[1M]` 后缀路线；删除操作要有 `ensure_safe_recursive_removal` 守卫。
