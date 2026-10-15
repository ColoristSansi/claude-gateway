# 归档：已移出路线的调研

`2026-10-14` 用户决定把「订阅制渠道的账号池 / 凭据接入」**整体移出本项目**，相关票据（06、07、12、20）已删除，地图、设计文档、术语表与交接单里不再出现。

这两份报告留档备查，**不在路线内**，不要据此重新提议该功能：

- [account-pool.md](account-pool.md)：六家参考项目怎么做多账号凭据池、轮换、冷却与恢复判定。
- [relay-credentials.md](relay-credentials.md)：Qoder / 腾讯 WorkBuddy / TRAE / Kiro / Gemini CLI 等十几个订阅服务的登录态形态、落盘位置、刷新端点、指纹校验与合规红线。

若将来重新考虑，注意这两份报告里已取证的两条硬事实：Anthropic 官方页面明文禁止代用户转发 Free/Pro/Max 订阅凭据、禁止收集或中转 Claude.ai 凭据；WorkBuddy 新版凭据的密钥只在运行中客户端的内存里且官方无续期接口。

参考仓库仍留在 `../vendor/`（zen-gate、deepseek-harness-codearts、Astra-Gate 等），它们贡献的**协议转换**教训仍在路线内（见 [../research/reverse-proxies.md](../reverse-proxies.md) 与票据 04）。
