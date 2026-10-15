# 从这里开始

> 本地模型网关（Claude Gateway）· 交接单 · 2026-10-15 更新
> 地图：[.scratch/model-gateway/map.md](.scratch/model-gateway/map.md) ｜ 设计文档：[.scratch/model-gateway/spec.md](.scratch/model-gateway/spec.md)
> **状态与票据清单以地图为准**，本文件只给「从哪句话接上」和「哪些坑别再踩」。

## 30 秒回到现场

- **要做什么**：本机跑一个网关（Tauri 2 + Rust + React），把第三方模型聚合成「本地模型目录」，让 Claude Desktop（3P 模式）、Claude Code、Codex 三个客户端都用上第三方模型。
- **走到哪了**：**地图已结清（2026-10-15）**——路线探明、设计文档齐（[spec.md](.scratch/model-gateway/spec.md)，新增 §12 实施顺序）、原型跑通并**冻结**（三个目标客户端里两个已端到端验收：Desktop 3P、Codex；Claude Code 链路归实现阶段）。**最后结的是 [票 38](.scratch/model-gateway/issues/38-grilling-implementation-order.md)（原型之后：实施顺序与里程碑）**：四段 **M1 地基 → M2 日常可用 → M3 保真长尾 → M4 分发收尾**，**「可以开始拿它日常用」画在 M2 收口**；骨架**另起 cargo workspace**（`crates/gateway-core` lib **不依赖 Tauri** + `src-tauri` 唯一 bin + `app/` React），搬迁一次性对齐 spec §3 边界、搬核心七个源文件 + 整套回归基建（门禁不重写）、**不搬**原型 `main.rs` 与两个 `.ps1` 写入器、`prototype/` 原地冻结不删；欠账插位（写入引擎与客户端档案覆盖入 M2，Ollama 面 / 结构化输出 / `cache_write` 入 M3，前端七页 4+3）；验收 = 三层网当门禁、真实客户端端到端只在 M2/M4 收口；**M1 顺手修三笔原型债**（store 连接策略、原型件分进程隔离、crate 拆分）并**上 GitHub Actions**（推送跑 `-Quick -Replay`，打 tag 出 NSIS + zip + `latest.json`）；仓库根 `git init`、代码与规划同库。已归档为 [ADR-0007](docs/adr/0007-skeleton-and-implementation-order.md)。逐票历史与全部定案看[地图](.scratch/model-gateway/map.md)的「Decisions so far」（票 32–37 也在那儿）。产物在 `.scratch/model-gateway/prototype/`（启动 `cargo run`，门禁 `pwsh -File .\check.ps1`）。
- **现在的 frontier**：**地图结清，不再开决策票**；实现期票据已拆好——`issues/39-*` 起 **25 张 `Type: task`**（M1 = 39–44、M2 = 45–55、M3 = 56–60、M4 = 61–63，依赖写在各自 `Blocked by:`）。**当前唯一可开工的是 [票 39](.scratch/model-gateway/issues/39-task-repo-workspace-and-ci.md)（建仓 + workspace + CI）**，它必须在主工作区先做完——此刻仓库根还没有 `.git`。之后拿这 25 张跑 `/implement-spec`；约定见 spec §12.1。
- **不用再管**：本文件旧版让人装客户端、填凭据的两件事**已经做完**（票 13 已结：Desktop = Store MSIX `Claude_2.37337.1.0` 且 1P 已登录、Claude Code 2.1.296、Codex CLI 0.162.1，`prototype-upstream.env` 已填且两上游实测可用）。**另一件也别捡**：`prototype/desktop-3p.ps1` 跟票 28 的行为差（profile 整份重建、`standard` 不维护 `_meta.json`、`restore` 整目录替换，还有原子替换/冲突检测/滚动备份没实现）**归实现阶段、本图不开票**——2026-10-15 用户定，见票 28 §10 与地图 Out of scope；票 30 第 8 格已注明「引擎落地前挂起」，两笔欠账已插进 spec §12 的 M2。

## 回来直接说这一句

```
手工做掉 39（建仓 + workspace + CI），然后跑 /implement-spec 从 40 起。
```

或直接 `/implement-spec`（它会先看到 39 是唯一可开工的票；39 里有一步是在主工作区 `git init`，做完才有分支可切 worktree）。

规矩：**一票一会话**，用 `tdd` 建；改完编解码 / SSE / quirks / 回放先跑 `check.ps1 -Quick -Replay` 并把结果贴出来。四张收口票（44 / 55 / 60 / 63）是现场验收 + 用户签字，别甩给子代理。**`prototype/` 已冻结，只读。**

## 已定死、不要再讨论的

| 维度 | 定论 |
| --- | --- |
| 外壳 / 后端 / 前端 | Tauri 2 + Rust（tokio + axum）+ React + 系统托盘；SQLite + DPAPI |
| 协议转换 | 自研薄转换层 + 中间表示（IR），**不**基于任何现成网关封装；也不引 Node sidecar |
| 模型暴露 | 四个家族槽位 `claude-fable / claude-opus / claude-sonnet / claude-haiku`；协议随**虚拟模型**走，不随上游走 |
| 上游协议 | Anthropic 原生直通优先，OpenAI Chat 兜底；差异写成声明式表 + `quirks` 枚举 |
| 供应商表粒度 | 认证按「上游 × wire」各一份（缺省继承上游默认）；可用 wire 逐「(上游, 真实模型)」存目录，取值 `anthropic`/`openai-chat`/`openai-responses`，空集=未声明；虚拟模型引用目录条目；「哪些端点面存在」不进 `quirks` |
| 日志 | 默认只记元数据，body 手动开且 1 小时自动关，留 7 天 |
| 配置写入 / 还原 | 显式**接管键清单** + 原子替换（临时文件 + fsync + `ReplaceFileW`）+ **冲突只看接管键**（冲突就放弃并提示）+ 幂等看语义（无变化不写文件）+ 默认**摘键还原**、整份回滚可选 + 备份在数据目录（首写快照常驻、滚动 5 份/90 天）；单实例互斥量，不移植跨文件事务（票 28 / ADR-0006） |
| 交付 | NSIS per-user 安装包 + 免安装 zip；不签名；升级走公开 GitHub Releases |

## 已证实、别再踩的硬约束

1. **Claude Desktop 的模型 ID 校验是「逐条剔除」**（不是整组 fail-all），判据先过非 Anthropic 厂商黑名单，再过「裸档位名」或「子串含 claude/档位名/anthropic」；四个家族名仍是最稳的解。
2. **Desktop 的推理探针是非流式 `max_tokens:1`，网关不实现非流式 `/v1/messages` 就会常驻「模型不可用」横幅**（`ConfigHealth=config_model_rejected`）。**（2026-10-15 票 26 更正归属：`provider: gateway` 的连接测试只打非流式 `/v1/messages` 且只看状态码；`count_tokens` 的唯一调用点是 Azure Foundry provider、SDK 的 `countTokens()` 零调用点——两个端点仍都该实现，票 26 已实现。）**
3. **`inferenceGatewayBaseUrl` 不带 `/v1`**（app 自己追加 `/v1/messages`）。
4. **Codex 的 `wire_api` 官方唯一合法值是 `responses`**；填 `chat` 会让整份 config.toml 失效。
5. **Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`**（写 `ANTHROPIC_API_KEY` 会触发交互批准，曾拒绝即静默失灵）；Codex 不写 `requires_openai_auth = true`。
6. **「未登录」对三个客户端都不是阻断性错误**；Codex 桌面应用的选择器**不**按登录门控——但它只显示 `model_catalog_json` 目录文件里的模型（不写目录就看不到网关模型），云线程 / 远程功能才需要官方登录（票 25 实测定案）。
7. **只有 commandcode 能从 `/models` 自动学到逐模型可用 wire**（89/89 带 `supported_endpoints`，路径数组，70/89 多值）；opencode 两面 `/models` 只有 `id/object/created/owned_by`，Anthropic 面 `/models` 直接 404，ID 前缀与 `-free`/`-contributor` 后缀都推不出 family——它的逐模型 wire 只能靠随包内置文档表 + 手工。
8. **网关绝不碰 `~/.codex/auth.json`**（官方登录态）；Codex 的切换与还原都只改 `config.toml`（票 25 定案）。

## 文件都在哪

```
Claude Gateway/
├── START-HERE.md                  ← 本文件
├── GLOSSARY.md                    ← 术语表（请求目标一律叫「上游」）
├── crates/gateway-core/           ← 正牌网关核心库（M1 起建，见 ADR-0007）
├── src-tauri/                     ← Tauri 2 外壳 + 管理 API + 托盘（唯一 bin）
├── app/                           ← React 前端（七页）
├── docs/adr/0001..0007            ← 七条硬决策（0005 测试策略=票 27、0006 配置写入引擎=票 28、0007 骨架与实施顺序=票 38）
└── .scratch/model-gateway/
    ├── map.md                     ← wayfinder 地图（已结清，历史与定案以它为准）
    ├── spec.md                    ← 技术选型与架构设计（§12 实施顺序）
    ├── prototype/                 ← 可运行原型（**已冻结**，Rust crate + fixtures + 自检脚本）
    ├── prototype-upstream.env     ← 两上游凭据（已 gitignore）
    ├── issues/                    ← 票据（编号有空洞：06/07/12/20 已移除）
    ├── research/*.md              ← 调研报告（archive/ 里的已归档）
    └── vendor/                    ← 参考仓库克隆（体积大，已 gitignore）
```
