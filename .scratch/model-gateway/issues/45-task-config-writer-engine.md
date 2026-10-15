# 配置写入引擎

Type: task
Status: resolved
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

## Answer

**引擎落地了**（2026-10-15）。代码提交 `d137db7`、收口提交 `cd52f97`；分支 tip `8209461`（已把集成分支 tip `e4de381` 并在自己分支里，见 `git merge-base --is-ancestor integration/model-gateway HEAD` 通过）。中途 `git merge integration/model-gateway` 就地解决 `crates/gateway-core/Cargo.toml` 一处冲突（只并 windows-sys 的特性列表），`cargo build --workspace` 自修锁。

### 做了什么

落点 `crates/gateway-core/src/writer/`（旁路模块，转发路径不依赖它；不依赖 Tauri）：

| 文件 | 内容 |
| --- | --- |
| `plan.rs` | 公开数据形状：`FileSpec` / `OwnedOp` / `Format` / `WritePlan` / `ConflictMode` / `WriteRoots` / `FileOutcome` / `RollbackOutcome` |
| `doc.rs` | JSON / JSONC / TOML **保序改写**；解析失败即停给行列；沿用原文件缩进 / 行尾 / BOM / 末尾换行 |
| `atomic.rs` | 同目录 `create_new` 临时文件 → fsync → `ReplaceFileW`（`NotFound` / `ERROR_NOT_SUPPORTED` 退 `fs::rename`，**最多 3 轮**）；占用 / 权限 / 只读退避重试 **3 次 × 200ms**；失败即删临时文件；不做父目录 fsync |
| `backup.rs` | `<备份根>\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`（本地时间）；首写快照常驻（路径记在库里，清理时跳过它）、滚动留最近 **5** 份 + 超 **90 天**（按 mtime）清；首写时文件不存在只写 marker |
| `state.rs` | `client_config_state` 的 SQL（本票自己写）：读 / upsert；`backup_path` 与 `existed_at_first_write` 一旦记下就不再改 |
| `log.rs` | `<日志根>\writer-YYYYMMDD.log`，一行一次写入 / 还原，**只记元数据**，按 mtime 清 7 天 |
| `engine.rs` | `Writer`：每文件一把锁（进程内注册表，不做跨进程锁文件）+ 冲突检测 + 幂等 + 备份 + 原子替换 + 记库 + 多文件依赖序；摘键还原 / 整份回滚 |
| `instance.rs` | 单实例命名互斥量（`acquire(name)`，外壳启动时取） |

新增依赖：`toml_edit = "0.25"`（本就锁在 0.25.17，无版本漂移）、`serde_json` 开 `preserve_order`、`windows-sys` 加 `Win32_Storage_FileSystem` + `Win32_System_SystemInformation` 两个特性。`Cargo.lock` 只多了三条依赖边。

### 接管键清单的数据形状与调用约定（票 46 / 47 / 48 照这个用）

**引擎不认任何客户端的键名与路径**：客户端 × 文件 = 调用方拼一份 `FileSpec` 传进来。

```rust
use gateway_core::writer::*;

let roots = WriteRoots::with_data_dir(real_config_root, store::default_data_dir()?); // 备份/日志挂数据目录
// 测试只换根：WriteRoots::under(temp_dir)
let writer = Writer::new(store.clone(), roots);

let spec = FileSpec::new(
        "claude-desktop",              // 客户端：备份子目录名 + client_config_state.client
        "configLibrary/<id>.json",     // 相对 config_root（真实根由你拼，引擎不猜）
        Format::Jsonc,                 // Json / Jsonc / Toml
        vec![                          // write_ops：写什么
            OwnedOp::Set { path: vec!["inferenceProvider".into()], value: json!("gateway") },
            OwnedOp::Set { path: vec!["inferenceModels".into()], value: json!(["claude-sonnet-4-5"]) },
        ],
    )
    .with_restore(vec![                // restore_ops：还原怎么办（空 = restore 响亮报错）
        OwnedOp::Remove { path: vec!["inferenceProvider".into()] },
    ])
    .with_order(0);                    // 依赖序：先被引用者、后指针

let report = writer.write(&WritePlan::new(vec![profile, meta])).await?;  // 多文件按 order 逐个原子替换
// 中途报错 / 冲突都即停（错误里列已改文件、冲突时后面的文件一个字节不动，避免指针先于被引用者生效）
// 冲突（report.conflicts() / FileOutcome::Conflict{path,recorded,current}）→ 界面提示；
// 用户点「认领并覆盖」→ WritePlan::new(files).claim() 再来一次。
writer.restore(&plan).await?;            // 默认摘键（含指针性条目）
writer.rollback(&spec).await?;           // 整份回滚：Rolled / Refused{reason} / Unchanged
```

六种 `OwnedOp`（路径是**段数组**，不是点分串——TOML 键里本来就有 `.`）：

| 动作 | 语义 |
| --- | --- |
| `Set { path, value }` | 设值 / 建值，缺的中间对象自动建；中间是别的类型 → 报错停写 |
| `Remove { path }` | 摘键 |
| `RemoveIfEmpty { path }` | 摘键，摘完空了就把空壳一层层也摘掉（Claude Code 的 `env`） |
| `SetElement { path, field, value }` | 数组里按 `elem[field]` upsert（Desktop `_meta.json` 的 `entries`） |
| `RemoveElement { path, field, matches }` | 数组里按字段摘元素 |
| `ReplaceIfEquals { path, equals, value: Option<Value> }` | 仅当现值等于 `equals` 时替换 / 摘掉（`appliedId` 回退；不等则一个字节不动） |

结果类型：`FileOutcome::{Unchanged, Written{backup,marker_only,first_write,notice}, Conflict{path,recorded,current}, Removed}`、`RollbackOutcome::{Rolled{backup,deleted}, Refused{reason}, Unchanged}`。

库里记的形状（`client_config_state.owned_keys_json`，`file_path` = 相对配置根、`/` 分隔）：

```json
{"v":1,"labels":["[\"env\",\"ANTHROPIC_BASE_URL\"]"],"values":["http://127.0.0.1:8787"]}
```

`labels` = `write_ops` 的标签序列（判「清单变没变」：不符就按首次接管处理并提示），`values` = 每条动作的**槽值**（`SetElement` / `RemoveElement` 记匹配到的那条元素，缺席记 `null`）。冲突判据 = 逐条比 `values[i]` 与磁盘现值，**只比接管键**。

三条给调用方的硬规矩：① 真实配置根由你拼（引擎没有默认根，测试才不会碰真机）；② `restore_ops` 必须给全（含指针性条目：`entries` 摘条目 + `appliedId` 条件回退）；③ 依赖序写进 `FileSpec::order`。

### 验收（真实命令与结论）

**1）票 30 第 8 格：幂等 / 首写备份 / 还原 / 用户手改（非接管键）保留 + 真实路径不碰**

用例：`json_write_is_idempotent_and_keeps_user_keys_and_layout`（语义相同完全不写、mtime 不变、要写则两次字节相同、JSON 保键序）、`first_write_snapshots_the_original_and_records_state`、`first_write_on_a_missing_file_writes_a_marker_only`、`snip_restore_drops_owned_keys_and_pointer_entries_only`、`restore_deletes_a_file_the_gateway_created_and_emptied`、`restore_keeps_a_file_that_has_user_content`、`user_edit_to_a_non_owned_key_is_not_a_conflict_and_survives`、`toml_rewrite_keeps_order_comments_and_other_tables`（Codex 桌面应用往同文件插 `[desktop]` → 不算冲突、不被抹掉）。

真实路径证据：跑门禁前 / 后各对 `%LOCALAPPDATA%\Claude-3p`、`~/.claude`、`~/.codex` 逐文件快照（相对路径 + 长度 + mtime + SHA256），两次快照文件**逐字节相同**：

```
snapshot before3.txt: 7355 files
=== 跑 pwsh -File ./check.ps1 -Quick -Replay ===
snapshot after3.txt: 7355 files
diff before3.txt after3.txt   →  exit 0，0 行
sha256  before3.txt = dc88f2457dbe8c9fcd1270a83a93362c9982cf8e89f6eb27271041b7792b66c7
sha256  after3.txt  = dc88f2457dbe8c9fcd1270a83a93362c9982cf8e89f6eb27271041b7792b66c7
```

（快照脚本没进仓库，逻辑就是：对三个目录 `Get-ChildItem -Recurse -File -Force` 逐个 `Get-FileHash -Algorithm SHA256`，输出 `路径|长度|mtime ticks|hash` 排序后比对；脚本本身只有读操作。）

**2）六类必需用例**（`cargo test -p gateway-core --lib writer::`，34 条全过）

| 要求 | 用例 | 断言要点 |
| --- | --- | --- |
| 崩溃窗口 | `crash_window_never_leaves_a_half_written_target`、`stale_temp_files_are_cleaned_and_foreign_temp_files_are_left_alone` | 同目录躺着写满没替换的 `*.cg-tmp*` 时目标文件仍是**完整合法 JSON**、混不进半截字节；>24h 陈旧临时文件顺手清掉，不陈旧的别人的文件不碰 |
| 占用重试 | `busy_file_is_retried_then_reported_with_paths_and_hint` | 目标以 `FILE_SHARE_READ` 被占住：退避重试 3 次（实测 `elapsed >= 600ms`）后报错，错误里带**目标路径 + 备份路径 + 「可能有客户端正占用该文件」**；目标字节与 mtime 不变、临时文件已删 |
| 冲突放弃 | `conflict_gives_up_and_leaves_the_file_untouched` | 手改接管键 → `Conflict{path,recorded:"gateway",current:"anthropic"}`，文件与 mtime 不变、不新增备份；配套 `claim_mode_overwrites_the_owned_key`（认领覆盖）、`a_different_owned_key_list_is_treated_as_first_takeover`、`a_conflict_stops_the_rest_of_the_plan`（多文件遇冲突即停，指针文件不动） |
| 幂等不写 | `json_write_is_idempotent_and_keeps_user_keys_and_layout` | 第二次 `Unchanged` 且 `mtime` 不变；把文件还原成写前内容再写一次 → 字节与第一次完全相同 |
| 摘键还原 | `snip_restore_drops_owned_keys_and_pointer_entries_only`、`restore_deletes_a_file_the_gateway_created_and_emptied` | `_meta.json` 只摘网关条目 + `appliedId` 条件回退，别的 profile 不动；网关新建且摘空只剩网关内容的文件整个删；已有用户内容的文件留着只摘键 |
| 整份回滚拒绝 | `rollback_refuses_when_the_file_drifted_after_our_write` | 网关写完后手改接管键 → `Refused{reason}` 里点出键名与「外部改过」，文件一个字节不动；配套 `rollback_restores_the_first_snapshot_byte_for_byte`（逐字节）、`rollback_deletes_a_file_the_gateway_created`、`rollback_without_any_record_is_refused` |

另外覆盖：解析失败即停（`parse_failure_stops_without_writing_or_backing_up`，报 `第 2 行第 8 列`，连备份都不留）、JSONC 注释 / 尾逗号 + BOM / CRLF 保真、TOML 保序 / 保留注释与别的段 / `Set` 到 `null` 报错、多文件依赖序 + 中途失败列已改文件、同文件并发写只真写一次、备份保留 5 份 / 90 天 / 首写快照常驻、日志 7 天、单实例互斥量认得出第二个实例。

**3）门禁**

```
pwsh -File ./check.ps1 -Quick -Replay
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===                           71 passed
=== clippy (mock 特性：两个测试 bin + 回放测试) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===      20 passed
=== replay-unit (回放器切法单测，mock 特性下的 bin) ===             5 passed
全部通过。
```

`serde_json` 打开 `preserve_order` 是**工作区级**特性（cargo 特性统一），全仓 JSON 从此保插入序——上面这次全量门禁（含 20 格回放与票 41 的回归套件）就是回归证据。

### 遗留与坑

1. **单实例互斥量只提供了接口，外壳还没接**：`writer::instance::acquire("ClaudeGateway.SingleInstance")` 要在启动时取一次（取不到就唤起已有窗口并退出）。`src-tauri/src/shell.rs` 属票 43 的落点，本票不越界改；ADR-0006 第 8 条的这条前提因此**尚未在运行时成立**，注册一张小票或并入外壳票。
2. **真实根由 46/47/48 拼**：引擎没有 `%LOCALAPPDATA%` / `~/.codex` 之类的默认路径，这是「不碰真机」的办法；反过来说，谁拼错根谁就能写坏真机——建议 46/47/48 各写一条「根来自环境变量」的断言。
3. **TOML 内联表的内部不改**：`a = { b = 1 }` 里 `Set(["a","b"])` 会报错停写（不摧毁用户数据）。实测 Codex 用的是 `[model_providers.x]` 表，够用；真遇到手写内联表要扩引擎。
4. **TOML 没有 `null`**：`Set` 成 `null` 报错（摘键请用 `Remove`）。JSON 侧 `null` 是合法值。
5. **`restore_ops` 为空 → 响亮报错**（`restore_without_restore_ops_is_a_loud_error`）：宁可报错也不要「点了还原什么都没发生」。
6. **多文件中途失败只报错 + 列已改文件**，不做跨文件 pending 事务与启动补完（票 28 §7）。
7. **清理口径**：备份「最近 5 份」按文件名里的时间戳排序，「超 90 天」与日志「7 天」按文件 mtime——测试用 `File::set_modified` 造时间，不引日期库。
8. **票间矛盾不本票裁决**（spec §12.1）：`inferenceCredentialKind`（票 46 清单漏列）、Codex `env_key` 变量名与 `http_headers` 内容（票 48，与票 24「变量必须真存在」冲突）、`appliedId` 回退到哪个字面值、目录条目 `priority` —— 引擎一律按「调用方传值」处理，这些键要写就加一条 `OwnedOp`，**不用动引擎一行**。
9. **一处与 ADR 的后果声明不一致（偏差向上）**：ADR-0006 第 32 行说「真写入会把 TOML 里用户自己的注释洗掉」，改用 `toml_edit` 后实测**注释与空白都留着**（`toml_rewrite_keeps_order_comments_and_other_tables` 断言 `# 我的注释` 仍在）。判据（票 28 第 4 条第 3 点「接受重序列化」）没变，只是做得更好；spec §8.4 第 5 条的措辞（「用户注释会丢」）若要跟着改，按要求回票据层面改，本票不改判据。
10. **本票没做**：界面态（票 17 的「值不匹配」卡片）、三个客户端的真实键清单（46/47/48）、注册表托管路径（spec §8 表下注：本期不做）。
