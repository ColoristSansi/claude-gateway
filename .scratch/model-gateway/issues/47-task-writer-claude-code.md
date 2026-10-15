# Claude Code 写入器

Type: task
Status: resolved
Blocked by: 45

## Question

在写入引擎上做 Claude Code 这一条：写 `%USERPROFILE%\.claude\settings.json` 的 `env` 块。**只写 `ANTHROPIC_AUTH_TOKEN`，不写 `ANTHROPIC_API_KEY`**（后者需一次 `/config` 交互批准、曾拒绝过就静默失灵）。

依据：spec §8 表 + §8.3、[票 01](../issues/01-research-client-integration.md)、[票 24](../issues/24-research-client-login-state.md)。

## 交付

- `env` 块写入：`ANTHROPIC_BASE_URL`（网关根地址；**是否带 `/v1` 由真实客户端实测定**——[票 01](../issues/01-research-client-integration.md) 只记了实际请求路径是 `/v1/messages?beta=true`，没记 base 该带什么）、`ANTHROPIC_AUTH_TOKEN`、四个 `ANTHROPIC_DEFAULT_*_MODEL`（槽位名或该客户端的覆盖名）。
- 接管键 = 上述几个 `env` 键；`settings.json` 里别的键与别的 `env` 键原样保留。
- 还原：摘掉这几个键，`env` 空则摘掉空对象。
- 凭据必须落在「首次运行向导之前可读」的位置（就是这份文件，项目级不行）。

## 收口验收

- 真实 Claude Code（本机零官方登录）走网关完成一次问答，`/api/logs` 能看到该次请求。
- 还原后 `settings.json` 里网关写入的键零残留。
- 全库与全盘反查：网关从未写过 `ANTHROPIC_API_KEY`。

## Answer

**Claude Code 这一条落地了**（2026-10-15）。**没有动引擎一行**（票 45 的 `FileSpec` / `OwnedOp::RemoveIfEmpty` / `WritePlan` / `WriteRoots` / `Writer` 原样用）。

### 做了什么

新增 `crates/gateway-core/src/writer/claude_code.rs`（`writer/mod.rs` 加一行 `pub mod claude_code;`）：

| 项 | 值 / 落点 |
| --- | --- |
| 客户端标识 | `CLIENT = "claude-code"`（= 备份子目录名 + `client_config_state.client`） |
| 配置根 | `config_root_from(home) = <home>\.claude`；`default_config_root()` 读 `%USERPROFILE%`（只算路径、不创建） |
| 目标文件 | `REL_PATH = "settings.json"`，**相对配置根**（不写绝对路径，真实根由调用方注入） |
| 格式 | `Format::Jsonc` —— 严格 JSON 的超集：用户手写的注释 / 尾逗号不至于让「解析失败即停」把整次写入挡掉 |
| 写清单 | `env` 下六个键，顺序固定：`ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_DEFAULT_FABLE_MODEL`、`ANTHROPIC_DEFAULT_OPUS_MODEL`、`ANTHROPIC_DEFAULT_SONNET_MODEL`、`ANTHROPIC_DEFAULT_HAIKU_MODEL` |
| 还原清单 | 同一份清单的 `OwnedOp::RemoveIfEmpty`：`env` 摘空就连空壳一起摘 |
| 取值 | `gateway_root_url(port) = http://127.0.0.1:<port>`（**不带 `/v1`**）；`ModelSlots` 默认 = 四个家族槽位名 `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`，可换成该客户端的覆盖名（spec §7「每客户端可覆盖」） |

调用方式（票 51/53/外壳照这个用）：

```rust
use gateway_core::writer::claude_code::{config_root_from, file_spec, gateway_root_url, ModelSlots};
let spec = file_spec(&gateway_root_url(port), &auth_token, &ModelSlots::default());
let writer = Writer::new(store, WriteRoots::with_data_dir(config_root_from(&home), store::default_data_dir()?));
writer.write(&WritePlan::new(vec![spec])).await?;   // 还原用 writer.restore(...)
```

`auth_token` 的值由调用方给：网关目前**不校验入站凭据**（`ingress` 里没有任何 token 检查），所以本地生成一个即可（与 Desktop 3P 允许占位 key 同理），但**必须非空**——实测不给就是 `Not logged in · Please run /login`，一个请求都不发（见下 §2 case C）。

### `ANTHROPIC_BASE_URL` 实测结论：**不带 `/v1`**

隔离 `CLAUDE_CONFIG_DIR` + 本机 Claude Code **2.1.299**（`C:\Users\ZHANQUAN\.local\bin\claude.exe`，零官方登录），`settings.json` 的 `env` 块指向一个只记请求行的本地 Node 探针（`127.0.0.1:18787`，一律回 404 —— 看的是**请求路径**，不是回答）：

```
=== case A base=http://127.0.0.1:18787 （不带 /v1） ===
[claude-code:unrecognized_model] {"model":"claude-opus[1m]","query_source":"sdk"}
--- 探针收到的请求行 ---
2026-10-15T13:32:11.776Z POST /v1/messages?beta=true host=127.0.0.1:18787 auth=bearer
2026-10-15T13:32:11.780Z POST /v1/messages?beta=true host=127.0.0.1:18787 auth=bearer

=== case B base=http://127.0.0.1:18787/v1 （负对照） ===
--- 探针收到的请求行 ---
2026-10-15T13:32:12.221Z POST /v1/v1/messages?beta=true host=127.0.0.1:18787 auth=bearer
2026-10-15T13:32:12.224Z POST /v1/v1/messages?beta=true host=127.0.0.1:18787 auth=bearer

=== case C base 不带 /v1、不给 token ===
Not logged in · Please run /login
--- 探针收到的请求行 ---
(无请求)
```

结论与旁证：

1. **base 是根地址，`/v1/messages` 由客户端自己追加**：带 `/v1` 会得到 `/v1/v1/messages?beta=true`（case B，同一次实验的负对照）。取值定为 `http://127.0.0.1:<port>`，已写成 `gateway_root_url()`，并有用例 `the_base_url_is_the_gateway_origin_without_a_v1_suffix` 钉住。
2. `ANTHROPIC_AUTH_TOKEN` → **`Authorization: Bearer`**（探针看到 `auth=bearer`），与票 01 H9 / 票 24 §1.3 一致。
3. **`settings.json` 的 `env` 块在零官方登录下就被读到并发请求**（case A/B 都打到了探针）——票 24 标为「未验证」的 **U1 就地结案**（只是打在 404 探针上，不是可用绑定，见留给票 55）。case C 反过来证明：只有 base、没有凭据时 CLI 认为「没登录」，所以 `ANTHROPIC_AUTH_TOKEN` 必须写且非空。
4. 同一次实验开头还看到过一次 `HEAD /api/hello`（base 带 `/v1` 时是 `HEAD /v1/api/hello`，13:27:45 / 13:27:54）——客户端的健康探测，网关回 404 不影响（case A/B 在它之后照样发了 `POST /v1/messages`）。
5. 客户端 `--model` / 家族变量解析出的名字（`claude-opus[1m]` 等）与「网关有没有这个模型」无关：探针回 404，CLI 这才报 `unrecognized_model` / 「selected model … may not exist」。**模型可用性要等真实绑定**（票 49/51），不在本票。

### 每条验收的真实命令与输出结论

**1）引擎级用例（临时根，一律不碰真实 `~/.claude`）**

```
cargo test -p gateway-core --lib writer::claude_code
running 9 tests
test writer::claude_code::tests::write_appends_the_six_env_keys_verbatim_and_keeps_the_rest ... ok
test writer::claude_code::tests::user_keys_and_a_user_owned_api_key_survive ... ok
test writer::claude_code::tests::second_write_is_a_noop_and_mtime_does_not_move ... ok
test writer::claude_code::tests::restore_drops_the_owned_keys_and_the_emptied_env_shell ... ok
test writer::claude_code::tests::restore_keeps_an_env_that_still_has_user_keys ... ok
test writer::claude_code::tests::the_owned_key_list_is_the_six_env_keys_and_never_the_api_key ... ok
test writer::claude_code::tests::the_base_url_is_the_gateway_origin_without_a_v1_suffix ... ok
test writer::claude_code::tests::model_values_default_to_the_family_slots_and_overrides_win ... ok
test writer::claude_code::tests::the_config_root_is_dot_claude_under_userprofile ... ok
test result: ok. 9 passed; 0 failed
```

- **写 → 逐字核对**：`write_appends_the_six_env_keys_verbatim_and_keeps_the_rest` 种一份 `{"model":"opus","env":{"MY_FLAG":"1"}}`，断言整份文件**逐字**等于手写期望值（`MY_FLAG` 留在原位，六个网关键按写清单顺序追加）：

```json
{
  "model": "opus",
  "env": {
    "MY_FLAG": "1",
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:8787",
    "ANTHROPIC_AUTH_TOKEN": "cg-test-token",
    "ANTHROPIC_DEFAULT_FABLE_MODEL": "claude-fable",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku"
  }
}
```

- **别的键与别的 `env` 键原样保留**：`user_keys_and_a_user_owned_api_key_survive` 的种子带 `permissions.allow`、`env.MY_FLAG`，还带**用户自己的** `env.ANTHROPIC_API_KEY = "user-owned-key"`；断言顶层键序仍是 `model / permissions / env`、`permissions` 与 `MY_FLAG` 一个字节没变、用户那把 `ANTHROPIC_API_KEY` 原样留在那里（网关没碰它）。
- **还原零残留**：`restore_drops_the_owned_keys_and_the_emptied_env_shell` 从 `{"model":"opus"}` 写→还原后**逐字**回到 `{"model":"opus"}`（`env` 空壳被 `RemoveIfEmpty` 摘掉）；`restore_keeps_an_env_that_still_has_user_keys` 种 `{"model":"opus","env":{"MY_FLAG":"1"}}` 写→还原后逐字回到原样（只摘六个键，`env` 留着）。
- **幂等**：`second_write_is_a_noop_and_mtime_does_not_move` —— 第二次 `FileOutcome::Unchanged`，mtime 与字节都不变。
- **接管键清单**：`the_owned_key_list_is_the_six_env_keys_and_never_the_api_key` —— 写清单逐字等于那六个 `env` 键、**不含** `ANTHROPIC_API_KEY`；还原清单是同一份路径且全是 `RemoveIfEmpty`；`rel_path` 是相对的（真实根由调用方给）。
- 根来自环境变量：`the_config_root_is_dot_claude_under_userprofile`（`config_root_from(r"C:\Users\me") == r"C:\Users\me\.claude"`，且 `default_config_root()` == `%USERPROFILE%\.claude`）。

**2）`ANTHROPIC_BASE_URL` 带不带 `/v1`** —— 见上「实测结论：不带 `/v1`」，命令是隔离 `CLAUDE_CONFIG_DIR` 跑三轮真客户端（脚本 `%TEMP%\cg47-probe\run-probe.ps1`，证据 `%TEMP%\cg47-probe\evidence.txt`，两者都在仓库外）。

**3）全库与全盘反查 `ANTHROPIC_API_KEY`**（脚本 `%TEMP%\cg47-probe\sweep.ps1`、`sweep-artifacts.ps1`；只打印命中位置与行号，不打印行内容）

全库（每条分支的 `crates/ app/ src-tauri/`）：

```
[A] 全库（每条分支的 crates/ app/ src-tauri/ 源码）
[integration/model-gateway] 0 处
[main] 0 处
[task/40…43/45/46/47/48/54] 0 处
[task/49-provider-table-catalog-prices]
task/49-…:crates/gateway-core/src/egress/models_dev_snapshot.json:5:    "env": ["ANTHROPIC_API_KEY"],
--- 未跟踪文件（本票新写的 claude_code.rs 还没提交）---
crates/gateway-core/src/writer/claude_code.rs:6,8,10,35,218,224,237,239,280,295,296
```

- 唯一一处「源码里的命中」是 `task/49` 分支的 **models.dev 快照**（第三方 provider 声明自己要哪个环境变量），是**数据**不是写入路径，且不在本票分支上。
- 本票文件里的 11 处命中全是**注释与断言**：模块文档「绝不写」、以及那条「用户自己的 `ANTHROPIC_API_KEY` 原样保留 / 接管键里不许出现它」的用例——没有任何一处构造 `Set`。

全盘（网关能落地的每个位置；`扫了 N 个文件 → 命中 M`）：

```
[~\.claude]                    扫了 8 个文件 → 命中 5     （全是 Claude Code 自己的 cache/changelog.md）
[~\.codex]                     扫了 6976 个文件 → 命中 12 （全是 Codex 自带的插件文档）
[%LOCALAPPDATA%\Claude-3p]     扫了 371 个文件 → 命中 221 （claude.exe / rootfs.vhdx 二进制，以及 Claude Desktop 自己写的 host-creds-*.json）
[%APPDATA%\com.claudegateway.desktop]  扫了 4 个文件 → 命中 0  （网关自己的 gateway.db + logs）
```

- 用户自己的 `~/.claude/settings.json`（37 字节，`autoUpdatesChannel`，**不是**网关写的）：`含 ANTHROPIC_API_KEY: False`。
- `%LOCALAPPDATA%\Claude-3p\host-creds-*.json` 里那个 `ANTHROPIC_API_KEY` 是 **Claude Desktop 自己**写的（同文件里是空串，票 24 已记录），不是本网关；网关的 Desktop 写入器（票 46）落 `configLibrary\`，两回事。
- 只看**网关这次真实产出的东西**（临时根 `%TEMP%\cg47-claude-code-tests\verbatim`：配置 / 备份 / `gateway.db`（含 `-wal` `-shm`）/ `logs\writer-*.log`）：

```
backups\claude-code\settings.json.20261015-213133: 0 （二进制扫）
gateway.db: 0   gateway.db-shm: 0   gateway.db-wal: 0 （二进制扫）
home\.claude\settings.json: 0
logs\writer-20261015.log: 0
```

- 唯一「命中 2」的临时目录是 `cg47-claude-code-tests\user-keys\`：那是**种子里用户自己那把** `user-owned-key`（以及它的首写备份），正是断言「网关没动它」的那份夹具。

**4）真实问答 + `/api/logs`** —— 本票做不到（网关此刻没有可用绑定，`forwarding = None`；日志页是票 53）。**留给票 55 现场验收**，步骤见下。

**5）门禁**

```
pwsh -File ./check.ps1 -Quick -Replay      → exit 0，全部通过。
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
test result: ok. 79 passed …        （票 45 时是 70，本票 +9）
test result: ok. 4 passed …          test result: ok. 1 passed …          test result: ok. 5 passed …
=== clippy (mock 特性：两个测试 bin + 回放测试) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===   test result: ok. 20 passed
=== replay-unit (回放器切法单测，mock 特性下的 bin) ===          test result: ok. 5 passed
```

### 留给票 55 的步骤（真实 Claude Code 走网关完成一次问答 + `/api/logs` 看得到）

1. 票 49/51 先把绑定与四家族槽位配好，网关起在 `127.0.0.1:8787`（`service::DEFAULT_PORT`），确认 `POST /v1/messages` 能出真答复（回放套件已覆盖转发面）。
2. 用真实运行路径写配置（**不是**测试临时根）：`default_config_root()` = `%USERPROFILE%\.claude`，`file_spec(&gateway_root_url(port), &token, &ModelSlots::default())`，`WriteRoots::with_data_dir(config_root, store::default_data_dir()?)`。
3. `claude -p "hello"`（零官方登录；本机 2.1.299）。**改配置后要完全退出重启**；**别用 `/logout` 清场**。
4. 看 `/api/logs`（票 53）：应有这次 `POST /v1/messages?beta=true` 的元数据记录。
5. 还原：`writer.restore(...)` → 逐字检查 `~/.claude/settings.json` 里网关六键零残留（用户自己的键还在）。
6. 顺带验票 24 的 **U2**：全新机器上 `env` 块能否抢在首次运行向导之前生效（本票只验到了「向导之后 + 零登录也能读到」这一档）。

### 遗留与坑

1. **`ANTHROPIC_BASE_URL` 的取值来源**：本票写的是「调用方传入的根地址」，`gateway_root_url(port)` 只是拼法（`HOST` + `port`，不带 `/v1`）。端口从 `settings.port` 读、还是 `shell::load_port` 那份 `config.json`，归票 51/53 定，本票不越界。
2. **`auth_token` 谁生成、存哪、要不要轮换**没定案：网关不校验入站凭据，写进配置的值是明文（`settings.json` 本来就是明文文件，用户自己的 key 也在那儿）。轮换/更换时按票 28 的冲突判据走：接管键现值和库里记录不一致 → 放弃 + 提示，或用户显式「认领并覆盖」。
3. **`ModelSlots` 默认是槽位名**（`claude-fable` 等）。哪一天产品要让 Claude Code 用覆盖名，只需调用方换一个 `ModelSlots`，引擎与本模块都不用改。
4. **`Format::Jsonc` 是放宽解析、不是放宽写入**：输出永远是严格 JSON（引擎重序列化）。副作用与 ADR-0006 第 32 行宣称的一样：真写入会把用户写在 `settings.json` 里的注释洗掉（无变化不写，所以平时不碰）——若哪天要保注释，那是引擎侧的事。
5. **`ANTHROPIC_DEFAULT_MODEL`（新会话默认模型，v2.1.236+）没写**：它不是家族变量（票 24 §1.6），写它是产品选择；要写就是再加一条 `OwnedOp`，不动引擎。
6. 本票**没有**碰 `wrap` / 注册表托管路径（spec §8 表下注：本期不做），也没碰 `apiKeyHelper`。
7. 探针脚本、快照脚本与证据都在仓库外（`%TEMP%\cg47-probe\`），**不进仓库**；`.scratch/model-gateway/prototype/` 只读未动。

