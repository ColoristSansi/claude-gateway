# 存储层与 DPAPI

Type: task
Status: resolved
Blocked by: 39

## Question

按 spec §9 把 SQLite 层做出来，并修掉原型那笔债：`store.rs` 的「每行单开连接 + 同步写库阻塞 runtime」（原型自注 shortcut）。

依据：spec §9、[ADR-0006](../../../docs/adr/0006-config-writer-owned-keys.md)（`client_config_state` 的列）、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- 建表：`upstreams`（含上次拉取时间 / 结果 / 警告）、`upstream_models`（真实模型 + `supported_wires` + `source` + 手工价四列）、`credentials`（密文，按「上游 × wire」）、`virtual_models`、`client_profiles`、`client_config_state`、`requests`、`request_bodies`、`model_catalog`、`settings`。
- 迁移机制（`PRAGMA user_version` + 顺序迁移函数），能在旧库上升级。
- 连接策略：单写线程 + 请求排队，或连接池——二者择一，判据是「不阻塞 tokio runtime」。
- DPAPI 加解密（`CryptProtectData` / `CryptUnprotectData`，当前用户范围），密钥只以密文入库；界面侧要的掩码读取接口。
- 数据目录解析：`%APPDATA%\com.claudegateway.desktop\`，首次启动自建（[票 18](../issues/18-grilling-packaging-and-updates.md)）。

## 收口验收

- 单测：建表 / 迁移（造一份旧版库升上来）/ 读写往返 / 连接并发（两个写不互相阻塞 runtime）。
- 库内反查不到任何明文密钥（拿已知密钥串在库文件里搜不到）。

## Answer

（2026-10-15，worktree `cg-wt/42` / 分支 `task/42-storage-and-dpapi`；实现提交 `cabcfeb`，再合集成分支 tip `87081cc`（票 40）→ merge `589c72c`，无冲突、`Cargo.lock` 没动）

### 做了什么

`crates/gateway-core/src/store/` 四个文件：

| 文件 | 内容 |
| --- | --- |
| `mod.rs` | `Store`（连接策略 / 读写入口）、DPAPI 凭据四个方法、数据目录解析、掩码 |
| `schema.rs` | `PRAGMA user_version` + 顺序迁移函数（v1 原型期 → v2 spec §9），十张表 DDL |
| `dpapi.rs` | `CryptProtectData` / `CryptUnprotectData`（CurrentUser 范围，`CRYPTPROTECT_UI_FORBIDDEN`） |
| `tests.rs` | 12 条单测（起建 / 迁移 / 往返 / 并发 / DPAPI / 掩码 / 数据目录） |

### 连接策略：单写线程 + 请求排队（选它，不选连接池）

- 写只有一个入口 `Store::write(f)`：闭包打包成 job 发进无界队列，专属线程 `gateway-store-write` 顺序执行、每条一个 `oneshot` 回执。**写库不在 runtime 线程上跑**，也不再每行开连接（原型 `prototype/src/store.rs` 的 `log()` 是「每行单开连接 + 同步写」，ADR-0007 第 6 条那笔债）。
- 读走 `Store::read(f)`：一条常驻读连接 + `spawn_blocking`；WAL 下读写不互斥，读不挤写。
- 选写线程而不是连接池的理由：库在本地、单进程、写点只有请求日志与配置两类。连接池要多维护尺寸 / 超时 / 回收三件事；单写线程把并发写天然变成排队，顺序即提交顺序（日志写序＝请求发生序），读并发靠 WAL 已够。真到读扛不住再换池，只有 `read` 一处要动。
- 无界队列是有意的：本地 sqlite 写线程不会落后，宁可涨一点内存也不让写阻塞请求路径（代码里留了 `shortcut:` 注记）。

### 表结构清单（§9 十张表）

- `upstreams`：`id` / `name` / `base_url` / `auth_json`（按 wire 的 scheme，§6 形状）/ `path_templates_json` / `header_passthrough_json` / `quirks_json` / `last_pull_at` / `last_pull_status` / `last_pull_warning`（票 35：反查不到的端点路径进这里，下次成功拉取清空）。
- `upstream_models`：`upstream_id` + `model_id` + `supported_wires`（JSON 数组，`[]` = 未声明）+ `source`（`manual`/`upstream`/`doc`，CHECK）+ 手工价四列 `price_input_usd` / `price_output_usd` / `price_cache_read_usd` / `price_cache_write_usd`（NULL = 回退 models.dev，票 33）。
- `credentials`：`upstream_id` + `wire`（`default`/`anthropic`/`openai-chat`/`openai-responses`，CHECK）+ `ciphertext`（BLOB，DPAPI 密文）。
- `virtual_models`：`client_profile_id` + `name` + `is_family` + `upstream_id` + `real_model` + `wire`（§6 三个值，CHECK；NULL = 未绑定）+ `purpose`，主键 `(client_profile_id, name)` —— 每客户端可覆盖（§7）。
- `client_profiles`：`id` / `display_name` / `match_ua` / `enabled`；种下四行 `global` / `claude-desktop` / `claude-code` / `codex`。
- `client_config_state`：`client` + `file_path`（主键）+ `owned_keys_json`（上次写入的接管键值）+ `last_op`（`write`/`restore`）+ `last_write_at` + `backup_path`（首写快照）+ `existed_at_first_write`（ADR-0006）。
- `requests`：`id` / `ts`（Unix 秒 UTC）/ `request_id` / `client` / `virtual_model` / `upstream_id` / `real_model` / `wire` / `status` / `first_byte_ms` / `total_ms` / `input_tokens` / `output_tokens` / `cache_read` / `aborted` / `error` / `cost_usd`（冻结，NULL = 当时无价）。
- `request_bodies`：`request_id`（主键 → `requests.id`）/ `request_body` / `response_body`（默认空，开关打开才有行）。
- `model_catalog`：`provider` + `model_id`（主键）+ `name` + `cost_*_usd` 四列 + `raw_json`（models.dev 条目原样）。
- `settings`：`key`/`value`（KV）；种下 `port=8787`、`log_retention_days=7`、`usd_cny_rate`、`catalog_snapshot_at` 四键，键名常量在 `store::settings_keys`。

迁移机制：`PRAGMA user_version` 逐级前进，一个版本一个函数、整段跑在一个事务里（DDL 与版本号同事务提交，不会出现「表改了版本没动」）。**新库也从 v1 顺序跑到 v2**，装一次机就把升级路径走一遍。v1→v2 三处是数据搬迁：`quirks` 表折进 `upstreams.quirks_json`（`[{name,arg}]`，旧 wire `openai`→`openai-chat`，其它取值退 NULL）、`virtual_models` 换复合主键（旧行落 `global` 档案）、`request_log` 重建为 `requests`（旧 `'unix:<秒>'` 文本转整数）且 `body` 搬进 `request_bodies`。

公开接口（给界面 / 票 45 / 49 / 51）：`store::Store::{open, open_default, root, db_path, read, write, put_credential, credential_secret, credential_mask, delete_credential}`；`store::{default_data_dir, mask_secret, DATA_DIR_NAME, SCHEMA_VERSION, settings_keys::*}`。表格读写由票 49 / 51 用 `read` / `write` 自己写 SQL（本票只做存储原语 + 本票自己拥有的凭据 API）。

### 收口验收（真跑）

**1. 单测（建表 / 迁移 / 往返 / 连接并发）**

```
$ CARGO_TARGET_DIR="C:/Users/ZHANQUAN/AIGC/Project/Claude Gateway/target" cargo test -p gateway-core --lib store::
running 12 tests
test store::tests::default_data_dir_follows_appdata_and_identifier ... ok
test store::tests::mask_keeps_head_and_tail_only ... ok
test store::tests::migrations_are_contiguous_and_cover_schema_version ... ok
test store::tests::dpapi_roundtrip_is_current_user_scoped ... ok
test store::tests::open_creates_missing_data_dir ... ok
test store::tests::read_write_roundtrip_across_spec9_tables ... ok
test store::tests::a_stuck_write_thread_does_not_stall_the_runtime ... ok
test store::tests::writes_and_reads_run_off_the_runtime_thread ... ok
test store::tests::credentials_are_dpapi_ciphertext_and_never_plaintext_on_disk ... ok
test store::tests::two_concurrent_writers_both_land ... ok
test store::tests::first_launch_creates_spec9_tables_and_seeds ... ok
test store::tests::upgrades_a_prototype_era_db_in_place ... ok
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
```

并发那两条的判据**不是计时**，是结构：`writes_and_reads_run_off_the_runtime_thread` 断言写闭包与读闭包执行的 `thread::id` ≠ 调用方 runtime 线程；`a_stuck_write_thread_does_not_stall_the_runtime` 先让写线程卡在一个自己控的 gate 上（`oneshot` 确认它已进 job），再验证**另一个写在排队、runtime 仍能按点跑定时器**，放行后两条写都落库。两个测试都做过变异验证：把 `write` 改回「在调用线程上同步执行」→ `assertion left != right failed: ThreadId(2) vs ThreadId(2)`，测试确实会红。

**2. 库内反查不到任何明文密钥**

测试摆下的假串是 `sk-cg42-known-plaintext-9f3a1c`（fixture，非真实凭据），落在临时目录 `%TEMP%\cg42-store-tests\credentials\`（**不碰真实 `%APPDATA%\com.claudegateway.desktop\`**；写入根参数化）。同一测试里另存一条**故意明文**的 `cg42-scan-probe-plain` 做正对照，证明这套反查不空转。

```
$ pwsh -File %TEMP%\cg42-byte-scan.ps1
目录: C:\Users\ZHANQUAN\AppData\Local\Temp\cg42-store-tests\credentials
  gateway.db  4096 字节
  gateway.db-shm  32768 字节
  gateway.db-wal  177192 字节

=== 字节搜：明文密钥 'sk-cg42-known-plaintext-9f3a1c' ===
  0 处命中（库里没有明文密钥）

=== 字节搜：正对照串 'cg42-scan-probe-plain'（故意存下的明文，验证搜索器不空转） ===
  gateway.db-wal @ offset 152369

=== findstr 同上两串 ===
  明文密钥 findstr exit=1
gateway.db-wal
  正对照 findstr exit=0
```

结论：整个数据目录（含 `-wal` / `-shm`）里搜不到明文密钥；正对照在同一目录同一工具下**搜得到**，所以「搜不到」不是工具失灵。同一测试另断言：库里 `credentials.ciphertext` 列 ≠ 明文、比明文长、不含明文子串；`credential_secret()` 能读回明文，`credential_mask()` 给界面的是 `sk-…3a1c`。

**3. 门禁**

```
$ pwsh -File ./check.ps1 -Quick -Replay
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
=== replay (集成：自起网关 + mock，回放 fixtures) ===
还没有回放测试目标（票 41 搬入 crates/gateway-core/tests/replay.rs）；这一步先跳过。

全部通过。
$ echo $?
0
```

`cargo build --workspace` 亦通过（`Finished dev profile ... in 1.84s`）。

### 遗留与坑

- `requests` **没有 `cache_write` 列**：票 33 把它列为已知缺口、与票 30 第 8 格同批补（M3），本票按 §9 字面不加。
- `upstreams` **没有「上次自检结果」列**：§10 上游列表要显示它、票 49 要用，但 §9 字面只写了拉取时间 / 结果 / 警告；M2 落地时按 v3 迁移加列（机制已就位）。
- **结构化输出能力位**（票 37：上游 × 出站协议 + `source`）同样还没有列，属 M3，按需 v3 迁移。
- `client_profiles.match_ua` 四行都留空：具体 UA / 客户端标识判据由票 51 用实测值填，这里不编字符串。
- `settings.usd_cny_rate` 初值 `7.10` 是**占位内置初值**（票 33：不联网、设置页可手改），设置页落地时定稿。
- `requests.ts` 统一 Unix 秒整数。**日边界别用 SQLite 的 `'localtime'` 算**（Windows 上不认系统时区），查询侧自己按本机本地时区转（票 33 / 56）。
- 迁移里旧 `virtual_models.wire` 只认 `anthropic` / `openai`（原型取值域），其它值退 NULL（未声明）；旧上游没有 name，先用 id 顶。
- 共享 `CARGO_TARGET_DIR` 的并发现象：期间另一个会话的 `claude-gateway.exe`（PID 22012）占着 `target\debug\claude-gateway.exe`，`cargo build --workspace` 报「failed to remove file ... 拒绝访问 (os error 5)」，进程退出后即通过——不是本票代码问题，`cargo check` / `cargo test` 当时都绿。
