# 供应商表、上游目录与价格快照

Type: task
Status: resolved
Blocked by: 42

## Question

做「上游」这一层的服务与校验：声明式供应商表（`base_url` + 按 wire 分开的认证 + 每 wire 路径模板 + `quirks`）、上游模型目录（逐「(上游, 真实模型)」记 `supported_wires` + `source`）、三层校验、手工价四列与 models.dev 随包快照。

依据：spec §6 / §9、[ADR-0004](../../../docs/adr/0004-provider-table-granularity.md)、[票 23](../issues/23-design-provider-table-granularity.md)、[票 33](../issues/33-grilling-cost-accounting.md)。

## 交付

- `upstreams` / `credentials`（按「上游 × wire」，未填继承上游默认）/ `upstream_models` 三层的读写与校验。
- 三层校验：配置期主拦（只许选声明过的 wire；未声明可手选并标注「未声明，靠自检」）+ 请求期早拒 400 + 自检为准。
- `pathTemplates` 反查：上游报的端点路径 → wire；查不到就忽略 + 记警告（落进「上次拉取警告」）。
- models.dev 随包快照：导入、`model_catalog` 表存储、设置页手动刷新（失败沿用旧快照）。
- 手工价四列（输入 / 输出 / 缓存读 / 缓存写，各自可填可留空、填了即锁定）；每一项标出实际来源。
- 删被引用的上游要能列出引用处。

## 收口验收

- 单测：三层校验各一条、路径反查（命中 / 查不到）、价格优先级（手工 > 快照 > 未知）、刷新不覆盖手工锁定的项。
- 两条真实上游各建一条并拉到目录，`supported_wires` 与观测一致（commandcode 带 `supported_endpoints`；opencode 两面 `/models` 都不报端点 → 只能靠内置文档表 + 手工）。

## Answer

（2026-10-15，task/49-provider-table-catalog-prices，基于集成分支 `8fea7cc`，收工前已并 `integration/model-gateway`
（先 `ac40394`，再 `e4de381`），冲突就地解决、门禁在合并后的树上重跑全绿）

### 做了什么

落点 `crates/gateway-core/src/egress/provider.rs`（+ `egress/mod.rs` 导出）：声明式供应商表、上游模型目录、
价格与拉模型。表读写用票 42 的 `store::Store` 自己写 SQL（不往 store 塞领域类型）。

- **三层**：`upstreams`（`auth` 按「上游 × wire」，未填继承 `default`；`path_templates` 按 wire；`header_passthrough`；`quirks`）、
  `credentials`（按「上游 × wire」，明文只在内存 / DPAPI 密文入库，读写走票 42 已有接口）、
  `upstream_models`（`supported_wires` 空集合 = 未声明 + `source` + 手工价四列）。
- **三层校验**：①配置期 `UpstreamModel::allowed_wires()` 是下拉唯一取值；②请求期 `validate_wire()` 回
  `EarlyReject{status:400, error_type:"invalid_request_error"}`（`upstream::guard` 已改成这个判据，接在 `ingress/proxy.rs`）；
  ③自检为准：`set_selfcheck_result()` / `last_selfcheck_*`，就绪判据只看它。
- **pathTemplates 反查**：`wire_for_endpoint` / `reverse_lookup`，双向匹配（模板 `/messages` 认上游报的
  `/v1/messages`，反之亦然）；查不到**忽略**并记一条警告（上游报的路径 + 我们期望的路径），
  落进 `last_pull_warning`，成功拉取自动清除。
- **models.dev 快照**：`import_catalog_snapshot`（整体换 `model_catalog`，快照语义）、
  `import_bundled_snapshot`、`refresh_catalog`（设置页手动刷新；失败不动旧快照、**不写**刷新时间）、
  `catalog_snapshot_at`。
- **手工价四列**：`set_manual_prices`（可填可留空，填了即锁定）、`resolved_prices`（逐项标来源）、
  `resolve_prices`（**手工 > 快照 > 未知**，未知 = `None` + `PriceSource::Unknown`，不显示 0）。
- **删被引用的上游**：`upstream_references` 列出 `(客户端档案, 虚拟模型名)`；`delete_upstream` 有人引用就
  `DeleteOutcome::Referenced`（不删），没人引用才删（目录与凭据随 `ON DELETE CASCADE` 走）。
- **拉模型**：`pull_models`（20 秒超时、不自动重试、三支结局：成功 / 404·405·零条 = 不提供模型发现 / 其他 = 失败）、
  `apply_pull`（`manual` 锁定不覆盖、失败与「不提供发现」都不动目录、成功才覆盖 `upstream`/`doc` 条目并清警告、
  上游清单里消失的非锁定条目清掉）、`pull_credential`（`default` 优先 → 唯一一份 → 多份回 `None` 交界面选）。
- **随包数据**：`models_dev_snapshot.json`（3 provider / 4 模型，真形状真数字）与
  `doc_models.json`（opencode Go 逐模型 wire，来自 research/providers.md §3；`mimo-*` 是前缀匹配）。
- **并集成分支时的裁决**：票 41 把原型 `provider.rs`（两值 `Wire`、`Auth`、`api_key` 在表里、
  `ClaudeModelsOnAnthropicOnly`）原样搬进了同一路径。按票 23 / ADR-0004 的判据，**本票的表是该模块的唯一形状**：
  合并时保留本票的 `provider.rs`，并把票 41 的调用方就地改成新形状 —— `upstream.rs` 的 `request()` 改为
  「认证方式 + key 由调用方传」（表里不再放密钥）、`Wire::OpenAi` → `Wire::OpenAiChat`、
  上游 Responses 面显式报 502「尚未实现」；`ingress/proxy.rs` 的 `Gateway` 新增 `keys`（M1 脚手架，
  正式路径 = 票 42 的 Store + DPAPI，票 51 接手后删）与 `catalog`（第二层早拒的查表位）；
  `guard()` 从 quirk 改成目录声明判据；`gateway-harness` 改用 `Upstream::new` + `set_path`；
  `tests/fixtures/quirk-exempt.json` 删掉 `ClaudeModelsOnAnthropicOnly`（ADR-0004 第 5 条落的这一刀）、
  补上 spec §6 那四个还没有执法点的值。票 41 的四条 quirk 双向守卫测试原地保留（搬进本模块的 `#[cfg(test)]`）。

### v3 迁移加在哪

`crates/gateway-core/src/store/schema.rs`：`SCHEMA_VERSION = 3`，`MIGRATIONS` 追加
`v3_selfcheck_columns` — `ALTER TABLE upstreams ADD COLUMN last_selfcheck_at INTEGER` +
`last_selfcheck_status TEXT`（取值 `ok` / `fail`，无记录 = NULL）。
理由：v2 只有拉取的三列，而就绪判据只看自检（spec §6 / 票 35 第 7 条），挤进 `last_pull_status` 会让
侧栏「未通过自检的上游数」说谎。v1 → v3 顺序迁移、老库照升（`store::tests` 那两条迁移用例仍绿）。

### 给票 50 / 51 / 57 的公开接口（都在 `gateway_core::egress::provider` 下）

| 用途 | 接口 |
| --- | --- |
| 票 50 拉模型 | `pull_models(&http, &up, MODELS_PATH, credential)`、`pull_credential(&store, id)`、`apply_pull(&store, &up, outcome, at)`（回 `PullApplied{added,updated,removed,kept_manual,undeclared,warnings}`）、`set_pull_result`、`PullOutcome{Fetched,NoDiscovery,Failed}`、`PullStatus` |
| 票 50 手工条目 | `save_manual_model(&store, up, model, &[Wire])`（`source=manual` 锁定）、`Upstream { set_path, set_auth, quirks, header_passthrough, last_pull, last_selfcheck }` |
| 票 51 绑定与早拒 | `UpstreamModel { allowed_wires, validate_wire }`、`WireChoice::{Declared,Undeclared}.note()`（后者 = `"未声明，靠自检"`）、`EarlyReject{status:400,error_type:"invalid_request_error",message}`、`upstream::guard(Option<&UpstreamModel>, Wire)`、`list_models`、`upstream_references` / `delete_upstream`、`BindingRef` |
| 票 51 / 57 wire | `Wire::{Anthropic,OpenAiChat,OpenAiResponses}` + `ALL` / `as_str` / `parse`（容忍 `openai`、camelCase）、`Source::{Manual,Upstream,Doc}`、`Quirk::all()/name()/payload()/acts_on()/parse` |
| 票 53 成本 | `resolved_prices(&store, upstream_id, model_id)` → `ResolvedPrices{input,output,cache_read,cache_write: ResolvedPrice{usd_per_mtok,source}}`；`catalog_prices`、`import_bundled_snapshot`（**首启灌快照要外壳自己调**：没写进 `Store::open`）。单位 = 每百万 token 美元 |
| 票 59 设置页 | `refresh_catalog(&store, &http, MODELS_DEV_URL)` → `CatalogRefresh{imported, at}`（失败回 `Err` 且不动旧快照）、`catalog_snapshot_at`、`MODELS_DEV_URL`、`CATALOG_REFRESH_TIMEOUT` |
| 票 51 会话头 | `Upstream::session_id(seed)`（FNV-1a，`gw-<16 hex>`，原型行为不变） |

**命名裁决**（蒸馏笔记的矛盾③）：库内键一律用 wire 串（`anthropic` / `openai-chat` / `openai-responses`）
+ `default`，与 `credentials.wire`、`virtual_models.wire`、`supported_wires` 的取值域同一套；
spec §6 样例的 camelCase（`openaiChat`）与 v2 迁移写进老库的键**读入时接受**（`Wire::parse` 别名），
写出去只用规范值。所以不必再为键名做一次数据迁移。

### 验收（逐条真跑）

1) **单测**（`crates/gateway-core/tests/provider_table.rs`，14 条）：
```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/Claude Gateway/target'
cargo test -p gateway-core --test provider_table
```
→ `test result: ok. 14 passed; 0 failed`。逐条对应：
- 三层校验：`config_period_only_offers_declared_wires`（下拉只给声明过的）、
  `request_period_rejects_undeclared_wire_with_400`（400 + `invalid_request_error`，文案带模型名与 wire 名）、
  `undeclared_is_normal_and_selfcheck_is_the_tiebreaker`（空集合 = 可自由选 + 「未声明，靠自检」；自检结果落库）。
- 路径反查：`endpoint_paths_reverse_lookup_to_wires`（命中 `/messages`、`/v1/messages`、`/chat/completions`、`/responses?x=1`）、
  同一条里验查不到：`/embeddings` 被忽略且警告含上游报的路径 + 我们期望的路径。
- 价格：`price_priority_is_manual_then_snapshot_then_unknown`（手工 > 快照 > 未知，未知是 `None` 不是 0）、
  `snapshot_refresh_never_overwrites_locked_manual_items`（刷新后手工「输入」仍是 9.0/`manual`，未锁定的「输出」跟新快照 7.0，新快照没有的「缓存写」回未知）。
- 另：`pull_maps_endpoints_to_wires_and_records_warnings`、`repull_keeps_manual_entries_and_clears_warnings_on_success`、
  `no_discovery_is_not_failure_and_keeps_old_entries`、`doc_table_fills_wires_when_upstream_declares_none`、
  `upstream_roundtrip_keeps_auth_paths_and_quirks`、`deleting_a_referenced_upstream_lists_the_references`、
  `catalog_refresh_failure_keeps_the_old_snapshot`、`timeouts_are_fixed_and_no_retry`。

2) **两条真上游**（联网，`crates/gateway-core/tests/upstream_live.rs`，`#[ignore]`；凭据用绝对路径的
   `.scratch/model-gateway/prototype-upstream.env` 现读，`CG_UPSTREAM_ENV` 可改指向；key 全程不进代码/库/输出）：
```
cargo test -p gateway-core --features mock --test upstream_live -- --ignored --nocapture
```
→ `test result: ok. 2 passed; 0 failed`。观测（2026-10-15）：
- commandcode `GET <base>/models`：**200，90 条，90/90 带 `supported_endpoints`**，取值仍只有
  `/chat/completions`、`/responses`、`/messages` 三种 → 反查**零警告**；wire 分布
  `chat+responses 70 / 仅 anthropic 11 / 仅 chat 9`；抽样 `MiniMaxAI/MiniMax-M2.5 → [openai-chat, openai-responses]`、
  `Qwen/Qwen3.7-Flash → [openai-chat, openai-responses]`（来源全 `upstream`）。逐条 `supported_wires`
  与「按 pathTemplates 反查观测端点」的结果**断言相等**（票 23 记的是 89/89，现在上游多了一条）。
- opencode `GET <base>/models`：**200，37 条，0 条带端点**（带凭据与不带凭据同一份）→ 目录条目全部
  `source=upstream` 起步，再用随包文档表判定：**31 条出货**（`openai-chat 19 / anthropic 6 / openai-responses 6`），
  **6 条保持未声明**（如 `gemini-4-argon-contributor`、`glm-5.1`）——不猜名字，符合票 23 第 5 节。
  抽样 `claude-haiku-5-5 → [anthropic]`、`deepseek-v4.1-flash → [openai-chat]`（来源 `doc`）。
- 两条测试都断言：拉取成功、条数 > 0、wire 集合与观测一致、**临时数据目录（含 `-wal`/`-shm`）字节里搜不到明文 key**。
- 另跑了一次全工作区明文反查（读 env 取密钥值，扫 268 个文件，只打印命中数）：
  `OPENCODE_API_KEY 命中 = 0`、`COMMANDCODE_API_KEY 命中 = 0`。

3) **门禁**：`pwsh -File ./check.ps1 -Quick -Replay`（worktree 根，`CARGO_TARGET_DIR` 指主工作区）
→ `全部通过。`（fmt + clippy 两套特性 + 单元 5+37+4+14 条 + 回放 **20 passed** + 回放器切法 5 条；
  并集成分支后票 41 的回放套件一条没红）。

### 遗留与坑

- **快照首启灌库没人自动跑**：`import_bundled_snapshot` 是给外壳的入口，没塞进 `Store::open`（那是票 43 的启动序列）；
  在它被接上之前，`model_catalog` 空着 → 价格一律回退「未知」，手工价不受影响。
- **刷新超时口径本票自定**：`CATALOG_REFRESH_TIMEOUT = 30 秒`、不重试（票 33 只定「失败沿用旧快照」）。
  比票 35 的 20 秒宽，因为载荷是整个 `api.json`（实测约 5.6 MB）；要改就改这一个常量。
- **models.dev 与中转上游没有 provider 映射**：`catalog_prices` 先按 `(上游 id, 模型 id)` 精确找，
  找不到退「任意 provider 的同名模型」；commandcode / opencode 在 models.dev 里没有条目，所以第二跳是常态。
  真按 provider 精确计价要等有 id 对齐（票 33 没定这事）。
- **上游 Responses 面没实现**：绑到 `openai-responses` 会得到 502 +「尚未实现（M3 起接）」，
  不是静默当 Chat 打。票 57 接上时把 `wire_not_implemented` 两处换掉即可。
- **`Gateway.keys` 是 M1 脚手架**（明文在内存、按上游一份）：正式路径是票 42 的 `credentials`（按上游 × wire + DPAPI），
  票 51 换掉它；`Debug` 不打印它。
- **opencode 文档表会过期**：`doc_models.json` 是随包静态表（票 23 留的尾巴），上游上新模型需要人工补；
  没进表的模型保持「未声明，靠自检」而不是猜。
- **反查是后缀匹配**：`/v1/messages` 与 `/messages` 互相认，代价是理论上「路径后缀相同」的两个面会先命中
  路径模板里靠前的那个（BTreeMap 序，Anthropic 在前）。现有两家的端点取值只有三种，不会撞。
- **回放套件有一次性的 exit 101**：在 fmt 之后那一次 `check.ps1` 里回放步骤报 101，直接单跑与随后两次
  完整门禁都 20/20 绿；共享 `CARGO_TARGET_DIR` 下并发 cargo 的抖动，与本票改动无因果关系（改动只删了一个 quirk、
  改了 `request()` 的参数传递）。

