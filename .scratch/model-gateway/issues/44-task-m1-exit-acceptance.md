# M1 收口：地基验收

Type: task
Status: resolved
Blocked by: 41, 42, 43

## Question

把前四张票接起来走 M1 收口验收（spec §12 的 M1 那两条）。这是**主会话 + 用户在场**的验收动作，不是可甩给 worktree 子代理的实现任务。

依据：spec §12 M1、[票 38](../issues/38-grilling-implementation-order.md)。

## 交付

- 网关的入站路由与存储接进 Tauri 生命周期（不是两个进程）。
- 应用数据目录 `%APPDATA%\com.claudegateway.desktop\` 首次启动自建，库落在那儿。
- 一份 M1 验收记录（贴进本票 Answer）：`check.ps1` 输出、`/api/status` 实样、托盘与关窗行为的观察。

## 收口验收

- `check.ps1` 全绿，且**单元测试与回放格数不减**（对照原型：单测 ≥ 29、回放 12 格）。
- 应用能启动、托盘常驻、窗口能开关、关窗按 §10 隐藏到托盘。
- 库与密钥能读写（界面上做一次接入即可，或 API 实测）。
- 端口被占时报错清楚。
- **用户点一次托盘与窗口并签字。** 此段结束仍不能日常用（还不能配客户端）。

## Answer

**M1 收口通过**（2026-10-15，主会话 + 用户在场；分支 `integration/model-gateway`，收口提交 `ac40394`）。

### 交付

- **转发面接进 Tauri 生命周期（同进程，不是 sidecar）**：票 43 的 `shell.rs` 起网关时把 `forwarding` 传成 `None`（当时转发面还是空挂载点），票 41 落地转发链后这是一处必须补的接线——本票在 `src-tauri/src/shell.rs` 里改由外壳 `Store::open(&data_dir)`，装配 `ingress::proxy::Gateway`（供应商表 / 绑定表此刻为空，归票 49 / 51），挂进 `GatewayConfig.forwarding`；`/api/*` 与 `/v1/*` 同一进程、分开挂，`service.rs` / `api.rs` 一行未改。
- **数据目录首次启动自建**：`%APPDATA%\com.claudegateway.desktop\`（`store::default_data_dir()` 单一定义），库落在 `gateway.db`。
- 顺手修回一笔回归：票 39 建的根 workspace 把冻结原型吞进去了，`.scratch/model-gateway/prototype/` 里 `cargo run` 直接报 *"current package believes it's in a workspace when it's not"*。在根 `Cargo.toml` 加 `exclude = [".scratch/model-gateway/prototype"]` 修回（**不动原型文件本身**），ADR-0007 要求的「要复现原型那条链路就跑冻结目录」恢复成立。

### 收口验收（逐条真跑）

**1. `check.ps1` 全绿，单测与回放格数不减**

```
$ pwsh -File .\check.ps1 -Quick -Replay
=== fmt (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit (cargo test --workspace) ===        bin 5 + lib 37 + gateway_service 4 passed
=== replay (--features mock --test replay) === 20 passed
=== replay-unit (--features mock --bins) ===   5 passed
全部通过。   exit 0
```

- **单测 46 个**（原型 23；票面要求 ≥ 29）——`cargo test --workspace -- --list` 计 46。
- **回放 25 格**，与冻结原型逐名比对**无缺失**：原型 `--test replay -- --list` 25 个用例，产品侧 20 个在 `tests/replay.rs`、5 个 `sse::tests::*` 落在 lib 单测里（`comment_frames_do_not_swallow_the_next_frame` / `crlf_terminator_is_understood` / `single_byte_chunks_are_buffered_until_the_frame_ends` / `splits_frames_and_keeps_order` / `unfinished_tail_is_still_parsed_on_finish`，姓名一致）。`fixtures/` 46 文件、`expected/` 15 份黄金。
- 冻结原型的 `--test replay` 现在能跑（上面那笔修复后）：`25 tests, 0 benchmarks`。

**2. 应用能启动、托盘常驻、窗口能开关、关窗隐藏到托盘**

`/api/status` 实样（真实外壳进程）：

```json
{"version":"0.1.0","port":8787,"running":true,"startedAt":"2026-10-15T13:09:57Z","uptimeSeconds":5,
 "dbPath":"C:\\Users\\ZHANQUAN\\AppData\\Roaming\\com.claudegateway.desktop\\gateway.db"}
```

转发面确认真挂在真实外壳上（不再是裸 404，而是入站协议的错误信封）：

```
$ curl -s -X POST http://127.0.0.1:8787/v1/messages -H 'content-type: application/json' \
    -d '{"model":"nope-model","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'
HTTP 404
{"error":{"message":"未知虚拟模型 \"nope-model\"","type":"not_found_error"},"type":"error"}
```

关窗机械面：向主窗口发 `WM_CLOSE`（等价点 ×）→ 窗口句柄在但 `visible=False`、进程在、`/api/status` 仍 200（uptime 继续涨）。托盘菜单文案、图标换色、左键开窗这几项由用户在下面第 5 条当场确认。

**3. 库与密钥能读写**

真实数据目录里的库（应用启动后自建）：

```
11 张表：client_config_state, client_profiles, credentials, model_catalog, request_bodies,
        requests, settings, sqlite_sequence, upstream_models, upstreams, virtual_models
PRAGMA user_version = 2
client_profiles = [claude-code, claude-desktop, codex, global]
settings = [(port,8787), (log_retention_days,7), (usd_cny_rate,7.10), (catalog_snapshot_at,"")]
```

密钥：票 42 的 12 条 `store::` 单测（DPAPI 往返 / 掩码 / 库内明文反查带正对照）在门禁里全绿，本票复核无回归。

**4. 端口被占时报错清楚**

先占住 8787 再启动应用（外壳照常存活，`hasExited=False`）：

```
[2026-10-15T13:09:22Z] 网关启动失败：端口 8787 已被占用（127.0.0.1:8787 绑定失败：Address already in use）。
该地址已写进客户端配置，网关不会自动换端口；请先关掉占用 8787 的程序，或在设置页改用空闲端口后重启。
```

同时 `/api/status` 不可达（监听没绑上），符合「故障态」而非静默换端口。

**5. 用户点一次托盘与窗口并签字**

用户当场逐项确认 6 条全过并签字：① 右下角托盘有图标（绿色=运行中）；② 右键五项文案正确，`Claude Desktop 1P↔3P 快切` 是灰色禁用占位；③ 左键单击图标弹出窗口；④ 点 × 关掉后托盘图标仍在、`/api/status` 仍可用；⑤ 托盘「退出」后进程消失、`/api/status` 打不开；⑥ 第二次双击 exe 被单实例挡住，不会出现两个进程。

### 遗留

- 托盘「Claude Desktop 1P↔3P 快切」仍是禁用占位（要动客户端配置，归票 45 / 46 的写入引擎）。
- `/api/status` 的端口仍读 `<数据目录>\config.json` 的 shortcut（票 43 留的），目标态是读库 `settings.port`；接上后行为不变。
- 供应商表与绑定表为空：`/v1/*` 现在能把请求解到「未知虚拟模型」，但**还不能配客户端**——这正是 spec §12 说的「此段结束仍不能日常用」。
