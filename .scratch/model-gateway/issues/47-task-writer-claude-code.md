# Claude Code 写入器

Type: task
Status: open
Blocked by: 45

## Question

在写入引擎上做 Claude Code 这一条：写 `%USERPROFILE%\.claude\settings.json` 的 `env` 块。**只写 `ANTHROPIC_AUTH_TOKEN`，不写 `ANTHROPIC_API_KEY`**（后者需一次 `/config` 交互批准、曾拒绝过就静默失灵）。

依据：spec §8 表 + §8.3、[票 01](../issues/01-research-client-integration.md)、[票 24](../issues/24-research-client-login-state.md)。

## 交付

- `env` 块写入：`ANTHROPIC_BASE_URL`（网关根地址；**是否带 `/v1` 由真实客户端实测定**——[票 01](../issues/01-research-client-integration.md) 只记了实际请求路径是 `/v1/messages?beta=true`，没记 base 该带什么）、`ANTHROPIC_AUTH_TOKEN`、四个 `ANTHROPIC_DEFAULT_*_MODEL`（槽位名或该客户端的覆盖名）。
- 接管键 = 上述几个 `env` 键；`settings.json` 里别的键与别的 `env` 键原样保留。
- 还原：摘掉这几个键，`env` 空则摘掉空对象。
- 凭据必须落在「首次运行向导之前可读」的位置（就是这份文件，项目级不行）。

## 收口验收

- 真实 Claude Code（本机零官方登录）走网关完成一次问答，`/api/logs` 能看到该次请求。
- 还原后 `settings.json` 里网关写入的键零残留。
- 全库与全盘反查：网关从未写过 `ANTHROPIC_API_KEY`。
