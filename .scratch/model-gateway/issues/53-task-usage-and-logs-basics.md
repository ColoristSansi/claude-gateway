# 用量与日志基础

Type: task
Status: resolved
Blocked by: 42, 51

## Question

做 M2 需要的那半可观测：请求元数据落库、成本按当时价冻结、保留 7 天自动清理、日志页元数据表格、body 开关与脱敏。

依据：spec §9、[票 33](../issues/33-grilling-cost-accounting.md)、[票 17](../issues/17-grilling-ui-information-architecture.md)、[票 30](../issues/30-task-build-regression-suite.md) 第 7 条。

## 交付

- `requests` 表写入：虚拟模型 / 上游 / 客户端 / 状态码 / 耗时 / 首字延迟 / token / `cost_usd`（**请求时按当时价冻结**）。
- 价格口径：手工 > models.dev 快照 > 未知；缺价不计入总额、另计「N 次无价格 / N 次 token 未知」；上游没回 usage 标「token 未知」，不用本地估算反算。
- 保留 7 天自动清理。
- 日志页：元数据表格（时间倒序 50 条、按客户端 / 上游 / 虚拟模型 / 状态码筛选 + 模型名与请求 ID 搜索）。
- body 开关：默认关，开启需隐私确认，**1 小时后自动关**。
- 脱敏：API 面、库、诊断包里都查不到密钥明文（含 `GATEWAY_LOG_BODY` 开着时）。

## 收口验收

- 请求落库且成本按当时价冻结；改手工价或刷新快照**不追古**。
- body 默认不写 → 开启可捞 → 到点自动关（票 30 的窗口断言照搬）。
- 拿已知密钥串在库与 API 响应里反查不到。

## Answer

（2026-10-15，`task/53-usage-and-logs-basics`，基于集成分支 `f630830`；收工时 `git merge
integration/model-gateway` 报 **Already up to date**，无需解冲突）

### 做了什么

**落库写入路径**（`crates/gateway-core/src/obs/`）

- 每一行 `requests` 现在自带 **`request_id`**（`req-<微秒><序号>`，落库时现生成；调用方也能先塞一个）。
  之前这一列一直是 NULL，日志页的「按请求 ID 搜索」与票 59 的详情页都无从下手。
- **`cost_usd` 在写这一行的事务里按当时价算好**：`obs::log` 用**同一条写连接**（`c.transaction()`
  之前）读上游目录的手工价 + models.dev 快照，`provider::resolved_prices_from(c, upstream, model)`
  是为此从票 49 的 `resolved_prices` 里抽出来的同一份逻辑（异步版改成调它）。
- 价格口径（票 33 第 1、3、4 节，纯函数 `obs::resolve_cost`）：
  **手工 > 快照 > 未知**，逐项判；`cache_read` 缺缓存读价**退输入价**；`cache_write` 本期不计（票 58）；
  **缺价 → 整条 NULL**（不半算、不记 0、不计入总额）；**三项 token 全空 = token 未知 → 同样 NULL**，
  不用本地估算反算。
- **保留期**：`obs::purge` 读 `settings.log_retention_days`（初值 7 天）删 `requests` 的老行，
  `request_bodies` 靠外键级联一起走；`obs::spawn_purge_sweeper` 由 `service::start` 挂上
  （**启动立刻一次** + 之后每小时一次），只在转发面挂载时起。
- **查询面**：`LogQuery` / `obs::list`（时间倒序、四个筛选维度、模型名 + 请求 ID 搜索、游标翻页、
  默认 50 条上限 500）、`obs::filters`（候选值取自库里真出现过的值）、`obs::usage_summary`
  （**今日 + 近 7 天** × 虚拟模型 / 上游 / 客户端，日边界按**本机本地时区自然日**）、`obs::usd_cny_rate`。

**body 开关**：窗口本体从 `ingress::proxy::Gateway` 搬进 `obs::BodyLogWindow`（`proxy` 的
`body_log_on` / `set_body_log` 保留为门面，`BODY_LOG_WINDOW` 由 `proxy` re-export），
转发面与管理面**共用同一个 `Arc` 实例**——日志页上的开关就是转发路径读的那个开关。

**管理面**（`ingress/api.rs`）：新增 4 个端点（`ApiState::attach_logs` 在 `ingress::router`
里从转发面接库与窗口）。转发面没挂（库开不了那一档）时这 4 个端点回 **503**，不装成空表。

**前端**：`app/` 里先挂**最小壳**（hash 路由 + 七页固定顺序的导航占位）与「日志与调试」页本体
（`app/src/App.tsx`、`app/src/pages/LogsPage.tsx`、`app/src/api.ts`、`app/src/app.css`）。
票 52 的壳层并进来后**以 52 的壳为准**，我的 `App.tsx` 让位（页面本体可直接搬）。

**测试**：`src/obs/tests.rs` 13 条单测、`tests/logs_api.rs` 4 条集成（真实 HTTP，自起一份
`ingress::router`）、回放套件新增 1 格 `body_recording_switch_drives_the_proxy_window`
（走管理面真实端点拨窗口，证明与转发路径同一个窗口）。`gateway-harness` 顺带把管理面也挂上，
日志页那几条验收才走得到真实 HTTP。

### 给票 59 的接口（端点与返回形状）

| 端点 | 返回 |
| --- | --- |
| `GET /api/logs?limit&cursor&client&upstream&virtual_model&status&q` | `{logs:[行…], limit, next_cursor, rate, retention_days}`；**行**：`id,ts(Unix 秒),request_id,virtual_model,client,upstream,real_model,wire,status,first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,aborted,error,cost_usd,body`（`body` = 出站请求体，只在开关开着时有；**`cost_usd: null` = 当时无价**，不是 0）。`status` 取值 `ok`/`4xx`/`5xx`/`error`；`q` 搜虚拟模型名 / 真实模型名 / 请求 ID（**不搜 body**）；`next_cursor` 只在满页时给，翻页传 `cursor=<上一页最后一行的 id>` |
| `GET /api/logs/filters` | `{clients:[],upstreams:[],virtual_models:[],statuses:["ok","4xx","5xx","error"]}` |
| `GET /api/logs/body-recording` | `{on, seconds_remaining, window_seconds:3600, retention_days}` |
| `POST /api/logs/body-recording` | 体 `{enabled, consent, seconds?}`（`seconds` 上限 3600，测试拨短用）；`enabled=true` 而 `consent!=true` → **400** `{error:{type:"consent_required", message}}`，开关不动；回同上那份状态 |
| `GET /api/usage/summary?range=today\|7d&dim=virtual_model\|upstream\|client` | `{range,dim,from,totals:{cost_usd,requests,input_tokens,output_tokens,cache_read,no_price,token_unknown},rows:[同形状 + key],rate}`；`cost_usd: null` = 这一段一条都算不出价；**`no_price` 与 `token_unknown` 是两句话、不重复计**。`range` 只认 `today`/`7d`，别的值回 **400**（近 30 天 / 全部 / 自定义区间 + 按天明细 + 详情四段归票 59） |

给票 52/59 的三条附注：① `rate` 只读 `settings.usd_cny_rate`，是**显示层**用的，库内一律美元；
② `virtual_model.switch` 那侧的 `client` 列现在真的有值了（票 51 起的档案 id），按客户端维度聚合可用；
③ `request_id` 是概览「跑一次端到端验收」跳日志详情要的那把钥匙（票 17 第 29 行）。

### 验收（逐条真跑）

**1) 请求落库且成本按当时价冻结；改手工价 / 刷新快照不追古**

```
> cargo test -p gateway-core --lib obs::tests::cost_is_frozen_at_write_time_and_not_recomputed
test obs::tests::cost_is_frozen_at_write_time_and_not_recomputed ... ok
test result: ok. 1 passed; 0 failed
```
证据两条（都在这一格断言里，期望值是手算字面量）：① 落库后 `cost_usd == $0.0105`
（1000 tok × $3/Mtok + 500 × $15/Mtok）；② 随后把手工价抬 10 倍**并**换了一份 models.dev 快照，
同一行的 `cost_usd` 仍是 `$0.0105`（不追古），而下一次请求记的是新价 `$0.105`（说明价是「当时」读的、
不是启动时缓存的）。旁证：`snapshot_fills_prices_that_manual_leaves_empty`（快照兜底）与
`unknown_price_or_unknown_tokens_yield_no_cost`（缺价 / token 未知 → NULL）各 ok。

**2) body 默认不写 → 开启可捞 → 到点自动关**（票 30 第 7 格的窗口断言照搬，窗口拨到 2 秒）

```
> cargo test -p gateway-core --features mock --test replay body_recording_switch_drives_the_proxy_window
test body_recording_switch_drives_the_proxy_window ... ok
test result: ok. 1 passed; 0 failed
```
这一格走的是**管理面真实端点** `/api/logs/body-recording`：① 默认 `on:false`、body 列 null；
② `{enabled:true}` 无确认 → HTTP 400 `consent_required` 且开关仍是 false；③ 带确认 + 窗口 2 秒 →
`on:true`、`seconds_remaining<=2`、这一行的 body 就是发给上游的那份（含 `你好` / `mock-text`）；
④ 2.2 秒后再看 `on:false`，那一发 body 又是 null；⑤ `{enabled:false}` 手动关也是同一条路。
票 30 原有的 `logs_are_metadata_only_and_redacted`（走 `/__bodylog`）同时仍绿。

**3) 已知密钥串在库与 API 响应里反查不到（带正对照）**

```
> cargo test -p gateway-core --test logs_api logs_api_and_db_never_contain_the_known_secret
test logs_api_and_db_never_contain_the_known_secret ... ok
test result: ok. 1 passed; 0 failed
```
已知密钥 `gw-test-secret-53-known-plaintext-9f3a` 以 DPAPI 密文入库（与真实装库同一条路）。反查面：
`/api/status`、`/api/logs`、`/api/logs/filters`、`/api/logs/body-recording`、
`/api/usage/summary`×2、**body 开关开着时的 `/api/logs`**，外加数据目录全部文件字节
（含 `-wal`）：**8 个面 + 库字节，零命中**；另断言库里连 `gw-test-` 这 7 字节片段都没有。
**正对照**（否则「查不到」可能只是查错了地方）：同一份库字节里必须找得到我们明知存在的
`claude-haiku-real`，同一个响应集合里必须找得到虚拟模型名 `claude-haiku`——两条都过。

**4) 保留期清理：8 天前的行清掉、7 天内的还在**

```
> cargo test -p gateway-core --lib obs::tests::purge_drops_rows_past_retention_and_keeps_recent_ones
test obs::tests::purge_drops_rows_past_retention_and_keeps_recent_ones ... ok
test result: ok. 1 passed; 0 failed
```
造两条行（`now-8d` 带 body、`now-6d` 不带）→ `obs::purge` 回 **1**（只清那条 8 天前的）→
`now-6d` 那条还在、`request_bodies` 计数回 **0**（body 随外键级联走）。
旁证 `retention_days_come_from_settings`：初值 7 天来自 `settings.log_retention_days`，
把它改成 30，`purge` 的判据跟着变。清理表本身由 `service::start` 挂（启动一次 + 每小时一次）。

**5) 门禁**

```
> pnpm --dir app build
$ tsc --noEmit && vite build
✓ 18 modules transformed.
dist/assets/index-DRFUxrDd.js   227.62 kB │ gzip: 71.75 kB
✓ built in 237ms            (exit 0)

> pwsh -File ./check.ps1 -Quick -Replay
=== fmt    (cargo fmt --all --check) ===
=== clippy (cargo clippy --workspace -D warnings) ===
=== unit   (cargo test --workspace) ===
test result: ok. 120 passed; 0 failed; 3 ignored      ← 库单测（含 obs 13 条）
test result: ok. 4 passed; 0 failed                   ← tests/logs_api.rs
=== clippy (mock 特性：两个测试 bin + 回放测试) ===
=== replay (集成：自起网关 + mock 两个进程，回放 fixtures) ===
test result: ok. 22 passed; 0 failed                  ← 含本票新增的那一格
=== replay-unit (回放器切法单测) ===
test result: ok. 5 passed; 0 failed
全部通过。
```

### 遗留与坑

1. **响应体与帧时间轴没有落点**：`request_bodies.response_body` 仍恒为 NULL——本票只按票面做了
   「body 开关 + 出站请求体」，票 59 的详情第②③段（响应体 / 帧时间轴 / 原始 SSE）要先定落点
   （扩 `request_bodies` 还是新增帧表，蒸馏笔记 §⑤.5 已标成缺口）。
2. **`/api/logs/:requestId`（详情四段）、`/api/logs/clear`（清空日志）、`/api/usage` 的
   近 30 天 / 全部 / 自定义 + 按天明细**都没做，按领地归票 59（`clear` 在蒸馏表里标 52/59）。
3. **前端跨源**：开发态页面在 `localhost:1420`、网关在 `127.0.0.1:8787`，`/api` 是跨源，
   当前 `app/src/api.ts` 里写死基址（可用 `VITE_API_BASE` 覆盖）且**还没有 CORS**。
   打包态由外壳决定怎么把 `/api` 暴露给 webview（同源反代或注入基址）——归票 52 / 43 的壳层。
4. **币种只做了显示口径**：日志页与 `/api/usage/summary` 都回 `rate`（读 `settings.usd_cny_rate`），
   页面可切 ¥/$；汇率手改入口与「概览跟随当前选择」的全局态归票 59。
5. **回放套件偶发一次端口冲突**：首次跑门禁时 `fixtures_replay_through_the_gateway` 报
   `连接被重置` 且两个 bin 各打了一条 `os error 10048`（端口已被占用）。票 30 的端口是
   「进程 id + 序号」现算的固定段（19000 起），与 `unit` 步里 `free_port()` 拿到的系统临时端口
   有撞上的可能；重跑即绿（本票收口的第二次门禁与单独重跑各一次都 22/22 绿）。
   本票没动那套端口算法（不越界），但**门禁偶发红的这一格值得票 30 或收口票留意**。
6. **`cache_write` 仍未采集**（票 33 的已知缺口，票 58 补列后再计）；本票的 `resolve_cost`
   已经把口径写成「三项」，补列时只需加一项。
