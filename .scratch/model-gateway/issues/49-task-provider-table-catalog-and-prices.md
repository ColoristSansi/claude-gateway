# 供应商表、上游目录与价格快照

Type: task
Status: open
Blocked by: 42

## Question

做「上游」这一层的服务与校验：声明式供应商表（`base_url` + 按 wire 分开的认证 + 每 wire 路径模板 + `quirks`）、上游模型目录（逐「(上游, 真实模型)」记 `supported_wires` + `source`）、三层校验、手工价四列与 models.dev 随包快照。

依据：spec §6 / §9、[ADR-0004](../../../docs/adr/0004-provider-table-granularity.md)、[票 23](../issues/23-design-provider-table-granularity.md)、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- `upstreams` / `credentials`（按「上游 × wire」，未填继承上游默认）/ `upstream_models` 三层的读写与校验。
- 三层校验：配置期主拦（只许选声明过的 wire；未声明可手选并标注「未声明，靠自检」）+ 请求期早拒 400 + 自检为准。
- `pathTemplates` 反查：上游报的端点路径 → wire；查不到就忽略 + 记警告（落进「上次拉取警告」）。
- models.dev 随包快照：导入、`model_catalog` 表存储、设置页手动刷新（失败沿用旧快照）。
- 手工价四列（输入 / 输出 / 缓存读 / 缓存写，各自可填可留空、填了即锁定）；每一项标出实际来源。
- 删被引用的上游要能列出引用处。

## 收口验收

- 单测：三层校验各一条、路径反查（命中 / 查不到）、价格优先级（手工 > 快照 > 未知）、刷新不覆盖手工锁定的项。
- 两条真实上游各建一条并拉到目录，`supported_wires` 与观测一致（commandcode 带 `supported_endpoints`；opencode 两面 `/models` 都不报端点 → 只能靠内置文档表 + 手工）。
