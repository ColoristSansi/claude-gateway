# cache_write 采集与计价

Type: task
Status: resolved
Blocked by: 53

## Question

补上 [票 33](../issues/33-grilling-cost-accounting.md) 留下的那一项：`cache_write` 本期不计，因为库里没有该列。现在补列、补采集、补计价。

依据：spec §9、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- `requests`（或 `request_log`）加 `cache_write` 列 + 迁移。
- 两条 wire 的 usage 映射：Anthropic 侧与 OpenAI 侧的缓存写字段按真实响应逐个核对后落代码（**以实测响应为准**，不照抄文档字段名）。
- 计价：`cache_write` 按缓存写价；`cache_read` 按缓存读价、缺则退输入价（这条已有，复核）。
- 界面：手工价四项现在四项都真的生效；「N 次无价格 / N 次 token 未知」计数含新列。

## 收口验收

- 单测：两条 wire 的 usage 映射各一条（用真实录制帧）。
- 一次带缓存写的真实请求（或录制回放）能让 `cache_write` 落库并计入成本；**历史不重算**。

## Answer

（2026-10-15，`task/58-cache-write-accounting`，基于集成分支 `42f33ff`；收工时
`git merge integration/model-gateway`（tip `ba025a0`，票 56/57/62）**两处冲突就地解决**：
`store/schema.rs`（票 57 先占了 v6 → 本票顺延 v7）与 `tests/upstream_live.rs`（两边都在文件尾
追加一条真上游用例 → 两条都留），合并后在合并树上重跑门禁与真上游验收全绿）

### 做了什么

- **列 + 迁移**：`requests` 加 `cache_write INTEGER`，迁移 `v7_cache_write`（`SCHEMA_VERSION = 7`）。
  只加不改：既有列的名字 / 类型 / 语义一个不动，老行新列是 **NULL**（当时没采，不是 0）。
- **两条 wire 的采集**：
  - Anthropic 侧：顶层 `usage.cache_creation_input_tokens`（`message_start` 与 `message_delta`
    都带，`message_delta` 那份是既有读法）→ 现在真的流进日志行（`AnthropicAssembler` 此前把这一项
    丢在 `..` 里）。
  - OpenAI Chat 侧：顶层 `usage.cache_creation_input_tokens`（**新落的一行代码**）——OpenAI 官方
    词表里没有缓存写这一项，名字与 Anthropic 侧同名，以实测为准（下节的证据）。
  - 入站是 Responses 或 Ollama 的客户端也一起收（`ResponsesAssembler` / `OllamaAssembler`）：
    这两个的**响应体**词表里没有缓存写，所以只进日志行、不回给客户端。
- **落库路径**：`RequestLog.cache_write` → `INSERT`；两条转发出口（流式 `Pump::handle`、非流式
  `forward_buffered`）与 `ClientAssembler::usage()` 都升成四项。
- **计价**：`resolve_cost(prices, input, output, cache_read, cache_write)` 四项相加；
  `cache_write` 按**缓存写价**，**缺缓存写价 = 缺价 → 整条 NULL**（票 33 第 4 节只给缓存读留了
  「退输入价」，缓存写没有这条退路，不拿输入价顶替、不半算）。`cache_read` 口径**没动**
  （复核：`prices.cache_read.value().or_else(|| prices.input.value())` 原样）。
- **计数与新列进汇总**：`TOKEN_UNKNOWN_SQL` 从「三项全空」改成**四项全空**；`usage_summary` 的
  `totals` 与分组行都加 `cache_write` 求和；`/api/logs` 行加 `cache_write`；前端 `app/src/api.ts`
  的 `LogRow` 与 `LogsPage.tokens()`（token 未知判据 + 显示「缓存写 N」）跟着改。
- **手工价四项真的都生效**：`resolved_prices` 本来就四项（票 49），本票起 `resolve_cost` 真的
  消费第四项 —— 真上游那一发就是按**手填的缓存写价**算出来的（下节验收 2）。

### 两条 wire 的实际字段名与来源证据

真录帧落在 `crates/gateway-core/tests/fixtures/cache-write/`（`tests/record.ps1` 录，
`origin: recorded`，**字节原样**；两份的 `.sse` 入库时 blob md5 与工作区 md5 一致）：

| wire | 上游 / 模型 | 真实字段（实测值） | 备注 |
| --- | --- | --- | --- |
| anthropic | commandcode `/messages` + `claude-sonnet-5-5`（system 带 `cache_control: ephemeral`） | `message_start.usage.cache_creation_input_tokens` = **3819**；`message_delta.usage.cache_creation_input_tokens` = **3819**；同帧 `cache_read_input_tokens` = 0、`input_tokens` = 9、`output_tokens` = 13 | 旁边还有嵌套的 `cache_creation.{ephemeral_5m_input_tokens:3819, ephemeral_1h_input_tokens:0}`：**那是拆分，不数**（顶层那个才是总数） |
| openai | commandcode `/chat/completions` + `gpt-6-luna`（>1024 token 的 system 前缀） | `usage.cache_creation_input_tokens` = **2623**（**顶层**）；`prompt_tokens` 2626、`completion_tokens` 6、`prompt_tokens_details.cached_tokens` = 0 | 同一份响应里没有任何 OpenAI 文档式的缓存写字段；名字与 Anthropic 侧同名 |

旁证（都实测过、都**没进代码**）：

- **opencode Go 的 Chat 面没有缓存写字段**：`deepseek-v4.1-flash` / `glm-5.3` / `qwen3.8-max`
  各打两发长前缀（第二发 `cached_tokens` 分别 2560 / 3348 / 4224），usage 里**只有**
  `prompt_tokens_details.cached_tokens`，没有写量。→ 这条上游的 `cache_write` 会是 NULL。
- **commandcode 的 Chat 面 deepseek 线**：`usage.prompt_tokens_details` 里有
  `cache_write_tokens` / `cache_creation_input_tokens` / `cache_read_input_tokens` **三个**字段，
  但实测两次（含触发缓存创建的那一发，`cached_tokens` 0→2560）**恒为 0**；真读量落在
  `cached_tokens`。拿不出非零证据 → **不读它**（不把没验证过的形状当口径）。
- 探针命令（凭据只从绝对路径 `prototype-upstream.env` 现读、不打印、不入库；`-H "user-agent: …"`
  与 `x-opencode-session` 是 opencode Go 的硬要求）：
  `curl -s -N -X POST "$COMMANDCODE_BASE_URL/chat/completions" -H "authorization: Bearer $KEY" --data-binary @body.json`
  （`body.json` 里 `system` 写 >1024 token 的固定前缀；第二次发同一前缀就从 `cache_creation` 变 `cached_tokens`）。

### 迁移版本

**v7**（票面写的 v6 在集成分支上被票 57 的结构化输出能力位占了；`MIGRATIONS` 连续、`SCHEMA_VERSION = 7`）。

### 验收（逐条真跑）

**1) 单测：两条 wire 的 usage 映射各一条（真实录制帧）**

```
> cargo test -p gateway-core --lib -- cache_creation
test anthropic::tests::anthropic_cache_creation_input_tokens_lands_in_cache_write ... ok
test openai::tests::openai_cache_creation_input_tokens_lands_in_cache_write ... ok
test openai::tests::absent_cache_creation_stays_none ... ok      ← 上游没报 → None（不编 0）
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 160 filtered out
```

断言（期望值是**帧文件里的实测值**，不是被测代码算的）：Anthropic 那格把真录帧逐帧过
`AnthropicDecoder` + `AnthropicAssembler`，断言 `usage() == (Some(9), Some(13), Some(0), Some(3819))`
（输入 / 输出 / 缓存读 / 缓存写）且 `ended()`；OpenAI 那格过 `OpenAiDecoder`，断言
`Some((Some(2626), Some(6), Some(0), Some(2623)))`。帧文件即上表那两份 `.sse`。

**2) 一次带缓存写的真实请求：落库 + 计入成本 + 历史不重算**

```
> cargo test -p gateway-core --features mock --test upstream_live ticket58 -- --ignored --nocapture
[t58] 真打一发：{"id":1,... "real_model":"claude-sonnet-5-5","wire":"anthropic","status":200,
                "input_tokens":9,"output_tokens":13,"cache_read":0,"cache_write":3819,
                "cost_usd":0.01454325,...}
[t58] cache_write=3819 cache_read=0 in=9 out=13 → cost_usd=$0.014543（改价后仍是 $0.014543）
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 6 filtered out
```

这一发走的是**产品侧同一条转发路径**（`egress::upstream::forward_buffered`，真 HTTP 打
commandcode），手工四项价手填（$3 / $15 / $0.30 / **$3.75** 每百万）。独立手算：
9×3 + 13×15 + 0×0.30 + 3819×3.75 = **$0.01454325**（与库里那行一致，说明第四项真的参与了）。
随后把四项价全抬 10 倍**并**换一份 models.dev 快照，同一行的 `cost_usd` 仍是 `$0.01454325`、
`cache_write` 仍是 3819 —— **历史不重算**。不联网的那半另有一格钉在门禁里（同一形状，期望值字面量）：
`obs::tests::cache_write_lands_in_the_db_with_cost_and_is_never_recomputed`（$0.0255 → 改价后仍是
$0.0255，新一发 $0.255）。

**3) 旧库迁移：v6 → v7，既有数据不丢**

```
> cargo test -p gateway-core --lib -- --nocapture store::tests::upgrades_a_v6
[v7] 升级前：user_version=6，requests 2 行，列里没有 cache_write
[v7] 升级后：user_version=7，requests 2 行；抽样
     (1750000000, "req-old", Some("claude-haiku"), Some(1000), Some(500), Some(80), Some(0.0105), None) /
     (1750000100, "req-old-2", Some("claude-opus"), None, None, None, None, None)
test result: ok. 1 passed; 0 failed
```

造一份**真的 v6 库**（`migrate_upto(6)` 走完 1..6 的建表路径）灌两行（一发行一发行错）再
`Store::open` 升到 v7：行数 2 → 2，既有列逐列不变（含 `cost_usd` 0.0105 与那条 NULL），
老行的 `cache_write` 是 NULL；升级后新行能写进 3819（列真的可用）；再开一次仍 7 且行数不变
（幂等）。v1 → 最新那条老用例（`upgrades_a_prototype_era_db_in_place`）同时仍绿。

**4) 门禁**

```
> pnpm --dir app build
$ tsc --noEmit && vite build
✓ 18 modules transformed.  dist/assets/index-Dgwqr_1I.js  227.70 kB │ gzip: 71.77 kB   (exit 0)

> pwsh -File ./check.ps1 -Quick -Replay
=== fmt / clippy / unit / clippy(mock) / replay / replay-unit ===
test result: ok. 160 passed; 0 failed; 3 ignored     ← 库单测（含本票新增 8 格）
test result: ok. 4 passed; 0 failed                  ← tests/logs_api.rs（本票加了缓存写的接口断言）
test result: ok. 22 passed; 0 failed                 ← 回放（15 格矩阵，本票未动矩阵）
test result: ok. 5 passed; 0 failed                  ← replay-unit
全部通过。
```

（上面那份是**合并后**在合并树上的门禁输出；`unit` 的 160 条里本票新增 8 条：anthropic 1 /
openai 2 / obs 4 / store 1。）

### 遗留与坑

1. **手工四项价的界面入口不在本票**：价格按钮那四项（含缓存写）是「模型与映射页」的活，归票 52/59。
   本票交的是后端四项真的参与计价 + 接口把新列吐出来。
2. **客户端响应体的 usage 只补了一半**：流式 Anthropic encoder 本来就回
   `cache_creation_input_tokens`；**非流式** `AnthropicAssembler::message()` 仍不回（本票只做采集与
   计价，不动客户端面形状）；Responses / Ollama 的词表里没有这一项，只进日志行。要有「响应面一律
   补齐」的需求，另开票（会动黄金帧）。
3. **上游不报就没有**：opencode Go 的 Chat 面实测没有缓存写字段 → 那条上游的 `cache_write`
   永远是 NULL；commandcode 的 Chat 面 deepseek 线的 `prompt_tokens_details.cache_write_tokens`
   实测恒 0，未采信。真出现写量再按那时的实测形状补。
4. **缺缓存写价 → 整条无价格**：这是票 33 第 4 节的直接推论（只给缓存读留了退路），代价是
   models.dev 快照里没有 `cache_write` 价的模型（如本机内置快照里的部分条目）一有缓存写就整条
   算不出价、只进「N 次无价格」。要在「无价就不计这一项、其余照算」与「不半算」之间改口径，
   得回票 33。
5. **`.sse` 的换行会被 `core.autocrlf=true` 改写**：仓库没有 `.gitattributes`，
   `tests/fixtures/keepalive/anthropic.sse`（票 30 的真录帧）在 checkout 时已经被转成 CRLF；
   本票这两份入库时是 LF 原样，但下次 checkout 同样会被转。解析不受影响（`SseParser` 认 CRLF），
   可「字节原样」这条迟早要 `*.sse -text` 钉住——不属本票领地，留给收口票。
6. **迁移顺延会再发生**：本票在原票面写「加 v6」，被票 57 抢先。并发取票时迁移编号只能在**合并那一刻**
   定（本票的处置：改版本号 + 注释里写清顺延缘由），别按票面数字直接落代码。
