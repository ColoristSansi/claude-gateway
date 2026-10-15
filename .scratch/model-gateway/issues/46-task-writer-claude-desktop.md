# Claude Desktop 写入器与 1P/3P 快切

Type: task
Status: resolved
Blocked by: 45

## Question

在写入引擎上做 Desktop 这一条：写 `%LOCALAPPDATA%\Claude-3p\configLibrary\` 的 profile JSON 与 `_meta.json`，覆盖键 = `inferenceProvider` / `inferenceGatewayBaseUrl` / `inferenceGatewayApiKey` / `inferenceGatewayAuthScheme` / `inferenceModels`；1P↔3P 一键切换。

原型的最小版三处被 ADR-0006 否决（profile 整份重建、`standard` 不维护 `_meta.json`、`restore` 删整个 configLibrary）——本票按引擎规则重做，`prototype/desktop-3p.ps1` 只当参照。

依据：spec §8 表 + §8.1 / §8.2 / §8.4、[票 15](../issues/15-prototype-desktop-3p-e2e.md)、[票 16](../issues/16-research-desktop-model-id-validation.md)、[票 28](../issues/28-grilling-config-writer-transactionality.md)。

## 交付

- profile JSON 写入：**baseUrl 不带 `/v1`**；`inferenceModels` 填完整模型 ID。
- `_meta.json` 维护：`entries` 增删网关条目、`appliedId` 指向 / 回退，**不动别的 profile**。
- 模型 ID 生成按 §8.2 判据（非 Anthropic 厂商黑名单 → 裸档位名或含 claude / 档位名 / anthropic 子串），四个家族名天然合法。
- 切回 1P = 摘 `inferenceProvider`（不还原备份）；「整份回滚」是第二选项。
- 还原：摘网关条目 + `_meta.json` 回退。

## 收口验收

- 真实客户端实测：切 3P → Desktop 菜单显示四个家族名、流式问答正常 → 切回 1P 后官方登录态完好。
- 还原后该 profile 回到首写前内容（别的 profile 一个字节不动）；网关当初新建且只剩网关内容的文件被整个删掉。
- 幂等：连切两次 3P，第二次不动文件。

## Answer

**Desktop 这一条落地了**（2026-10-15）：客户端逻辑在 `crates/gateway-core/src/writer/desktop.rs`（引擎一行没改语义，只修了一处 `SetElement` 的使能 bug，见「对引擎的唯一一处改动」）。

### 做了什么

| 文件 | 内容 |
| --- | --- |
| `writer/desktop.rs` | 接管键清单 → `FileSpec` / `WritePlan`；1P↔3P 快切；`_meta.json` 的 `entries`/`appliedId`；§8.2 模型 ID 判据；整份回滚第二选项 |
| `writer/desktop_tests.rs` | 13 条临时根用例 + 1 条 `#[ignore]` 的真实路径用例（`CG_DESKTOP_REAL_*` 环境变量三重门控） |
| `writer/mod.rs` | 挂上 `pub mod desktop;` + `mod desktop_tests;` |
| `writer/doc.rs` | 一行：`json_ensure_array` 把「缺失键建出来的空对象」也归一成空数组（原因见下文） |

调用约定（票 47/48 照抄形状；全是客户端数据，引擎不认键名）：

```rust
use gateway_core::writer::{desktop, Writer};

let roots = WriteRoots::with_data_dir(desktop::config_library_root().unwrap(), store::default_data_dir()?);
let writer = Writer::new(store.clone(), roots);
let cfg = desktop::DesktopConfig::new(base_url, api_key);          // baseUrl 自动去掉 /v1 与结尾 /
let dropped = desktop::filter_anthropic_models(&names).1;          // §8.2 预检（可选，票 16 §2 的逐条剔除）
let report = desktop::switch_to_3p(&writer, &cfg).await?;          // 写 profile → 写 _meta.json
let report = desktop::switch_to_1p(&writer, &cfg).await?;          // 摘键（不还原备份）
let outcomes = desktop::rollback(&writer, &cfg).await?;            // 第二选项：按首写快照整份回滚
writer.write(&desktop::plan(root, &cfg)?.claim()).await?;          // 冲突后「认领并覆盖」同引擎用法
```

`desktop::specs(config_root, cfg)` 每次现读 `_meta.json` 算两个**动态**还原判据（[`MetaRestore`]）：① `appliedId` 回退目标 = 摘掉网关条目后剩下的第一条 entry 的 id（**本机 pristine 1P 的 `_meta.json` 正是 `appliedId = <Default 的 id>`**，见 `configLibrary.backup-20261014-191819`，所以这就是能回到 1P 原状的值）；② 摘完 `entries` 会空就连 `entries` 键一起摘（网关新建的 `_meta.json` 才满足引擎的「只剩下网关内容 → 整个删」判据）。写侧与还原侧共用同一份 `write_ops`（标签一致 → 记录/回滚判据不被自己搅乱），只有 `restore_ops` 不同。

### 接管键清单最终版 + `inferenceCredentialKind` 的裁决

**最终版 6 个键**（`desktop::GATEWAY_KEYS`，顺序即写入与比对顺序）：`inferenceProvider`（`"gateway"`，激活动词）、`inferenceGatewayBaseUrl`（**不带 `/v1`**）、`inferenceGatewayApiKey`、`inferenceGatewayAuthScheme`（`bearer`）、`inferenceCredentialKind`（`static`）、`inferenceModels`（`[{"name": <完整模型 ID>}]`）。`_meta.json` 侧：`entries` 里网关那一条（id = 固定 `9f1c2b3a-…-000000000001`、name = `Claude Gateway`）+ `appliedId`。

**裁决：`inferenceCredentialKind` 进清单。** 依据（都不需要新实验）：

1. 装机版 app.asar 的凭据层（`research/client-login-state.md` A.3）：`needs_interactive` / `signInRequiredMessage` 只在 `inferenceCredentialKind` 取**交互式**时才成立，票 15 用的是 `static` → 不经过登录步骤。缺这个键时 app 取什么默认值**没验证过**。
2. §8.3 的「3P 不需要官方登录态、`inferenceGatewayApiKey` 允许占位」这条结论，靠的正是静态凭据路径；票 15 端到端跑通的那份配置就是带 `static` 的。
3. 一次切换的判据里，风险不对称：写了 `static` = 复现已实测通过的配置；不写 = 赌未验证的默认值（赌错就是 3P 要交互登录、票 46 的收口验收根本不成立）。

**风险（写进遗留）**：① 与 spec §8 表 / §8.4 现有的 5 键清单不一致——本票没动 spec/ADR（§12.1「实现期不改判据」，且 47/48 在并行读同一份 spec），建议**票据层面**把第 6 个键补进清单，本文件只报道这次裁决；② 该键也是 app 自己「Configure Third-Party Inference…」弹窗的设置面：若用户本来写着 `interactive`（SSO 等），首次接管按「接管键以网关为准」覆盖成 `static`，还原时是**摘掉**而不是写回旧值 → 退路是首写快照 + `desktop::rollback`（整份回滚）；③ 接管键一旦被外部改过，下一次写入会按票 28 放弃 + 提示（不静默覆盖）。

### 验收（逐条真跑）

#### 1）引擎级用例（临时根，真实路径一个字节不碰）

```
CARGO_TARGET_DIR=C:/Users/ZHANQUAN/AIGC/Project/Claude Gateway/target cargo test -p gateway-core --lib writer::desktop_tests
running 14 tests
... test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 70 filtered out
```

首写前的现场是**照着本机 pristine 1P 摆的**：`_meta.json` = `{appliedId: <Default id>, entries:[Default]}`、另一个 profile 带用户内容、网关 profile 不存在（`marker_only` 用例）/存在为 `{}`（逐字节还原用例）。

| 收口验收 | 用例 | 断言（都按字节/按 mtime 比） |
| --- | --- | --- |
| 装 3P → 目标文件逐字 | `switch_to_3p_writes_the_expected_profile_and_meta` | profile 全文与期望**逐字相同**（6 键、key 序、2 空格缩进、末尾换行、`inferenceModels` 四项 `{name}`）；`_meta.json` 与期望**逐字相同**（CRLF 保真、`appliedId` 指向网关、Default 条目原样、网关条目追加在末尾）；报告顺序 = profile → `_meta.json`；首写记 marker（原先没这个文件）；**别的 profile 字节与 mtime 都不变** |
| baseUrl 不带 `/v1` | `base_url_is_written_without_the_v1_suffix` | 传 `http://127.0.0.1:8787/v1/` → 落盘 `http://127.0.0.1:8787` |
| 还原 → 回到首写前 + 别的 profile 一字节不动 | `restore_returns_every_file_to_its_pre_write_bytes` | 三个文件全字节比对通过；3P 之后 profile 确实变过；别的 profile mtime 不变 |
| 还原只摘网关键、留用户键、可重复 | `restore_is_idempotent_and_keeps_user_keys` | `custom` 还在、6 个网关键没了；第二次还原 `Unchanged` 且字节不变 |
| 幂等（连切两次 3P） | `switching_to_3p_twice_does_not_touch_the_files` | 两个文件都 `Unchanged`，字节与 **mtime** 都不变 |
| 网关新建且只剩网关内容 → 整个删 | `restore_deletes_the_files_the_gateway_created` | 干净目录：profile 与 `_meta.json` 都 `Removed`、文件消失，别的 profile 没被造出来 |
| `_meta.json` 回退 / 摘掉 | `applied_id_falls_back_to_the_surviving_entry`、`applied_id_is_dropped_when_no_entry_survives`、`meta_restore_reads_the_current_disk_state` | 回退到 Default 的 id；没有存活条目时 `appliedId`/`entries` 都摘掉；`_meta.json` 解析不过就报错（形状不对不猜） |
| §8.2 模型 ID 判据 | `model_id_predicate_follows_spec_8_2` | 41 个样本：四个家族名 + 裸档位名（`opus-4.5`、`haiku-4`）+ 子串命中（`mock-anthropic-text`、`my-claude-thing`）合法；黑名单（`deepseek-v4.1-flash`、`claude-deepseek-v4-pro`、`gpt-5`、`phi4`、`k2.5`、`m2.1`、`ling-1`、`unic`、`amazon.nova-micro`…）非法；`filter_anthropic_models` 逐条剔除、大小写不敏感 |
| 整份回滚（第二选项） | `rollback_restores_the_first_snapshot_byte_for_byte` | 两个文件都 `Rolled` 且回到首写前逐字节 |
| 根来自环境变量 | `config_library_root_comes_from_local_app_data` | `%LOCALAPPDATA%\Claude-3p\configLibrary`（票 45 遗留 2 的那条断言） |

#### 2）真实路径那半（只做配置读写与还原；**没启动 Desktop**，网关也没起）

脚本（临时目录，不进仓库）：动手前把 `configLibrary` 整目录复制成安全副本 → 两次 `#[ignore]` 用例（`STEP=3p` / `STEP=1p`，共用同一个临时 `CG_DESKTOP_REAL_DATA`，记录才连续）→ 三次 SHA256 快照。

```
$env:CG_DESKTOP_REAL_RUN='1'
$env:CG_DESKTOP_REAL_ROOT="$env:LOCALAPPDATA\Claude-3p\configLibrary"
$env:CG_DESKTOP_REAL_DATA="$env:TEMP\cg46-real-data"
cargo test -p gateway-core --lib -- --ignored --nocapture real_config_library_switch_and_restore
```

装 3P 之后（用例打印的落盘内容就是交付形状）：profile = 6 键齐全、baseUrl `http://127.0.0.1:8787`、`inferenceModels` 四项家族名；`_meta.json` = `appliedId` 指向网关 profile、`entries` 里 Default 原样 + 网关条目在末尾。切回 1P 之后：profile 回到 `{}`、`_meta.json` 只剩 Default 且 `appliedId` 回退到它。两次写入的 `first_write` / `notice` 与备份路径都在用例输出里（首写各留一份快照，还原各留一份滚动备份，都落在临时数据目录）。

校验和（`名字|字节|SHA256`）：

```
=== 动手前 ===
_meta.json|287|2BC66EB2C4E5F674D5B89FE3910EB0FC5FAD515DDE53A85C56A12BECFCBCA930
9f1c2b3a-4d5e-4f60-8a71-000000000001.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F
ac278ce7-9506-4f19-b147-25637015682c.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F

=== 装 3P 后 ===
_meta.json|275|A26A134435727E72B35C8907ECAF0F65DD11AA0D824D066AB250E028BEE5D595
9f1c2b3a-4d5e-4f60-8a71-000000000001.json|432|AFC48BA3367B6F08279149EA01E4D3A93688305EF88D658B8F0E93E2FEA2E3D2
ac278ce7-9506-4f19-b147-25637015682c.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F   ← 一个字节没动

=== 切回 1P 后（引擎摘键）===
_meta.json|175|492A09C2AF9F4773C0A2B52A5669DABD5E8C4FC894DE4122A43E872B9E0692EE
9f1c2b3a-4d5e-4f60-8a71-000000000001.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F   ← 与动手前相同
ac278ce7-9506-4f19-b147-25637015682c.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F   ← 与动手前相同
```

结论：**profile 与另一个 profile 与动手前逐字节相同**；`_meta.json` 被正确地清成 1P（`appliedId` 回退到 Default、网关条目摘掉）——它与动手前不同，是因为**动手前那份本身就是原型 `standard` 模式留下的残留**：`appliedId` 悬在网关 profile 上（那个 profile 里已经没有 `inferenceProvider`）、`entries` 里还留着网关条目（`client-login-state.md` 已记录这个残留，原型不维护 `_meta.json`）。为严格满足「做完回 1P 原状」，把 `_meta.json` 从动手前的安全副本逐字节放回，复验：

```
=== 收工后（_meta.json 从安全副本逐字节放回）===
_meta.json|287|2BC66EB2C4E5F674D5B89FE3910EB0FC5FAD515DDE53A85C56A12BECFCBCA930
9f1c2b3a-4d5e-4f60-8a71-000000000001.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F
ac278ce7-9506-4f19-b147-25637015682c.json|4|E3566B3A06430868D71E9287DFD6C6C520A3DA027AABEA01951D407EE131DC2F
与动手前逐字节相同：diff 0 行。
```

（安全副本留在 `%TEMP%\cg46-real-safety`，没进仓库；Desktop 全程没启动，也没让它停在 3P。）

#### 3）门禁

```
pwsh -File ./check.ps1 -Quick -Replay
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
test result: ok. 5 passed; 0 failed; 0 ignored                              (claude-gateway)
test result: ok. 83 passed; 0 failed; 1 ignored                             (gateway-core lib：含 desktop 14 条，1 条 #[ignore] 的真实路径用例)
test result: ok. 4 passed; 0 failed; 0 ignored                              (gateway_service)
test result: ok. 14 passed; 0 failed; 0 ignored                             (provider_table，票 49)
test result: ok. 0 passed; 0 failed; 2 ignored                              (upstream_live，票 49 的真上游用例)
test result: ok. 1 passed; 0 failed; 0 ignored                              (doc-test)
=== clippy (mock 特性：两个测试 bin + 回放测试) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===   20 passed
=== replay-unit (回放器切法单测，mock 特性下的 bin) ===          5 passed
全部通过。
```

这是合并集成分支（`e303329` 票 49）之后的运行；`gateway_core-2dea75f1da6774a5.exe --list` 里 `desktop_tests` 计 14 条，确认门禁跑的就是带本票用例的那个二进制（下面遗留 8 说的那件事）。

#### 4）留给票 55 现场验收（用户在场）的步骤与前置

不在本票范围的是「UI 观察 + 需要能用的上游」那半：桌面应用菜单显示四个家族名、流式问答、切回 1P 登录态完好。

- **前置**：① 票 49 的供应商表 / 上游目录（已进集成分支）；② 票 51 的绑定层 + 客户端档案自检要把 Desktop 档案接到 `desktop::switch_to_3p/switch_to_1p`（根用 `desktop::config_library_root()`，备份/日志挂 `store::default_data_dir()`）；③ 至少一个可用上游，四个家族槽位都绑上。
- **步骤**：① 网关起来，界面点「切到 3P」→ 看卡片状态与写库记录；② **完全退出并重启** Claude Desktop → `Developer → Configure Third-Party Inference…` 核对 baseUrl 与模型列表；模型选择器应显示 `claude-fable / claude-opus / claude-sonnet / claude-haiku`；③ 四个家族名各问一句，确认流式正常、没有常驻「Configured model not available」横幅（非流式探针 `POST /v1/messages`（`max_tokens:1`）已由票 26 实现，正是这条横幅的成因）；④ 点「切回 1P」→ 重启 Desktop → 确认官方登录态完好（3P/1P 的 userData 与登录态隔离，§8.1 第 3 条）；⑤ 动手前后各留一份 `%LOCALAPPDATA%\Claude-3p\configLibrary\` 的校验和（做法同本票「真实路径那半」）。
- **提醒**：本机 `_meta.json` 现在还是原型残留状态（`appliedId` 指向网关 profile、`entries` 里留着网关条目）。第 ② 步写入会把它接管成正确形状（那条 name 变 `Claude Gateway`），第 ④ 步切回后 `appliedId` 回退到 Default —— 比动手前更干净，这是预期行为，不是缺陷。

### 对引擎的唯一一处改动（报备）

`writer/doc.rs` 的 `json_ensure_array`：`json_ensure` 给**缺失**的键建的是**空对象**（不是 `null`），于是 `is_null()` 那条归一分支永远不触发 —— 网关在干净机器上第一次新建 `_meta.json`（`entries` 第一次出现）时，`SetElement` 直接报「接管键 entries 不是数组，停写」，Desktop 永远装不上 3P。改法是 1 个条件：`null` **或空对象**都归一成空数组。回归 = `restore_deletes_the_files_the_gateway_created`（先红后绿：红是 `接管键 entries 不是数组，停写`）+ 引擎原有 33 条一条不改全过。

备选做法（桌面侧不用 `SetElement`，改成整份 `Set` 重建 `entries` 数组）**否决**：那会把别的 profile 的条目一起卷进「接管键现值 vs 记录值」的冲突判据，正是 ADR-0006 专门否掉的整份哈希式误报来源（app 自己加/删 profile 就会常驻报冲突）。

### 遗留与坑

1. **spec 与实现不一致（待票据层面同步）**：spec §8 表 / §8.4 / `research` 蒸馏稿里的 Desktop 接管键还是 5 个，实现是 6 个（多了 `inferenceCredentialKind`）。本票按上面「裁决」实现，**没动 spec / ADR**（§12.1 + 47/48 并行读同一份 spec），建议编排者把这一键补进 spec 清单。
2. 引擎那处 `json_ensure_array` 的改动属于票 45 的代码（本票最小修，理由如上）；若票 45 作者有别的打算，可回退成「空对象也认」，但 Desktop 新建 `_meta.json` 这条路必须能走通。
3. **网关新建的 profile 是 LF**（引擎对不存在的文件用默认排版），而 app 自己写的是 CRLF；app 读得动（实测这份 3P 配置被 app 正常读入过，见票 15），但若要逐字节一致，得给引擎加「新文件排版模板」——本票没做。
4. `entries` 里网关那条被外部改/删时，下一次「切到 3P」按票 28 的冲突判据**放弃 + 提示**（不自动补条目）；界面要给「认领并覆盖」入口（`WritePlan::claim()`，票 17/52）。
5. `switch_to_1p` 走引擎的摘键还原，**不做冲突检测**（引擎设计如此，§8.4 第 8 条「默认摘键」）：若用户在 app 里另配了一套 3P provider，摘键会连那套一起摘——这是「接管键以网关为准」的直接后果，界面提示归票 17。
6. 未做（票面外）：界面态、注册表托管路径（`HKLM|HKCU\SOFTWARE\Policies\Claude`，spec §8 表下注：本期不做）、Desktop 的 `/v1/models` 过滤（§8.2 第 2 条：不需要）。
7. 单实例互斥量仍只提供接口（票 45 遗留 1），外壳接上之前，「同进程写入串行化」的前提在运行时还没成立。
8. **共享 `CARGO_TARGET_DIR` 会让门禁跑到别的 worktree 编译出的测试二进制**（本次实测到）：第一次跑门禁时 `=== unit ===` 里 gateway-core lib 只报 **70** 条、`desktop_tests` 一条没有；`cargo test -p gateway-core --lib` 却是 84 条。原因是多 worktree 共用一份 target（编排约定第 13 行），另一个 worktree 的构建会覆盖同名产物 `gateway_core-<hash>.exe`，cargo 按 mtime 认为「不需要重编」，于是**静默跑了别人那份**。办法：跑门禁前 `touch` 自己改过的源文件（本次 `desktop.rs` / `desktop_tests.rs` / `mod.rs`），再核对 `=== unit ===` 的条数与 `--list` 里的用例名。这条对所有并行票都成立，建议编排者写进 `orchestration.md`。
