# 绑定层：家族槽位、虚拟模型、客户端档案覆盖与自检

Type: task
Status: resolved
Blocked by: 49

## Question

做「入站模型名 → (上游, 真实模型, 出站协议)」的绑定与解析层：四个家族槽位、普通虚拟模型、每客户端覆盖、请求期早拒、以及非流式自检探针。

依据：spec §3 / §7、[票 10](../issues/10-design-model-exposure-and-client-config.md)、[票 23](../issues/23-design-provider-table-granularity.md)、[票 26](../issues/26-grilling-count-tokens-and-non-streaming.md)、[票 30](../issues/30-task-build-regression-suite.md)（`model` 规则）。

## 交付

- 四个家族槽位（`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`）+ 普通虚拟模型；绑定只能从上游模型目录里选（目录没有的模型可手工新增一条），再在 `supported_wires` 里选 wire。
- **每客户端可覆盖**（全局 / Claude Desktop / Claude Code / Codex 四套）：同一槽位按客户端走不同上游；`client_profiles` 表 + 解析时按入站 `user-agent` / 客户端标识归属。
- 请求期解析：模型名**去首尾空白 + 大小写敏感**；未命中 → 404 `not_found_error`；wire 未声明却硬用 → 400 早拒。
- 自检探针：非流式 `/v1/messages` + `max_tokens:1`，只看状态码；探针档（`max_tokens <= 16`）上游上限 **20 秒** → 超时 504。
- `/v1/models` 列出网关暴露的模型（无需按客户端过滤）。

## 收口验收

- 单测：四边界模型名（空白 / 大小写 / 未知 / 槽位名）、覆盖解析（同一槽位两客户端不同上游）、wire 未声明早拒 400。
- 探针在两条真实上游各跑一次并记录结果（含 commandcode 的 Chat wire + `max_tokens:1` 挂住那条已知形状）。

## Answer

（2026-10-15，`task/51-binding-slots-profiles-selftest`，基于集成分支 `50ab7ff`；实现 `ae92a23`，
收工前并 `integration/model-gateway`（当时 tip `7eea068`，含票 46 / 48 / 50）为 `df344bc`，
冲突就地解决、门禁在合并后的树上重跑全绿。门禁用的 target 目录是本票私有的
`C:/Users/ZHANQUAN/AIGC/Project/cg-target/51`——共用主工作区那份会让 `unit` 步跑到别的
worktree 编出的二进制、少报测试条数（票 46 记的坑）。）

### 做了什么

落点 `crates/gateway-core/src/binding/mod.rs`（票 41 的 `Route` 退场）+ `ingress/proxy.rs` 接表 +
`egress/provider.rs` 两个小接口 + `store/schema.rs` 的 v5 迁移。

- **绑定表**：`FAMILY_SLOTS` 四个家族槽位与普通虚拟模型都是 `virtual_models` 的一行，
  **没有特殊命中路径**；`BindingTable::{load, resolve, profile_for, exposed, exposed_for}`。
- **只能从目录里选**：`binding::save` 先查 `upstream_models`，目录里没有这条 `(上游, 真实模型)` →
  `SaveError::NoCatalogEntry`；目录声明过 wire 而选的不在集合里 → `WireNotDeclared(EarlyReject{400,
  invalid_request_error})`；空集合（未声明）照票 23 放行并回 `WireChoice::Undeclared`（界面标注
  「未声明，靠自检」）。
- **每客户端覆盖**：`Profile` 四套（`global` / `claude-desktop` / `claude-code` / `codex`），
  解析时按入站身份归属（`BindingTable::profile_for`），档案里没有那份就落回 `global`。
- **请求期解析**：去首尾空白 + 大小写敏感；未命中 → 404 `not_found_error`（`ResolveError`）。
  wire 未声明却硬用的 400 早拒仍在票 49 的目录判据（`upstream::guard`）上，本票把它排在取凭据之前。
- **自检探针** `binding::selfcheck`：非流式 `/v1/messages` + `max_tokens:1`、单条 `"."` 的 user，
  **只看状态码**；超时 504（文案带秒数）、连不上 502、上游非 2xx 记该状态码；结果落
  `upstreams.last_selfcheck_at/last_selfcheck_status`（票 49 的 v3 列）。
- **接进真实路由**：`ingress/proxy.rs` 的 `Gateway` 用 `bindings: BindingTable` 取代 `Vec<Route>`，
  出站凭据从 `Gateway.keys`（M1 脚手架）换成票 42 的 `Store`（`provider::wire_credential`：按
  「上游 × wire」取，未填继承 `default`）——票 49 留给本票的那笔脚手架这时退场。
- **`/v1/models`**：自己实现（不依赖上游），集合 = `BindingTable::exposed()`（四家族名在最前，
  普通虚拟模型按名排序，跨档案去重）；口径见下「口径差」。
- **日志补 `requests.client`**：归属结果随请求落库（`RequestLog.client` + `obs::recent` 的
  `"client"` 键），票 53 按客户端聚合才有依据。

### 绑定解析与档案归属怎么落

1. **入站身份**（`ingress::proxy::client_identity`）：把 `user-agent`、`x-app`、`originator`、
   `x-codex-installation-id`、`x-codex-turn-metadata`、`x-codex-window-id` 的头名与值拼成一串
   （头名也进串：Codex 桌面应用可能只给标识头、不给可辨认的 UA）。
2. **判据在库里**：`client_profiles.match_ua` = `;` 分隔的**子串**，大小写不敏感，命中任意一个即归该档案；
   先命中先归属，认不出就是 `global`。v5 迁移把它从空串填成三条**实测**片段：
   `Claude/`（Claude Desktop）、`claude-cli/`（Claude Code）、`codex_exec/;x-codex-`（Codex）。
   `binding::DEFAULT_MATCH_UA` 是与迁移逐字一致的常量，`tests/binding_table.rs` 拿库里的行跟它比对
   （两处漂了就红）；界面能改这一列（判据是可配置项，库是唯一真相）。
3. **解析**：`resolve(raw_name, identity)` → `trim` → `find(名字, 归属档案)` → 没有就 `find(名字, global)`
   → 还没有就是 `ResolveError`；`find` 是 `resolve` 与 `exposed_for` 共用的那一处查表。
4. **三元组进请求**：`wire = binding.wire`、`req.model = binding.real_model`、上游取
   `Gateway.upstreams[binding.upstream_id]`，出站日志记 `virtual_model = 客户端原样名字`、
   `client = 档案 id`、`upstream_id` / `real_model` / `wire`。

实测的 UA（本机 2026-10-15；探针脚本与日志在仓库外 `%TEMP%\cg51-ua`，只记请求行与头，不留凭据）：
隔离 `CLAUDE_CONFIG_DIR` 跑 Claude Code 2.1.299 → `POST /v1/messages?beta=true`，
`user-agent: claude-cli/2.1.299 (external, sdk-cli)`、`x-app: cli`（另有一次探活 `HEAD /api/hello`，
UA `Bun/1.4.3`）；隔离 `CODEX_HOME` 跑 Codex CLI 0.162.1 → `POST /v1/responses`，
`user-agent: codex_exec/0.162.1 (Windows 10.0.26200; x86_64) unknown (codex_exec; 0.162.1)`、
`originator: codex_exec`；Claude Desktop 3P 的 Electron UA 里那一段 `Claude/2.37337.1` 取自票 15 的
端到端实测（票 32 的真录入站体里另有 `x-codex-installation-id` / `x-codex-turn-metadata`，
就是 Codex 桌面应用的判据）。

### 每条验收的真实命令与输出结论

**1）单测**（`crates/gateway-core/tests/binding_table.rs`，14 条）：

```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/51'
cargo test -p gateway-core --test binding_table
```

→ `test result: ok. 14 passed; 0 failed`。逐条对应：

- 四边界模型名：`model_names_trim_are_case_sensitive_and_slots_are_plain_rows`——
  `"  claude-haiku  "` 命中（空白去掉）、`"CLAUDE-HAIKU"` / `"Claude-Haiku"` 未命中、
  `"claude-opus-6"` / `"根本没有这个模型"` / `""` / `"   "` 全部 404 `not_found_error`；
  槽位名就是普通一行，没有特殊命中路径。
- 覆盖解析：`profile_override_wins_and_global_is_the_fallback`（`claude-haiku` 在 `codex` 下走
  `oc / gpt-5.1-codex / openai-chat`，在同一次调用换成 Claude Code 的 UA 就走
  `cc / claude-haiku-5-5 / anthropic`；档案私有绑定别的客户端看不见）、
  `bindings_roundtrip_through_the_store`（同一份关系从库里读回来仍然成立）。
- wire 未声明早拒 400：`saving_a_wire_the_catalog_does_not_declare_is_refused`（配置期拦，
  `EarlyReject{status:400, error_type:"invalid_request_error"}`）、
  `undeclared_catalog_entry_allows_any_wire_with_the_note`（空集合放行 + `Undeclared` +
  「未声明，靠自检」）、`a_binding_whose_wire_the_catalog_does_not_declare_is_rejected_with_400`
  （就算库被绕过，**真实 HTTP** `/v1/messages` 也回 400 `invalid_request_error`）。
- 另：`unknown_model_names_are_404_on_the_real_route`（真实 HTTP 上的四条边界）、
  `exposed_models_list_family_slots_first_then_normal_models`、
  `measured_user_agents_map_to_their_profiles`、`saving_needs_a_catalog_entry_first`、
  探针三条（`probe_is_non_streaming_max_tokens_one_and_records_the_status_code` 断言发出的是
  `max_tokens:1` 且**没有** `stream`、`probe_sees_a_non_2xx_status_as_a_failure_even_with_a_body`、
  `probe_timeout_is_504_and_records_a_failure`、`probe_connect_failure_is_502`），
  三条都断言 `upstreams.last_selfcheck_*` 落库。

**2）探针在两条真实上游**（联网，`crates/gateway-core/tests/upstream_live.rs`，`#[ignore]`；
凭据用绝对路径的 `.scratch/model-gateway/prototype-upstream.env` 现读，`CG_UPSTREAM_ENV` 可改指向；
key 全程不进代码 / 库 / 输出）：

```
cargo test -p gateway-core --test upstream_live selfcheck_probe -- --ignored --nocapture
```

→ `test result: ok. 1 passed; 0 failed`。观测（2026-10-15，本票跑了三次，**上游行为会变**）：

| 上游 / 真实模型 / wire | 上限 | 观测 |
| --- | --- | --- |
| opencode / `deepseek-v4.1-flash` / openai-chat | 20s | 200 ok 3419ms（另两次 4085ms / 6331ms） |
| 同上 | 90s | 200 ok 2065ms（另一次 5852ms） |
| commandcode / `claude-sonnet-5-5` / anthropic | 20s | 200 ok 2788ms（另两次 2950ms / 4874ms） |
| commandcode / `deepseek/deepseek-v4.1-flash` / openai-chat | 20s | **504 ok=false 20003ms「上游超时（20 秒内无响应）」**（第一次）；后两次 2249ms / 1680ms → 200 |

- 那条**已知挂住**的形状（票 26 / 票 51:24）第一次就是 **20.003 秒 → 504**、`last_selfcheck_status`
  记 `fail`；同一天再跑两次变成 1.4 / 2.2 秒 → 200。**这条挂住是间歇的**，所以 live 用例断言的是
  契约（这一档只该是 200 或 504，504 的文案必须说「超时」，两者都如实落库），不把 504 钉死——
  钉死会变成一条随机红的门禁。
- 同一用例还在真数据上验了「绑定只能从目录里选」：目录里有的 `claude-haiku → commandcode /
  claude-sonnet-5-5 / anthropic` 绑得上（`WireChoice::Declared`），目录里没有的
  `codex / codex-不存在 / 根本没有这个模型` 回 `NoCatalogEntry`。
- 顺带一条真上游事实：opencode 的 Chat 面**必须**自称 UA 并带 `x-opencode-session`（票 02 / 19），
  不带就是 `400 MissingSessionID`（直连 curl 复现）；探针走的是出站组装点，所以上游表里的
  quirks 照常生效。opencode 的 Anthropic 面用同一把 default 凭据仍是 `401 Upstream request
  failed: Go requires a credential`（票 49 / harness 的负例 1 一致），所以那一发没进表的正例。
- 另：`selfcheck` 的探针请求带的是 `upstream.quirks` 与 `up.auth_for(wire)`——也就是说它验的是
  「这条绑定三元组按真实配置能不能用」，不是裸 curl。

**3）门禁**：`pwsh -File ./check.ps1 -Quick -Replay`（worktree 根，`CARGO_TARGET_DIR` 指本票私有那份）
→ `全部通过。`：fmt + clippy（workspace 两套特性全绿）+ 单元
`5（外壳）+ 107（gateway-core lib，含票 46/48/50 的）+ 14（binding_table）+ 8（discovery）
+ 4（gateway_service）+ 14（provider_table）+ 1（doc-test）` +
回放 **21 passed**（票 41 的 20 格一格没红，外加本票新加的那格）+ 回放器切法 5 条。

- 新加的回放格 `tests/replay.rs::client_profile_override_and_models_exposure`：
  harness 种库时给 `codex` 档案加了 `claude-haiku → mock / mock-text / anthropic` 的覆盖，
  于是同一个模型名在 codex UA 下走 mock 回放（200 + 真文本）、Claude Code UA 与无 UA 下都落回
  `global`（commandcode，测试里不可达 → 502）；同一用例还断言两个客户端的 `/v1/models`
  **集合一致**而信封不同（Anthropic `{data,has_more,first_id,last_id}` vs Codex
  `{object:"list",data:[…]}`），以及日志行的 `client` 分别是 `codex` / `claude-code` / `global`。
- `gateway-harness` 改成**种库再读回**（`save_upstream` → `save_manual_model` → `binding::save`
  → `put_credential` → `binding::load` / `provider::catalog_map`），与外壳（`src-tauri/src/shell.rs`）
  走的是同一条装配路径；跑起来打印「绑定表（39 条绑定 / 38 个暴露模型）」。

### 给票 52 / 53 / 56 / 57 的公开接口

| 用途 | 接口 |
| --- | --- |
| 槽位 / 虚拟模型 / 档案（票 52 界面、票 59 设置页） | `binding::{FAMILY_SLOTS, is_family, Profile::{ALL,id,display,parse}, Binding::{new,is_family,triple}, BindingTable::{new,rows,len,is_empty,profile_for,resolve,exposed,exposed_for}, ExposedModel{name,is_family,binding: Option<Binding>}}` |
| 读写绑定 | `binding::save(&store, &Binding) -> Result<WireChoice, SaveError>`（`EmptyName` / `NoCatalogEntry{upstream_id,real_model}` / `WireNotDeclared(EarlyReject)` / `Store`）、`binding::load(&store) -> Result<BindingTable>` |
| 档案判据 | 库列 `client_profiles.match_ua`（`;` 分隔子串、大小写不敏感）+ `binding::{DEFAULT_MATCH_UA, default_profiles}`；入站线索的拼法在 `ingress::proxy::client_identity`（要看/要改就改它一处） |
| 自检探针 | `binding::selfcheck(&reqwest::Client, &Store, &Upstream, Wire, real_model, Duration) -> ProbeResult{upstream_id,real_model,wire,status,ok,elapsed_ms,detail}`（自己落 `upstreams.last_selfcheck_*`）；常量 `binding::{PROBE_DEADLINE=20s, PROBE_MAX_TOKENS=16, PROBE_PROMPT="."}`（只有一处定义，在 `egress::upstream`） |
| 转发面 | `Gateway::{bindings, catalog, upstreams, store, client, ping, probe}`、`Gateway::new(store, upstreams, bindings)`、`ingress::proxy::router()` |
| 凭据 / 目录（票 53 计价、外壳装配） | `provider::wire_credential(&store, upstream_id, wire)`（本面 → 继承 `default`）、`provider::catalog_map(&store)`、票 49 的 `resolved_prices` |
| 按客户端聚合（票 53） | `requests.client` 列（档案 id）+ `obs::recent` 返回的 `"client"` 键；虚拟模型维度用 `virtual_model`（客户端原样名字）、上游维度用 `upstream_id` |
| Codex 模型目录（票 48 / 31） | `writer::codex::{plan,write,claim}(&[ExposedModel], model)`——入参换成 `ExposedModel`（`Route` 已删）；按客户端那份用 `BindingTable::exposed_for(Profile::Codex)` |
| 库 | schema **v5**（票 50 的 v4 是 `last_pull_ok_at`；本票的 `match_ua` 迁移是 v5） |

### 口径差（以 spec 为准）

1. **`/v1/models` 到底按不按客户端过滤**：票 51 第 19 行写「无需按客户端过滤」，
   spec §4.5:140 写「对 Claude 系客户端要暴露四个家族名；对 Codex 返回标准 `models` 数组」。
   按 prompt 的裁决**以 spec 为准**，落法 = spec §8.2 第 2 条那两句话拆开读：
   **集合只有一份、不按客户端过滤**（过滤发生在配置层，发现层用同一判据），
   **只有信封形状按客户端家族给**——Claude 系（含认不出的 UA）拿 Anthropic 的
   `{data,has_more,first_id,last_id}`，Codex 拿标准 `{object:"list",data:[{id,object:"model",
   created,owned_by}]}`。两条断言都在
   `tests/replay.rs::client_profile_override_and_models_exposure` 里。
2. 顺带销掉票 30 台账里那条「客户端档案覆盖 = 明确不覆盖」：`tests/LEDGER.md` 改成
   「明确不覆盖的一处」，并注明覆盖它的用例。

### 遗留与坑

- **没有解绑 API**：`binding::save` 只做 upsert，删一条绑定（或把家族槽位退回未绑定）要界面自己写；
  票 59 需要它。`virtual_models.purpose`（普通虚拟模型的「用途」列）也没进 `Binding` /
  `ExposedModel`，界面要显示得自己读库。
- **`match_ua` 只能改库**：没有界面入口（票 59）。库里那份是唯一真相；
  `binding::DEFAULT_MATCH_UA` 只是 v5 迁移的字面量，有测试钉住两处不许漂。
- **判据是稳定片段、不是整串 UA**：版本号天天变，所以 `Claude/`、`claude-cli/`、`codex_exec/`
  都是前缀；Codex 桌面应用靠 `x-codex-*` 标识头认（它的 UA 没实测到）。真出现「两个客户端
  同时命中」就改库里那列（`;` 分隔、先命中先归属）。
- **未绑定的家族槽位照样暴露**：四个家族名是网关的固定契约（spec §7），`/v1/models` 与 Codex
  目录里恒在，但请求期 404。界面（票 59）得把「未绑定」标出来，别让用户以为能用。
- **`Gateway.catalog` 还是内存快照**（49 的设计）：改目录 / 绑定之后外壳要重新
  `binding::load` / `catalog_map` 才生效，本票没做热更新。
- **commandcode 那条挂住是间歇的**（见验收 2 的表）：别把它写成必然的 504，也别在门禁里钉死。
- **opencode 的 Chat 面又慢又挑头**：缺 `x-opencode-session` → 400 `MissingSessionID`；
  带齐头也可能 2～29 秒 → 20 秒探针档会把健康上游判成 504（票 26 已接受的取舍，旋钮 =
  `Gateway.probe` / `PROBE_DEADLINE`）。
- **出站凭据只剩库里一条路**（`Gateway.keys` 已退场）：回放 harness 与 live 用例都先
  `put_credential` 再转发；DPAPI 密文入库、明文只在内存，`Gateway` 的 `Debug` 不再有可打印密钥的字段。
- **并集成分支就地的两处适配**（记在这里免得下游找不到出处）：票 50 与本票都写了 v4 迁移，
  票 50 保持 v4（`last_pull_ok_at`）、本票的 `match_ua` 顺延为 **v5**；票 48 的
  `writer/codex.rs` 原来吃 `binding::Route`，本票删掉 `Route` 后它改成吃
  `binding::ExposedModel`（未绑定的家族槽位在目录描述里写「尚未绑定」，而不是让目录自检失败）；
  `tests/upstream_live.rs` 两边都在文件尾追加用例，两段都留（本票的探针 + 票 50 的两条拉取流程）。
