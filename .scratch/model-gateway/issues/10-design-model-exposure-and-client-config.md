# 模型暴露方式与客户端配置写入

Type: grilling
Status: resolved
Blocked by: —

## Question

多个供应商、多个模型时客户端怎么选到它们（Claude Desktop 只让填 claude-* 系列 ID）？客户端配置文件要不要自动改写？Claude Desktop 的 1P/3P 怎么切？

## Answer

- **全局四个家族槽位 + 每客户端可覆盖**（用户选的方案）：`claude-fable`、`claude-opus`、`claude-sonnet`、`claude-haiku` 固定填在 Claude Desktop 设置里；**网关把它们映射到真实模型 ID，不同系列可以绑定不同上游**。Claude Code 与 Codex 各自可以另存一套覆盖，互不干扰。
- 每个槽位**只绑一条上游**（不做自动切换，见 [08](08-design-destination-and-scope.md)）。
- 普通虚拟模型（给 Codex 与其他客户端用）由用户取名，同样绑定三元组；Codex 侧生成 `model_catalog_json`。
- **配置写入（用户原话「1+初始化」）**：自动写入 + **首写备份** + 幂等重写 + **一键还原到程序刚安装时的状态**。
- **Claude Desktop 额外做 1P（官方标准）与 3P（走网关）的一键切换**：切 3P 写入 `inferenceProvider=gateway` 与网关地址、密钥、四个模型名；切 1P 时移除这些字段回到官方登录态，原文件从备份还原。
- 本期不做注册表托管策略模式（`HKLM|HKCU\SOFTWARE\Policies\Claude`），单机自用价值有限且需要管理员权限。
