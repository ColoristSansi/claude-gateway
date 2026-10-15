# 桌面形态用 Tauri 2 + Rust 核心 + React 前端

网关要常驻托盘、安装包要小、请求路径上要做 SSE 逐帧转发与协议转换。选 Tauri 2 外壳、Rust 后端（tokio + axum）、React 前端，与同类项目 cc-switch、CodexPlusPlus 同构，已有先例可参照。Electron 开发更省事但安装包与内存占用最大；Tauri 加 Node sidecar 在体积上也不占优。

**Consequences**: 需要浏览器自动化（内嵌 Chromium、CDP）的能力本项目不做；将来若真要做，得在 Rust 侧接 CDP 或另起一个 sidecar——这是这条选择的主要代价。
