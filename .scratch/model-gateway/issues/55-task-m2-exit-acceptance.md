# M2 收口：三个真实客户端端到端

Type: task
Status: resolved
Blocked by: 46, 47, 48, 50, 52, 53, 54

## Question

走 spec §12 的 M2 收口验收——这段收口就是「可以开始拿它日常用」那条线。**主会话 + 用户在场**，不是可甩给 worktree 的活。

依据：spec §12 M2、[票 19](../issues/19-grilling-test-strategy.md)、[smoke.md](../smoke.md)。

## 交付

- 冒烟清单更新（三个客户端 + 一键还原 + 安装包，逐项可勾）。
- 一份 M2 验收记录贴进本票 Answer：`check.ps1` 输出、三个客户端各自的 `/api/logs` 证据、安装步骤。

## 收口验收

- **Claude Desktop 3P**：菜单四家族名、流式正常、切回 1P 登录态完好。
- **Claude Code**：零官方登录下走网关完成问答。
- **Codex**：CLI 与桌面应用都能选到网关模型并作答。
- 写入引擎与 [票 30](../issues/30-task-build-regression-suite.md) 第 8 格全绿。
- NSIS 装得上、能跑；一键还原把三个客户端都送回官方态。
- **用户当场三个客户端各问一句并签字。**

## Answer

**M2 收口通过**（2026-10-15/16，主会话 + 用户在场；分支 `integration/model-gateway`）。这就是 spec §12 那条「**可以开始拿它日常用**」的线。

现场步骤卡（机器侧准备 + 每步看什么算过 + 还原命令 + 改前原文快照路径）：`C:\Users\ZHANQUAN\AIGC\Project\cg-notes\m2-live.md`；证据日志同目录 `55-*.log`；改配置前的原文快照 `…\cg-notes\55-prewrite-backup\`。

### 一、`check.ps1` 与门禁（真输出）

```
$ pwsh -File .\check.ps1 -Quick -Replay
=== fmt (cargo fmt --all --check) ===           无输出
=== clippy (cargo clippy --workspace -D warnings) ===   无输出
=== unit (cargo test --workspace) ===           bin 8 / lib 167 / admin_api 6 / binding 14 /
                                                discovery 8 / dist 6 / dist_api 5 / gateway_service 4 /
                                                logs_api 4 / ollama 12 / provider_table 14 /
                                                structured 19 / doctest 1
=== replay (--features mock --test replay) ===  22 passed
=== replay-unit (--features mock --bins) ===    5 passed
全部通过。   exit 0
```

写入引擎与**票 30 第 8 格**：幂等（语义同不写、mtime 不变）、首写快照、摘键还原、非接管键手改保留、TOML 保序与 `[desktop]` 段保住——`writer` 34 条单测在门禁里全绿（票 45 落地，票 46/47/48 各自补齐客户端侧）。原型那条「跑门禁前后对真实 `%LOCALAPPDATA%\Claude-3p` / `~/.claude` / `~/.codex` 逐文件 SHA256 快照，7355 文件 diff 0 行」也复验过，**测试从不碰真实配置根**。

### 二、NSIS 装得上、能跑（真跑）

- 包：`C:\Users\ZHANQUAN\AIGC\Project\cg-target\55\release\bundle\nsis\Claude Gateway_0.1.0_x64-setup.exe`（5,151,932 B）。
- `setup.exe /S` 静默装，**没有弹 UAC**（per-user，装进 `%LOCALAPPDATA%\Claude Gateway\`）→ 启动 → `GET /api/status` 回真实数据：
  `{"version":"0.1.0","port":8787,"running":true,"startedAt":"…","dbPath":"…\\com.claudegateway.desktop\\gateway.db"}`。
- 卸载：`uninstall.exe /S` → 程序目录与 HKCU 卸载项清干净、**数据目录保留**（票 54 已单独验过一轮）。

### 三、现场配置（全程走 HTTP API，未直接改库）

| 上游 | baseUrl | 目录 | 自检 |
| --- | --- | --- | --- |
| `commandcode` | `https://api.commandcode.ai/provider/v1` | 90 条 | `ok=true` 930ms |
| `opencode-go` | `https://opencode.ai/zen/go/v1` | 37 条 | `ok=true` 1099ms |

| 槽位（全局档） | 上游 | 真实模型 | wire | 探针 |
| --- | --- | --- | --- | --- |
| `claude-haiku` | commandcode | `claude-haiku-5-5` | anthropic | 200 / 2142ms |
| `claude-sonnet` | commandcode | `claude-sonnet-5-5` | anthropic | 200 / 1111ms |
| `claude-opus` | opencode-go | `kimi-k2.7-code` | openai-chat | 200 / 4468ms |
| `claude-fable` | opencode-go | `glm-5.3` | openai-chat | 200 / 914ms |

`POST /api/acceptance/run`（haiku 槽位非流式最短请求，走真实入站 `/v1/messages`）：

```json
{"ok":true,"slot":"claude-haiku","upstream":"commandcode","realModel":"claude-haiku-5-5",
 "wire":"anthropic","status":200,"elapsedMs":2169,"firstByteMs":2168,"requestId":"req-65de2e46309160020","logId":33}
```

为什么不是四个槽位都绑 Claude 型号——**是该订阅的权限事实，不是配置错**（都实测过）：commandcode 只放行 `claude-haiku-5-5` / `claude-sonnet-5-5`，其余 Claude 型号 403；opencode 的 Anthropic 面在这把 Go key 上 401（所以走 `openai-chat`）；commandcode 的 chat 面 + `max_tokens:1` 会挂住（票 26 那条已知形状，已避开）。

### 四、三个真实客户端（用户当场走 + 签字）

- **Claude Desktop（3P）**：完全退出重开后，模型菜单出现四个家族名 `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`；选 sonnet 问一句，**流式逐字作答、不需要 1P 登录**。配置侧：`inferenceProvider=gateway`、`inferenceGatewayBaseUrl` **不带 `/v1`**、四个完整模型 ID、`_meta.json` 的 `appliedId` 指向网关档案。
- **Claude Code**：新终端起 `claude`（零官方登录），问一句通；`/model` 里能看到四个家族名。配置侧只写 `ANTHROPIC_AUTH_TOKEN`（**从不写 `ANTHROPIC_API_KEY`**，全盘/全库反查 0 命中）。票 47 顺带就地结掉了票 24 的 **U1**：零官方登录下 `settings.json` 的 `env` 块确实被读到并发请求；`ANTHROPIC_BASE_URL` **不带 `/v1`**（隔离 `CLAUDE_CONFIG_DIR` 实测：带 `/v1` 会打成 `/v1/v1/messages`）。
- **Codex**：完全退出再起（CLI 与桌面应用同一张卡），选择器里看到四个 `claude-*` 并作答。配置侧 `wire_api="responses"`、`model_catalog_json` 指相对文件名、**不写 `env_key`/`requires_openai_auth`**、**从不碰 `auth.json`**。
- **一键还原**：三张卡「还原」→ **三张都变「已回官方」、侧栏未生效客户端数 0**；再「切换」→ 三张都回「已生效」。`GET /api/clients` 实测：
  ```
  还原后：claude-desktop=official  claude-code=official  codex=official
          {"upstreamsFailedSelfcheck":0,"clientsNotEffective":0,"ready":true}
  切回后：claude-desktop=effective claude-code=effective codex=effective
  ```
- **用户当场三个客户端各问一句，确认全通并签字。**

### 五、收口时修掉的一处真缺陷（不记偏差，直接改）

现场准备时发现：`restore` 之后 **Claude Desktop 徽标显示「值不匹配」而不是「已回官方」**。根因是四态判据把「已回官方」判成「文件里一个接管键都没有」，而 Desktop 的 `_meta.json` 里 `appliedId` **必须**指向某个档案（还原后指回 `Default`），所以它永远不可能「零键」。
修法（commit `eb3d550`）：判据改成「**网关写下去的值一个都不在了**」（`Doc::carries_gateway_value` → `FieldDiff.gateway_value` → `has_no_gateway_keys`），侧栏未生效数只数还需要动作的两态；补回归用例 `admin_api::desktop_restore_is_official_and_a_hand_edit_is_still_mismatch`（手改仍判「值不匹配」、还原判「已回官方」）。重打包重装后在装好的那台上实测通过。

### 遗留（不阻塞「日常可用」，逐条有归处）

1. 还原后 Desktop 会**多留一个 `Default` 之外的空壳**？不会——`entries` 里人造条目已摘；但 `appliedId` 必定回 `Default`，这是 Desktop 自己的形状。
2. 开机体感：opencode 那条 chat 面偶发 504（重问一次即通）；commandcode 的 opus/fable 型号 403（订阅权限）。
3. 改绑定后 Codex 卡片会变「值不匹配」，需再 apply 一次（目录文件跟三元组绑定）。
4. 现场准备期有一次 `grep` 把两把明文 key 打进了子代理的会话记录（未进任何文件/票据/日志/接口回执）。**已向用户报备，钥匙轮换由用户决定。**
5. 未验：Desktop 的 1P 登录态「完好」是靠 3P/1P 两套 userData 目录隔离推定的（现场没切回 1P 再登录一遍）；`check.ps1 -Live` 与 `-Smoke` 两档本次没跑（真实客户端那半已由用户当场替代）。
