# M2 收口：三个真实客户端端到端

Type: task
Status: open
Blocked by: 46, 47, 48, 50, 52, 53, 54

## Question

走 spec §12 的 M2 收口验收——这段收口就是「可以开始拿它日常用」那条线。**主会话 + 用户在场**，不是可甩给 worktree 的活。

依据：spec §12 M2、[票 19](../issues/19-grilling-test-strategy.md)、[smoke.md](../smoke.md)。

## 交付

- 冒烟清单更新（三个客户端 + 一键还原 + 安装包，逐项可勾）。
- 一份 M2 验收记录贴进本票 Answer：`check.ps1` 输出、三个客户端各自的 `/api/logs` 证据、安装步骤。

## 收口验收

- **Claude Desktop 3P**：菜单四家族名、流式正常、切回 1P 登录态完好。
- **Claude Code**：零官方登录下走网关完成问答。
- **Codex**：CLI 与桌面应用都能选到网关模型并作答。
- 写入引擎与 [票 30](../issues/30-task-build-regression-suite.md) 第 8 格全绿。
- NSIS 装得上、能跑；一键还原把三个客户端都送回官方态。
- **用户当场三个客户端各问一句并签字。**
