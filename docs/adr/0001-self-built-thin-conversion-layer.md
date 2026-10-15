# 自研薄转换层，不基于现成网关二次封装

本机单用户的桌面形态，要求协议转换与客户端配置写入在同一个进程里，并打成小安装包。评估的五个候选全部出局：LiteLLM 是 Python 且代理依赖 39 项、安装包 300–600MB；new-api 技术形态最合，但是 AGPL-3.0 加附加署名条款，闭源分发不通；one-api 停滞 20 个月且能力是 new-api 的子集；claude-code-router（`musistudio/claude-code-router`，MIT）的仓库本身不含协议转换主体——它由 ccr 启动的外部包 `@the-next-ai/ai-gateway` 承担，当初的「黑盒」指的就是这一点；而该外部包**有公开 MIT 源码**（`The-NeXT-AI/ai-gateway`，与 npm 1.0.21 版本对齐），所以「黑盒」不成立，真正的出局理由是形态与保真缺口——它是 Node/Fastify 服务器（Redis、Docker、计费、MCP、插件系统，175 个 TS 文件 11.8 万行），装进 Tauri 应用要额外带一个 Node sidecar，且不实现 `count_tokens`、几乎没有 prompt caching；AIClient-2-API 是 GPL-3.0。因此自研薄转换层，只借鉴许可友好的做法：配置写入抄 cc-switch(MIT)，协议编解码与 DeepSeek 的 reasoning_content 补齐照 ai-gateway(MIT) 与 LiteLLM 的规格重写。

**Status**: accepted（2026-10-14 修订过理由：核实 `@the-next-ai/ai-gateway` 有公开 MIT 源码后，「黑盒」这条出局理由作废，结论不变，理由改为形态与保真缺口。是否重新评估基于它二次封装见 `.scratch/model-gateway/issues/22-grilling-reconsider-ccr-gateway-core.md`。）

**Considered Options**: 上述五个候选，逐条判定、许可分析与 MVP 路径见 `.scratch/model-gateway/research/build-vs-wrap.md`。
