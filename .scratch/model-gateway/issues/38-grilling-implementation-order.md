# 原型之后：正牌应用的实施顺序与里程碑

Type: grilling
Status: resolved (2026-10-15)
Blocked by: —

## Question

地图终点的交付物里包含**实施顺序**，而 spec 现在只有原型那一条（§11 末行：mock 上游回归 → Anthropic 链路 → Desktop 3P 端到端 → 再补 OpenAI Chat / Responses 与另外两个客户端），它描述的是**已经做完的原型**。从原型到正牌 Tauri 应用之间没有任何顺序、里程碑与逐步验收。要定：

1. **里程碑怎么切、顺序是什么**：起手做什么，做到哪一步算「可以开始拿它日常用」。
2. **挂着「归实现阶段」的欠账各插在哪一步**——至少这几笔：
   - 配置写入引擎（[票 28](28-grilling-config-writer-transactionality.md) 的 21 条 + [票 30](30-task-build-regression-suite.md) 第 8 格那几条断言）；
   - 客户端档案覆盖（[票 10](10-design-model-exposure-and-client-config.md)，原型明确不造）；
   - Ollama 兼容面落代码（spec §4.4 + [票 36](36-grilling-ollama-endpoint-scope.md)；原型没有 `ollama.rs`）；
   - 结构化输出的实现（[票 37](37-grilling-structured-output-slot.md)：IR 槽位 + 三条出站映射 + 能力位与自检探针）；
   - `cache_write` 的采集（[票 33](33-grilling-cost-accounting.md)：库无该列）；
   - Claude Code 链路（spec §8 的写入硬约束 + [票 24](24-research-client-login-state.md)，原型只验过 Desktop 与 Codex）；
   - 前端七页（spec §10 已定信息架构，原型只有一个页面）。
3. **每一步的验收形态**：沿原型那套「真实客户端端到端」，还是一律吃 [票 19](19-grilling-test-strategy.md) / ADR-0005 的三层网（`check.ps1` 的 `-Quick` / `-Replay` / `-Live` / `-Smoke`）？
4. **原型代码怎么处置**：`prototype/` 是原样演进成正式 crate，还是另起骨架、只搬走 IR 与三个编解码 + `replay.rs` + `check.ps1`？

## 背景

- spec §3（模块划分已定：ingress / IR / binding / egress / 旁路 writer）与 §11（原型范围、进度、未做项）。
- 地图 Out of scope 里那两条（配置写入引擎、客户端档案覆盖）明确「归实现阶段」，但没说插在哪一步。
- [票 32](32-prototype-codex-e2e.md) 已经证明原型代码可以被真实客户端验收，因此顺序问题不是「能不能验」，而是「先验什么」。
- 结论要回写 spec：新增一节实施顺序（或把 §11 拆成「原型」与「实施顺序」两节）。

## Answer

2026-10-15，四轮弹窗与用户定案。落地：spec §11 拆成「原型（已达成）」与「12. 实施顺序（里程碑）」两节；骨架与搬运这条硬决策归档为 [ADR-0007](../../../docs/adr/0007-skeleton-and-implementation-order.md)。

### 1. 里程碑四段，走到 M2 收口即可开始日常用

四段：**M1 地基 → M2 日常可用 → M3 保真长尾 → M4 分发收尾**。「可以开始拿它日常用」这条线画在 **M2 收口**，不在功能全齐之后。

- **M1 地基**（规模 M，最大不确定性 = Tauri 2 外壳与 axum 同进程共存的形态：托盘、单实例、关窗隐藏、网关生命周期由谁管）。
  交付：cargo workspace 立起来（`crates/gateway-core` lib + `src-tauri` bin + `app/` React）、核心搬迁（见第 4 条）、SQLite 按 spec §9 建表并带迁移、密钥 DPAPI 存取、管理 API 骨架（`/api/status`）、单实例互斥量、端口占用明确报错、GitHub Actions 门禁。
  收口验收：`check.ps1` 在新骨架里全绿（单元测试与回放格数不减）；应用能启动、托盘常驻、窗口能开关、关窗按票 18 隐藏到托盘；库与密钥能读写；端口被占时报错清楚。**此段结束仍不能日常用**。
- **M2 日常可用**（规模 L，最大不确定性 = Claude Code 的写入与还原在真实客户端上的行为）。
  交付：配置写入引擎（票 28 全 21 条，含票 30 第 8 格那几条断言）、三个客户端的写入与还原（Desktop 含 1P↔3P 一键切换；Claude Code 只写 `ANTHROPIC_AUTH_TOKEN`；Codex 含模型目录与逐行认领）、客户端档案覆盖（票 10）、供应商表三层校验（票 23）、模型发现与三种结局的降级（票 35）、models.dev 随包快照与手工价（票 33 的价格优先级）、上游强制自检、前端四页（概览 / 上游 / 模型与映射 / 客户端）、用量与日志基础、NSIS per-user 安装包。
  收口验收：三个真实客户端端到端各走一遍（Desktop 3P 四家族名 + 流式 + 切回 1P；Claude Code；Codex CLI 与桌面应用）；写入引擎与票 30 第 8 格全绿；NSIS 装得上、能跑；一键还原回官方态；用户当场三个客户端各问一句并签字。
- **M3 保真长尾**（规模 M，最大不确定性 = 结构化输出的敌意探针判定）。
  交付：Ollama 兼容面四个端点（票 34 / 36）、结构化输出实现（票 37：IR 槽位 + 三条出站映射 + 能力位 + 探针）、`cache_write` 采集与计价（票 33）、用量与成本页完整粒度、日志页四段与诊断包、其余三页。
  收口验收：`check.ps1` 全绿（含 Ollama 面回放断言）；结构化输出探针在两条真实上游实测并给出能力位结论；用量数字与上游 usage 对得上。
- **M4 分发收尾**（规模 S，最大不确定性 = updater 签名与 passive 静默安装在本机的行为）。
  交付：自动升级闭环（公开 GitHub Releases + `latest.json` + 签名校验）、开机自启开关、卸载保留数据并询问是否还原、免安装 zip、导出 / 导入与版本回退指引。
  收口验收：干净机器上装 → 升一版 → 卸载（选还原）全流程走通。

### 2. 欠账插位（按四段，不提前）

- **M1**：核心搬迁 + 回归基建搬入 + SQLite 建表 + DPAPI + 管理 API 骨架 + 单实例 / 端口检测。无功能欠账。
- **M2**：配置写入引擎 21 条（含票 30 第 8 格断言）、客户端档案覆盖（票 10）、上游自检、模型发现降级（票 35）、models.dev 与手工价（票 33）、Claude Code 链路（票 24 的两条写入硬约束）、前端四页、NSIS 打包。
- **M3**：Ollama 面（票 34 / 36）、结构化输出（票 37）、`cache_write` 采集（票 33）、用量页完整粒度、日志四段与诊断包、其余三页。
- **M4**：自动升级（票 18）、自启动、卸载还原询问、免安装 zip、导出 / 导入与回退。

被明确否掉的两个提前量：写入引擎不提前到 M1（否则 M1 变胖、地基与功能同期做）；Ollama 面与结构化输出不提前到 M2（三个客户端都不用它，提上来只会拖晚「日常可用」）。

### 3. 每一步的验收形态

三层网当**门禁**，真实客户端端到端只在**里程碑收口**做：日常改动必跑 `check.ps1 -Quick` + `-Replay`；`-Live` 与三个真实客户端端到端只在 M2、M4 收口时人工签字。不选「每个里程碑都跑三客户端」（每轮都要人工开三个客户端，成本高、易走过场），也不选「端到端整条只做一次」（写入引擎与客户端档案这类与真实客户端强耦合的改动会没有真实验收）。

### 4. 原型代码处置与骨架形状

**另起正牌骨架，不原地演进。** 形状：cargo workspace —— `crates/gateway-core`（lib：ingress / ir / binding / egress / obs / store / writer，无 Tauri 依赖，回放测试裸跑）+ `src-tauri`（Tauri 2 外壳 + 管理 API + 托盘，bin）+ `app/`（React / Vite）。搬迁时**一次性对齐 spec §3 的模块边界**，不留二次搬迁；三个编解码文件各自保持完整（`anthropic.rs` / `openai.rs` / `responses.rs` 各含入站与出站，不硬拆成两半）。

随行走的：`ir.rs` / `sse.rs` / `anthropic.rs` / `openai.rs` / `responses.rs` / `provider.rs` / `upstream.rs`（共约 3500 行），外加整套回归基建 `mock.rs` / `replay.rs` / `fixtures/` / `tests/` / `check.ps1` / `record.ps1` —— ADR-0005 定的门禁就是它，**不重写**。
不搬的：`main.rs` 的路由与硬编码路由表（原型把它与 mock 挤在同一个 bin、同一个 `App` 里）、`desktop-3p.ps1` / `codex-gateway.ps1`（由正牌写入引擎取代；两份脚本留作行为参照与回归参考）。
`prototype/` 目录**原地冻结不删**，当参照物。

顺带在 M1 修三笔原型技术债：`store` 去掉「每行单开连接 + 同步写库阻塞 runtime」（改单写线程或连接池）；mock / echo / `/__chunks` / `/__bodylog` 这类原型件与网关分进程隔离，不进产品路径；crate 拆成 lib + app（即上面的骨架）。其余欠账按第 2 条插位，不另列清单。

### 5. 版本库与 CI

仓库根 `git init`，产品代码与规划（`.scratch/`）同库，`vendor/` 与 `target/` 已在 `.gitignore`；票 18 的 GitHub Releases 与升级链路从这个仓发。CI **M1 就上**：推送与 PR 跑 `check.ps1 -Quick -Replay`（Windows runner），打 tag 才产出 NSIS + zip + `latest.json`（票 18 的发布前置：updater 私钥进 GitHub Secret、公钥进配置，私钥另交用户保管）。票 19 §86 的「暂不上 CI」只针对当时的测试策略，此处按用户 2026-10-15 的决定覆盖——门禁仍以本地为准，CI 是第二道。
