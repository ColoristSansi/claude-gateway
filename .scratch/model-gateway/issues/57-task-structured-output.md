# 结构化输出实现

Type: task
Status: open
Blocked by: 49, 51

## Question

按 [票 37](../issues/37-grilling-structured-output-slot.md) 的十条定案落结构化输出：IR 归一化槽位 + 三条出站映射 + 「上游 × 出站协议」能力位 + 带敌意提示词的自检探针。本票只实现契约，不改判据。

依据：spec §5 / §4.2 / §4.3 / §4.4 / §6、[票 37](../issues/37-grilling-structured-output-slot.md)、`research/structured-output-wires.md`。

## 交付

- IR 槽位 `format: Option<ResponseFormat>` = `Json` / `JsonSchema{schema,name,strict}` / `Other(Value)`（**归一化**，不原样透传）。
- 出站逐字形状：Anthropic `output_config.format`（3 层、无 `name`/`strict`/`json_schema` 包裹、递归补 `additionalProperties:false`）；Chat `response_format` → `json_schema`（4 层）；Responses `text.format`（3 层，`name` 必填）。OpenAI 两条的 `name` 一律填常量 `"response"`，`strict` 只透传客户端显式值。
- 表达不了 → **请求期 400**（不静默丢弃、也不放行加警告）：无 schema 的 JSON mode 打到 Anthropic wire 一律 400。
- 能力位（`native` 缺省 / `ignore` + `source`）挂在「上游 × 出站协议」上；标 `ignore` 时客户端要结构化输出 → 400 并说明。
- 自检探针：带敌意提示词判定能力位——**状态码骗人**（commandcode 的 Anthropic 兼容端点对六个变体全 200 且逐字静默忽略）。

## 收口验收

- 三条出站映射各一条黄金用例（逐字形状，含 `additionalProperties:false` 的递归补法）。
- 400 两条路径（Anthropic + JSON mode；`ignore` 能力位）。
- 探针在两条真实上游实测并给出能力位结论；commandcode 的 Anthropic 面判出 `ignore`。
