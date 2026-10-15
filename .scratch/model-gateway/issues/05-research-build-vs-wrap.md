# 自研还是基于现成网关二次封装

Type: research
Status: resolved
Blocked by: —

## Question

LiteLLM、new-api、one-api、claude-code-router、AIClient-2-API 里有没有适合当核心的？还是自研薄转换层？

## Answer

调研报告：[build-vs-wrap.md](../research/build-vs-wrap.md)（753 行，逐条判定 + 许可分析 + MVP 路径）；五个候选已克隆到 `vendor/`。

**结论：自研薄转换层，不基于任何候选二次封装。**

| 候选 | 许可 | 致命问题 |
| --- | --- | --- |
| LiteLLM | MIT + `enterprise/` 商业 | 形态错配：Python + 39 项代理依赖 + Prisma，安装包 300–600MB；`proxy` extra 钉了 proprietary 的 `litellm-enterprise` |
| new-api | **AGPL-3.0 + 附加署名条款** | 技术形态最优（单 Go 二进制 + 内嵌前端 + 托盘 + 路径映射），但闭源分发不通；依赖的 `go-epay` 也是 proprietary |
| one-api | MIT | 停滞 20 个月，能力是 new-api 真子集，且硬覆盖 `anthropic-beta` |
| claude-code-router | MIT | 客户端半边最好；转换主体在外部包 `@the-next-ai/ai-gateway`——**此判断已被 [21](21-research-ccr-gateway-package-source.md) 更正：该包有公开 MIT 源码，不是黑盒**；修订后的出局理由是形态与保真缺口 |
| AIClient-2-API | **GPL-3.0** | 一票否决 |

三个改变下一步动作的发现：

1. **DeepSeek 的 `reasoning_content` 补齐逻辑，唯一现成正确答案在 LiteLLM**（`llms/deepseek/chat/transformation.py` 的 `_fill_reasoning_content`，docstring 直接引上游报错原文）；keep-alive 容忍在 `json_fragment_accumulator.py` 与 `sse_keepalive.py`。**照规格重写，不要复制**——LiteLLM 自己警告：缺失时注入占位会静默降低多轮质量。
2. **路径契约有现成范式**：new-api 的 `advancedcustom` 通道做了 `IncomingPath → UpstreamPath` 的声明式映射，commandcode 的 `/provider/v1/messages` 与 opencode 的 family 前缀都能变成配置，不用 if-else。→ 本设计第 6 节的声明式供应商表照这个形状。
3. new-api 把 `/v1/messages/count_tokens` 主动注释掉了（路由里的 TODO，controller 实现还在）→ 走它必须自己放回。

**务实组合**：配置写入抄 cc-switch(MIT)；`reasoning_content` 那层照 LiteLLM 的规格重写；其余（IR、四个编解码、声明式供应商表）自写，编解码参考 ai-gateway(MIT)。

未核实：~~未克隆 `@the-next-ai/ai-gateway`~~ → **已由 [21](21-research-ccr-gateway-package-source.md) 补齐：该包有公开 MIT 源码并与 npm 1.0.21 对齐**；未实机抓上游 SSE 帧（原型阶段补）。

## Comments

- 2026-10-14（补做）：用户要求核实「转换协议是黑盒」这一条。结论是该包有公开 MIT 源码（`github.com/The-NeXT-AI/ai-gateway`，`gitHead` 与 1.0.21 版本对齐），误判根源是它的 `package.json` 没有 `repository` 字段。详见 [21](21-research-ccr-gateway-package-source.md) 与 [research/ccr-gateway-package.md](../research/ccr-gateway-package.md)。本票据的最终结论（自研薄转换层）不变，但 ccr/ai-gateway 从「黑盒」升级为「一手移植参照」；是否重开二次封装的决定见 [22](22-grilling-reconsider-ccr-gateway-core.md)。
- 2026-10-14（范围调整）：订阅制渠道的账号接入已整体移出本项目，本票据里与账号池相关的候选评价（`provider-pool-manager.js` 那段）随之删除；候选的许可与形态结论不受影响。
