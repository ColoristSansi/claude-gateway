# 技术栈、存储与可观测基线

Type: grilling
Status: resolved
Blocked by: 05

## Question

桌面外壳与后端语言选什么？保真能力要哪些？日志默认记多少、留多久？

## Answer

- **技术栈（等 build-vs-wrap 结论后选定）**：**Tauri 2 + Rust 核心 + React 前端 + 系统托盘**。理由：与 cc-switch / CodexPlusPlus 同构；协议转换与流式转发在 Rust 侧，前端纯 UI；安装包小、托盘常驻省内存。
- **保真能力六项全要**（用户全选）：工具调用 / function calling、推理与思考内容、图片与视觉输入、Anthropic prompt caching（`cache_control`）、结构化输出（json_schema / response_format）、使用量字段（token 计数与缓存命中）。
- **日志与隐私**：默认只记元数据（虚拟模型 / 上游 / 状态码 / 耗时 / 首字延迟 / token / 成本）；**完整 body 默认不写、可在界面临时开启**；保留 7 天。
- **密钥**：DPAPI 加密后入库，界面只显示掩码。
- 前端只经 `127.0.0.1:8787/api/*` 与后端通信，`/v1/*`（转发）与 `/api/*`（管理）完全分离；前端不参与转发、不直接读写客户端配置文件。
