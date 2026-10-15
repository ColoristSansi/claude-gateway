# 终点、范围与基本形态

Type: grilling
Status: resolved
Blocked by: —

## Question

这个 effort 走完之后用户手里应该拿到什么？覆盖哪些客户端？给谁用？配置与日志存哪里？除了转发还要哪些能力？

## Answer

- **终点**：设计文档 + **最小可运行原型**（用户明确覆盖了 wayfinder 默认的「只出决策」）。
- **客户端**：Claude Desktop（3P）、Claude Code、Codex **三个都要**，按「Claude Code / Codex 先易、Claude Desktop 3P 后难」的顺序设计。
- **使用范围**：仅本机单用户，只监听 127.0.0.1，不做鉴权体系。
- **运行形态**：桌面应用（Tauri 2）+ 系统托盘；本机没有 Docker，不做容器交付。
- **存储**：SQLite 存配置与日志；密钥用系统级加密（Windows DPAPI）；支持导出 / 导入 JSON。
- **要的功能**：用量与成本统计、请求日志与调试回放、从 models.dev 半自动导入供应商配置、从上游 `/v1/models` 自动发现模型。
- **不要的功能**：多上游优先级与**故障转移**（用户明确否决，一个家族槽位只绑一条上游）。
