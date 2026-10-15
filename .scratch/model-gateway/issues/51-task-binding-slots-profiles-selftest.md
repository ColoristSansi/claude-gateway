# 绑定层：家族槽位、虚拟模型、客户端档案覆盖与自检

Type: task
Status: open
Blocked by: 49

## Question

做「入站模型名 → (上游, 真实模型, 出站协议)」的绑定与解析层：四个家族槽位、普通虚拟模型、每客户端覆盖、请求期早拒、以及非流式自检探针。

依据：spec §3 / §7、[票 10](../issues/10-design-model-exposure-and-client-config.md)、[票 23](../issues/23-design-provider-table-granularity.md)、[票 26](../issues/26-grilling-count-tokens-and-non-streaming.md)、[票 30](../issues/30-task-build-regression-suite.md)（`model` 规则）。

## 交付

- 四个家族槽位（`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`）+ 普通虚拟模型；绑定只能从上游模型目录里选（目录没有的模型可手工新增一条），再在 `supported_wires` 里选 wire。
- **每客户端可覆盖**（全局 / Claude Desktop / Claude Code / Codex 四套）：同一槽位按客户端走不同上游；`client_profiles` 表 + 解析时按入站 `user-agent` / 客户端标识归属。
- 请求期解析：模型名**去首尾空白 + 大小写敏感**；未命中 → 404 `not_found_error`；wire 未声明却硬用 → 400 早拒。
- 自检探针：非流式 `/v1/messages` + `max_tokens:1`，只看状态码；探针档（`max_tokens <= 16`）上游上限 **20 秒** → 超时 504。
- `/v1/models` 列出网关暴露的模型（无需按客户端过滤）。

## 收口验收

- 单测：四边界模型名（空白 / 大小写 / 未知 / 槽位名）、覆盖解析（同一槽位两客户端不同上游）、wire 未声明早拒 400。
- 探针在两条真实上游各跑一次并记录结果（含 commandcode 的 Chat wire + `max_tokens:1` 挂住那条已知形状）。
