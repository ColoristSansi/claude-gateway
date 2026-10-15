# 用量 / 日志 / 设置三页补齐

Type: task
Status: resolved
Blocked by: 52, 53, 58

## Question

把七页补齐到 spec §10 第 5–7 节的完整形态（M2 只做了「用量与日志基础」）。

依据：spec §10 第 5–7 节、[票 33](../issues/33-grilling-cost-accounting.md)、[票 17](../issues/17-grilling-ui-information-architecture.md)。

## 交付

- **用量与成本页**：时间范围今日 / 近 7 天 / 近 30 天 / 全部 / 自定义区间（日边界按本机本地时区自然日）× 三维度（虚拟模型 / 上游 / 客户端）+ 按天明细表；花费、请求数、token 图表；**人民币为主、可切美元**（汇率随包初值 + 设置页手改，不联网）；两位小数、不足 ¥0.01 显示 `<¥0.01`。
- **日志页详情四段**：概要 / 请求与响应体 / 帧时间轴（可切原始 SSE 文本）/ 错误信息。
- **诊断包**：版本、端口与运行状态、上游配置的脱敏形式、错误原文、相关请求元数据；**不含密钥明文与 body**。
- **设置页**：端口、开机自启（开关本体归 [票 62](../issues/62-task-autostart-uninstall-export.md)）、托盘行为、日志保留期与 body 开关（与日志页同一个）、数据库位置与打开目录、导出 / 导入、models.dev 快照刷新、汇率手改、关于与版本。
- 侧栏徽标：未通过自检的上游数与未生效的客户端数。

## 收口验收

- 七页齐且导航不合并、不增设。
- 用量口径与 [票 33](../issues/33-grilling-cost-accounting.md) 逐条对得上（含无价 / token 未知的计数与不计入总额）。
- 诊断包与导出文件里反查不到密钥明文。

## Answer

（2026-10-15，`task/59-remaining-three-pages`，基于集成分支 `ac1becf`（票 58 tip）；收工时
`git merge integration/model-gateway` 报 **Already up to date**，无需解冲突。实现 commit `83b8d52`，
本 Answer 随后单独提交。）

### 做了什么

**后端（`/api/*` 与数据层）**

- **用量**（`crates/gateway-core/src/obs/mod.rs`）：`range_window` 把时间范围从票 53 的 today/7d 扩到
  **today / 7d / 30d / all / custom**，日边界一律按**本机本地时区自然日**；`custom` 的 `from`/`to`
  是界面给的本地日期 `YYYY-MM-DD`，`to` **含当天**（右端取下一天 0 点，左闭右开）。`usage`
  返回三维度分组 + 总计 + **按天明细** `daily`（日期串是本地 `YYYY-MM-DD`）；`usage_summary`
  变成它的薄壳——概览与用量页**共用一份实现**（`frontend.md` §③ 第 10/39 条要的就是这个）。
  分组与总计都带 `no_price` / `token_unknown` 两个计数，**都不计入总额**。
- **详情三段的数据源**：`obs::log_detail`（概要 / 请求与响应体 / 帧）与 `obs::clear_logs`
  （删 `requests`，body 与帧随外键级联）。
- **迁移 v8**（`store/schema.rs`）：新建 `request_frames`；`SCHEMA_VERSION = 8`。
- **采集**（`egress/upstream.rs`）：body 开关开着时，流式（`Pump`）与非流式（`forward_buffered`）
  两条出口都记**上游原样的整条 SSE** 与**逐帧元数据**（序号 / 相对开始毫秒 / 事件名 / 摘要 / 帧原文）。
- **顺手修的老问题**：流式 Anthropic 的**输入 token 一直没落库**——`message_start` 报的
  `usage.input_tokens` 走的是 `IrEvent::Start`，而 `Pump` 只从 `IrEvent::Usage` 取 token，
  上游不在 `message_delta` 里重复报时整条就是 NULL（输入 token 少算、用量页跟着少）。
  两条出口现在都从 `Start` 取一次。
- **管理面**（`ingress/api.rs`）：`GET /api/usage`（与 `/api/usage/summary` **同一个 handler**）、
  `GET /api/logs/{id}`（`:id` 认行 id 也认 `request_id`，查不到 **404**）、`POST /api/logs/clear`。
  新模块 `ingress/api/settings.rs`：`GET/PUT /api/settings`、`POST /api/settings/db/open`、
  `POST /api/catalog/refresh`、`GET /api/about`。诊断包的「运行状态」从写死 `true` 改成如实读，
  错误模板函数提成 `pub(crate)` 与日志详情第四段**共用一份**。
- **外壳**（`src-tauri/src/shell.rs`）：托盘行为读 `settings.tray_close_action`
  ——`hide`（默认，关窗隐藏到托盘、网关继续跑）/ `quit`（关窗退出应用）。

**前端（`app/`，导航仍是七项）**

- `pages/UsagePage.tsx`：五个范围 × 三个维度 + 条形图（花费 / 请求数 / token）+ 分组表 +
  **按天明细表**；无价与 token 未知两句单独列在总额卡下面；币种可切。
- `pages/SettingsPage.tsx`：端口（探测预填 → 确认后写入，回执报已同步客户端数）、开机自启
  （票 62 的后端，文案用它给的原文）、托盘行为、日志保留期与 body 开关（**与日志页同一个端点**）、
  数据库位置与打开目录、导出 / 导入（票 62 的后端 + 它的 DPAPI 原文与导入回执）、
  models.dev 快照刷新、汇率手改、关于与版本（含回退指引与免安装提示）。**整页没有密钥输入框。**
- `pages/LogsPage.tsx`：列表那半原样保留，新增右侧**详情抽屉四段**（逐字：概要 / 请求与响应体 /
  帧时间轴（可切「逐帧」与「**原始 SSE 文本**」）/ 错误信息）、「复制诊断」与「清空日志」
  （走二次确认）。
- `currency.ts` + `components.CurrencySwitch` + `money()`：币种是**全局显示态**（用量页切了，
  概览与日志跟着变，记在 localStorage）；金额一律**两位小数**，不足一个最小单位显示
  `<¥0.01` / `<$0.01`，没价显示「未知」。
- 侧栏徽标沿用票 52 的 `/api/sidebar/badges`，本票复核口径未变（未通过自检的上游数 / 未生效的
  客户端数；拉取失败与发现警告都不计入）。

### 帧时间轴与原始 SSE 的落点、迁移版本

| 项 | 落点 |
| --- | --- |
| 逐帧元数据 | **新表 `request_frames(request_id, seq, at_ms, event, summary, raw)`**，一帧一行，`request_id` 外键 → `requests(id)` `ON DELETE CASCADE`（清空日志与保留期清理同一条路） |
| 原始 SSE 文本 | **`request_bodies.response_body`**（v2 建表时就有这一列，票 53 之前一直 NULL）：存**上游原样的整条字节**（含注释帧与 CRLF），不另建列 |
| 迁移版本 | **v8**（`SCHEMA_VERSION = 8`，`v8_request_frames`）；v7→v8 只加一张表，既有数据一列不动 |

- **为什么自定落点**：spec §9 的表草案里没有帧 / 事件的落点（`frontend.md` §⑤.5 记的就是这个缺口），
  票 53 只落了出站请求体。按票面「自己定落点并写清」，选了一帧一行（好查、随请求级联删），
  帧上的 `raw` 是解析结果的重建（`sse::frame`），**原始 SSE 那一份是字节原样的**。
- **闸门与上限**：与「记录完整 body」**同一个开关**（响应内容也是内容）。上限 500 帧 / 256 KiB
  （`shortcut:` 注释在 `egress/upstream.rs`）。开关关着时详情第二、三段为空并说明原因。

### 验收（逐条真跑）

**1) 七页齐、导航不合并不增设**

```
> pnpm --dir app build
$ tsc --noEmit && vite build
✓ 26 modules transformed.
dist/index.html                   0.41 kB │ gzip:  0.28 kB
dist/assets/index-CpFfDjgy.css    6.74 kB │ gzip:  2.11 kB
dist/assets/index-DnM250x6.js   298.53 kB │ gzip: 89.28 kB
✓ built in 92ms                                        (exit 0)
```

路由表（`app/src/App.tsx` 的 `PAGES`，顺序与 spec §10 / 票 17 逐字一致）：

| # | slug | 标题 | 归属 |
| --- | --- | --- | --- |
| 1 | `overview` | 概览 | 票 52 |
| 2 | `upstreams` | 上游（徽标：未通过自检的上游数） | 票 52 |
| 3 | `models` | 模型与映射 | 票 52 |
| 4 | `clients` | 客户端（徽标：未生效的客户端数） | 票 52 |
| 5 | `logs` | 日志与调试（列表 53 + 详情四段 59） | 53 + 59 |
| 6 | `usage` | 用量与成本 | **本票** |
| 7 | `settings` | 设置 | **本票** |

`grep -c "slug: '" app/src/App.tsx` = **7**（不合并、不增设）；产物里七个标题各命中 1 次，
`按天明细 / 帧时间轴 / 原始 SSE 文本 / 逐帧 / 复制诊断 / 记录完整 body / 开机自启 / 托盘行为 / 汇率`
都在包里。**没有截图**：本会话没有浏览器自动化，见遗留 5。

**2) 用量口径与票 33 逐条对得上（含无价 / token 未知的计数与不计入总额；三个维度都验；
自定义区间的日边界按本地时区）**

```
> cargo test -p gateway-core --lib obs::  → test result: ok. 21 passed（含本票 4 条）
> cargo test -p gateway-core --test m59_api usage_report → ok
```

- **三个维度**：`obs::tests::usage_report_groups_by_every_dimension_and_lists_local_days`
  对同一份数据按 `virtual_model`（2 行，按花费降序）/ `upstream`（1 行 `u1`）/ `client`
  （`claude-code`, `codex`）分别断言分组键与行数；`m59_api` 那一格再走一遍真实 HTTP 的三个维度。
- **无价 / token 未知单独计数、不计入总额**：`usage_summary_counts_unpriced_and_unknown_tokens_separately`
  （今日 3 条：1 有价 $0.0105、1 无价、1 token 未知 → 总额只算有价的 0.0105；无价那组的
  `cost_usd` 是 `null` 不是 0）；票 58 的 `usage_summary_counts_cache_write_rows_and_totals_it`
  仍绿（四项 token 判据）。
- **日边界按本机本地时区自然日**：`range_windows_use_local_natural_days`——东八区
  `now = 2026-10-15T00:00:00Z` → `today` 起点 `1_791_993_600`（= 本地 10-14 16:00Z），
  期望值用 .NET `DateTimeOffset` 独立算过；同一刻在 UTC 时区下 `today` 起点不同（日边界真的跟着时区走）。
- **自定义区间**：`custom_range_covers_whole_local_days_including_the_end_date`——
  `from=2026-10-01, to=2026-10-15` → `[1_790_784_000, 1_792_080_000)` = **15 个整天**（`to` 含当天）；
  只给 `from` = 到此刻；日期写坏 / 起止倒置都报 400，不静默给空表。
- **按天明细**：`daily` 一行一天（`day` 是本地 `YYYY-MM-DD`），样例里昨天 / 今天 / 40 天前各一行，
  每天的花费等于那天的行之和。
- **一处必须改的旧断言（已改）**：票 53 的 `usage_summary(.., "30d", ..).is_err()` 与
  `tests/logs_api.rs` 里「30d → 400」是本票票面明确要扩的范围（「扩展 `usage_summary` 的
  range / dim」），两处改成 **30d / all 回真数据** + **认不出的范围仍 400**；其余判据一个没动。

**3) 诊断包与导出文件里反查不到密钥明文（带正对照）**

```
> cargo test -p gateway-core --test m59_api diagnostics_and_export -- --nocapture
[t59] 诊断包 1327 字节：密钥明文 0 / 掩码片段 sk- 0 / 入站提示词 0 / 响应体标记 0；正对照 版本 1 / 上游 2
[t59] 导出 1962 字节：密钥明文 0 / 上游 id 4 / 目录条目 2 / 绑定 1 / 掩码前缀 1
test result: ok. 4 passed; 0 failed; 1 ignored
```

- **前提断言**（否则「查不到」是废话）：同一发请求的请求体与响应体真的记下来了
  （`bodies.request` 含 `mock-fast`、`bodies.response` 含 `M59-DELTA-MARKER`）。
- **反查面**：诊断包文本里 `SECRET`（`gw-test-secret-59-known-plaintext-7c1e`）/ `sk-` /
  入站提示词标记 / 响应体标记 / `event: message_start` **各 0 次**；导出文件里密钥明文 **0 次**。
- **正对照**：诊断包里有版本 `0.1.0-test`（1 次）、上游 `"u1"`（2 次）、错误原文与 `request_id`、
  `"running": true` 与 `"port"`；导出文件里有上游 id、目录条目、绑定、掩码前 3 位（`gw-`）。
- 诊断包的边界照旧（spec §10）：版本 / 端口 / 运行状态 / 上游配置的**脱敏形式**（地址 + 掩码 +
  协议 + quirks）/ 错误原文 / 相关请求元数据；**不含密钥明文，也不含 body**。

**4) 日志详情四段能开（含帧时间轴切原始 SSE 文本）**

```
> cargo test -p gateway-core --test m59_api log_detail -- --nocapture
[t59] 详情四段：6 帧 / SSE 730 字节 / 清空 3 行
test result: ok. 4 passed; 0 failed; 1 ignored
```

- 六帧的事件名逐个断言（`message_start` / `content_block_start` / `content_block_delta` /
  `content_block_stop` / `message_delta` / `message_stop`），`seq` 从 0 起、`at_ms` 单调不减、
  第三帧的摘要是流里的 delta 文本。
- **切原始 SSE 文本**：`frames.sse == bodies.response`（同一份**上游原样字节**，730 字节，
  含 `event: message_start` 与 delta 标记）——不是把重建的帧拼起来。
- 开关**关着**时四段里的第二、三段为空（`bodies.recorded=false` / `frames.frames=[]` / `sse=null`），
  前半段就是钉这一条的。
- 非流式出口（`forward_buffered`）也记 6 帧；`/api/logs/{行 id}` 与 `/api/logs/{request_id}`
  都取得到同一条；`/api/logs/999999` 是 **404**；`POST /api/logs/clear` 后行、body、帧一起没了。
- 设置页那一格（`settings_page_reads_and_writes_its_fields`）：`GET /api/settings` 的十项格子
  （版本 / 端口 / 库路径 / 数据目录 / 保留期 7 / 汇率 7.10 / 快照时间 null / 托盘行为 hide /
  body 关 / 自启关）、`PUT` 写三项后读回来、越界与坏取值全 400、`db/open` 只认 data|logs、
  `GET /api/about` 的回退指引与免安装提示、端口写入 200 + 被占端口的 409。
- **联网那一发**（`#[ignore]`，手动跑一次）：

```
> cargo test -p gateway-core --test m59_api live_catalog_refresh -- --ignored --nocapture
[t59] /api/catalog/refresh -> {"refreshed":true,"imported":8868,"at":1792079535,
      "reusedOldSnapshot":false,"message":"已刷新 8868 条模型价，快照时间已更新"}
test result: ok. 1 passed
```

  失败路径由 `provider::refresh_catalog`（票 49 的集成测试：成功与失败两条）覆盖，端点是它的薄壳；
  失败时回 `reusedOldSnapshot: true` 且库里那份快照一个字节没动。

**5) 门禁**

```
> pwsh -File ./check.ps1 -Quick -Replay
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
test result: ok. 8 / 167 / 5 / 14 / 8 / 6 / 5 / 4 / 4 / 0(2 ignored) / 4 / 12 / 14 / 19 / 0(7 ignored) / 1  passed; 0 failed
=== clippy (mock 特性：两个测试 bin + 回放测试) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===
test result: ok. 22 passed; 0 failed
=== replay-unit (回放器切法单测，mock 特性下的 bin) ===
test result: ok. 5 passed; 0 failed
全部通过。                                              (exit 0)
```

本票新增的格子在输出里都能看到：`obs::tests::{range_windows_use_local_natural_days,
custom_range_covers_whole_local_days_including_the_end_date,
usage_report_groups_by_every_dimension_and_lists_local_days,
log_detail_serves_summary_bodies_and_frames_by_both_keys}`、
`store::tests::upgrades_a_v7_db_to_v8_adding_the_frame_table`、
`settings::tests::{db_target_only_knows_data_and_logs, list_files_of_a_missing_dir_is_empty_not_an_error}`、
`m59_api` 的 4 格（+1 条 ignored 的联网格）、`shell::tests::close_hides_unless_the_setting_says_quit`。
`unit` 步的库单测从票 58 的 160 涨到 **167**。

### 留给票 60 的现场步骤

1. 打开应用：侧栏**七个**项各点一遍；「用量与成本」页选五个范围、切三个维度、切美元，
   看总计卡下面的「N 次无价格 / N 次 token 未知」两句话与「按天明细」表。
2. 日志页右上角开「记录完整 body」（隐私确认）→ 用任一客户端发一句 → 点某行「详情」：
   四段依次有内容；第三段点「**原始 SSE 文本**」看整条 SSE；「复制诊断」粘出来确认
   **没有密钥明文、没有 body**。
3. 设置页：改保留期 / 汇率 / 托盘行为各存一次；「探测空闲端口」→ 确认换端口 → 应提示
   「已同步 N 个客户端」（Desktop 提示重启）；开机自启开一次（`reg query
   HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v "Claude Gateway"` 能查到）再关（查不到）；
   「刷新快照」（联网）→ 报刷新条数或「沿用旧快照」；导出一次（默认不含密钥）→ 导入一次；
   「打开目录」起资源管理器。
4. 托盘行为选「关窗退出应用」→ 关窗应整个退出（网关跟着停）；选回「关窗隐藏到托盘」→ 只隐藏。
5. 快照刷新失败路径（临时改 hosts 或拔网）应只提示「沿用旧快照」，旧快照仍在、价格照旧可用。
6. 客户端页与概览的「复制诊断」也点一次，粘出来复核同一套脱敏边界。

### 遗留与坑

1. **「检查更新」未接**：`/api/about` 回 `update.supported = false`，页面上那颗按钮是禁用的
   说明态。自动更新闭环归票 61（公开 Releases + `latest.json` + 签名校验）。
2. **帧与 body 同一个闸门**：开关关着时**没有**帧时间轴（只有元数据）。这是刻意的最小隐私面
   （响应内容也是内容）。若票 60 / 用户要「只记帧元数据、不记 body」，得另加一档闸门，属新票。
3. **上限**：一条超长回复超过 500 帧或 256 KiB 时只留前段（`shortcut:` 注释在
   `egress/upstream.rs`）；要全量另开票。
4. **流式 Anthropic 输入 token 是顺手修的**（见「做了什么」）：票 58 那发真上游在
   `message_delta` 里重复报了 input，所以当时没露出来；不重复报的上游 / mock 原先输入 token
   整条 NULL。改的是采集，不改编码器输出，黄金帧不受影响。
5. **前端交互没人眼点按**：本会话没有浏览器自动化，四段与设置项的**数据源、端点、状态码**由
   `tests/m59_api.rs` 真跑（真实 HTTP + 真库 + 真 SSE），UI 走查就是上面留给票 60 的那五步。
6. **`PUT /api/settings` 的端口项不起外壳重启**：它走 `apply_port_change`（写库 + 同步已生效
   客户端）后只 `set_real_port`，与 `/api/gateway/port` 完全同一条路——按新端口重启归外壳
   （票 60 现场验）。
7. **托盘行为每次关窗开一次库**读一条设置（`shortcut:` 注释在 `shell.rs`）：关窗低频，为它常驻
   一个库句柄不值当。
8. **`request_frames` 只有一条删除路径**（随 `requests` 外键级联）：清空日志与保留期清理共用它，
   没有第二处 DELETE。
9. **旧断言只动了两处**（票 53 的 30d → 400），按本票票面扩范围而改，理由写在测试注释里。
10. **`.gitignore` 的裸 `dist/` 坑已确认不影响本票**：新文件（`ingress/api/settings.rs`、
    `tests/m59_api.rs`、`app/src/pages/{Usage,Settings}Page.tsx`、`app/src/currency.ts`）在
    `git status` 里都真的出现了；`app/dist/*` 仍是构建产物的忽略项。
