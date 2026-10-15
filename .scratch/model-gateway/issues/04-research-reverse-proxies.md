# 四个参考程序的协议转换实现

Type: research
Status: resolved
Blocked by: —

## Question

dsh-our-free-model、deepseek-harness-codearts、Astra-Gate、zen-gate 各自怎么实现协议转换与对外仿真？哪些实现细节值得直接照搬？

## Answer

调研报告：[reverse-proxies.md](../research/reverse-proxies.md)；四个仓库已克隆到 `vendor/`。

- **四家共识：协议转换都经中间表示，不做两两直转。** Astra 用显式 IR（`Ir.cs` + 每协议 codec）；zen-gate 用统一 `lane.Chunk` + 三个 wire 解码器；dsh-our-free-model 用纯函数编解码器；codearts 每 provider 家族一个 adapter。
- **对外 Anthropic 入站**：Astra / zen-gate / dsh-our-free-model 三家都有 `/v1/messages`；**deepseek-harness-codearts 没有**，只出 OpenAI Chat + Responses，Claude Code 接不上它。
- 仿真最激进的是 Astra 的 `ClaudeCodeMimicry.cs`：钉死 UA `claude-cli/2.1.281`、整套 X-Stainless 指纹、规范三块 system、`metadata.user_id`；默认关，文件头自陈可能违反条款。配 `ClaudeOAuthHeaders.cs` 区分 Relay（真 CLI，白名单转发 + beta 合并）与 Compat（只给最小 beta）。
- **可直接抄的四条**：① Anthropic 的 `event:` 名必须从顶层 `type` 字段取，不能字符串扫描；② `flushed` 标志决定错误走 HTTP 还是流内 error 事件；③ `tool_use` 块「名字未到不开块，始终无名则不宣布」+「截断压过 tool_use」；④ 上游不可用时的冷却默认短（10 秒），不用长黑名单。
- **重试建模**：zen-gate 把「值得换模型」与「值得重试同模型」分开，且都受 `sawAny`（已吐字就不许切）闸门约束——这条对「流中途失败要不要重试」同样成立。
- **档位归一化**（codearts `messages.ts`）：返回值三态 `string | undefined | null`——`undefined`（没传）与 `null`（传了但本模型不适用）必须分开，混用会被下游 `!== undefined` 判定吃掉修正。
- **配置注入的安全底线**（三家都做接管客户端）：写前备份 + 成对标记注释 + 幂等重写 + 关闭还原 + 跳过用户自己改过的值；Astra 最严（per-keyPath 原值 + 永久首写备份 + 20 滚动 + 离线 restore-all）。
