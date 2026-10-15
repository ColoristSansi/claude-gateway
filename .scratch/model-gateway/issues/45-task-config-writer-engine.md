# 配置写入引擎

Type: task
Status: open
Blocked by: 42

## Question

按 [ADR-0006](../../../docs/adr/0006-config-writer-owned-keys.md) / [票 28](../issues/28-grilling-config-writer-transactionality.md) 的 21 条实现写入引擎（Rust，旁路模块 `clients::writer`）。三个客户端写入器（46 / 47 / 48）都长在它上面。

依据：ADR-0006、spec §8 五条规则 + §8.4、[票 30](../issues/30-task-build-regression-suite.md) 第 8 格。

## 交付

- **显式接管键清单**（每个「客户端 × 文件」一份）+ 按格式保序改写（JSON / JSONC / TOML；TOML 保序）；解析失败即停。
- 原子替换：同目录临时文件 + fsync + `ReplaceFileW`（`NotFound` / `ERROR_NOT_SUPPORTED` 退 `rename`，最多 3 轮）；被占用 / 权限失败退避重试 3 次后报错并给出文件路径、备份路径、占用提示。
- 冲突检测：只比接管键现值与 `client_config_state` 记录的上次写入值；记录缺失按首次接管处理。
- 幂等：语义相同**完全不写**（mtime 不变）；要写则连续两次字节相同。
- 备份：`%APPDATA%\com.claudegateway.desktop\backups\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`；首写快照常驻、滚动保留 5 份 / 90 天；首写时文件不存在只写 marker。
- 还原：默认**摘键**（含指针性条目）；**整份回滚**仅当文件自上次网关写入后未被外部改过；网关新建且只剩网关内容的文件整个删。
- 多文件按依赖顺序逐个原子替换（先被引用者后指针）；单实例互斥量 + 每文件一把锁，不做跨进程锁文件。
- 写入 / 还原记库 + 元数据日志。

## 收口验收

- [票 30](../issues/30-task-build-regression-suite.md) 第 8 格的断言全绿，且**真实 `%LOCALAPPDATA%\Claude-3p\`、`~/.claude`、`~/.codex` 一律不碰**（写入根参数化到临时目录）。
- 崩溃窗口 / 占用重试 / 冲突放弃 / 幂等不写 / 摘键还原 / 整份回滚被外部改过即拒绝，各至少一条用例。
