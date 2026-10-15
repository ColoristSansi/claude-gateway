# 正牌骨架另起（core lib + Tauri 外壳 + React），回归基建随行走，里程碑以 M2 收口为「日常可用」

`.scratch/model-gateway/prototype/` 是**一次性原型**：单 bin、无 `lib.rs`、无 workspace，网关与 mock 上游挤在同一个 bin 里（`src/main.rs` 起两个 `axum::serve`，`App` 里放着 `echo` / `chunk_scheme` 这类只服务测试的字段），路由表与四个家族槽位硬编码在 `main.rs:160-319`，配置写入是最小版 `.ps1`（`desktop-3p.ps1` 三处已被 [ADR-0006](0006-config-writer-owned-keys.md) 否决）。它证明了协议转换、SSE 逐帧转发、三个客户端接入与两个真实上游都能跑通，但**它的形状不是产品形状**：mock 与回环端点会进产品路径，测试要连同一个 bin 一起编，模块边界与 spec §3 的划分（ingress / IR / binding / egress / 旁路 writer / obs）不一致。

同时 spec §11 里那条「实施顺序」描述的是**已经做完的原型**（mock 上游回归 → Anthropic 链路 → Desktop 3P 端到端 → 再补 OpenAI Chat / Responses 与另外两个客户端），从原型到正牌 Tauri 应用之间没有顺序、里程碑与逐步验收；多笔「归实现阶段」的欠账（配置写入引擎 21 条、客户端档案覆盖、Ollama 兼容面、结构化输出、`cache_write` 采集、Claude Code 链路、前端七页）也没有落点。[票 38](../../.scratch/model-gateway/issues/38-grilling-implementation-order.md) 定案如下。

决定：

1. **另起正牌骨架，不原地演进。** cargo workspace：`crates/gateway-core`（lib：ingress / ir / binding / egress / obs / store / writer）**不依赖 Tauri**，回放测试裸跑；`src-tauri`（Tauri 2 外壳 + 管理 API + 托盘）是唯一的 bin；`app/`（React + Vite）是前端。
2. **搬迁一次性对齐 spec §3 的模块边界**，不留二次搬迁；三个编解码文件各自保持完整（`anthropic.rs` / `openai.rs` / `responses.rs` 各同时含入站与出站，不硬拆成两半）。随行走 `ir.rs` / `sse.rs` / `anthropic.rs` / `openai.rs` / `responses.rs` / `provider.rs` / `upstream.rs` 与整套回归基建（`mock.rs` / `replay.rs` / `fixtures/` / `tests/` / `check.ps1` / `record.ps1`）；不搬 `main.rs` 的路由与硬编码表，不搬两个 `.ps1` 写入器（留作行为参照）；`prototype/` 原地冻结不删。
3. **里程碑四段：M1 地基 → M2 日常可用 → M3 保真长尾 → M4 分发收尾。**「可以开始拿它日常用」这条线画在 **M2 收口**（三个真实客户端端到端 + 写入引擎落地 + 四页可用 + NSIS 装得上），不在功能全齐之后。
4. **欠账插位**：M1 = 搬迁 + 建表 + DPAPI + 管理 API 骨架 + 单实例与端口检测；M2 = 配置写入引擎 21 条（含票 30 第 8 格断言）+ 客户端档案覆盖 + 供应商表三层校验 + 模型发现降级 + models.dev 快照与手工价 + 上游自检 + Claude Code 链路 + 前端四页 + 用量日志基础 + NSIS；M3 = Ollama 面 + 结构化输出实现 + `cache_write` 采集 + 用量页完整粒度 + 日志四段与诊断包 + 其余三页；M4 = 自动升级 + 自启动 + 卸载还原询问 + 免安装 zip + 导出导入与回退。**写入引擎不提前到 M1，Ollama 面与结构化输出不提前到 M2。**
5. **验收形态**：三层网（Rust 单测 / 黄金 fixture 回放 / 真实客户端半自动冒烟）当**门禁**，日常改动必跑 `check.ps1 -Quick` + `-Replay`；真实客户端端到端只在 **M2、M4 收口**时人工签字。
6. **M1 顺手修三笔原型技术债**：`store` 去掉「每行单开连接 + 同步写库阻塞 runtime」；mock / echo / `/__chunks` / `/__bodylog` 与网关分进程隔离，不进产品路径；crate 拆成 lib + app。
7. **仓库根 `git init`，产品代码与规划同库**（`vendor/` 与 `target/` 已在 `.gitignore`）；**CI 在 M1 就上** GitHub Actions：推送与 PR 跑 `check.ps1 -Quick -Replay`，打 tag 才产出 NSIS + zip + `latest.json`（updater 私钥进 GitHub Secret、公钥进配置、私钥另交用户保管）。
8. **结论落点**：spec §11 拆成「原型（已达成）」与「12. 实施顺序（里程碑）」两节；本 ADR 记骨架与顺序这两条硬决策。

理由：把「哪些代码是产品、哪些是脚手架」在搬的那一刻划清，比先照搬再剔便宜——原型里 mock 与网关同 bin、测试与产品同 crate 这两处，越晚拆越贵；而回归基建（fixtures 与黄金回放）是 [ADR-0005](0005-protocol-conversion-regression-strategy.md) 定的门禁，重写等于重录 15 格黄金并重新论证语义，收益为零。里程碑以「三个客户端都能用」而不是「功能全齐」为日常可用的界线，是因为剩余项（Ollama 面、结构化输出、`cache_write`）三个目标客户端一个都不用，让它们挡住日常使用只会把风险留在后面。CI 上在 M1 而不是发布前，是因为构建链路本身就是一条要在 M4 用的验收路径。

**Status**: accepted（2026-10-15，由 [票 38](../../.scratch/model-gateway/issues/38-grilling-implementation-order.md) 四轮弹窗定案；规则同步进 spec §11 / §12）

**Considered Options**:

- **原地演进**（prototype 目录直接长成正牌 crate）：历史连续、不用搬，但原型专用的双 Router、`App` 里的测试字段与平铺模块会先混进产品结构再剔。否决。
- **另起骨架但连回归基建也重写**（只搬 IR 与三个编解码）：丢掉 15 格结构级黄金与 quirk 双向守卫，与 ADR-0005 相抵。否决。
- **先照搬原型结构，模块边界拆分推到 M3**：同一份代码搬两次。否决。
- **配置写入引擎提前到 M1**：能最早拿到端到端，但 M1 变胖、地基与功能同期做。否决，引擎留在 M2。
- **Ollama 面与结构化输出提到 M2**：一次把入站协议面补齐，但三个目标客户端都不用它们，只会把「日常可用」推晚。否决，留 M3。
- **每个里程碑都跑三个真实客户端端到端**：客户端侧回归发现更早，但每轮都要人工开三个客户端，成本高、易走过场。否决；改为只在 M2 / M4 收口。
- **只靠 `check.ps1`、整条链路端到端只做一次**：配置写入引擎与客户端档案覆盖这类与真实客户端强耦合的改动会没有真实验收。否决。
- **暂不上 CI（票 19 §86 的原判）／只在打 tag 时上 CI**：省配置，但回归可能忘跑，且到打包才知道构建链路能不能重现。否决；票 19 那句只针对当时的测试策略。
- **另开新仓只放产品代码**：版本、Issue、Release 分家，规划与代码的引用会断。否决。
- **暂不建 git，M4 再建**：M1–M3 没有提交历史与回退点。否决。

**Consequences**: 实现期第一件事是搭 workspace 与 CI，而不是继续往原型里加功能；`prototype/` 从此冻结，改行为只发生在正牌骨架里（要复现原型那条链路就跑冻结目录）。`crates/gateway-core` 不依赖 Tauri 这条约束要在评审里守住，否则回放测试会重新被 WebView 依赖拖住。M2 是四段里唯一的大块（L），三客户端端到端与写入引擎 21 条同时收口；M3 的 Ollama 面与结构化输出在 M2 期间不写代码，但 spec 的契约已经定死（§4.4 / §5），实现时不再改判据。票 30 第 8 格（配置写入引擎断言）与「客户端档案覆盖」两笔 Out of scope 欠账随 M2 一起销账。
