# 冒烟清单（agent 做不到、必须人做的那半）

票 19 §8 把客户端冒烟分两层：**脚本能断言的**那半已经在 `tests/replay.rs` 里自动跑了（回放、槽位路由、
切法不变性、头注入、错误信封）；这份只列**必须真人看**的动作。

跑法：`pwsh -File .\check.ps1 -Smoke`（打印本清单，逐项等你确认）。
前置：网关已起（`cd prototype; cargo run`，或安装版应用），已跑过 `desktop-3p.ps1 -Mode gateway`。

## 一、Claude Desktop（3P 模式）

1. **完全退出** Claude Desktop（托盘也要退），再重新启动。
2. 打开模型选择器：**恰好**四个家族名 —— `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`，
   没有第五个、没有多出来的官方模型，且**没有**「模型不可用」横幅（横幅 = 非流式探针那条路坏了，见票 26）。
3. 四个槽位各问一句（例如「用一句话介绍你自己」）：四段回答都要**逐字流式**出现，不是憋一下整段蹦出来。
4. 对照 `curl.exe -s http://127.0.0.1:8787/api/logs`：刚才四条都记上了，且**分落两家上游**
   （`claude-fable` / `claude-opus` → opencode；`claude-sonnet` / `claude-haiku` → commandcode），`input_tokens` 非零。
5. 发一条**会用工具**的对话（例：让它先查天气再回答）：工具调用卡片要出现，工具结果回灌后模型能接着答完
   （这是工具往返的实机那一半；自动那半在 `tool_roundtrip_multiturn_and_partial_input`）。
6. 挑一个长回答：盯着看是否有**长时间完全不动**的段落。网关在上游静默超过保活间隔时会向前发 `ping`
   （`GATEWAY_PING_SECS`，默认 15 秒），客户端应该保持连接而不是报断流。

## 二、切回 1P（大退路，每次验收都要走一遍）

7. `pwsh -File .\desktop-3p.ps1 -Mode standard` → 完全退出并重启 Desktop → 官方登录态**完好**
   （头像 / 账号还在，能选官方模型并正常对话）。
8. 确认 `%LOCALAPPDATA%\Claude-3p\configLibrary\` 里**没有**残留的 `inferenceProvider` 字段。
   （要整份回滚就用 `-Mode restore`，它拿首次备份整目录还原。）

## 三、Codex（CLI 与桌面应用）

前置：网关已起，且跑过 `pwsh -File .\prototype\codex-gateway.ps1`（写模型目录 + `config.toml`）。

9. **CLI**：`codex exec --skip-git-repo-check "只回答一个数字：1+1等于几？"` —— 抬头必须是
   `provider: claudegateway` / `model: claude-sonnet`，答案正确；`/api/logs` 有一条对应记录。
10. **CLI 工具往返**：`codex exec --skip-git-repo-check "请用 shell 工具运行 echo hi，并原样告诉我输出"` ——
    要么真的跑出 `hi`，要么把 Codex 自己的沙箱拒绝文案原样贴回来。两种都算**往返通**；
    只有「模型假装跑了」或「卡住不动」算失败（Codex 的 Windows 沙箱默认会拦起进程）。
11. **桌面应用**：**完全退出**（含托盘）再重开 → 新建会话 → 模型选择器里能看到四个家族名
    （没有就该只看到官方模型，或一个都没有）→ 选 `claude-sonnet` 问一句 → **流式**作答、无报错。
12. **还原**：`pwsh -File .\prototype\codex-gateway.ps1 -Mode restore` → 确认
    `~/.codex/config.toml` 里查不到 `claudegateway`、`claudegateway-model-catalog.json` 已删，
    而 `[desktop]` / `[plugins.*]` / `[mcp_servers.*]` / `[windows]` 原样还在（桌面应用自己写的那些**不许**被带走）。

## 四、还没进这份清单的

- **Claude Code**：`ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` 的写入与还原属票 28 的配置写入引擎
  （实现阶段），引擎落地后这里补一条「换上游后 Claude Code 仍能对话」。
