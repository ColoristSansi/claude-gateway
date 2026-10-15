# Codex 写入器与模型目录

Type: task
Status: resolved
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

## Answer

**过了**（2026-10-15）。落点 `crates/gateway-core/src/writer/codex.rs` + `codex_tests.rs`（分支 `task/48-writer-codex-and-catalog`）。
引擎（票 45）**一行没改**：调用约定照票 45 的 Answer——`FileSpec` / `OwnedOp` / `WritePlan` / `WriteRoots` 数据 + `Writer::{write_one,restore,rollback}`；
Codex 专属、引擎表达不了的两件小事（哨兵注释行、目录文件的「逐字节一致才删」）在本模块里做，见下「裁决」。

### 做了什么

| 文件 | 内容 |
| --- | --- |
| `writer/codex.rs` | 接管键清单 + 模型目录生成/自检 + 哨兵逐行认领 + `CodexWriter`（写 / 认领覆盖 / 还原 / 回滚）+ `home_from`（写入根参数化） |
| `writer/codex_tests.rs` | 17 条单测（15 常跑 + 2 条 `#[ignore]` 真机走查） |
| `writer/mod.rs` | 挂上 `pub mod codex;` + `#[cfg(test)] mod codex_tests;` |

写入根 = `WriteRoots.config_root`（`$CODEX_HOME`，没设就 `%USERPROFILE%\.codex`；`home_from` 是纯函数，测试全指临时目录），备份 / 日志挂网关数据目录。`config.toml` 与目录文件都在同一个 `config_root` 下（`FileSpec::order`：目录 0、config 1，先被引用者后指针）。

**接管键（逐字，写进 TOML 的只有这些）**
顶层 `model_provider = "claudegateway"`、`model = "<调用方给的默认槽位>"`、`model_catalog_json = "claudegateway-model-catalog.json"`；
`[model_providers.claudegateway]` 的 `name = "Claude Gateway"`、`base_url = "<BaseUrl>/v1"`、`wire_api = "responses"`、`http_headers = { X-Claude-Gateway-Client = "codex" }`。
**不写** `requires_openai_auth`、**不写** `env_key`、**绝不碰** `auth.json`（票 25 第 4 条；走查前后 `auth.json` 都不存在，断言过）。
还原 = 摘这四个顶层键 + `RemoveIfEmpty(["model_providers","claudegateway"])`（后者连引擎顺手建的空壳父表 `[model_providers]` 一起摘，别人的 provider 还在就不摘）。

**模型目录（逐字段取值）**
文件 `<CODEX_HOME>/claudegateway-model-catalog.json`，形状 `{"models":[...]}`；指针写**相对文件名**；只装网关暴露的虚拟模型，**不抄官方条目**。
行模板 = 中性模板（源头：cc-switch `codex_native_responses_template.json`，`shell_type = "shell_command"`、无 freeform `apply_patch`），逐字段：
`slug` / `display_name` = 虚拟模型名；`description` = `Claude Gateway -> <upstream_id> / <real_model>`；`base_instructions` = 模板原文；
`default_reasoning_level = "high"`；`supported_reasoning_levels` = `[none(Disable Thinking), high(Enabled Thinking)]`；`visibility = "list"`；`supported_in_api = true`；
`priority = 1000 + index`；`supports_reasoning_summaries = true`；`default_reasoning_summary = "none"`；`support_verbosity = false`；
`truncation_policy = {mode="bytes", limit=10000}`；`supports_parallel_tool_calls = false`；`supports_image_detail_original = false`；
`context_window` / `max_context_window` = **262144**；`effective_context_window_percent = 95`；`experimental_supported_tools = []`；
`input_modalities = ["text","image"]`；`supports_search_tool = false`。
顺序 = 四家族名（fable / opus / sonnet / haiku）+ 普通虚拟模型按名排（`is_family` 不参与排序，判据就是名字）。
写前自检 `check_catalog`：可解析 / `models` 非空 / 至少一条 `visibility=list` / `slug` 非空且唯一 /（`base_instructions` 或 `model_messages.instructions_template`）+ `supports_reasoning_summaries` + `supports_parallel_tool_calls` 齐；不过就 `Err`，**两个文件都一个字节不写**（自检在拼计划时跑，序列化→再解析一遍）。

### 裁决 1（票面矛盾 ②）：`env_key` 不写

spec §8 表与票 24 都要求「`env_key` 声明的环境变量必须真存在」，票 24 的 B/C 组实测是**硬失败**：`env_key = "PROBE_KEY"` 而变量没设 → `[XX] active model provider auth env var is missing`，`OPENAI_API_KEY` 顶替不了，Codex 直接拒发。
本票**选择不写 `env_key`**（同原型 `codex-gateway.ps1:95-96` 的刻意做法），依据：

1. 网关入站不鉴权（spec §8.3：三个客户端都不需要官方账号登录；原型端到端就是不带 `env_key` 跑通的，票 32 §2）；
2. 「真去创建那个环境变量」要求网关写 `HKCU\Environment` + `WM_SETTINGCHANGE` 广播，而**已在跑的 Codex 看不到新变量**（环境块在进程创建时继承），仍会硬失败；还得在还原 / 卸载时配对删掉，副作用面大；
3. 不带 `env_key` 时 Codex 走的是免登录分支——本机实测 `codex doctor --summary` 报 `✓ auth OpenAI auth is not required for the active model provider`（见验收 2）。

`requires_openai_auth` 也不写（写了会强制要 Codex 账号凭据，票 24 D 组）。
将来网关要鉴权时的两条正路，留给 49/51：**① 改用 `experimental_bearer_token`（内联，票 24 F 组实测可用）；② 写 `env_key` 就必须由网关真去设那个用户级环境变量**（建议名 `CLAUDEGATEWAY_API_KEY`，切换时设、还原/卸载时删，并要求用户重启客户端）。本票不越界做。
`http_headers` 按票面写，值是常量 `{ "X-Claude-Gateway-Client" = "codex" }`：静态、网关目前不读，但无害且给「按客户端记账 / 日志」留了抓手；Codex 实测接受（config loaded）。

### 裁决 2：哨兵行怎么落地（引擎没有「写注释行」这个动作）

引擎的 `OwnedOp` 只有键级动作，写不了注释行；但票 48:9 / 票 32:55 明确要两对哨兵行、且「按区域删」被实测否决。
做法：引擎写完 / 还原之后，`codex.rs` 自己按**逐行**整理那四行（`mark_managed` / `strip_managed`，保留原文件的 BOM / 行尾 / 末尾换行），用引擎的 `atomic::replace` 写回；
**只增删这四行注释，插进哨兵之间的别人的表原样留在原地**。幂等（摘了再插，位置由当前接管键决定），且注释行不参与 `client_config_state` 的接管键值（引擎刚备份、刚记账，这里不再单留备份）。
票 32 的那一幕做成了回归：`restore_keeps_a_table_inserted_between_the_provider_sentinels`。

### 裁决 3：目录文件「逐字节一致才删」

引擎的删除判据是「网关新建 且 摘键后文件空了」（只剩网关内容），比票 31:45 的「与网关最后写入**逐字节**一致」松。
做法：还原前 `catalog_drift()` 用**当前文件的排版保真 + `client_config_state` 记下的 `models` 值**重新序列化一遍，与磁盘**比字节**：
一致（或文件不在）→ 交给引擎摘键，引擎按上述规则删文件；不一致 / 没有记录 → **目录文件一个字节不动**，在报告里给一条 `FileOutcome::Conflict{models, recorded, current}` 让界面提示（票 48:19 的「放弃并提示」）。
`config.toml` 那一半照常摘键（票 31:63 的「发现目录坏了也照常摘指针、不把坏目录留给用户」）。

### 裁决 4：留白处的取值

`priority = 1000 + index`（票 31 未定，沿用原型）；顶层 `model` 由调用方给（票 51 的默认槽位，本票走查用 `claude-sonnet`）；
provider 表键顺序 = `name` / `base_url` / `wire_api` / `http_headers`（票 48:15 的顺序）；`http_headers` 值见裁决 1。
另一处与票 45 Answer 遗留 8 的呼应：这些键全是 `OwnedOp` 数据，引擎一行没动。

### 验收（真实命令与输出结论）

**1）引擎级回归（票面「引擎级回归」三条 + 其余）** — `cargo test -p gateway-core --lib writer::codex`（15 passed / 0 failed / 2 ignored）：

| 要求 | 用例 | 结论 |
| --- | --- | --- |
| 外部往哨兵之间插一张表 → 摘除后回到原样且保住那张表 | `restore_keeps_a_table_inserted_between_the_provider_sentinels` | 写完之后在 `http_headers` 行后插 `[plugins."computer-use@openai-bundled"] / enabled = true`（票 32 实测的写法）→ 还原后 `claudegateway` / `model_provider` / 哨兵行零残留，别人的表仍在，字节 `= REALISH + 插入的两行` |
| 重写幂等 | `rewrite_is_idempotent_and_keeps_the_sentinels` | 第二次 `write` 全 `Unchanged`、字节相同、**mtime 不变**；`mark_managed(mark_managed(x)) == mark_managed(x)`、`strip_managed(mark_managed(x)) == x` |
| 目录写前自检挡住坏输入 | `catalog_self_check_rejects_bad_input`（7 种坏输入：models 非数组 / 空 / 全 hide / slug 空 / slug 重复 / 缺 `supports_parallel_tool_calls` / 缺 `base_instructions`）、`catalog_self_check_blocks_the_whole_write`（重复 slug → `Err`，`config.toml` 一个字节没动、目录文件没造出来） | 全过 |
| 还原逐字节 / 目录删除判据 | `restore_returns_the_file_to_its_bytes_when_nobody_else_wrote`、`catalog_is_deleted_on_restore_only_while_it_is_still_the_gateway_bytes`（① 没被改过 → 删；② 只换了排版、语义相同 → 留着不动 + 报告 Conflict） | 全过 |
| 冲突放弃 + 显式认领 | `conflict_gives_up_and_the_explicit_claim_takes_over` | 手改 `model` → `Conflict`、文件不动；`claim()` → 覆盖成功且留了快照 |
| 首写 / 别人家的表 / 回滚 / auth.json | `fresh_install_creates_both_files_then_removes_them_on_restore`（首写 marker-only、还原整个删）、`another_provider_in_the_same_parent_table_survives_restore`（`[model_providers.other]` 与父表都留住、字节回原样）、`rollback_puts_the_first_snapshot_back_byte_for_byte`、`write_owns_the_codex_keys_and_never_writes_env_key_or_auth_json`（`auth.json` 逐字节不变、TOML 里没有 `env_key` / `requires_openai_auth`） | 全过 |
| 写入根参数化 | `home_honors_codex_home_and_falls_back_to_userprofile`（空白值当没设；两个都没有就报错） | 全过 |

**2）真实 `~/.codex` 配置层走查（Codex CLI 0.162.1 在位）** — 机器已回到原状：

```
# 动手前
sha256(config.toml) = a70e7321f31009d0ef871a66bb4209ef41903db096c0533dfe4a6209b3bafb98   （2660 B，40 个顶层项，6976 个文件；无 auth.json、无目录文件）

$env:CG_CODEX_HOME = "$env:USERPROFILE\.codex"
cargo test -p gateway-core --lib codex_live_write -- --ignored --nocapture
  --- 写入后的 config.toml ---
  notify = [ ... ]
  # >>> claudegateway managed (top-level) >>>
  model_provider = "claudegateway"
  model = "claude-sonnet"
  model_catalog_json = "claudegateway-model-catalog.json"
  # <<< claudegateway managed (top-level) <<<
  [desktop] ... [marketplaces.*] ... [plugins.*] ... [mcp_servers.*] ... [windows] ...   ← 一个字节没动
  # >>> claudegateway managed (provider) >>>
  [model_providers]
  [model_providers.claudegateway]
  name = "Claude Gateway"
  base_url = "http://127.0.0.1:8787/v1"
  wire_api = "responses"
  http_headers = { X-Claude-Gateway-Client = "codex" }
  # <<< claudegateway managed (provider) <<<

  codex debug models → status=Some(0) bytes=9499 条目数=6 四家族名在目录里：true true true true
  ✓ config       loaded                                     ← Codex 读我们的配置不报错
  ✓ auth         OpenAI auth is not required for the active model provider
  真机走查通过：C:\Users\ZHANQUAN\.codex
  test result: ok. 1 passed

# 还原后
sha256(config.toml) = a70e7321f31009d0ef871a66bb4209ef41903db096c0533dfe4a6209b3bafb98   （与动手前相同）
diff 动手前 还原后  →  0 行
目录文件 claudegateway-model-catalog.json → 已删（不存在）；auth.json → 仍不存在
```

`codex debug models` 那 9499 B / 6 条 = 我们目录里的 6 条（不是内置的 11 条 / 658 KB），四家族名逐字可见——这是「选择器显示四个家族名」在配置 / 目录层的可验部分。
**机器原状的例外（都不是写入器干的）**：`codex doctor` 自己打开了状态库，留下 4 个 sqlite sidecar 文件（`goals_1` / `memories_1` 的 `-wal` / `-shm`）并把若干 `-shm` 的 mtime 刷新；`config.toml` 的 mtime 变成还原时刻（**内容逐字节相同**）。写入器只碰 `config.toml` 与目录文件这两个路径，备份 / 日志 / 状态库都在临时目录。原型遗留的 `config.toml.bak-claudegateway` 没碰。

**3）留给票 55 现场验收（需要可用绑定与上游）**

前置：票 49 / 51 已给真实上游 + 凭据 + 四家族槽位绑定；网关跑在 `127.0.0.1:8787`；Codex 不需要官方登录。

1. 界面「客户端」页点 Codex 卡切换 → 走 `CodexWriter::from_home(store, base_url)` + `write(&routes, 默认槽位)`（默认模型名由 51 给；目录条目顺序 = 四家族名在前）。
2. **完全退出并重启 Codex**（`model_catalog_json` 只在启动时读；CLI 新开一个终端即可）。
3. `codex exec "hi"`：抬头 `provider: claudegateway` + 指定模型、答对，网关 `/api/logs` 有 `POST /v1/responses`（票 32 的判据）。
4. 桌面应用完全重启 → 选择器里看到四个家族名（票 25：只看目录文件，与登录无关）；发一条 → mock / 网关日志收到 `POST /v1/responses` 且按 SSE 回。云线程要官方登录才行（票 25 第 2 条），与本票无关。
5. 点「还原」→ 摘键 + 删目录文件（逐字节判据）+ 备份保留；再比一次 `~/.codex/config.toml` 与动手前逐字节相同。

**4）门禁** — `CARGO_TARGET_DIR=C:/Users/ZHANQUAN/AIGC/Project/cg-target/48 pwsh -File ./check.ps1 -Quick -Replay`（**换掉共享 target：票 46 实测共享会跑到别的 worktree 的测试二进制**）：

```
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
  claude_gateway: 5 passed
  gateway_core(lib): 85 passed; 0 failed; 2 ignored     ← 里面 15 条是 writer::codex_tests
  gateway_service: 4 passed
  doc-tests: 1 passed
=== clippy (mock 特性) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===  20 passed
=== replay-unit (回放器切法单测) ===                            5 passed
全部通过。
```

收工前 `git merge integration/model-gateway`（tip `0f394ae`，票 46/47/49/50）后**重跑同一门禁**：`unit` → `gateway_core(lib)` **107 passed / 3 ignored**（含本票 15 条）、`claude_gateway` 5、`gateway_service` 4、集成 `provider_table` / `discovery` 14 passed / 4 ignored（真实上游那条）、`doc-tests` 1；`replay` 20、`replay-unit` 5 —— **全绿**。
唯一冲突在 `writer/mod.rs` 的模块清单（本票的 `codex` / `codex_tests` vs 票 46 的 `desktop` / `desktop_tests`），两边都留。集成里票 46 顺手改了引擎 `doc.rs` 的 `json_ensure_array`（`SetElement` 自建数组），与本票用的键级 `Set` / `Remove` 无关，本票 15 条用例在合并后仍全过。

### 遗留与坑

1. **引擎工件：空的 `[model_providers]` 父表**。引擎 `toml_ensure` 建中间层时是非隐式表，所以写出来的文件多一行 `[model_providers]`（Codex 实测接受：`✓ config loaded` + `debug models` 6 条）。还原用 `RemoveIfEmpty` 把它一起摘掉，逐字节回原样有测试；哨兵区域把那行也圈进去。要让它消失得改引擎（给中间层设 implicit），本票不动。
2. **引擎的多文件计划在集成分支这一版「遇冲突还往下走」**（票 45 后续的 `cd52f97` 修了，但没并进 `e4de381`）。所以本模块**自己按 `order` 逐个调 `write_one`**：第一个冲突 / 失败即停，后面的文件一个字节不动——spec §8.4 第 6 条「不出现悬空指针」。若那条修复后并进集成，这段可以简化成一次 `Writer::write`，行为不变（单测已覆盖两种路径）。
3. **用户自己往我们的 provider 表里写 `requires_openai_auth = true`** 时本模块不动它（不在接管键清单里，清单外原样保留），但它会让 Codex 强制要账号凭据。要不要把它列成「摘除型接管键」是票面没定的判断，46/47 侧建议一致处理；本票按「只写自己那几个键」的最小口径做。
4. **`env_key` 的替代路**（见裁决 1）：要鉴权就用 `experimental_bearer_token` 或由网关真去设 `CLAUDEGATEWAY_API_KEY`。这两条都要动 49/51 的凭据面，本票只留判据。
5. **`http_headers` 目前是常量、网关不读**；按客户端记账 / 日志要用它时，值是现成的 `X-Claude-Gateway-Client: codex`。
6. **`priority = 1000 + index`、顶层 `model` 的值** 都是票面留白，取值与来源见裁决 4；若 51 想按别的口径排，只改 `catalog_value` 一处。
7. **真机走查是 `#[ignore]` 用例**（要显式设 `CG_CODEX_HOME` 才会碰真机）：`cargo test -p gateway-core --lib codex_live -- --ignored --nocapture`；`codex_live_restore_only` 是走查被打断时的回收工具。走查只证明「配置 / 目录层」，端到端（`codex exec` + 桌面选择器）归票 55。
8. **`codex doctor` 会在真实 `~/.codex` 留下 sqlite sidecar 与 mtime 变化**（见验收 2），票 55 复跑时同样如此；配置内容本身逐字节相同。
