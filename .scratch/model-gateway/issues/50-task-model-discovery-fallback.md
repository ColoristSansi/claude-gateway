# 模型发现与降级

Type: task
Status: resolved
Blocked by: 49

## Question

做「拉模型」（模型发现）：打上游 `/models`，按三种结局分流处置，并把反查警告落库。九条定案见 [票 35](../issues/35-grilling-model-discovery-fallback.md)。

依据：spec §6、[票 35](../issues/35-grilling-model-discovery-fallback.md)、[票 23](../issues/23-design-provider-table-granularity.md)。

## 交付

- 拉取：用上游 `default` 凭据、**不为每个 wire 面重复拉**、**20 秒超时**、**不自动重试**（失败提示里给「重试」按钮）。
- 三种结局：成功 / **该上游不提供模型发现**（404、405、或 200 但零条 → 软提示 + 手填入口，不报红）/ **失败**（其他非 2xx、超时、断网 → 失败提示里内联「手工添加模型」，`manual` 锁定，上游仍可保存）。
- **重拉失败保留旧条目**，只有成功才覆盖 `source = upstream` 的条目；手改过的条目锁定不被覆盖。
- 优先级：手改（锁定）> 上游 `/v1/models` > 随包内置文档表 > 未声明。
- 警告落进上游记录的「上次拉取警告」，卡片 / 列表给角标，下次成功清除；**不阻断保存、不进概览「最近错误」**；也不计入侧栏「未通过自检的上游数」（判据只看自检）。
- opencode 特殊：只拉带凭据那份目录；带凭据拉取 404 / 空结果才退匿名。

## 收口验收

- 三种结局各一条用例（mock 或录制）。
- 重拉失败后旧条目仍在、警告角标可查、清空时机正确。
- 真实上游各拉一次：commandcode 得到 89 条带 `supported_endpoints`；opencode 按凭据分支拉到预期集合。

## Answer

（2026-10-15，task/50-model-discovery-fallback，基于集成分支 `50ab7ff`，收工前已并
`integration/model-gateway`（`5cd8219`，与票 46 无冲突），门禁在合并后的树上重跑全绿）

### 做了什么

新模块 `crates/gateway-core/src/egress/discovery.rs`：把票 49 的**原语**拼成用户按一次「拉模型」时
真正发生的那件事，并回一份界面能直接渲染的 `PullReport`。票 49 的原语一行没重写。

- **三种结局分流与降级处置**（`run_pull` → `PullDisposition`）：
  - `Fetched`：2xx 且有条目 → 灌目录（成功才覆盖 `upstream` / `doc` 条目、才清角标、才移动成功时间）。
  - `NoDiscovery`：404 / 405 / **200 但零条** / 声明了 `NoModelsEndpoint` → 软提示「请手填模型 ID」
    + 手填入口（`PullAction::AddManualModel`），**不报红**、不动目录。声明了 `NoModelsEndpoint`
    的上游**一次网络都不打**（spec §6 那句「不必尝试」的执法点，票 49 只落了库没接）。
  - `Failed`：其他非 2xx / 超时 / 断网 → `message` = 失败原因，按钮 `[手工添加模型, 重试]`，
    目录一条不动，上游行照旧存着（`run_pull` 回回执不 `Err`，所以永不阻断保存）。
- **拉取前的两道凭据门**（也要界面出手，所以同样是结局）：`NoCredential`（该上游的清单只认带凭据
  那份、库里一份凭据都没有 → 不拉，按钮 `[去填 key, 手工添加模型]`）与 `CredentialChoice`（多份凭据
  又没有 `default` → 不瞎猜，回 `choose_from` 让界面下拉选）。这两支与三种结局共用一份回执形状，
  票 52 的向导可以只写一套渲染。
- **重拉失败保留旧条目 + 「沿用上次结果（时间）」**：`upstreams` 加一列 `last_pull_ok_at`
  （schema v4）。`last_pull_at` = 上次**尝试**，`last_pull_ok_at` = 上次**成功**（NULL = 从没成功过）。
  失败与「不提供发现」都不动目录、不动 `last_pull_ok_at`，`PullReport.reuse_at` 把那个时间交给界面报
  「沿用上次结果（时间）」。`manual` 条目**在拉取清单里也不被覆盖**（票 49 的 `kept_manual` 分支照旧）。
- **告警角标与清除时机**：`last_pull_warning` 仍是「上次拉取警告」的落点（成功时 = 本次反查警告，
  无警告即清；失败时 = 失败原因；**无发现时保留原值**）。只有**成功拉取**才清——把票 49 里
  「NoDiscovery 顺手写 `None`」改成保留，符合票 35 第 1 条的字面（下次**成功**才清）。
- **不计入侧栏 / 就绪**：新 `upstream_tally()` → `UpstreamTally{total, selfcheck_passed, with_pull_warning}`，
  `selfcheck_failed()`（侧栏「未通过自检的上游数」，**没自检过也算未通过**）与 `ready()`
  （概览三步第一步 = 至少一条已保存且自检通过）都只看自检；角标是**另一个维度**（`with_pull_warning`）。
  拉取流程从不写 `selfcheck`，也从不写 `requests`（「最近错误」是请求错误的地盘）。
- **opencode 的凭据分支**：新 quirk `Quirk::ModelsListNeedsCredential`（上游级事实，无载荷）。
  该 quirk 的上游：**没凭据就不拉**（`NoCredential`，一次网络都不打）；带凭据打一次，**只有**
  带凭据那次 404 / 零条才退匿名再打一次（`PullCredential::AnonymousFallback`），且界面拿得到
  「这份目录是未授权的全量列表，未授权模型调用会 403」的提示。没有该 quirk 的上游维持票 49 的
  「尽力而为」：有凭据就用，没凭据就匿名拉（不能让自建/本地上游失去发现能力）。一次 `run_pull`
  至多两次网络，都不是自动重试（重试按钮在界面上）。

### 与票 49 的分工边界（我改了什么、没改什么）

| 票 49 已做（我复用，未重写） | 票 50 补的 |
| --- | --- |
| `pull_models`（20 秒、不重试、三支 `PullOutcome`）、`pull_credential`、`apply_pull`、`save_manual_model`、`set_pull_result`、`reverse_lookup` | `run_pull` 编排（凭据门 / 退匿名 / 回执）、`PullReport` 一族、`upstream_tally` |
| `apply_pull` 的「失败与无发现都不动目录」「`manual` 不覆盖」「清单里没了的非锁定条目清掉」 | 失败/无发现时**保留** `last_pull_ok_at` 与旧角标；新增 `last_pull_ok_at` 落库 |
| `PullState{at,status,warning}` | `PullState.ok_at`（+ v4 迁移）、`PullApplied.fetched`（界面要「拉到 N 条」） |
| `credential_wires` 无（`pull_credential` 把「一份都没有」与「多份没 default」都压成 `None`） | 新增 `credential_wires` 分开这两个事实 |
| `Quirk::NAMES` 6 值 | 加 `ModelsListNeedsCredential`（7 值）+ 豁免表条目（执法点在发现流程、不产生回放 fixture） |

### 给票 52（界面）/ 票 53 的接口

都在 `gateway_core::egress::discovery` 下；结果行 / 提示 / 按钮不必在界面上再写一套判据：

| 界面要的东西 | 拿法 |
| --- | --- |
| 一次拉模型的回执 | `run_pull(&store, &http, &up, at)` → `PullReport{disposition, message, credential, applied, actions, reuse_at, warnings, choose_from}` |
| 结局分支 | `PullDisposition::{Fetched, NoDiscovery, Failed, NoCredential, CredentialChoice}`（前三支是票 35 的三支） |
| 按钮（含文案与顺序） | `report.actions: Vec<PullAction>`（`label()` = 「手工添加模型」/「重试」/「去填 key」/「选一份凭据」；`report.offers(_)` 查单项） |
| 结果行文案 | `report.message`（成功带条数与增/改/删/手改锁定数；无发现与失败各一句人话） |
| 「沿用上次结果（时间）」 | `report.reuse_at: Option<i64>`（Unix 秒，**格式化留前端**）；回执失焦后从 `upstreams.last_pull_ok_at` 读 |
| 角标（列表/卡片） | `Upstream::last_pull.warning`（非空即亮；`PullState.ok_at` 给「上次成功」那一刻） |
| 侧栏「未通过自检的上游数」/ 概览三步第一步 | `upstream_tally()` → `selfcheck_failed()` / `ready()`（**角标与拉取失败都不进**） |
| 勾选列表 | `list_models(&store, id)`（`supported_wires` 空 = 未声明，配 `WireChoice::note()` = 「未声明，靠自检」）；手改走 `save_manual_model`（`manual` 锁定） |
| 拉取前两道门要的凭据下拉 | 只列 `report.choose_from` 里的 wire 名；用户选定后按该 wire 重拉（`pull_credential` 仍是 default 优先） |

**向导要注意的两件事**：①opencode 预设必须带上 `ModelsListNeedsCredential`（否则走「尽力而为」，
没填 key 就会去拉匿名那份 85 条的未授权目录）；②凭据表与目录表都对上游有外键，**先存上游行再填 key**；
`run_pull` 自己也会先 `save_upstream`（声明式字段），所以「填 baseUrl/key → 拉模型 → 强制自检并保存」
这个自然顺序能走通（运行期事实不会被这一步覆盖）。

### 验收（逐条真跑）

1) **三种结局各一条用例**（`crates/gateway-core/tests/discovery.rs::three_outcomes_are_branched_apart`，
mock 上游 = 本进程 axum）：
```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/50'
cargo test -p gateway-core --test discovery
```
→ `test result: ok. 8 passed; 0 failed`。这一条里：①`Fetched`（两条，反查成 `anthropic`/`openai-chat`，
`credential = Anonymous`，不给重试按钮）；②`NoDiscovery` 三次（**404 / 405 / 200 零条**各一次：`message`
含「不提供模型发现」+「手填模型 ID」、有手填入口、无重试、上游行仍在且 `last_pull_status = no-discovery`）；
③`Failed`（503：`message` 含「拉取失败」、按钮 `[手工添加模型, 重试]`、上游仍可保存，且
`SELECT COUNT(*) FROM requests` = 0 → **不进概览「最近错误」**）。另加 `no_models_endpoint_quirk_*`
（声明的上游 `hits == 0`：一次网络都没打）。

2) **重拉失败保留旧条目 + 角标可查 + 清空时机**
（`repull_failure_keeps_old_entries_and_the_badge_clears_only_on_success`，同一命令）：
先成功一次（夹一条 `/embeddings` → `ok.warnings.len() == 1`，落库后 `last_pull.warning` 含 `/embeddings`，
`ok_at = 1000`）→ 手改一条 `my-handmade`（`manual`）→ **503 重拉失败**：目录 3 条一条不少、
`reuse_at = Some(1000)`、`at = 2000`、`ok_at` 仍是 `1000`、角标换成失败原因 → **404「不提供发现」**：
目录仍 3 条、`ok_at` 仍 `1000`、`warning` 仍在（**不算成功、不清角标**）→ **再次成功**：`warning == None`
（下次成功才清）、`ok_at = 4000`、`kept_manual == 1` 且 `my-handmade` 仍按手填的 `anthropic`（上游这次报的是
`openai-chat`，**手改锁定不被覆盖**）、上游不再报的 `weird-1` 被清（`removed == 1`）。
就绪判据那条：`pull_failure_and_warnings_never_count_toward_unready` —— 两条上游拉取全失败后
`selfcheck_failed() == 2`、`with_pull_warning == 2`、`ready() == false`；一条自检通过后
`selfcheck_failed() == 1`、`ready() == true`（角标数不变）。

3) **真上游各拉一次**（`tests/upstream_live.rs`，`#[ignore]`；凭据用绝对路径的
`.scratch/model-gateway/prototype-upstream.env` 现读，key 全程不进代码/库/输出）：
```
cargo test -p gateway-core --test upstream_live -- --ignored --nocapture
```
→ `test result: ok. 4 passed; 0 failed`（票 49 原有两条 + 票 50 新增两条）。观测：
- commandcode（走整条 `run_pull`）：`结局 Fetched / 凭据 Credentialed / 拉到 90 条 / 反查警告 0 条`，
  `undeclared == 0`，逐条 `supported_wires` 非空且 `source = upstream`（即 90/90 带
  `supported_endpoints` 且全部反查成功；票 49 的原用例同批打印 `带 supported_endpoints 的 90 条`）。
  断言区间 `89..=90`，落在 90。
- opencode（走整条 `run_pull`，按向导预设挂 `ModelsListNeedsCredential`）：`结局 Fetched /
  凭据 Credentialed / 拉到 37 条 / 未声明 6`，退回匿名那条**没有**被走到；逐条 wire 等于
  `doc_wires()`（文档表判定 31 条，6 条保持未声明）。同一条上游再匿名拉一次**只打印**：`匿名那份 37 条`
  —— 本机 `OPENCODE_BASE_URL` 是 Go 面（`/zen/go/v1`），Go 面与凭据无关（票 35 实测），所以
  19/85 那个权限过滤现象**在真上游上验不到**，退匿名的分支靠 mock 用例覆盖（见下）。
- 两条都断言临时数据目录（含 `-wal`/`-shm`）字节里搜不到明文 key。

4) **门禁**（收工前已并 `integration/model-gateway`，在合并后的树上重跑）：
```
$env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/50'
pwsh -File ./check.ps1 -Quick -Replay
```
→ `全部通过。`（exit 0）：fmt + clippy（workspace + mock 特性两套）+ 单元 5 + **92** + **8（discovery）**
+ 4 + **14（provider_table，票 49 那套语义没被改坏）** + 回放 **20 passed** + 回放器切法 5 条。
用的是**本票自己的**
`CARGO_TARGET_DIR`（编排者提醒：共用 `Claude Gateway/target` 会让 `unit` 步跑到别的 worktree 编出的
测试二进制、少报测试数——所以本票全程用 `cg-target/50`，上面的 `8 passed` 里逐条能看到本票的用例名）。

### 遗留与坑

- **`ModelsListNeedsCredential` 是 spec §6 没列的枚举值**。它是票 35 第 8 条落地的唯一数据化位置
  （不许在转换/发现代码里写供应商名），执法点只在 `discovery::run_pull`，不产生回放 fixture；
  已写进 `tests/fixtures/quirk-exempt.json`（与 `NoModelsEndpoint` 同款理由 + 指向本票用例）。
  **spec §6 的 quirks 示例表该补这一条**（下次动 spec 时并进去）。
- **勾选式落库没有接口**：`apply_pull` 是**全量**灌库；spec §10.2 说拉模型那一步是「勾选列表」。
  票 52 若要真的部分勾选，得在这里加一个「按选中集合落库」的变体（现在没有，别默默当全选）。
- **失败会顶掉上一次的 wire 反查警告**：`last_pull_warning` 一列同时承载「反查警告」与「失败原因」
  （票 49 的口径），失败时角标显示失败原因、原反查警告被覆盖；下次成功会连新警告一起重算，所以不会永久丢。
  要两件事都留住就得再分一列（本票没做）。
- **`pull_credential` 的 DPAPI 解密失败会让 `run_pull` 回 `Err`**（不是「没凭据」那一支）：
  换机器/换用户后库里密文解不开时，界面看到的是错误提示而不是软提示——票 52 的错误模板兜住即可。
- **`reuse_at` 只给 Unix 秒**：时区与格式是前端的事（spec §10.2 只要求「沿用上次结果（时间）」）。
- **`last_pull_ok_at` 只从 v4 起有值**：老库升上来是 NULL，卡片会显示「从没成功拉过」——真实事实，
  不做回填（回填只能拿 `last_pull_at` 猜）。
