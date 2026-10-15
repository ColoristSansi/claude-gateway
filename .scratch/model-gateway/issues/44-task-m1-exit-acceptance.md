# M1 收口：地基验收

Type: task
Status: open
Blocked by: 41, 42, 43

## Question

把前四张票接起来走 M1 收口验收（spec §12 的 M1 那两条）。这是**主会话 + 用户在场**的验收动作，不是可甩给 worktree 子代理的实现任务。

依据：spec §12 M1、[票 38](../issues/38-grilling-implementation-order.md)。

## 交付

- 网关的入站路由与存储接进 Tauri 生命周期（不是两个进程）。
- 应用数据目录 `%APPDATA%\com.claudegateway.desktop\` 首次启动自建，库落在那儿。
- 一份 M1 验收记录（贴进本票 Answer）：`check.ps1` 输出、`/api/status` 实样、托盘与关窗行为的观察。

## 收口验收

- `check.ps1` 全绿，且**单元测试与回放格数不减**（对照原型：单测 ≥ 29、回放 12 格）。
- 应用能启动、托盘常驻、窗口能开关、关窗按 §10 隐藏到托盘。
- 库与密钥能读写（界面上做一次接入即可，或 API 实测）。
- 端口被占时报错清楚。
- **用户点一次托盘与窗口并签字。** 此段结束仍不能日常用（还不能配客户端）。
