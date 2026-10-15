# 用量与日志基础

Type: task
Status: open
Blocked by: 42, 51

## Question

做 M2 需要的那半可观测：请求元数据落库、成本按当时价冻结、保留 7 天自动清理、日志页元数据表格、body 开关与脱敏。

依据：spec §9、[票 33](../issues/33-grilling-cost-accounting.md)、[票 17](../issues/17-grilling-ui-information-architecture.md)、[票 30](../issues/30-task-build-regression-suite.md) 第 7 条。

## 交付

- `requests` 表写入：虚拟模型 / 上游 / 客户端 / 状态码 / 耗时 / 首字延迟 / token / `cost_usd`（**请求时按当时价冻结**）。
- 价格口径：手工 > models.dev 快照 > 未知；缺价不计入总额、另计「N 次无价格 / N 次 token 未知」；上游没回 usage 标「token 未知」，不用本地估算反算。
- 保留 7 天自动清理。
- 日志页：元数据表格（时间倒序 50 条、按客户端 / 上游 / 虚拟模型 / 状态码筛选 + 模型名与请求 ID 搜索）。
- body 开关：默认关，开启需隐私确认，**1 小时后自动关**。
- 脱敏：API 面、库、诊断包里都查不到密钥明文（含 `GATEWAY_LOG_BODY` 开着时）。

## 收口验收

- 请求落库且成本按当时价冻结；改手工价或刷新快照**不追古**。
- body 默认不写 → 开启可捞 → 到点自动关（票 30 的窗口断言照搬）。
- 拿已知密钥串在库与 API 响应里反查不到。
