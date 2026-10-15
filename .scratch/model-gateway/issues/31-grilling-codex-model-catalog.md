# Codex 模型目录：内容、并存与还原

Type: grilling
Status: resolved
Blocked by: —

## Question

票 25 实测定案：网关必须往 `~/.codex/config.toml` 写 `model_catalog_json`（指向一个模型目录文件），否则 Codex（CLI 与桌面应用）的模型选择器看不到网关模型。而**一旦写了目录，Codex 只认目录文件里的模型**，不再刷新自己的内置列表。

要定：

1. 目录里放哪些模型？四个家族名是必放；每客户端覆盖的模型、普通虚拟模型、上游真实模型名要不要也放？
2. 每条模型的字段怎么填（`display_name`、`description`、`default_reasoning_level` / `supported_reasoning_levels`、`visibility`、上下文窗口）？**官方模型条目要不要一并抄进目录**（不抄就会从选择器里消失，cc-switch 踩过这个坑）？
3. 目录文件放哪、叫什么、写相对路径还是绝对路径？一键还原时怎么处理（删指针 / 删文件 / 留备份）？
4. 与别人写的目录并存：`~/.codex/config.toml` 里已有 cc-switch 之类工具写的 `model_catalog_json` 时，网关认领 / 让位 / 合并？
5. 脏数据护栏：目录写坏、为空或缺少 `visibility=list` 的条目时，Codex 会把官方模型全藏起来——网关要不要在写入前自检、并在还原时兜底？

## 背景

- 已解析：[票 25](25-grilling-codex-desktop-card-split.md)（实测定案与证据）、[票 23](23-design-provider-table-granularity.md)（虚拟模型与 wire 的粒度）、[票 10](10-design-model-exposure-and-client-config.md)（配置自动写入 + 首写备份 + 幂等重写 + 一键还原）。
- spec §7 已写「Codex 侧需要模型目录」（`spec.md:188`），本票定它的形状与并存规则。
- 证据：`research/codex-desktop-model-picker-gating.md`（目录格式 `{"models":[…]}`、模板取自 `codex debug models`、两个模式都生效）。
- 参考实现与它的坑：`vendor/cc-switch/src-tauri/src/services/provider/codex_official_models.rs`（官方模型必须写全、来源与版本核对）、`vendor/cc-switch/docs/user-manual/zh/2-providers/2.1-add.md:502-505`（改目录后必须重启 Codex）。

## Answer

（2026-10-15，grilling 两轮，8 条定案 + 4 项事实）

### 1. 目录里放哪些模型：只放网关暴露的虚拟模型

四个家族名（`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`）+ 用户自建的普通虚拟模型；给 Codex 做过客户端档案覆盖时用**覆盖后的名字**。**不放**上游真实模型名，也不放官方模型条目。目录内容 = 网关给 Codex 的那份 `/v1/models` 清单，单一来源。

### 2. 官方模型条目：不抄

不抄的理由：网关模式下 `model_provider` 指向网关，官方条目即使留在选择器里，选中也会发到网关（**未实测，票 32 可验**）；摘键还原后官方模型自然回来。与 cc-switch 第三方 provider 的行为一致（它的第三方目录只装那家的模型，官方行只在「模型名撞上官方名」时照搬）。

- 已知后果（认知即可）：网关模式期间 Codex 选择器里没有官方模型。
- 若票 32 实测发现官方模型在网关模式下仍能用（桌面应用按模式自己选后端），回来重开这条。

### 3. 文件位置 / 指针 / 还原

- 文件：`<CODEX_HOME>/claudegateway-model-catalog.json`（本机 = `%USERPROFILE%\.codex\claudegateway-model-catalog.json`）。
- 指针：`model_catalog_json = "claudegateway-model-catalog.json"`，**相对文件名**——Codex 按 `CODEX_HOME` 解析（cc-switch 同一写法）。
- 还原（摘键还原）：摘掉 `model_catalog_json`；**仅当文件内容与网关最后一次写入逐字节一致**时才删文件；首写快照与滚动备份照票 28。

### 4. 每行的字段：模板默认，不做可配

`display_name` = 虚拟模型名；`description` 注明来自网关；`visibility = "list"`；`context_window` / `max_context_window` = 262144；`effective_context_window_percent` = 95；`input_modalities` = `["text","image"]`；推理档位沿用所用模板。不做「上下文窗口 / 思考等级」用户可配列。

### 5. 模板按出站协议分

- 上游 `anthropic` → cc-switch 的**中性 native-responses 模板**：`shell_type = "shell_command"`、无 freeform `apply_patch` / `web_search`（避免上游拒收 `type=="custom"` 工具）。
- 上游 `openai-chat` → cc-switch 的**经典 gpt-5.5 工具模板**：freeform `apply_patch` + `unified_exec`，带 GPT-5 harness 的 `base_instructions`。
- 两套模板都随包内置；第 4 条与模板冲突的字段以模板为准。

### 6. 与别人写的目录并存：默认冲突即停，另有显式「认领」

`model_catalog_json` 是网关接管键。磁盘上的值不是网关上回写的（或没有记录）→ 按票 28 判冲突：**放弃写入 + 提示**。冲突弹窗里另给一个**显式的「认领并覆盖」**动作（用户点确认才改指针）。**不合并**：把别人的模型读进来一起写会让两边互相顶掉对方的文件；别人那份目录文件不删、不改。

### 7. 脏数据护栏：写前自检 + 还原兜底

写入前自检，任一项不过就**不写**并报错（指针不动）：JSON 可解析、`models` 非空、至少一条 `visibility = "list"`、`slug` 非空且唯一、解析必需字段齐。还原时兜底：发现目录文件缺失 / 空 / 无 `list` 条目，照常摘指针并提示，不把坏目录留给用户。

### 8. 顺序与默认模型：固定顺序，不做默认模型设置

四个家族名在前（fable / opus / sonnet / haiku），普通虚拟模型按名字排序。cc-switch 实测：`config.toml` 没写顶层 `model` 时 Codex 拿目录第一条当默认模型；**顶层 `model` 写不写归票 32** 的接管键清单。

### 生效方式

写完只提示「**完全退出并重启 Codex**」（`model_catalog_json` 只在启动时读）；网关不杀 Codex 进程（桌面应用杀不干净，cc-switch 也只重启自家 daemon）。写入本身照票 28：接管键判冲突、原子替换、语义幂等。

### 事实（本次查证）

1. 本机 Codex CLI `0.162.1`：`codex debug models` 与 `codex debug models --bundled` 输出**逐字节一致**——11 条（8 `list` / 3 `hide`），**658,608 字节**，每条带 `base_instructions` / `model_messages`。
2. 目录解析必需字段（cc-switch 实测 + 文档）：`slug` + `base_instructions`（或 `model_messages.instructions_template`）+ `supports_reasoning_summaries` + `supports_parallel_tool_calls`；缺一条 Codex 启动即拒整份目录。
3. cc-switch 的指针是相对文件名（`cc-switch-model-catalog.json`，落在 `CODEX_HOME`），第三方 provider 的目录只装自家模型。
4. cc-switch 备有两套模板：`src/resources/codex_native_responses_template.json`（1099 字节，中性）与 `src/resources/gpt5_5_template.json`（47075 字节，经典）。

### 不改的既有定案

`/v1/models` 不按客户端过滤（票 16 §8.2）；`wire_api` 只能 `responses`；不写 `requires_openai_auth = true`；绝不碰 `auth.json`（票 25）。

### 交给票 32 的

顶层 `model` 与 `model_provider` / `model_providers.<id>` 的接管键清单；官方模型在网关模式下选中是否真失败（第 2 条未实测项）；Codex CLI（`codex exec`）与桌面应用的端到端验收。
