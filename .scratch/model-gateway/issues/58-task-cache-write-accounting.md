# cache_write 采集与计价

Type: task
Status: open
Blocked by: 53

## Question

补上 [票 33](../issues/33-grilling-cost-accounting.md) 留下的那一项：`cache_write` 本期不计，因为库里没有该列。现在补列、补采集、补计价。

依据：spec §9、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- `requests`（或 `request_log`）加 `cache_write` 列 + 迁移。
- 两条 wire 的 usage 映射：Anthropic 侧与 OpenAI 侧的缓存写字段按真实响应逐个核对后落代码（**以实测响应为准**，不照抄文档字段名）。
- 计价：`cache_write` 按缓存写价；`cache_read` 按缓存读价、缺则退输入价（这条已有，复核）。
- 界面：手工价四项现在四项都真的生效；「N 次无价格 / N 次 token 未知」计数含新列。

## 收口验收

- 单测：两条 wire 的 usage 映射各一条（用真实录制帧）。
- 一次带缓存写的真实请求（或录制回放）能让 `cache_write` 落库并计入成本；**历史不重算**。
