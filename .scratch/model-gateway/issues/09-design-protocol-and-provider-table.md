# 协议策略与声明式供应商表

Type: grilling
Status: resolved
Blocked by: —

## Question

网关连上游时优先用哪种协议？各家路径与行为差异怎么表达，才不至于变成一堆 if-else？

## Answer

- **上游协议策略（用户原话「1+2」）**：**优先 Anthropic 原生直通，缺失时降级 OpenAI Chat**；并且**部分供应商不同协议支持的模型不同**。
- 因此**协议不记在供应商上，记在虚拟模型上**：每个虚拟模型绑定 `(上游, 真实模型 ID, 出站协议)` 三元组。
- **供应商差异用声明式表表达**，照 new-api `advancedcustom` 的 `IncomingPath → UpstreamPath` 形状：`baseUrl` + `auth.scheme` + `pathTemplates`（按协议）+ `headerPassthrough` + `quirks`（枚举）。
- `quirks` 只允许枚举值；新增一个 quirk 必须同时新增一条回归 fixture。**转换代码里不允许出现供应商名判断。**
- 入站协议范围（用户为「1+3」）：Anthropic Messages、OpenAI Chat Completions、OpenAI Responses **加 Ollama 兼容**；Gemini 原生不做，只在中间表示上预留扩展点。
- models.dev 只当**预设先验**：`npm` 字段可预填协议倾向，但单一 `api` 表达不了「一个供应商两个 base URL / 两种协议」（opencode-go、deepseek 即反例），所以必须允许用户逐字段覆盖。
