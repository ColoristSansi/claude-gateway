# Codex 写入器与模型目录

Type: task
Status: open
Blocked by: 45

## Question

在写入引擎上做 Codex 这一条：写 `%USERPROFILE%\.codex\config.toml` 与模型目录文件。**逐行认领**接管键（顶层 `model_provider` / `model` / `model_catalog_json` + `[model_providers.claudegateway]` 这张表 + 两对哨兵行）——实测桌面应用会把 `[windows] sandbox = "elevated"` 插进哨兵之间，按「区域」删会连它一起带走。

依据：spec §7 末 + §8 表 + §8.3 / §8.4、[票 31](../issues/31-grilling-codex-model-catalog.md)、[票 32](../issues/32-prototype-codex-e2e.md)、[票 25](../issues/25-grilling-codex-desktop-card-split.md)、`prototype/codex-gateway.ps1`（参照）。

## 交付

- `config.toml`：`[model_providers.claudegateway]`（`base_url` / `env_key` / `wire_api="responses"` / `http_headers`），**不写 `requires_openai_auth = true`**，`env_key` 声明的环境变量必须真存在。
- 模型目录：`<CODEX_HOME>/claudegateway-model-catalog.json`，指针写**相对文件名**；只装网关暴露的虚拟模型、不抄官方条目；行模板用中性模板（`shell_type="shell_command"`、无 freeform `apply_patch`）；其余字段沿用默认（`visibility="list"`、窗口 262144）；顺序 = 四家族名在前 + 普通模型按名排。
- 写前自检（可解析 / 非空 / 至少一条 `list` / `slug` 非空唯一 / 解析必需字段齐）不过就不写。
- 冲突：`model_catalog_json` 等按 §8.4 判冲突即放弃并提示，另给显式「认领并覆盖」。
- 还原：摘键（默认）+ 仅当目录文件内容与网关最后写入逐字节一致才删文件 + 备份保留。
- **绝不碰 `auth.json`。**

## 收口验收

- 真实 `~/.codex` 实测：CLI（`codex exec`）与桌面应用都跑通，选择器显示四个家族名。
- 回归：外部往哨兵之间插表 → 摘除后回到原样且保住那张表；重写幂等（对齐原型 `-Mode selftest` 的场景）。
