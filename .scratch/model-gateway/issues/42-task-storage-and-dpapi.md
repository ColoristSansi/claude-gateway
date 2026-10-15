# 存储层与 DPAPI

Type: task
Status: open
Blocked by: 39

## Question

按 spec §9 把 SQLite 层做出来，并修掉原型那笔债：`store.rs` 的「每行单开连接 + 同步写库阻塞 runtime」（原型自注 shortcut）。

依据：spec §9、[ADR-0006](../../../docs/adr/0006-config-writer-owned-keys.md)（`client_config_state` 的列）、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- 建表：`upstreams`（含上次拉取时间 / 结果 / 警告）、`upstream_models`（真实模型 + `supported_wires` + `source` + 手工价四列）、`credentials`（密文，按「上游 × wire」）、`virtual_models`、`client_profiles`、`client_config_state`、`requests`、`request_bodies`、`model_catalog`、`settings`。
- 迁移机制（`PRAGMA user_version` + 顺序迁移函数），能在旧库上升级。
- 连接策略：单写线程 + 请求排队，或连接池——二者择一，判据是「不阻塞 tokio runtime」。
- DPAPI 加解密（`CryptProtectData` / `CryptUnprotectData`，当前用户范围），密钥只以密文入库；界面侧要的掩码读取接口。
- 数据目录解析：`%APPDATA%\com.claudegateway.desktop\`，首次启动自建（[票 18](../issues/18-grilling-packaging-and-updates.md)）。

## 收口验收

- 单测：建表 / 迁移（造一份旧版库升上来）/ 读写往返 / 连接并发（两个写不互相阻塞 runtime）。
- 库内反查不到任何明文密钥（拿已知密钥串在库文件里搜不到）。
