# Claude Desktop 写入器与 1P/3P 快切

Type: task
Status: open
Blocked by: 45

## Question

在写入引擎上做 Desktop 这一条：写 `%LOCALAPPDATA%\Claude-3p\configLibrary\` 的 profile JSON 与 `_meta.json`，覆盖键 = `inferenceProvider` / `inferenceGatewayBaseUrl` / `inferenceGatewayApiKey` / `inferenceGatewayAuthScheme` / `inferenceModels`；1P↔3P 一键切换。

原型的最小版三处被 ADR-0006 否决（profile 整份重建、`standard` 不维护 `_meta.json`、`restore` 删整个 configLibrary）——本票按引擎规则重做，`prototype/desktop-3p.ps1` 只当参照。

依据：spec §8 表 + §8.1 / §8.2 / §8.4、[票 15](../issues/15-prototype-desktop-3p-e2e.md)、[票 16](../issues/16-research-desktop-model-id-validation.md)、[票 28](../issues/28-grilling-config-writer-transactionality.md)。

## 交付

- profile JSON 写入：**baseUrl 不带 `/v1`**；`inferenceModels` 填完整模型 ID。
- `_meta.json` 维护：`entries` 增删网关条目、`appliedId` 指向 / 回退，**不动别的 profile**。
- 模型 ID 生成按 §8.2 判据（非 Anthropic 厂商黑名单 → 裸档位名或含 claude / 档位名 / anthropic 子串），四个家族名天然合法。
- 切回 1P = 摘 `inferenceProvider`（不还原备份）；「整份回滚」是第二选项。
- 还原：摘网关条目 + `_meta.json` 回退。

## 收口验收

- 真实客户端实测：切 3P → Desktop 菜单显示四个家族名、流式问答正常 → 切回 1P 后官方登录态完好。
- 还原后该 profile 回到首写前内容（别的 profile 一个字节不动）；网关当初新建且只剩网关内容的文件被整个删掉。
- 幂等：连切两次 3P，第二次不动文件。
