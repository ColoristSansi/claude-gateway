# 模型发现与降级

Type: task
Status: open
Blocked by: 49

## Question

做「拉模型」（模型发现）：打上游 `/models`，按三种结局分流处置，并把反查警告落库。九条定案见 [票 35](../issues/35-grilling-model-discovery-fallback.md)。

依据：spec §6、[票 35](../issues/35-grilling-model-discovery-fallback.md)、[票 23](../issues/23-design-provider-table-granularity.md)。

## 交付

- 拉取：用上游 `default` 凭据、**不为每个 wire 面重复拉**、**20 秒超时**、**不自动重试**（失败提示里给「重试」按钮）。
- 三种结局：成功 / **该上游不提供模型发现**（404、405、或 200 但零条 → 软提示 + 手填入口，不报红）/ **失败**（其他非 2xx、超时、断网 → 失败提示里内联「手工添加模型」，`manual` 锁定，上游仍可保存）。
- **重拉失败保留旧条目**，只有成功才覆盖 `source = upstream` 的条目；手改过的条目锁定不被覆盖。
- 优先级：手改（锁定）> 上游 `/v1/models` > 随包内置文档表 > 未声明。
- 警告落进上游记录的「上次拉取警告」，卡片 / 列表给角标，下次成功清除；**不阻断保存、不进概览「最近错误」**；也不计入侧栏「未通过自检的上游数」（判据只看自检）。
- opencode 特殊：只拉带凭据那份目录；带凭据拉取 404 / 空结果才退匿名。

## 收口验收

- 三种结局各一条用例（mock 或录制）。
- 重拉失败后旧条目仍在、警告角标可查、清空时机正确。
- 真实上游各拉一次：commandcode 得到 89 条带 `supported_endpoints`；opencode 按凭据分支拉到预期集合。
