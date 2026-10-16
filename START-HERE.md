# 从这里开始

> 本地模型网关（Claude Gateway）· 交接单 · 2026-10-16 更新（收尾第二轮收工版）
> 设计文档：[.scratch/model-gateway/spec.md](.scratch/model-gateway/spec.md) ｜ 决策：[docs/adr/0001..0007](docs/adr/) ｜ 历史与定案：[.scratch/model-gateway/map.md](.scratch/model-gateway/map.md)
> 仓库已公开：<https://github.com/ColoristSansi/claude-gateway>（`main` + tag `v0.2.0` + Release）
> 本文件只给「从哪句话接上」和「哪些坑别再踩」。

## 30 秒回到现场

- **要做什么**：本机跑一个网关（Tauri 2 + Rust + React），把第三方模型聚合成「本地模型目录」，让 Claude Desktop（3P 模式）、Claude Code、Codex 三个客户端都用上第三方模型。
- **走到哪了**：**收尾第二轮已收工（2026-10-16）**。`integration/model-gateway` 上 **29 张 `Type: task` 票据全部 `resolved`** —— 原计划 §12 的 25 张（M1 = 39–44、M2 = 45–55、M3 = 56–60、M4 = 61–63）**加收口后评审揪出缺口补的 3 张**（64 OpenAI Chat 入站、65 上游 Responses 出站、66 三个 quirk 的执法点），**加收尾第二轮修的 [票 67](.scratch/model-gateway/issues/67-task-quirk-name-camelcase-parse.md)（quirk 名 camelCase 只认一半）**。相对 `main`（只有票 39 的建仓提交）**129 次提交**、354 个受跟踪文件。
- **四段里程碑都已现场验收 + 你签字**：[44](.scratch/model-gateway/issues/44-task-m1-exit-acceptance.md)（托盘/窗口/库/端口被占）、[55](.scratch/model-gateway/issues/55-task-m2-exit-acceptance.md)（**三个真实客户端各问一句全通** + 一键还原）、[60](.scratch/model-gateway/issues/60-task-m3-exit-acceptance.md)（用量对账三条逐项零差异 + Ollama 面 + 能力位；走查时抓到的白屏 bug 当场修掉）、[63](.scratch/model-gateway/issues/63-task-m4-exit-acceptance.md)（完整卸载 → 干净装 → 应用内升一版 → 再卸载选还原）。**「可以开始拿它日常用」那条线（M2 收口）已过。**
- **门禁**：`pwsh -File .\check.ps1 -Quick -Replay` → `全部通过。` exit 0，**358 条通过 / 15 条 `#[ignore]`**（真上游与人眼项，按票据 Answer 里的命令手动跑）；含 lib 191、**回放 23 格**、ollama 12、chat_inbound 11、responses_outbound 10、structured 19，以及**前端类型检查步**（`tsc --noEmit && vite build`）。最后一轮是在**全新 `CARGO_TARGET_DIR`（`cg-target\67`）** 上跑的，日志 `cg-notes\67-check.log`。
- **GitHub 那半步已走完（2026-10-16）**：公开仓库 [`ColoristSansi/claude-gateway`](https://github.com/ColoristSansi/claude-gateway)，`main` 快进到与 tag 同一提交，tag **`v0.2.0`** 触发 CI `bundle` job 出签名产物，Release `v0.2.0` 上传了 `Claude Gateway_0.2.0_x64-setup.exe` 与 `latest.json`（外加免安装 zip）。**升级闭环走的是真 https**，本机不再依赖 `127.0.0.1:8791` 那个本地源。
- **本机现状**：装的是 **0.2.0（本地验收源版，端点指 `127.0.0.1:8791`）**，数据目录 `%APPDATA%\com.claudegateway.desktop\`，两个真实上游（`commandcode` / `opencode-go`）与四个家族槽位都配好且探针 200，三个客户端「已生效」。**想换成公开仓库那一版**：见下面待办①。

## 回来直接说这一句

```
接手 Claude Gateway 收尾第三轮：读 START-HERE.md 与 cg-notes 里的现场卡。收尾第二轮的 ②④ 都已办（公开仓库 + v0.2.0 Release 已发、票 67 已 resolved），别重做；剩下三件都要人：①把本机换成公开仓库那一版 0.2.0 并跑一次真 https 更新闭环、②离线备份 cg-notes\keys（丢了发不了升级）、③三个客户端「真问一句」+ 交互式卸载走查（默认还原那条）。
```

想直接开工其中一件，就说：

- ~~同步 spec 口径差~~：**已办（2026-10-16）**，四条原文见下面待办①的记录。
- ~~走 GitHub 那半步~~：**已办（2026-10-16）**，现场小节见 `cg-notes/m4-live.md` 第 6 节 + 本文件「走到哪了」。
- ~~修 quirk 名解析缺口~~：**已办（2026-10-16）**，票 67 `resolved`，见待办④。
- 换回生产版：`把本机换装公开仓库的 0.2.0（见 cg-notes/m4-live.md）`

**没有别的实现期票据了**——要开新票请先看下面的「待办」与「遗留」，别顺手改判据（spec §12.1）。

## 待办（收尾第二轮之后剩下的三件，都要人）

1. **把本机换成公开仓库那一版 0.2.0，跑一次真 https 更新闭环**（`m4-live.md` §6 第 6 条）：装一份**生产配置**的旧版（不带 `--config`，端点已是真仓库）→ 启动 → 设置页「关于与版本 → 检查更新」或顶栏横幅 → 看到 0.2.0 → 下载并安装 → 重启后 `/api/status` 的 `version` 是 `0.2.0`。**收尾自查**：`git grep 127.0.0.1:8791 src-tauri/tauri.conf.json` 必须是空。做完把实测结果写进[票 63 的 Answer](.scratch/model-gateway/issues/63-task-m4-exit-acceptance.md)（第 7 条）。本机现在装的是**本地验收源**那一版（端点指临时起的 `127.0.0.1:8791`），公开仓库的 Release 一上，它就该换成生产版。
2. **离线备份 `cg-notes\keys\`**（签名私钥，用户资产，丢了发不了升级）。
3. **三个客户端「真问一句」+ 交互式卸载走查**（选默认还原那条；M4 现场只跑过 `/S` 静默那条）。

## 收尾第二轮已办（留作记录，别重做）

1. ~~**spec 与实现的口径差 4 条，要你拍**~~ **已办（2026-10-16）**：第 1/3/4 条已同步进 spec（§8 表 Desktop 接管键补 `inferenceCredentialKind`、§6 quirks 示例与例外段补 `ModelsListNeedsCredential`、§9 表清单补 `request_frames`）；第 2 条 `cache_write` 票 58 当时就写进了 §9，已在 §12.1 记「收尾追认」。四条原文留作记录：
   - Desktop 的接管键：spec §8 表写 **5 个**，实现是 **6 个**（多 `inferenceCredentialKind="static"`；依据在[票 46](.scratch/model-gateway/issues/46-task-writer-claude-desktop.md) 的 Answer）。
   - `cache_write` 缺价 → 整条 `cost_usd` NULL：[票 58](.scratch/model-gateway/issues/58-task-cache-write-accounting.md) 在实现期自定**并回写了 spec §9**——这是全程唯一一次改 spec，严格说越出 §12.1。
   - [票 59](.scratch/model-gateway/issues/59-task-remaining-three-pages.md) 新增的 `request_frames` 表没进 spec §9 的表清单。
   - quirk 示例表没收录 `ModelsListNeedsCredential`（[票 50](.scratch/model-gateway/issues/50-task-model-discovery-fallback.md)）。
2. ~~**GitHub 那半步**~~ **已办（2026-10-16）**：公开仓库 `ColoristSansi/claude-gateway`（README + MIT LICENSE 一并补上，README 里写了 SmartScreen 那条），`tauri.conf.json` 与 `Cargo.toml` 一起抬到 **0.2.0**、`endpoints` 换成真仓库，`main` 快进到该提交，tag **`v0.2.0`** → CI `bundle` job 出签名产物 → Release 上传 `Claude Gateway_0.2.0_x64-setup.exe` + `latest.json`（+ 免安装 zip）。逐条命令仍是 `cg-notes\m4-live.md` 第 6 节（抄自票 61 的 Answer），**只剩第 6 条「现场验一次真 https 闭环」与第 7 条「把实测结果写进票 63 的 Answer」没做**（见上面待办第 1 条）。顺带修了 CI 的一个真洞：两个 job 都缺「装前端依赖」（`check.ps1` 的 web 步与 tauri 的 `beforeBuildCommand` 都要 `app/node_modules`），commit `60aace9`。
3. **签名私钥是你资产**：`C:\Users\ZHANQUAN\AIGC\Project\cg-notes\keys\`（`updater.key` + `.password.txt` + `.pub`）。**丢了以后再也发不了升级，请离线备份整个目录。** ACL 已于 2026-10-16 收紧（原先是继承来的 `CodexSandboxUsers:(RX)` + `SYSTEM`/`Administrators`，现在**只有 `ZHANQUAN:(OI)(CI)F`**）；若以后要用管理员身份的备份/CI 工具读它，得先加回 `Administrators`。**公开仓库里的两个 Secret（`TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD`）就是从这个目录取的，构建产物里只有公钥。**
4. ~~**票 67（新，实现期收工后由收尾轮发现）**~~ **已办（2026-10-16）**：`Quirk::parse` 三个值补了 camelCase 分支（一处改，库 / 预填表 / 管理面三个调用点同时受益），并按编排者拍板做了第 3 条——解析不出的名字不再静默丢，而是回在 `POST/PUT /api/upstreams` 回执的 **`ignoredQuirks`** 里。两条新用例（单测从 `Quirk::all()` 现派生 camelCase 全过一遍 + `admin_api.rs` 照 spec §6:211 示例逐字 POST/PUT），先红后绿证据与门禁原文都写在[票 67 的 Answer](.scratch/model-gateway/issues/67-task-quirk-name-camelcase-parse.md) 里。

## 已定死、不要再讨论的

| 维度 | 定论 |
| --- | --- |
| 外壳 / 后端 / 前端 | Tauri 2 + Rust（tokio + axum）+ React + 系统托盘；SQLite + DPAPI |
| 协议转换 | 自研薄转换层 + 中间表示（IR），**不**基于任何现成网关封装；也不引 Node sidecar。入站四个面（Anthropic / OpenAI Chat / Responses / Ollama）与出站三个面（Anthropic / Chat / Responses）**都已实现** |
| 模型暴露 | 四个家族槽位 `claude-fable / claude-opus / claude-sonnet / claude-haiku`；协议随**虚拟模型**走，不随上游走 |
| 供应商表粒度 | 认证按「上游 × wire」各一份（缺省继承上游默认）；可用 wire 逐「(上游, 真实模型)」存目录，取值 `anthropic`/`openai-chat`/`openai-responses`，空集=未声明；虚拟模型引用目录条目；「哪些端点面存在」不进 `quirks` |
| 日志 | 默认只记元数据，body 手动开且 1 小时自动关，留 7 天 |
| 配置写入 / 还原 | 显式**接管键清单** + 原子替换（临时文件 + fsync + `ReplaceFileW`）+ **冲突只看接管键**（冲突就放弃并提示）+ 幂等看语义（无变化不写文件）+ 默认**摘键还原**（`RestoreFirstWrite` 可写回首写旧值）、整份回滚可选 + 备份在数据目录（首写快照常驻、滚动 5 份/90 天）；单实例互斥量（票 28 / ADR-0006） |
| 交付 | NSIS per-user 安装包 + 免安装 zip（共用同一数据目录）；**升级走签名校验**（本地源与公开 GitHub Releases 两条路都已跑通，仓库 `ColoristSansi/claude-gateway`） |

## 已证实、别再踩的硬约束

1. **Claude Desktop 的模型 ID 校验是「逐条剔除」**（不是整组 fail-all），判据先过非 Anthropic 厂商黑名单，再过「裸档位名」或「子串含 claude/档位名/anthropic」；四个家族名仍是最稳的解。
2. **Desktop 的推理探针是非流式 `max_tokens:1`**；网关的 `/v1/messages` 非流式与 `count_tokens` 都已实现（票 26）。
3. **`inferenceGatewayBaseUrl` 不带 `/v1`**（app 自己追加 `/v1/messages`）；**`ANTHROPIC_BASE_URL` 同样不带 `/v1`**（票 47 隔离 `CLAUDE_CONFIG_DIR` 实测：带 `/v1` 会打成 `/v1/v1/messages`）。
4. **Codex 的 `wire_api` 官方唯一合法值是 `responses`**；填 `chat` 会让整份 `config.toml` 失效。
5. **Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`**（写 `ANTHROPIC_API_KEY` 会触发交互批准，曾拒绝即静默失灵）；Codex **不写 `env_key`**（声明了变量不存在会硬失败）、**不写 `requires_openai_auth`**、**绝不碰 `~/.codex/auth.json`**。
6. **「未登录」对三个客户端都不是阻断性错误**；Codex 桌面应用的选择器只显示 `model_catalog_json` 目录文件里的模型，云线程 / 远程功能才要官方登录（票 25）。
7. **上游权限是订阅事实，不是配置错**（都是实测）：`commandcode` 只放行 `claude-haiku-5-5` / `claude-sonnet-5-5`，其余 Claude 型号 **403**；`opencode-go` 的 **Anthropic 面在这把 Go key 上 401**（只能走 `openai-chat`）；`commandcode` 的 **Chat 面 + `max_tokens:1` 会间歇挂住 → 504**（别把槽位都压这条组合，现场遇到重问一次即可）。
8. **规范 §6「状态码骗人」当场复现**：`commandcode` 的 **Anthropic 面**对 `output_config.format` 六个变体全 200 却逐字静默忽略 → 能力位判 **`ignore`**；同上游 **chat 面**判 `native`。被标 `ignore` 时客户端要结构化输出怕 **请求期 400** 并说明原因。
9. **commandcode 的 Responses 面是可用的**（`gpt-6-luna` 流式/非流式各 200）；票 57 见过的「必须走 `/messages`」是**模型级**拒绝，不是端点不认。
10. **弹窗/进程收尾一律用精确 exe 路径 + PID**，不许按进程名模糊匹配——有人这么干误关过用户的 Chrome 窗口。
11. **门禁别用共享 `CARGO_TARGET_DIR`**：会跑到别的 worktree 编出的测试二进制，少报测试数（实测 70 vs 92）＝**假绿**。一票一 target，收口用全新目录复验。
12. **`.gitignore` 别写裸 `dist/`**：它会把 `crates/gateway-core/src/dist/` 这类源码目录整个吞掉，`git add -A` 静默跳过（票 62 就这么丢过一个模块，`pub mod dist;` 直接编译失败）。文件里已留警示注释。
13. **卸载会触发票 62 的默认客户端还原**（把三个客户端切回官方、删掉网关建的 `.codex\claudegateway-model-catalog.json`）；卸载确认页 **「Delete app data」默认未勾**、询问框**默认「是（还原）」**。

## 材料都在哪

**仓库内**

```
Claude Gateway/                       ← 主工作区，当前在分支 integration/model-gateway
├── START-HERE.md                     ← 本文件
├── README.md / LICENSE               ← 公开仓库的门面（README 含 SmartScreen 那条；MIT）
├── GLOSSARY.md                       ← 术语表（请求目标一律叫「上游」）
├── check.ps1                         ← 门禁（-Quick / -Replay / -Live / -Smoke）
├── pack.ps1                          ← 打包（-Sign 出签名产物 + latest.json）；serve-updates.ps1 本地更新源
├── crates/gateway-core/              ← 核心库（不依赖 Tauri）：ir / sse / anthropic / openai / responses
│                                        + ingress（proxy / api / ollama）· egress（upstream / provider / structured / discovery）
│                                        + binding · obs · store · writer · dist
├── src-tauri/                        ← Tauri 2 外壳 + 管理 API + 托盘 + 维护面（maint / updater）
├── app/                              ← React 前端（七页：概览 / 上游 / 模型与映射 / 客户端 / 日志 / 用量 / 设置）
├── docs/adr/0001..0007               ← 七条硬决策
└── .scratch/model-gateway/
    ├── spec.md                       ← 技术选型与架构设计（§12 实施顺序 + §12.1 实现期约定）
    ├── map.md                        ← wayfinder 地图（已结清，历史与定案以它为准）
    ├── issues/39..66-*.md            ← 28 张实现期票据（都 resolved；每张末尾有 ## Answer）
    ├── prototype/                    ← 冻结的一次性原型（**只读**，仍可 `cargo test`：25 个用例）
    ├── prototype-upstream.env        ← 两上游凭据（已 gitignore）
    ├── research/*.md · vendor/       ← 调研报告 · 参考仓库克隆（已 gitignore）
    └── smoke.md                      ← 给人做的冒烟清单（check.ps1 -Smoke 打印）
```

**仓库外（`C:\Users\ZHANQUAN\AIGC\Project\cg-notes\`）**

| 文件 | 内容 |
| --- | --- |
| `m2-live.md` / `m3-live.md` / `m4-live.md` | 三次里程碑的**现场步骤卡**（每步「看什么算过」+ 回退路径 + 未做成清单）；m4 第 6 节是 GitHub 逐条命令 |
| `code-review.md` | 收口后代码评审（16 条：2 阻断 / 5 高 / 6 中 / 3 低；含「已核验干净」清单） |
| `55-*.log` `60-*.log` `63-*.log` | 三次收口的真实证据日志（含用量对账原始响应、密钥反查、Ollama curl） |
| `63-packages\` | 四个安装包 + 两个 zip + `latest.json`（含生产配置版与本地源版） |
| `keys\` | updater 签名密钥（**用户资产，丢了发不了升级**） |
| `55-prewrite-backup\` `60-prewrite-backup\` `63-prewrite-backup\` | 改客户端配置前的逐字节快照 |
| `orchestration.md` · `new-worktree.sh` · `hold8787.py` | 编排约定 / 一票一 worktree 脚本 / 端口占用测试小工具 |
| `writer-engine.md` · `upstream-binding.md` · `frontend.md` · `ollama.md` | 实现期四份前置蒸馏笔记 |

**分支**：`main`（建仓快照）→ `integration/model-gateway`（28 张票全部并入）。30 条 `task/*` 分支保留作回退点；实现期的 worktree 已清理，只剩主工作区。
