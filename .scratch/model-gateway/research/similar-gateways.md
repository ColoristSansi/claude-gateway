# 同类客户端配置切换 / 网关项目调研

调研对象（浅克隆 `--depth 1`，路径 `.scratch/model-gateway/vendor/`）：

| 仓库 | clone HEAD | License |
|---|---|---|
| [cc-switch](https://github.com/farion1231/cc-switch) | `2635dff docs(release): add v4.0.11 release notes` | **MIT**（`LICENSE`，Copyright (c) 2025 Jason Young） |
| [CodexPlusPlus](https://github.com/BigPizzaV3/CodexPlusPlus) | `f6092f2 site: 新增官网页面，Pages 发布源切到 Actions/site` | **AGPL-3.0**（`LICENSE`，`README.md` badge: AGPL-3.0-only） |

一句话对比：cc-switch 是「多客户端配置切换器 + 可选本地网关」，覆盖 10 个客户端；CodexPlusPlus 是「Codex/ChatGPT 桌面版增强器 + 本地协议代理」，用 CDP 注入 + 进程内 helper，不改官方 `app.asar`。

---

## 1. cc-switch

### 1.1 语言 / 框架 / 构建

- 前端：React 18 + TypeScript + Vite + Tailwind + shadcn/ui（`src/`，`package.json` 有 `vite`/`vitest`/`tsc --noEmit`）。
- 后端：Rust + **Tauri 2**（`src-tauri/`，`tauri.conf.json` productName `CC Switch`，version `4.0.11`）。
- 构建：`pnpm tauri build`（`package.json:7`），`pnpm run dev` = `tauri dev`。`pnpm-workspace.yaml` + `rust-toolchain.toml`。`vitest` 跑前端单测，Rust 用 `cargo test`（`src-tauri/tests/`，含 `golden/` 字节级 golden 测试）。
- 规模：`src/` 约 300 个 TS/TSX 文件；`src-tauri/src/` 下 proxy 子系统最大（`forwarder.rs` 7818 行、`handlers.rs` 3825 行）。

### 1.2 目录结构概览

```
src/                      React 前端（components/ hooks/ lib/api lib/query config/ i18n/）
src/config/*ProviderPresets.ts   每个客户端的供应商预设（claude/codex/gemini/grok/opencode/...）
src-tauri/src/
  app_config.rs           AppType 枚举：Claude, ClaudeDesktop, Codex, Gemini, GrokBuild,
                          OpenCode, OpenClaw, Hermes, Pi, Mcode（共 10 个）
  provider.rs             供应商模型
  live/                   ★ v4 关键字段写入引擎（floor/project/patch/residue/engine）
  mode/                   ★ 直连 / 路由 / 聚合 三态模式层（controller/state/stack/operation）
  config.rs codex_config.rs gemini_config.rs grok_config.rs
  claude_desktop_config.rs  ★ Claude Desktop 3P profile（2204 行）
  proxy/                  ★ 本地网关（见 1.4）
  database/               SQLite（schema/migration/dao/backup）
  session_manager/        各客户端会话读取
  services/provider/      claude_direct / codex_direct / codex_editor / live.rs 等
docs/guides/, docs/user-manual/{zh,en,ja}/   官方文档（写得很细，含踩坑表）
```

### 1.3 配置管理方式（路径 / 写入 / 切换 / 备份）

**配置文件路径**（`docs/user-manual/zh/2-providers/2.6-claude-desktop.md`、`src-tauri/src/claude_desktop_config.rs:1257-1379`）：

| 客户端 | 路径 |
|---|---|
| Claude Code | `~/.claude/settings.json`（含 `env` 块） |
| Claude Desktop | macOS `~/Library/Application Support/Claude{,‑3p}/`；Windows `%LOCALAPPDATA%\Claude{,‑3p}\`；Linux `$XDG_CONFIG_HOME\|~/.config/Claude{,‑3p}/`<br>具体 4 个文件：`claude_desktop_config.json`、`Claude-3p/claude_desktop_config.json`、`Claude-3p/configLibrary/_meta.json`、`Claude-3p/configLibrary/00000000-0000-4000-8000-000000157210.json` |
| Codex | `~/.codex/config.toml` + `auth.json` |
| Gemini CLI | env + settings |
| Grok Build | `~/.grok/config.toml` |

**切换机制 = 只改关键字段，不重建文件。** v4 把「用供应商快照重写整个配置文件」换成一个写入引擎 `live/`（`src-tauri/src/live/mod.rs` 头注释）：

- `floor`：定义「关键字段」（请求发到哪 / 凭什么鉴权 / 哪个模型名 / 说哪种协议），切换时清空再写目标值；其余键归用户，CC Switch 不写不删。Claude 侧按前缀匹配 `ANTHROPIC_`、`AWS_`、`VERTEX_REGION_`，其余按名单列（`live/floor.rs:1-40`）。
- `project`：纯函数投影器，供应商行 → 关键字段目标值。
- `patch`：按格式保序改写 JSON / TOML / `.env`；**解析不了就停**（不猜）。
- `engine`：写锁 + 计划 + 临时文件 + **首写备份** + 发布前重读比对 hash，hash 变了就以新内容为底重算（`live/engine.rs:1-20`）。文档明确承认这是**乐观并发**：读-比对-rename 之间仍有一个极小窗口会被外部进程写入覆盖。
- `mode/operation.rs` 负责多文件写入的「写前意图 + 崩溃恢复」。

**备份**：
- 每文件**只做一次**「首写备份」，放在设备级目录 `~/.cc-switch/backups/live-first-write/`（`live/engine.rs:43-59`）。v4 起不再每次切换都备份 live 文件。
- 设备级状态 `~/.cc-switch/live-state.json` 记录每个 app 的 direct/routing 模式——刻意不跟配置目录覆盖，也不进云同步。
- 数据库自身备份：SQL 导出 + SQLite 二进制快照（`database/backup.rs`，用 `rusqlite::backup`）。导入 SQL 时有 authorizer 拒绝 `ATTACH`/`VACUUM INTO`/`temp_store_directory` 等能离开临时库的动作（`database/backup.rs:44-58`）。
- 「备份与恢复」页列出全部备份位置与大小（`services/backup_storage.rs`），删除只按类别 id 由后端算路径，**绝不接收前端给的路径**。
- 含密钥的文件按 `0600` 写（owner-only），已存在的 0600 不会被放松。

**三态模式层**（`docs/user-manual/zh/4-proxy/4.2-routing.md`）：

| 模式 | 行为 |
|---|---|
| 直连 | 只改客户端配置里的关键字段，请求直连供应商 |
| 路由 | 客户端基址改写成 `http://127.0.0.1:15721/v1`，CC Switch 常驻转发，可做格式转换 + 故障转移 |
| 聚合 | 仅 Claude Code / Codex：多家供应商的模型进同一个 `/model` 列表，按选中项分发；**不做故障转移**；官方订阅不能作为普通成员 |

聚合用 `ccs-` 前缀的模型 ID 标记「供应商 + 模型」，请求按条目路由（`docs/guides/aggregation-mode-guide-zh.md`）。退出 CC Switch 时会先「写回直连」，下次启动再接上。

### 1.4 是否自己实现协议转换？—— 是，自研本地网关

`src-tauri/src/proxy/`，默认监听 `127.0.0.1:15721`（`proxy/types.rs:46`）。核心分层：

- `providers/adapter.rs`：`ProviderAdapter` trait，统一「base_url 提取 / 认证头注入 / 请求响应转换」。
- `providers/{claude,codex,gemini}.rs`：三个客户端方向。`claude.rs` 头注释列出支持的上游格式：`anthropic`（透传）、`openai_chat`、`openai_responses`、`gemini_native`。
- 转换器按方向成对存在：
  - `transform.rs` + `streaming.rs`：Anthropic ↔ OpenAI Chat（Claude 侧）
  - `transform_responses.rs` + `streaming_responses.rs`：Anthropic 请求 → Responses 请求；Responses 响应 → Anthropic 响应
  - `transform_codex_chat*.rs` + `streaming_codex_chat.rs`：Responses ↔ Chat（Codex 侧）
  - `transform_codex_anthropic.rs` + `streaming_codex_anthropic.rs`：Responses ↔ Anthropic **反向**（Codex 客户端说 Responses，上游只给原生 Anthropic）
  - `transform_gemini.rs` + `streaming_gemini.rs`、`gemini_shadow.rs`
- 流式：专门处理 SSE 分块（`proxy/sse.rs` 的 `take_sse_block` / `strip_sse_field`），逐分片转换而非聚合整包。CHANGELOG 明确记过一个坑：「Mixed LF / CRLF SSE streams no longer lose tool calls」——混合行尾的 SSE 分块处理曾丢工具调用。
- 工具调用：`transform_codex_chat.rs` 有 `CodexToolContext`；还有 `responses_late_arguments.rs`（Codex 把工具参数**延迟**到 `response.output_item.done` 才给全，必须缓冲）、`transform_codex_responses_namespace.rs`（命名空间摊平）、`transform_codex_chat_moonshot_schema.rs`（供应商 schema 怪癖）、`transform_codex_responses_xai_sanitize.rs`。
- 有状态处理值得抄：`reasoning_bridge.rs` 因为 Anthropic Messages 协议**没有**放 Responses `reasoning` item 的字段，就用**版本化 thinking signature / redacted-thinking 载荷**（前缀 `ccswitch-openai-reasoning-v1:`）把它带过去，客户端回放 assistant 消息时再还原。且「还原侧必须永久保留白名单」，因为老信封还躺在用户历史里（`reasoning_bridge.rs:1-20`）。
- 其它整流器：`thinking_rectifier.rs` / `thinking_budget_rectifier.rs` / `thinking_optimizer.rs`（思考档位与预算）、`inline_think.rs`（把 `<think>` / `<thinking>` 内联思考从正文里分流出来，流式用 splitter、非流式用前缀剥离）、`media_sanitizer.rs` + `tool_media.rs`（工具返回里的图片搬运）、`opaque_state_rectifier.rs`、`cache_injector.rs`（cache_control）、`body_filter.rs`、`circuit_breaker.rs` + `failover_switch.rs`（故障转移熔断）。
- 用量统计：`proxy/usage/{parser,calculator,logger}.rs`。

### 1.5 对「Claude Desktop 只能填 Anthropic 模型 ID」的处理 ★

这是本调研最直接相关的一节。cc-switch 的结论写在 `docs/user-manual/zh/2-providers/2.6-claude-desktop.md`：**Claude Desktop 现在会拒绝不属于 Sonnet / Opus / Haiku 三类角色的模型**，所以它做了两条路：

**A. 直连（Direct）—— 只允许合法角色名**

写入 `Claude-3p/configLibrary/<PROFILE_ID>.json` 的 profile：

```json
{
  "inferenceProvider": "gateway",
  "inferenceGatewayBaseUrl": "https://api.example.com",
  "inferenceGatewayAuthScheme": "bearer",
  "inferenceGatewayApiKey": "<key>"
}
```

（`claude_desktop_config.rs:1196-1216` 的 `build_gateway_profile`，另加 `coworkEgressAllowedHosts: ["*"]`、`disableDeploymentModeChooser: true`。）

校验函数 `is_claude_safe_model_id`（`:251-276`）要求模型名必须 `claude-` 或 `anthropic/claude-` 开头，**且角色前缀后还有内容**，角色白名单 `["sonnet-","opus-","haiku-","fable-"]`。注释写明白名单来源：**Claude Desktop 1.12603.1+ 的 fail-all validator 角色白名单直接取自 `app.asar` 内的 `["sonnet","opus","haiku","fable","mythos"]`**，所以 `claude-fable-*` 可安全写入；并特意拒绝了 `claude-sonnet-` 这种退化值，因为「会写入 profile 并**触发 Claude Desktop fail-all 拒收整组**」。直连模式下 `route_id != upstream_model` 的映射会被显式拒绝，报错让人改用本地路由模式（`:541-550`）。旧式 ID 如 `claude-3-5-sonnet-…` 也被拒。

**B. 模型映射（Proxy）—— 把真实模型藏在本地网关后面**

客户端基址改成 `http://127.0.0.1:15721/claude-desktop`；Desktop 只看到 `claude-sonnet-5` / `claude-opus-5` / `claude-haiku-4-5` / `claude-fable-5` 这四个安全 route id（`DEFAULT_PROXY_ROUTES`，`:53-77`）。真实上游模型名**永远不写进 Desktop profile**，只存在 CC Switch 的供应商配置里，请求过网关时由 `map_proxy_request_model`（`:701`）按 route_id / 角色关键词（opus / haiku / fable / sonnet）映射到上游模型。

几个聪明的细节：

- 显示名与真实名分离：`inferenceModels` 数组元素可以是字符串，也可以是对象——用 `{"name": "claude-sonnet-5", "labelOverride": "Kimi K2", "supports1m": true}` 让菜单显示品牌名而 route id 保持安全（`inference_model_json` `:278-291`）。
- 非安全 route id 会被**自动改写成安全 id**：`proxy_model_routes`（`:575-637`）遇到非法 route_id 就借一个未被占用的合法角色名，兜底是 `claude-sonnet-5-r<N>`；`next_catalog_safe_route_id`（`:639`）保证不与已保留的 id 撞车。有专门的回归测试 `claude_desktop_proxy_repairs_legacy_unsafe_route_without_colliding`。
- `[1M]` 后缀在边界翻译：Claude Code env 里用 `[1M]` 声明 1M 上下文，**Claude Desktop schema 不接受这个后缀**，import 时翻译成 `supports1m` 字段（`:39-41`）。`is_claude_safe_model_id` 也直接拒绝含 `[1m]` 的名字。
- 网关认证：给 Desktop 写一个随机生成的本地 token（`ccs-<uuid>`，`get_or_create_gateway_token` `:293-304`），存在 DB settings 里，`/claude-desktop` 路由单独校验 Bearer（`proxy/handlers.rs:401-417`），用独立 provider namespace（`handlers.rs:251-278`），**不与其他客户端的 provider 选择混淆**。
- `/claude-desktop/v1/models` 自己返回模型列表响应（`model_list_response`），带 `supports1m`、`created_at`、`has_more/first_id/last_id`，因为不能依赖上游 `/v1/models` 返回的角色名。
- `configLibrary/_meta.json` 的 `entries` 里登记 `{id: PROFILE_ID, name: "CC Switch"}`，`appliedId` 指向当前生效项（`MetaPatch` `:1135-1194`）；切回官方时**保留 profile 文件、只清关键字段**，所以判「是否配置过」不能看文件在不在（`:156-158`）。
- 状态自检：`ClaudeDesktopStatus` 带 `stale_raw_models`（profile 里混进了非安全模型名）、`missing_route_mappings`、`expected_base_url != actual_base_url` 等，UI 顶部给出「重新切换当前供应商」的处理建议。

另一条重要边界：**Claude Desktop 不做热重载，每次切换必须完全退出重启**；模型映射模式下 CC Switch 必须常驻（服务自动启停，不用手动开）。

### 1.6 值得抄的实现细节 / 踩坑记录

- **fail-all validator**（⚠️ **2026-10-14 更正，见[票 16](../issues/16-research-desktop-model-id-validation.md)**：本机装机版 `Claude_2.37337.1.0` / app 44.7.0 只做**逐条剔除**，代码里没有整组拒收分支；cc-switch 的 `is_claude_safe_model_id` 是真实规则的保守子集——照它写安全但过窄）：写进 profile 的模型名不合规会让 Claude Desktop **拒收整组**，所以写入前必须校验，且校验逻辑要跟着官方白名单走（白名单从 `app.asar` 里读）。
- **profile 文件保留 + 清字段**：恢复官方时删文件会让「配置过」的判定失真。
- **只改关键字段的写入引擎**：把「我拥有哪些键」显式建模（`floor`），比整文件重建安全得多；失败方向要选「把未知键保留下来」。
- **乐观并发要写进文档**：`lock_app` 只保护自家写入方，外部进程（Claude Code 自己）不参与锁，读-比对-rename 有窗口——老实写出来，别假装原子。
- **混合行尾 SSE 丢工具调用**（CHANGELOG 4.0.x）。
- **Chat → Anthropic 流式缺块**（#7986 / #7984）：只在类型切换和 `finish_reason` 路径关块；上游省略 `finish_reason` 或在它之后继续发正文时，最后一个 text / tool 块没关闭，客户端丢内容。修法是终止事件前无条件关掉所有打开的块。
- **路由写死的角色别名会影响客户端思考行为**（fixes #8118）：`claude-haiku-4-5` 在 Claude Code 自己的模型表里被标为「可关闭 thinking」，导致 haiku 档请求带 disabled thinking 被要求 thinking 的上游拒。别名改成 `claude-haiku-5-5`，代理侧仍按 "haiku" 关键词映射。
- **WSL2 里的 Claude Desktop 连不上宿主 loopback**（#7878 / fixes #7877）：写进客户端配置的代理 URL 从监听地址推导，`0.0.0.0` 被写成 `127.0.0.1`，NAT 模式下的 WSL2 客户端够不到。加了「连接地址」设置，存在设备级 `settings.json` 里（不进云同步）。
- **Codex 桌面版模型门控**（`docs/guides/codex-desktop-custom-model-visibility-zh.md`）：桌面 GUI 按登录身份放行模型，检测不到官方登录态就把选择器回落官方默认模型，自定义模型被藏起来（CLI 不受影响）。官方已标记 not planned。CC Switch 的缓解是「保留官方登录态」开关（默认关闭，开启后切换第三方供应商保留 `auth.json` 的官方 OAuth），但**保留的 token 不会被发往第三方**。
- **Codex 顶层 `base_url` 被保存吞掉**（fixes #8039）：editor 把这种配置当 route-less 卡片，保存时把 `base_url` 和 `wire_api` 一起删了。
- **不支持的供应商要显式拒绝而不是尽力而为**：`validate_proxy_provider` 对 `api_format` 非 anthropic 的场景直接报错。
- **Linux / Flatpak 边界**：Flatpak 会用私有的 `XDG_CONFIG_HOME` 替换环境变量，所以刻意固定用宿主 `~/.config`，且**不提供配置目录覆盖项**（`:1280-1314`）。

---

## 2. CodexPlusPlus

### 2.1 语言 / 框架 / 构建

- Rust workspace（`Cargo.toml` + `Cargo.lock`），edition/version 走 `[workspace]`。
- 桌面管理器：**Tauri**（`apps/codex-plus-manager/src-tauri/`，前端 React + TS 在 `apps/codex-plus-manager/src/`）。
- 另有 `apps/codex-plus-launcher`（Rust bin，**被当库链接进管理器进程**，见 `docs/plans/2026-10-13-官方App自动接管-design.md` §1.1）、`apps/codex-plus-mobile-relay`。
- 关键依赖：`reqwest`、`tokio`、`tokio-tungstenite`（CDP WebSocket）、`rusqlite`、`toml_edit`、`fs2`（文件锁）、`zstd`、`sha2`、`aes-gcm`、`qrcode`、`zip`，Windows 上直接用 `windows` crate 调 Win32。
- 规模（`crates/codex-plus-core/src/`）：`protocol_proxy.rs` 7130 行、`launcher.rs` 5396 行、`relay_config.rs` 5411 行、`settings.rs` 3853 行；测试非常厚（`crates/codex-plus-core/tests/` 60+ 个集成测试文件）。

### 2.2 目录结构概览

```
crates/codex-plus-core/src/     核心库
  settings.rs                   ★ RelayProfile / BackendSettings / RelayMode / RelayProtocol
  relay_config.rs               ★ config.toml 生成与 apply（5411 行）
  relay_switch.rs               ★ 切换供应商：快照 + 回滚
  relay_rotation.rs             聚合供应商轮转（失败 / 会话 / 请求 / 权重）
  relay_headers.rs relay_environment.rs relay_switch.rs
  protocol_proxy.rs             ★ Responses ↔ Chat Completions 本地协议转换（7130 行）
  model_catalog.rs model_suffix.rs aggregate_catalog.rs  模型目录与 [1M] 后缀
  codex_home.rs codex_app_state.rs codex_local_storage.rs codex_sqlite.rs
  launcher.rs packaged_launch.rs watcher.rs ports.rs auto_takeover.rs
  cdp.rs bridge.rs native_browser*.rs                 CDP 与渲染进程注入
  claude_localization.rs                              Claude Desktop 汉化（受管副本）
  mcp_config.rs skills.rs plugin_market*.rs dream_skin*.rs whale*.rs
crates/codex-plus-data/src/     storage / backup / provider_sync / session_index_*
apps/codex-plus-manager/        Tauri 管理器 + React 前端
docs/specs docs/plans docs/reports docs/release     设计稿、计划、issue 审核报告
```

### 2.3 配置管理方式（路径 / 写入 / 切换 / 备份）

**路径**（`README.md` 数据表 + `paths.rs` / `codex_home.rs`）：

| 数据 | 位置 |
|---|---|
| Codex 配置与登录 | `~/.codex/config.toml`、`~/.codex/auth.json`（`CODEX_HOME` 可覆盖，必须是已存在的目录） |
| Codex 本地数据库 | 优先 `~/.codex/sqlite/*.db`，旧版回落 `~/.codex/state_5.sqlite` |
| Codex++ 状态与日志 | `~/.codex-session-delete/`（`settings.json`、`codex-plus.log`、`latest-status.json`；可用 `CODEX_PLUS_STATE_DIR` 覆盖） |
| Provider 同步备份 | `~/.codex/backups_state/provider-sync` |
| 首写备份 | `~/.codex/backups/config.toml` + `auth.json`（`relay_config.rs:4904-4923`） |
| Codex 全局状态 | `~/.codex/.codex-global-state.json`（+ `.bak`，快照 `backups_state/app-state-sync/latest-safe-state.json`） |

**写入方式**：`toml_edit` 保序改写 `config.toml`，`atomic_write` 落盘。生成的关键结构：

```toml
model_provider = "custom"
model = "<model>"
model_catalog_json = "<相对路径或 catalog>"

[model_providers.custom]
name = "<transport_provider_id>"
base_url = "http://127.0.0.1:57321/v1"   # 走本地协议代理时
wire_api = "responses"                    # ★ 硬性恒为 responses
requires_openai_auth = true|false
experimental_bearer_token = "<key>"
```

写死 `wire_api = "responses"` 的原因写在 `relay_config.rs:4526-4530`：**Codex 26.901 起不再支持 `wire_api = "chat"`，一旦出现会让整份 `config.toml` 被判无效并回退内置默认模型**；Chat Completions 上游改由本地协议代理做 responses→chat 转换。同时 `requires_openai_auth = true`（官方登录 + API 混合模式）但因为「Codex 0.149 起不再为 `requires_openai_auth=false` 的 provider 借用 `auth.json` 凭据」，纯 API 必须显式带 `experimental_bearer_token`（`:4553-4560`）。

**切换机制**（`relay_switch.rs:18-87`）是**带完整回滚的事务**：

1. 先 `store.load()` 读当前设置——**读失败直接中止切换**，注释写明「不要用默认值继续：回滚分支会把『仅剩默认供应商』写回磁盘，这正是供应商列表被清空的成因」。
2. 快照：settings 原始字节（`snapshot_raw_bytes`）+ Codex live 文件（`LiveFilesSnapshot::capture`）+ app state 快照。
3. 补回请求里缺失的供应商（`restore_missing_saved_profiles`）——前端曾按分页过滤后提交子集，导致模型聚合失败且**其它分页的供应商从 settings.json 里消失**（`:89-93`）。
4. 回填上一个 active profile（`backfill_profile_before_switch`），保存 settings，再 `apply_selected_relay_profile`。
5. 成功 → `sync_app_state_after_provider_switch`；失败 → 恢复 settings 原始字节 + 恢复 live 文件，两个都失败就一起报出来。
6. 切换后额外清理：`prune_stale_model_references_after_switch` 按新目录清掉模型选择器里残留的旧模型 ID（例如别的管理器写下的 `ccs-xxx/model`）。

**备份**：`create_live_backup` 把 `config.toml` / `auth.json` 复制到 `~/.codex/backups/`。`codex-plus-data/src/backup.rs` 是数据层。

**安全兜底值得抄**（`codex_home.rs:13-30`）：issue #2146 里有用户在一次自更新 + Codex 自动更新过程中丢掉整个 `%USERPROFILE%\.codex`（454 MB 会话历史，只剩 1.1% 可恢复）。审计全部 `remove_dir_all` 调用点没找到能删到 `.codex` 的既有路径，但还是加了一道与触发源无关的兜底 `ensure_safe_recursive_removal`：**绝不允许删 `CODEX_HOME` 本身、它的任何祖先目录、或文件系统根**，判定在路径规范化之后做，`..` / 符号链接 / 大小写差异都拦得住。

**四种供应商模式**（`README.md` §供应商与模型，`settings.rs` 的 `RelayMode`）：

| 模式 | 说明 |
|---|---|
| 官方登录 `Official` | 用 ChatGPT / Codex 官方账号 |
| 官方登录 + API `MixedApi`（默认） | 保留官方登录态与插件入口，模型请求走配置的兼容 API，**不消耗官方额度** |
| 纯 API `PureApi` | 自定义 Base URL / Key，不需要官方账号 |
| 聚合 `Aggregate` | 分「故障转移」（多供应商失败切换 / 按会话 / 请求 / 权重轮转）与「模型聚合」（按模型固定到指定成员）；成员可以是 API 供应商，也可以是已登录的官方账号 |

模型聚合把成员模型合并进 Codex 模型列表，`cpx-<供应商ID>/<模型名>` 前缀的条目固定发给对应成员；模型选择器按供应商分组折叠。

### 2.4 是否自己实现协议转换？—— 是，`protocol_proxy.rs`（7130 行）

模块头一句话：**「Codex Responses API 与 OpenAI Chat Completions 的本地协议转换」**。默认端口 `57321`（`DEFAULT_PROTOCOL_PROXY_PORT`），可用 `CODEX_PLUS_PROTOCOL_PROXY_PORT` 覆盖。

- 端口为什么不随机（`protocol_proxy.rs:116-123`）：端口要**写进 `config.toml` 的 `base_url`**，不能像普通 helper 端口那样自动换；但 issue #2189 里少数机器上 57321 恰好被 Hyper-V/WSL 开机划进 Windows 动态端口排除区间，bind 报 `os error 10013` 永远起不来，只能整体挪。于是**写入 base_url 与读取校验都走同一个 `protocol_proxy_port()`**，保证同进程一致。
- 方向：`responses_to_chat_completions` / `responses_to_chat_completions_with_options`（`:391-520`）、`chat_completion_to_response` / `..._with_request`（`:520-596`）。
- **流式**：`ChatSseToResponsesConverter`（`:704`，impl 在 `:1607`）：把 Chat Completions 的 SSE 增量流实时转成 Responses 事件流。回复方向是 `streaming_responses.rs`。`NativeResponsesSseObserver`（`:711-861`）对原生 Responses **只旁路观察终止事件，不改写任何响应字节**，且只保留每个事件的有限前缀以避免图片等大 payload 让诊断缓冲无界增长。
- **工具调用**：
  - `CodexToolContext` / `CodexCustomToolSpec` / `CodexFunctionToolSpec`（`:219-292`）承载工具定义。
  - **跨 turn 原样回传供应商附带数据**（`:19-113`）：Gemini 3 系在 `functionCall` 上要求回传 `thought_signature`（OpenAI 兼容层放在 tool_call 的 `extra_content.google.thought_signature`），缺失时**整轮 400**（issue #332 / #1012）。做法是**不硬编码任何字段名**，把整个 `extra_content` 按 `call_id` 记住（`BTreeMap` + 插入序 `VecDeque`，容量 256 的 LRU 淘汰），下一轮构造请求时挂回对应 tool_call；解析失败 / 无该字段时行为与改动前完全一致。两种形态都收：`extra_content` 里，或直接挂在 tool_call 上的 `thought_signature` / `thoughtSignature`。
  - 有意的**不支持列表**：`UnsupportedEncryptedAgentContent`（`:300`）、`UnsupportedNativeWebSearch`（`:325`）、`remove_unsupported_native_search`（`:345`）、`input_has_encrypted_agent_content`（`:371`）——遇到这类输入显式报错而不是瞎转。
- **远程压缩（compaction）**是这块最复杂的部分：Codex 的 `compact_remote_v2` 在 `input` 末尾带控制 item，期望响应里**恰好一个** `compaction` 输出项。
  - `request_has_compaction_trigger` / `strip_compaction_trigger` / `rewrite_request_for_compaction`（`:167-218`）：把压缩摘要请求改写成上游能理解的普通生成请求（末尾注入 user 摘要指令，清空 `tools` / `parallel_tool_calls` 避免摘要阶段触发工具调用）。
  - `CompactionSseConverter`（`:862-1122`）：把上游摘要文本（Responses 或 Chat、流式或非流式）封装成「恰好一个 `compaction` 输出项」的 Responses SSE 流；输出项通过 `output_item.done` 显式交付，**不能只放进 `response.completed.response.output`**（`:862-865`）。
  - 本地摘要持久化格式有自己的命名空间标识，后缀是 JSON string，「不依赖客户端可能丢弃的 item id 或额外字段」（`:159-163`）。
- 上游 HTTP：`upstream_header_timeout` / `upstream_stream_header_timeout`、`send_upstream_request_for_responses`、`ProxySessionHeaders`（**客户端会话头白名单，不接受鉴权或任意其他请求头**，`:617-634`）。
- 模型路由也在这个模块：`ModelRouteSelection`（`:635`）+ `relay_rotation.rs` 按策略选供应商成员。

### 2.5 对「只能填固定模型 ID」这类限制的处理 ★

CodexPlusPlus 主要面向 Codex/ChatGPT 桌面版，遇到的同类限制与对策：

**A. 模型目录（`model_catalog_json`）+ 后缀语法**

Codex 原生支持 `model_catalog_json` 指针。CodexPlusPlus 用它做**按模型粒度的上下文窗口与自动压缩阈值**（`AGENTS.md` 项目概述，对应 issue #1171 / #931）：

- 用户在 `model_list` 文本域写 `deepseek-v4-pro[1M]`；`model_suffix.rs` 解析后缀（`K/k`=1000、`M/m`=1000000、纯数字也接受，单位为 u64），生成 catalog 时**剥离后缀**，窗口值进 catalog。
- 后缀解析有明确边界：**只有 `]` 是最后一个字符时**才视为后缀；括号内不是合法窗口 token 时整串当 slug 且不剥离（`model_suffix.rs:26-40`）。
- `migrate_model_list_with_suffixes` 一次性把旧格式 `slug[suffix]` 拆成「无后缀列表 + 窗口 map」。
- `relay_config.rs:2837` 附近标注了外部 `model_catalog_json` 与每模型配置冲突时的降级路径（issue #2203）；用户已手写 `model_catalog_json` 指针时**保留不覆盖**（有 `preserves_user_model_catalog_json` 测试）。
- 生成的 catalog 可能很大：直连官方目录时约 **75 KB**，因为含完整 GPT-5 harness 文本，且要求 Codex CLI ≥ 0.144.0（freeform `apply_patch` 注册需要）。

**B. `name == "OpenAI"` 触发 v2 远程压缩的坑**（`relay_config.rs:4513-4524`）

Codex 用**严格匹配 `name == "OpenAI"`** 判定 provider 是否走 v2 远程压缩（`RemoteCompactionSupport::V2`，见 openai/codex issue #42313）。第三方中转顶着这个名字会被要求返回 `compaction` 输出项而**稳定报错**（issue #2217）。对策：只有真正的 OpenAI 会话身份（官方 OAuth / 混合模式）保留该名称，其余一律改写成 provider id，让 Codex 回退到本地压缩。

**C. 桌面应用 Local Storage 里的残留模型名**

模型选择器的最近使用等状态持久化在 Electron 的 atom state / Local Storage 里（`codex_app_state.rs` 有 `COMPOSER_RECENT_MODELS_KEY = "composer-recent-model-configurations-v1"`，只留最近 3 条）。换目录后残留旧 ID，所以有：

- `codex_app_state.rs`：只操作白名单键（`SAFE_TOP_LEVEL_KEYS`、`SAFE_ATOM_KEYS`），切换前后各拍一次快照，`prune_stale_model_references_after_switch`。
- `codex_local_storage.rs:44-97`：**通过 CDP 的 `Runtime.evaluate` 清理页面 Local Storage 里残留的带后缀模型名**（如 `deepseek-v4-pro[1M]`），失败时**不抛错只记诊断日志**（`sanitize_local_storage_model_suffixes_nonfatal`）。
- `model_catalog.rs:155-198` 的 `active_published_model_ids` 明确定义「哪些模型 ID 算当前该公布的」：模型聚合是全部 `cpx-*`（目录由自己生成，是权威）；官方登录且没混用 API Key 时返回**空集表示「不要清理」**；普通 API 供应商收 `model_list` + 当前 `model` + 模型路由源模型名，**并同时收下剥掉 `[1M]` 后缀的形式**——原则写得很清楚：「宁可多留不可误删」，目录算不出来时调用方跳过清理、**绝不误删**。

**D. 模型列表接口的假成功**

`model_catalog.rs:685-718`：部分网关用 HTTP 200 + 业务信封承载失败。智谱 Codex 专属端点（`/api/v1`）在 key 缺失或无效时返回 `{"code":401,"msg":"令牌已过期或验证不正确","success":false}`，HTTP 状态仍是 200，之前只认状态码，这类失败被解析成「0 个模型」，报错完全不指向真因（**#2190**）。对策：**只在模型列表解析为空时才判信封**——能取到数据就优先信数据，避免误伤非标成功响应。`numeric_business_code` 把 `code != 0/200` 视为失败，非数字 code 不参与判定。模型列表请求还加了 30 秒整体预算，「此前请求没有任何超时，上游不响应时管理器的『获取模型』会永久挂起」（`:575-578`）。

**E. 认证位置不能混**

`README.md` FAQ：纯 API 和「官方登录 + API」**使用不同的认证位置**，不要手工复制两种模式的 `auth.json`。

### 2.6 它怎么「接管」官方 App（与配置切换强相关）

不改 `app.asar`、不往安装目录写补丁。链路（`docs/plans/2026-10-13-官方App自动接管-design.md` §1.1）：

`launch_and_inject_with_hooks` = 端口选择 → 起 helper → provider sync → 起协议代理 → 拉起官方 App（带 `--remote-debugging-port=<port> --remote-allow-origins=http://127.0.0.1:<port>`）→ CDP 注入 → watchdog（5s tick，断链重注入，退避 10s→300s）→ `ready`。

- 端口挑选（`ports.rs:84`）：能 bind 就用请求值；已被占用但**确实是 Codex 自己的 CDP 端点**也沿用（否则注入会打到另一个端口）。
- 就绪判定（`packaged_launch.rs:393`）：CDP 端口能连上**并且返回 Codex 页面**才算启动成功——「端口上有 HTTP 服务但不是 Codex 的 CDP」不能判就绪。
- CDP 安全（`cdp.rs:279-306`）：WebSocket URL 必须是 loopback IP、端口必须等于期望的 debug port，拒绝其他主机。
- 窗口选择器分组折叠、增强设置等 UI 层的东西走 renderer 注入（`assets/inject/renderer-inject/*.js`，改分片后必须跑 `node scripts/assemble-renderer-inject.mjs` 重新组装产物）。注入的节点必须带 `data-codex-plus-ext` 属性，否则触发自喂扫描循环（issue #1960）。

### 2.7 值得抄的实现细节 / 踩坑记录

- **切换必须能整体回滚**：settings 字节 + live 文件字节双快照，失败时双恢复；**读设置失败时中止而不是用默认值继续**（否则会把「仅剩默认供应商」写回磁盘，清空用户的供应商列表）。
- **`restore_missing_saved_profiles`**：永远按 id 把磁盘上已有、请求里缺失的供应商补回——**任何调用方都不该把用户配置越切越少**。这是对「前端分页提交子集」这类缺陷的纵深防御。
- **递归删除守卫**（`ensure_safe_recursive_removal`）：数据丢失不可逆时，加一道与触发源无关的兜底；规范化后比较，拦住 `..` / 符号链接 / 大小写 / UNC。
- **`wire_api` 必须恒为 `responses`**：客户端升级会否掉整个配置文件，格式转换的责任全部转移到本地代理。
- **`name == "OpenAI"` 是行为开关**：上游客户端对字符串字面量的严格匹配会在你想不到的地方改变行为（v2 远程压缩）。
- **HTTP 200 + 业务错误码**：只在解析为空时才判信封，避免误伤非标成功响应。
- **模型列表请求必须有超时**：否则 UI 永久挂起。
- **端口要写进配置文件时不能自动换端口**：要换就得整体换，且读写的函数必须同一份（`protocol_proxy_port()`），并解释为什么（Windows 动态端口排除区间，issue #2189）。
- **原生 Responses 只观察不改写**，诊断缓冲要有界并只留前缀。
- **tool_call 附带数据按 `call_id` 缓存并原样回传**，不硬编码字段名——上游加字段时不会 400。
- **压缩响应必须恰好一个 `compaction` 输出项，且经 `output_item.done` 交付**。
- **issue 记录**（`docs/reports/2026-10-09-latest-open-issue-audit.md` 等）：CDP 探测超时只有 300 ms（`cdp.rs:8`）且要求 `/json` 当前含匹配的 Codex 页面 target，页面重载 / target 暂空 / 慢响应都会假阴性；macOS 侧 `open -a X --args` 对**已在运行**的 App **不传参**（只激活窗口），所以补调试端口必须先退出再带参启动；`osascript` 退出码被丢弃 / 「osascript 失败」与「未运行」被压成同一个 `false`，会导致「第一次查询失败就认为 App 已退出」。

---

## 3. 横向结论（对本项目的可复用点）

| 主题 | cc-switch 做法 | CodexPlusPlus 做法 | 建议 |
|---|---|---|---|
| 客户端只认固定模型 ID | 本地网关 + 安全 route id（`claude-sonnet-5`…）+ `labelOverride` 显示品牌名 | `model_catalog_json` 目录 + `[1M]` 后缀剥离 + CDP 清 Local Storage 残留 | 两者结合：profile 里只写安全角色 id，真实模型名留在网关侧配置；显示名用 label 覆盖 |
| 写客户端配置 | 只改「关键字段」（floor），其余字节不动，首写备份一次 | `toml_edit` 保序改写 + 写前快照 + 失败整体回滚 | 采用 cc-switch 的 floor 思路 + CodexPlusPlus 的事务回滚 |
| 协议转换 | 按方向成对写 transform/streaming，覆盖 4 种上游格式，专门处理 SSE 分块与延迟工具参数 | 单一 `protocol_proxy.rs`，Responses ↔ Chat，重点在工具调用跨 turn 附带数据与远程压缩 | 都自己实现；至少要处理：SSE 分块跨包、工具参数延迟到 done、工具附带数据的跨 turn 回传 |
| 状态型协议信息 | 版本化 thinking signature 载荷（`ccswitch-openai-reasoning-v1:`），还原侧永久保留白名单 | `extra_content` 按 `call_id` 缓存 + LRU 淘汰 | 传输「本仓不理解语义」的字段时原样透传 + 版本化命名空间 |
| 备份 | 首写备份 + 设备级 `live-state.json`，0600 写密钥 | `~/.codex/backups/` + app state 快照 | 首写备份 + 设备级状态足够，别每次切换都全量备份 |
| License | **MIT** —— 允许借鉴、可闭源衍生，需保留版权与许可声明 | **AGPL-3.0** —— 网络服务化也会触发开源义务；另有 `THIRD_PARTY_NOTICES.md` | 借 cc-switch 代码风险最低；CodexPlusPlus **只借鉴设计思路，不要拷代码** |

**直接抄代码的许可结论**：cc-switch 是 MIT，可以抄（保留 `LICENSE` 与版权声明即可）。CodexPlusPlus 是 AGPL-3.0-only：修改、分发、以及**通过网络提供服务**都会触发源码提供义务，若本项目不想整体 AGPL，只能参考思路重写实现。

---

## 4. 覆盖范围与未核实项

- 两个仓库都是 `--depth 1` 浅克隆，**只有 HEAD 一个提交**，无法从 git log 看演进过程；版本与历史结论来自仓库内的 `CHANGELOG.md`、`docs/release/`、`docs/reports/`。
- **没有读 GitHub Issues 原文**。本文所有 issue 编号与现象均转引自仓库内文档/代码注释（尤其 cc-switch `CHANGELOG.md` 与 CodexPlusPlus `docs/reports/*.md`），未逐条回链到 issue 页面对账。
- CodexPlusPlus 的 `protocol_proxy.rs` 只精读了头部与函数大纲（`Select-String` 抽取的 fn/struct 清单），转换函数体的逐行实现未读；`relay_config.rs`、`launcher.rs` 同样只读关键段落。
- cc-switch 前端（`src/`）只看了目录结构，未读具体组件实现。
- 两个项目都**没有**「Claude Desktop 只能填 Anthropic 模型 ID」以外更细的 Claude Desktop schema 逆向记录；cc-switch 提到的白名单 `["sonnet","opus","haiku","fable","mythos"]` 来自其注释声称的 `app.asar` 内部，本文未独立验证。**（2026-10-14 票 16 已独立验证：该角色表在本机装机版 app.asar 内逐字存在（`Po`）；但「非法名让整组被拒」不成立——真实判据与处置见[票 16](../issues/16-research-desktop-model-id-validation.md) 与 spec §8.2。）**
- CodexPlusPlus 的 Claude Desktop 相关能力只有 `claude_localization.rs`（汉化，基于受管副本 + 签名与 `app.asar` 一致性校验），**不做配置切换**——它主要服务 Codex / ChatGPT 桌面版。
