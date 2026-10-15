# 纯自研 vs 二次封装：五个候选网关逐条评估（Windows 单机聚合网关）

本文件回答一个问题：**本机单用户 Windows 桌面模型聚合网关，是该纯自研、基于某个开源项目二次封装，还是混合（只移植 codec / 只抄配置写入引擎）？**

配套调研（本目录其它文件，结论直接引用不重复论证）：
- `providers.md` — 上游协议与各家路径契约一手事实
- `client-integration.md` — Claude Code / Claude Desktop / Codex 客户端接入强制约束
- `reverse-proxies.md` — 自有仓库 zen-gate / Astra-Gate / dsh-our-free-model / deepseek-harness-codearts 的转换层实现对比
- `similar-gateways.md` — cc-switch / CodexPlusPlus 的可复用实现细节

---

## 0. 方法与取证口径

- 一手来源优先：**官方文档 + 克隆仓库读代码**。本报告里每条"支持/不支持"都尽量给 `文件:行号`；给不出的写"未核实"。
- 五个候选全部 `git clone --depth 1` 到 `vendor/`，锁定 commit 后逐条读源码，**不采信 README 的市场文案**。
- 唯一例外：claude-code-router 的实际网关 codec（协议转换主体）**不在本仓库内**，是运行时动态加载的独立 npm 包 `@the-next-ai/ai-gateway`（见 §4.2）。这一点决定了它的评估口径。

### 0.1 仓库快照（本次取证基线）

| 候选 | commit | 最后提交 | 语言/运行时 | 仓库体积 | License |
|---|---|---|---|---|---|
| LiteLLM (`BerriAI/litellm`) | `719c44d` | 2026-10-13 | Python 3 (`pyproject` v1.107.0)；另有 Rust crate + TS dashboard | 13,626 文件 / 210 MB | MIT **+ `enterprise/` 目录独立商业许可** |
| new-api (`Calcium-Ion/new-api`) | `7de10f2` | 2026-10-13 | Go 1.26 + React 19 前端 + Electron 壳 | 2,613 文件 / 25 MB | **AGPL-3.0 + §7 附加条款** |
| one-api (`songquanpeng/one-api`) | `8df4a26` | **2025-02-21** | Go（Gin）+ React 前端 | 564 文件 / 2.6 MB | MIT |
| claude-code-router (`musistudio/claude-code-router`) | `057abaa` | 2026-10-10 | TypeScript/Node ≥20（pnpm workspace：core/cli/electron/ui）v3.1.3 | 863 文件 / 54 MB | MIT（但 codec 在外部 npm 包） |
| AIClient-2-API (`justlovemaki/AIClient-2-API`) | `03d90e9` | 2026-10-11 | Node.js ESM v3.5.8 + Go TLS sidecar | 259 文件 / 7.4 MB | **GPL-3.0** |

复现命令：`git clone --depth 1 <repo> .scratch/model-gateway/vendor/<name>`。

### 0.2 评分口径

- ✅ 开箱即可满足（有代码证据）
- ⚠️ 部分满足：能力存在但缺关键分支，或需要改代码/加配置才能达到要求
- ❌ 缺失：没有这条通路，必须自己写

---

## 1. LiteLLM

### 1.1 语言 / 依赖 / 部署形态 / License

**语言与依赖**（`litellm/pyproject.toml:12-39`）：纯 Python。核心依赖含 `tiktoken`、`tokenizers`、`huggingface-hub`、`boto3`、`aiohttp`、`pydantic`。走代理模式还要 `proxy` extra（`pyproject.toml:53-90`）：`fastapi`/`uvicorn`/`gunicorn`/`granian`/`rq`/`redis`/`hiredis`/`apscheduler`/`fastapi-sso`/`PyJWT`/`cryptography`/`pynacl`/`mcp`/`polars`/`RestrictedPython` 等 39 项——这是**重依赖集合**，不是薄封装。

**部署形态**：`litellm-proxy` CLI（`pyproject.toml` `[project.scripts]`：`litellm` / `lite` / `litellm-proxy`），官方主推 Docker + Postgres。另有两个同仓子包：`litellm-proxy-extras`（MIT，`litellm-proxy-extras/LICENSE`）、`litellm-rust/`（未发现 LICENSE 文件，仅 `Cargo.toml`；lint 用）。

**License 与商用/再分发限制——重点**：

- 根 `LICENSE` 是**分段许可**，原文：
  > Portions of this software are licensed as follows:
  > \* All content that resides under the "enterprise/" directory of this repository, if that directory exists, is licensed under the license defined in "enterprise/LICENSE".
  > \* Content outside of the above mentioned directories or restrictions above is available under the MIT license as defined below.
- `enterprise/LICENSE.md` 是关键限制，原文（节选）：
  > This software and associated documentation files (the "Software") may only be used in production, if you (and any entity that you represent) have agreed to, and are in compliance with, the BerriAI Subscription Terms of Service … notwithstanding the foregoing, you may copy and modify the Software for development and testing purposes, without requiring a subscription. … **it is forbidden to copy, merge, publish, distribute, sublicense, and/or sell the Software.**
- 更麻烦的是 `litellm-enterprise` 是 **PyPI 上的独立 proprietary 包**（`license_expression: "LicenseRef-Proprietary"`，见 `https://pypi.org/pypi/litellm-enterprise/json`），且被 `proxy` extra 直接钉为依赖（`pyproject.toml:81`：`litellm-enterprise==0.1.77`）。

**结论**：**可以**把 LiteLLM 的 MIT 部分用于闭源桌面产品的再分发——`enterprise/` 的引用全部是防御性 import（`litellm/litellm_logging.py:278`、`litellm/proxy/proxy_server.py:1007-1015`、`litellm/proxy/hooks/__init__.py:69`、`litellm/proxy/common_utils/callback_utils.py:233-312`、`litellm/proxy/management_endpoints/customer_endpoints.py:233`，都在 `try/except` 或函数内，缺失后降级而非崩溃），所以删掉 `enterprise/` 目录并**从依赖清单里剔除 `litellm-enterprise`** 即可。但必须做两件事：(1) 法务层面确认你不分发 enterprise 包；(2) 不要使用任何 enterprise 功能。**风险等级：中**（不是纯 MIT 仓库，边界靠目录划分，打包脚本一旦整仓打进安装器就侵权）。

### 1.2 需求 1–11 逐条判定

**1. Anthropic Messages 入站 — ✅（本组最强）**

- 路由已注册：`litellm/proxy/anthropic_endpoints/endpoints.py:124`（`POST /v1/messages`）、`:293`（`POST /v1/messages/count_tokens`）；同时在默认允许路由表里 `litellm/proxy/_types.py:442-445`。
- count_tokens 是真的实现，不是桩：`litellm/llms/anthropic/count_tokens/transformation.py:83` 注释写明 `Beta header required: anthropic-beta: token-counting-2024-11-01`，并要求上游；对非 Anthropic 上游由通用 counter 兜底。
- `anthropic-beta` / `anthropic-version` 透传与合并有专门基建：`litellm/anthropic_beta_headers_manager.py:326-390`（去重、过滤、按 provider 映射）、`litellm/constants.py:1674-1679`（`anthropic-beta` 在 header 白名单里，且支持 `x-pass-anthropic-beta:` 前缀改写）。注意 `should_filter_anthropic_beta_headers`（`litellm/llms/base_llm/anthropic_messages/transformation.py:130`）默认**会按 provider 过滤**——原生 Anthropic 直连场景在 `litellm/llms/openai_like/messages/transformation.py:63-64` 被显式关掉（`return False`），即透传。这正好对应"原样转发"的要求。
- 顺序与生命周期：有专门的 Anthropic SSE 组装/重写模块 `litellm/llms/anthropic/pass_through/messages/streaming_iterator.py`、`litellm/llms/anthropic/pass_through/adapters/transformation.py:1674-1731`（`thinking_delta` 合成），以及 `litellm/proxy/anthropic_endpoints/streaming_model_restamp.py:22` 对 ping 帧的识别（`_PING_MARKERS = (b"event: ping", b'"type": "ping"', ...)`）。
- ping 转发：`litellm/llms/anthropic/pass_through/messages/streaming_iterator.py:46-68` 明确判断"整块只有 ping 帧"并单独处理；另有主动 keepalive 注入 `litellm/proxy/common_utils/sse_keepalive.py:12`（`SSE_COMMENT_PING = ": ping\n\n"`）。

**2. OpenAI Chat + Responses（含 Codex 语义）— ✅**

- `/v1/responses` 全套：`litellm/proxy/response_api_endpoints/endpoints.py:199`（POST）、`:682`（GET 单条，支持 `previous_response_id` 回放）、`:798`（DELETE）、`:907`（`input_items`）、`:1133`（`input_tokens`）、`:1206`（cancel）、`:1616`（WebSocket）。
- 流式结束事件与 function_call 配对：`litellm/responses/litellm_completion_transformation/streaming_iterator.py:405-406` 处理 tool args 增量续传（`already_streamed` → `remaining_args`），`:1297` 注释"回传它中途观察到的同一组 ID"。
- 跨 provider 的中途续写：`litellm/router.py:3340-3382` 构造 `continuation`（注入 developer 指令 + 已生成 assistant 文本），`:3384-3409` 说明与 chat 路径的 parity——这解决"流中断后不能从头再来"。

**3. Gemini 原生 / Ollama 入站 — ✅**

LiteLLM provider 覆盖极广：`litellm/llms/vertex_ai/`、`litellm/google_genai/`、Ollama 在 `litellm/llms/` 下有对应 provider。

**4. 三种 wire + 每家路径契约 — ✅（唯一有 native Anthropic 直通抽象的）**

这一点是 LiteLLM 与其它候选的分水岭：

- 原生 Anthropic 转发基类：`litellm/llms/openai_like/messages/transformation.py:15-29` — 文档字符串直说"把 Anthropic /v1/messages 请求转发给一个原生暴露 Anthropic Messages API 的 OpenAI-compatible 服务，**不做翻译**"，并靠 `model_info.supported_endpoints` 含 `"/v1/messages"` 来开启。这对 Qwen / 任一"Anthropic 形状但非 Anthropic 官方"的上游是关键开关。
- 路径契约由 `get_complete_url` 集中控制：`litellm/llms/openai_like/messages/transformation.py:95-110`（`removesuffix("/v1")` 后拼 `/v1/messages`）；DeepSeek 有专用实现。
- DeepSeek 专用：`litellm/llms/deepseek/messages/transformation.py:37-43` 默认 base 就是 `https://api.deepseek.com/anthropic`；`:74-94` 的 `get_complete_url` 会 `removesuffix("/v1")`、`removesuffix("/beta")`、必要时补 `/anthropic`；`:126-127` 对 `tools` 做 DeepSeek 专用洗白（去掉 Anthropic 独有的 `{"type":"custom"}` 判别符，`:96-109` 实现）——这与 `providers.md` 记录的上游约束一致。
- JSON 配置型 provider 走 `providers.json`：`litellm/llms/openai_like/providers.json` + `JSONProviderAnthropicMessagesConfig`（`litellm/llms/openai_like/messages/transformation.py:113-176`），可以在不改 Python 代码的前提下新增一家上游。
- 其它已内建的原生 Anthropic 兼容上游：`litellm/llms/deepseek/messages/`、`litellm/llms/github_copilot/messages/`、`litellm/llms/bedrock/messages/`、`litellm/llms/vertex_ai/vertex_ai_partner_models/anthropic/`、`litellm/llms/azure_ai/anthropic/`。

⚠️ 缺口：**commandcode `/provider/v1/messages` 和 opencode 的 `/openai` `/anthropic` family 前缀没有现成实现**。但抽象层在（`get_complete_url` 可子类化 / `providers.json` 可配 base_url），属于"加一个 config 或一个小 subclass"的量级，不是重写。

**5. 非标准陷阱 — ✅（本组唯一逐条命中）**

- **DeepSeek + tools 必须回传 `reasoning_content`**：`litellm/llms/deepseek/chat/transformation.py:64-102`，方法名就叫 `_fill_reasoning_content`，docstring 直接引用了上游报错原文 `"The reasoning_content in the thinking mode must be passed back to the API."`；逻辑是优先从 `provider_specific_fields["reasoning_content"]` 提升，否则注入单空格占位并打 warning。调用点 `:305-315`（同步）与 `:333-338`（异步）。
- **DeepSeek 流式 usage 附在最后内容块**：litellm 的流式累积器专门处理这种"usage 不单独发 chunk"的形态（`litellm/litellm_core_utils/streaming_handler.py:242`、`litellm/litellm_core_utils/streaming_chunk_builder_utils.py:1498-1503` 从最后一块回收 `reasoning_content`/usage；`litellm/proxy/spend_tracking/spend_tracking_utils.py:551` 注释"中途失败的请求没有可用的 response_obj usage"）。
- **keep-alive 空行 / SSE 注释容忍**：`litellm/litellm_core_utils/json_fragment_accumulator.py:42` 明确"跳过空白 keepalive 片段（例如来自畸形上游流）"；`litellm/proxy/common_utils/sse_keepalive.py` 有 `split_complete_sse_frames`；`litellm/proxy/guardrails/guardrail_hooks/presidio.py:146` 注释"转发前导的无 data SSE 帧（注释型 keepalive）"。
- **Qwen thinking signature 恒空 / stop_sequence 恒 null**：⚠️ 未找到 Qwen 专属的 signature 规范化代码。LiteLLM 走 native passthrough 时是**原样转发**（这正是 `openai_like/messages/transformation.py:15-29` 的设计意图），所以恒空 signature 会原样透传给客户端——能不能被 Claude Code 接受，取决于客户端容错，属于**需要在网关自己加一层规范化**的点。

**6. 固定 4 别名 + 候选与轮换 — ✅**

- 别名：`model_group_alias`（`litellm/types/router.py`）、`model_name` 即逻辑模型名，`model_list` 里多个 deployment 共享同一 `model_name` 即成为候选池。
- 轮换策略可选（`litellm/router_strategy/`）：`simple-shuffle`（默认）、`lowest_latency.py:53`、`lowest_cost.py:15`、`lowest_tpm_rpm*.py`、`least_busy.py:114`、`lar1_routing.py:89`。

**7. Codex `model_catalog_json` + `wire_api=responses` — ⚠️ 部分**

`/v1/models` 存在且可自定义模型列表（LiteLLM 的 model_list 就是权威），`/v1/responses` 完整（见第 2 条）。但**"替换 Codex 内置 catalog 文件"这件事属于客户端侧写文件**（`model_catalog_json` 是 Codex 的 `config.toml` 字段），LiteLLM 完全不做客户端配置——见第 8 条。网关侧要做的只是"`/v1/models` 返回你想要的四个别名 + 上游真实 model id"。

**8. 客户端配置自动写入/备份/幂等/还原 — ❌**

零能力。仓库里没有任何 `.claude/settings.json`、`.codex/config.toml`、`Claude-3p`、注册表策略键的写入代码。唯一相关的是 `litellm/proxy/client/cli/commands/configure_setup.py:61`（给 CLI 自己发请求时加 `anthropic-version` 头）和 `litellm/proxy/client/cli/commands/agents.py:107`（一条注释说明 Claude Code 会把 `/v1/messages` 追加到 `ANTHROPIC_BASE_URL`）——是提示，不是实现。**这一块必须自己写（或抄 cc-switch / claude-code-router，见 §6）。**

**9. 账号池 + 冷却 + 粘性（已吐字不许切）— ✅（机制最完整的通用实现）**

- 多凭据：同一个 `model_name` 下配多个 deployment，各带自己的 `api_key`/`api_base`。这是 LiteLLM 的标准用法。
- 失败冷却：`router.py:956-958`（`allowed_fails` / `allowed_fails_policy` / `cooldown_time`）、`litellm/router_utils/cooldown_handlers.py:631-679`（`should_cooldown_based_on_allowed_fails_policy`，按异常类型配 `AllowedFailsPolicy`：`router.py:15223-15275` 区分 Authentication/Timeout/RateLimit/BadRequest/InternalServer/ServiceUnavailable…）、`litellm/router_utils/cooldown_cache.py:79-151`（冷却 TTL 与剩余时间）。
- **粘性 / 不中途切号**：`litellm/router.py:3340-3409` 的 mid-stream fallback 语义是**续写而不是重放**——"Partial content: inject a developer instruction + prior assistant message carrying the generated text so the fallback model continues rather than restarts."并做 usage 合并（`:3408-3409`）。这比"绝不切号"更进一步：切了也不会让已吐字重复。另有 session-id 亲和：`litellm/litellm_core_utils/provider_affinity.py:56-83`（`get_stable_session_id`）+ `:86-98`（`add_provider_affinity_header`），通过可配置 header 把 session 粘到同一上游；`litellm/router.py:594`、`:5668` 明确"ping keepalive 不携带生命周期"因此不影响该判定。
- ⚠️ 注意：亲和是**按 session_id / 请求 header**，不是按"本机单用户单会话"自动来。要满足本题的"已吐字不许切"，实际依赖的是 mid-stream 续写机制 + `num_retries` 预算，需要在配置里显式调（`litellm/proxy/proxy_server.py:9884-10142` 一整套 keepalive/fallback 协同）。

**10. 桌面应用 + 托盘 + SQLite + DPAPI + JSON 导入导出 — ⚠️/❌**

- SQLite：**能**（Prisma schema，`schema.prisma`；`pyproject.toml` 的 `extra_proxy` 里有 `prisma`，`migrations/` 目录存在）。
- 桌面壳 + 托盘：**没有**。`ui/litellm-dashboard` 是 Next.js 网页后台（`ui/Dockerfile`、`ui/nginx.conf`），不是桌面应用。
- DPAPI：**没有**。密钥走 `litellm/secret_managers/`（环境变量/AWS/GCP/Azure KeyVault 等），没有 Windows 用户态加密。
- JSON 配置导入导出：配置本身就是 YAML（`proxy_server_config.yaml`），可读写，但不是"面向终端用户的导出/导入"。

**11. 用量与成本统计、请求日志、调试回放 — ✅**

- 成本/单价数据：`litellm/cost_calculator.py` + `litellm/model_prices_and_context_window.json`（含 schema 文件与备份副本）。
- 花费日志/RES 行：`litellm/proxy/db/db_spend_update_writer.py`；预算：`litellm/budget_manager.py`。
- 请求日志与追踪：`litellm/proxy/spend_tracking/`、`litellm/integrations/otel/`、`litellm/integrations/datadog/`。
- 调试：管理端点有 `/reload/anthropic_beta_headers`、`/schedule/...` 之类运维面（`litellm/proxy/proxy_server.py:19807+`），以及 `/ui/?page=logs` 网页日志页。**但没有"逐请求 raw trace 回放"**（那是 claude-code-router 的强项，见 §4）。

### 1.3 若以它为核心做 Windows 桌面应用

要额外写的：
1. 桌面壳（Tauri/Electron）+ 托盘 + 开机自启 + 单实例。
2. **Python 运行时打包**：PyInstaller/Nuitka 或嵌 CPython。难点是依赖集合里 `tiktoken`/`tokenizers`/`huggingface-hub`/`boto3`/`polars`/`cryptography`/`lxml` 类二进制轮子——体积与杀软误报是实打实的成本，保守估计**安装包 300–600 MB 量级**。
3. `litellm-enterprise` 剔除 + 依赖裁剪（否则侵权）。Prisma 需要 `prisma generate` 与迁移文件，`extra_proxy` 里还带 `psycopg`/`google-cloud-kms` 等你用不到的云依赖，得做依赖白名单。
4. 客户端配置写入引擎（第 8 条）——完全自研。
5. DPAPI 密钥封装 + 把上游 key 从 YAML/环境变量迁到你的加密存储。
6. 桌面 UI 要替代/收编 `ui/litellm-dashboard`（Next.js 服务端渲染 + 需要登录体系，与"单用户本机"模型不匹配）。

**打包分发成本：高。** 它不是为嵌入式/单机分发的形态设计的（自定位是团队级 LLM 网关 + 计费 + 多租户）。

### 1.4 账号池/多凭据能力评级

**强**：多 deployment 池 + 按异常类型细分允许失败次数 + 冷却 TTL + 多种路由策略 + mid-stream 续写 + 可配置 session 亲和 header。
**弱/局限**：亲和粒度是 session_id，不是"当前这次对话已经吐字了"的运行时状态；单机单用户场景下这套（Redis 缓存、多 worker 一致性的 `allowed_fails` 计数器）是**过度设计**——`cooldown_handlers.py:682-708` 专门处理"Redis 不可用时回退到本 worker 内存计数"，说明它假设的是多实例部署。

---

## 2. new-api

### 2.1 语言 / 依赖 / 部署形态 / License

**语言栈**（`AGENTS.md` + `go.mod`）：Go 1.26 后端（Gin + GORM v2），React 19 + Rsbuild 2 前端（`web/`），**已带 Electron 桌面壳（`electron/`）**，两个独立子模块 `relaykit/`（协议 DTO 与转换）与 `tokenkit/`（token 估算）。数据库 SQLite / MySQL / PostgreSQL 三选一（`go.mod:20` 有 `github.com/glebarez/sqlite`），日志库可另配 ClickHouse。缓存 Redis + 内存。另有 JS 任务插件运行时（moejs）。

**部署形态**：单一 Go 二进制 + 内嵌前端（`router/main.go` 用 `gin-contrib/static`），官方是 Docker Compose；`electron/main.js` + `electron/create-tray-icon.js` + `tray-icon-windows.png` 说明托盘壳已经写好。

**License 与再分发限制——重点**：

- `LICENSE` 是 **AGPL-3.0**。
- `NOTICE` 追加了 **AGPLv3 §7 附加条款**，原文：
  > Pursuant to Section 7(b) of the GNU Affero General Public License version 3, the following reasonable legal notice and author attribution must be preserved by modified versions in the Appropriate Legal Notices and in any prominent about, legal, footer, or attribution location presented by the user interface: "Frontend design and development by New API contributors."
  > Modified versions that present a user interface must also preserve a visible link to the original project in a prominent about, legal, footer, or attribution location: https://github.com/QuantumNous/new-api
  > Modified versions must not misrepresent the origin of the software and must mark their changes in accordance with AGPLv3 Section 7(c).
- **依赖里有一个 proprietary 组件**：`THIRD-PARTY-LICENSES.md` 首行表格明确 `github.com/Calcium-Ion/go-epay` 是 `Proprietary/Internal - owned by project maintainer`（生产依赖）。
- `AGENTS.md` 里还有项目方的品牌保护条款（不得移除 new-api / QuantumNous 署名），以及**明确不受理**的问题类型清单："Coding Plan, reverse-engineered channels, third-party wrappers, **Codex reverse-proxy compatibility**, pass-through-only forwarding, third-party hosts"。

**结论**：AGPL-3.0 对"分发给用户的桌面应用"意味着**你必须把整个应用的对应源码以 AGPL 提供给用户**（§13 网络服务条款在纯本机场景不是主要触发点，但 §5/§6 的分发义务仍在，Electron 安装器就是在分发）。加上 §7 附加条款要求 UI 里保留"Frontend design and development by New API contributors."和原项目链接，再加上 `go-epay` 是 maintainer 私有许可证的生产依赖。**对一个要闭源/带品牌的商业桌面产品，AGPL 基本是阻断项；即便你愿意开源，`go-epay` 的 proprietary 属性仍需要清理或替换。** 这是本次评估里**最硬的许可阻断**。

### 2.2 需求 1–11 逐条判定

**1. Anthropic Messages 入站 — ⚠️（count_tokens 被作者主动关掉）**

- `/v1/messages` 已注册并按 relay 格式分派：`router/relay-router.go:97-99`（`controller.Relay(c, types.RelayFormatClaude)`）。
- header 透传做得很到位：`relay/channel/claude/adaptor.go:105-113`（`CommonClaudeHeadersOperation`，把客户端多行 `anthropic-beta` 用逗号合并后转发）、`:119-123`（`anthropic-version` 透传，缺省补 `2023-06-01`）；有回归测试 `relay/channel/claude/adaptor_test.go:229-238`。
- **count_tokens 是 ❌**：实现存在（`controller/relay.go:227` `CountClaudeTokens`，用 `service.CountRequestToken` 本地估算，跳过上游与计费），但**路由被注释掉了**：
  ```go
  // TODO: /messages/count_tokens is disabled. The current controller.CountClaudeTokens
  // httpRouter.POST("/messages/count_tokens", controller.CountClaudeTokens)
  ```
  （`router/relay-router.go:95-96`）。`constant/context_key.go:57` 与 `service/token_counter.go:57` 还留着相关开关痕迹。要满足需求 1 必须自己把这一行放回去（并承担作者"disabled"所暗示的未明问题）。
- 另有流式 `message_delta` usage 修补逻辑：`relay/channel/claude/adaptor.go:65-77`（`buildMessageDeltaPatchUsage` / `shouldSkipClaudeMessageDeltaUsagePatch`），以及测试 `relay/channel/claude/message_delta_usage_patch_test.go`——对"Anthropic 客户端只认 message_delta 里的 usage"这类兼容问题有针对性处理。

**2. OpenAI Chat + Responses — ⚠️**

- Chat Completions：`router/relay-router.go:105-107`。✅
- Responses：`RelayFormatOpenAIResponses`（`relaykit/types/relay_format.go:9`）与 `RelayFormatOpenAIResponsesCompaction`（`:10`）都在，`POST /responses/compact` 已注册（`router/relay-router.go:110-112`），`GET /responses` 走 WebSocket（`:78`，`controller.ResponsesWebSocket`，有 `controller/responses_websocket_test.go` 与 `relay/responses_websocket_test.go`）。`POST /v1/responses` 由任务插件协议路由承接（`router/task-plugin-protocol-router.go:29-34`，`case "openai_responses.create"` → `controller.RelayTaskPluginEndpoint`，回退到 `RelayFormatOpenAIResponses`）。
- 专门的 Codex/ChatGPT 上游通道：`relay/channel/codex/adaptor.go:141-151`，路径 `/backend-api/codex/responses`、`/backend-api/codex/responses/compact`、`/backend-api/codex/alpha/search`；`:29/:44/:48/:52` 明确"codex channel 不支持 /v1/messages、/v1/chat/completions、/v1/rerank、/v1/embeddings"（即该通道**只**做 responses——与 `providers.md` 里"每个模型只属一个 family"的现象同源）。
- ⚠️ 缺口：**`POST /v1/responses` 的注册路径绕经任务插件协议路由**，是否 100% 满足 Codex 的 `response.completed` / `previous_response_id` 回放 / `function_call`+`function_call_output` 配对，需要实机验证（本报告不实机，标为未核实）。

**3. Gemini 原生 / Ollama 入站 — ✅**

- Gemini：`router/relay-router.go:46-53`（`/v1beta/models`）、`:55-62`（`/v1beta/openai/models`）、`:29-30`（按 `x-goog-api-key` 或 `?key=` 分派）、relay 格式 `RelayFormatGemini`（`relaykit/types/relay_format.go:8`）；通道目录有 `relay/channel/gemini`、`relay/channel/palm`、`relay/channel/vertex`。
- Ollama：`relay/channel/ollama`，且 `relay/channel/ollama/adaptor.go:75-79` 也做 `anthropic-version` 透传。

**4. 三种 wire + 每家路径契约 — ✅（隐藏王牌：advanced custom 通道）**

- 常规通道的路径是硬编码的：Claude 通道 `relay/channel/claude/adaptor.go:76-77` 固定 `fmt.Sprintf("%s/v1/messages", info.ChannelBaseUrl)`——所以只想靠"自定义 base_url"覆盖 `https://api.deepseek.com/anthropic`（✅ 能）和 Qwen 不带 `/v1`（✅ 能，因为 base 由你给），但**覆盖不了 commandcode 的 `/provider/v1/messages`，也覆盖不了 opencode 的 family 前缀**。
- **但 `advancedcustom` 通道解决了这个问题**：`relaykit/dto/channel_settings.go:183-184` 定义了 `IncomingPath` / `UpstreamPath` 路由映射；`:238-304` 是匹配逻辑（`MatchPath` / `MatchPathForModel` / `SupportsPathForModel`）；`relay/channel/advancedcustom/adaptor.go:367-379` 按下游请求路径查表，`:397-465` 支持 `UpstreamPath` 是**完整 URL 或带模板的相对路径**（`applyUpstreamPathTemplate`，`:467+`），并可拼接 `ChannelBaseUrl`。`relay/channel/advancedcustom/adaptor.go:500-505` 处理 `anthropic-version`；`relay/channel/advancedcustom/adaptor_test.go:169` 断言 `anthropic-version == "2023-06-01"`。
  → 也就是说：**给 commandcode 配一条 `IncomingPath: /v1/messages` → `UpstreamPath: /provider/v1/messages` 的规则，协议问题就变成配置问题。** 这直接命中需求 4 的"每家路径契约不同"。
- 上游 head/body 覆写能力也在：`relay/common/override_test.go:1850-2043` 覆盖 `anthropic-beta` 的保留/追加/删除/去重，:2205-2228 覆盖运行时 header 改写。

**5. 非标准陷阱 — ⚠️/❓**

- DeepSeek 通道存在（`relay/channel/deepseek/`，仅 `adaptor.go` + `constants.go`，很薄），但**未找到 `reasoning_content` 回填逻辑**（在全仓 Go 代码中搜索 `reasoning_content` 无命中）。同族的 GitHub Copilot 依赖 `@github/copilot-sdk`。→ 需求 5 的第一项（DeepSeek+tools 必须回传历史 `reasoning_content`）在 Go 侧**看不出实现**，标 ❌/需自研。
- 流式 usage 落位、keep-alive 空行容忍、Qwen signature 恒空：**未核实**（本报告未逐条追 Go 侧流式解析）。
- 有 `relay/helper/` 与 `relay/relayconvert`（在 `relaykit`）承载转换，是二次封装的可用切入点。

**6. 固定 4 别名 + 候选与轮换 — ✅**

- 模型映射有专门字段（`model_mapping`，见 `controller/model_management_test.go:1330` 构造样例），别名/分组通过 `group` + `models` 字符串 + `ability` 表实现。
- 轮换：`model/ability.go:66-152`（按 `priority` 分层，层内按 `weight` 加权随机；`:144-153` 是权重抽签实现）、`model/channel_cache.go:117-208`（`GetRandomSatisfiedChannel`，含"权重全 0 时平滑"处理）。**但这是"每请求随机"，天然不粘**——见第 9 条的补救。

**7. Codex `model_catalog_json` + `wire_api=responses` — ⚠️**

`/v1/models` 已实现且按客户端类型分派返回形状（`router/relay-router.go:21-44`，`controller.ListModels` in `controller/model.go`，按 `x-api-key`+`anthropic-version` 判 Anthropic 形状、按 `x-goog-api-key` 判 Gemini 形状、否则 OpenAI 形状）。上游侧 Codex responses 也通（第 2 条）。但**客户端侧 `model_catalog_json` 写文件依然不做**——而且这是本项目**明确声明不受理**的场景（`AGENTS.md`: "Codex reverse-proxy compatibility"）。

**8. 客户端配置自动写入/备份/幂等/还原 — ❌**

零能力。全仓搜不到 `.claude/settings.json` / `.codex/config.toml` / `Claude-3p` 的写入。它自带 `electron/` 壳但那是"跑自己的 Go 服务 + 管理台"的壳，不是客户端配置注入器。

**9. 账号池 + 冷却 + 粘性 — ✅（意外地强，与"每请求随机"并不矛盾）**

这是本次评估里 new-api 最超预期的一块：

- 多凭据 = 同 `group` + 同 `models` 的多条 channel，带 `priority`（`model/channel.go:45`）、`weight`（`:31`）。
- 失败与自动禁用：`retry` 次数驱动 priority 层递进（`service/channel_select.go:94-179` 有一整段 ASCII 图解释 `retry → priorityRetry → group 递进`）。
- **粘性会话（session affinity）是完整子系统**：`setting/operation_setting/channel_affinity_setting.go` 定义 `ChannelAffinitySetting{Enabled, SessionMode, SwitchOnSuccess, KeepOnChannelDisabled, MaxEntries, DefaultTTLSeconds, Rules}` 与 `ChannelAffinityRule{ModelRegex, PathRegex, UserAgentInclude, KeySources, ValueRegex, TTLSeconds, ParamOverrideTemplate, SkipRetryOnFailure, SessionMode}`（`SessionMode` 取 `off|prefer|strict`，`:34-36`）。实现体 `service/channel_affinity.go`（1019 行）+ `pkg/cachex` 命名空间 `new-api:channel_affinity:v1`。
- 选择逻辑：`service/channel_select.go:279-345` 注释与代码写明"pinned channel 优先 → **session affinity（仅首次尝试）** → 随机合格 channel"，`:343` 在 `SessionMode == "strict"` 且亲和不可用时直接返回 `503 strict_session_binding_unavailable`——**这就是"已吐字不许切号"的硬保证**。
- `switch_on_success` / `keep_on_channel_disabled` 是两个很贴题的旋钮。
- 还预置了 Codex CLI 与 Claude CLI 的 header 透传清单：`channel_affinity_setting.go:56-89`（`Originator`/`Session_id`/`Thread_id`/`X-Codex-Turn-State`/`Anthropic-Beta`/`Anthropic-Version` 等，且注释里给了 openai/codex 的三个 commit 作为一手来源）。
- 后台管理面齐全：`router/api-router.go:225-226`（`GET/DELETE /api/option/channel_affinity_cache`）、`:326`（`GET /api/log/channel_affinity_usage_cache`）。

**10. 桌面应用 + 托盘 + SQLite + DPAPI + JSON 导入导出 — ⚠️（四项里中两项）**

- Electron 壳 + 托盘：**已有**（`electron/main.js`、`electron/preload.js`、`electron/create-tray-icon.js`、`tray-icon-windows.png`、`entitlements.mac.plist`），`NOTICE` 提醒"Electron/Chromium 第三方许可必须随桌面安装器保留"。
- SQLite：**已有且是默认选项**（`go.mod:20` `github.com/glebarez/sqlite`；`AGENTS.md` 要求三库同时可用）。
- DPAPI：**❌ 未见**。上游 key 存在 channel 表里，加密与否未核实（`model/channel.go` 的 `Key` 字段未见加密钩子；`setting/` 下也没有 DPAPI 相关）。→ 需要自研。
- JSON 导出/导入：**❌ 面向终端用户的一键导出/导入未见**（有后台的通道批量导入/导出接口，量级不同）。

**11. 用量与成本、日志、调试回放 — ✅/⚠️**

- 用量与计费：`model/log.go`、`model/pricing.go`、`model/model_pricing_config.go`、`model/pricing_default.go`、`model/legacy_dalle_pricing.go`、`model/usedata.go`、`service/tiered_settle.go`、`pkg/billingexpr/`（表达式计费，测试里能看到 `tier(...)` / `param(...)` / `header("anthropic-beta")` 这类条件）。日志库可选 ClickHouse（`model/clickhouse_log_test.go`）。
- 请求体留存/回放：`common/body_storage.go`（+ 测试），配合 `controller/responses_websocket_test.go:567` 里对 `previous_response_id` 的断言——有"两轮续跑"的测试证据。

### 2.3 若以它为核心做 Windows 桌面应用

要额外写的：
1. **放回 count_tokens 路由**（一行）+ 验证它 disable 的原因是否影响你。
2. **实现 DeepSeek `reasoning_content` 回填**（Go 侧重写一遍，可参照 LiteLLM 的 `_fill_reasoning_content` 思路）。
3. **DPAPI 密钥加密层**（channel key 落库前的加解密 + 迁移）。
4. **客户端配置写入引擎**（`~/.claude/settings.json`、`~/.codex/config.toml`、`Claude-3p\configLibrary\*.json` + 注册表策略键）——完全自研。这一块可以直接抄 cc-switch（MIT）或 claude-code-router（MIT）。
5. 删掉你不需要的模块以降体积（支付 `controller/topup.go`、`subscription_payment_*`、订阅、第三方登录、多租户、OAuth/OIDC、Casbin 权限——本机单用户全用不上）。砍掉支付模块同时也**解决 `go-epay` proprietary 依赖问题**。
6. 从"多用户计费网关"退化到"单用户配置中心"的 UI 改造（现 UI 是 7 语言的管理台）。

**打包分发成本：中。** 好的一面是**单 Go 二进制 + 内嵌静态前端 + 自带 Electron 壳**，工程上比 LiteLLM 的 Python 分发轻得多。坏的一面是 **AGPL-3.0 + §7 附加条款 + `go-epay` proprietary** 三重法律成本。

### 2.4 账号池/多凭据能力评级

**强，且比 LiteLLM 更贴本需求**：priority/weight 池 + 失败重试分层 + 自动禁用 + **session affinity（off/prefer/strict）+ TTL + 规则化 key 来源 + Codex/Claude CLI header 白名单 + 专门的一致性缓存与统计**。这套是**为反代/CLI 场景长出来的**（注释直接引用 Codex 上游 commit），不是通用负载均衡的副产品。

---

## 3. one-api

### 3.1 定性：已停滞，且能力是 new-api 的真子集

- 最后提交 **2025-02-21**（`8df4a26`），距本次调研约 20 个月无更新；仓库只有 564 文件 / 2.6 MB。
- License：MIT（`LICENSE` 头部 `MIT License / Copyright (c) 2023 JustSong`），无 §7 附加条款、无 proprietary 依赖。**这是它唯一明显优于 new-api 的地方。**
- Go + Gin + React 前端 + GORM。new-api 是它的 fork（new-api 的 `go.mod` 模块名是 `github.com/QuantumNous/new-api`，代码结构与 one-api 高度同源）。

### 3.2 需求 1–11 逐条判定（重点是与 new-api 的差集）

**1. Anthropic Messages 入站 — ❌**

`router/relay.go` 的完整路由表（`:20-73`）里**只有** completions/chat/completions/edits/images/embeddings/audio/files/moderations/assistants/threads，**没有任何 `/v1/messages` 或 `/v1/messages/count_tokens`**。

**2. OpenAI Chat + Responses — ⚠️（仅 Chat）**

`:24-25`（completions / chat/completions）✅；`/v1/responses` **❌ 完全没有**。

**3. Gemini / Ollama 入站 — ⚠️**

有 `relay/adaptor/palm`、`relay/adaptor/gemini`（旧版）等目录，但**没有 `/v1beta/models` 这类 Gemini 原生入站路由**（见 `router/relay.go` 全表）。Ollama 未核实。

**4. 三种 wire + 路径契约 — ❌**

`relay/adaptor/anthropic/adaptor.go:23-25` 固定 `fmt.Sprintf("%s/v1/messages", meta.BaseURL)`——只能满足 DeepSeek / Qwen 那种"换 base_url"的情形，**无法**表达 commandcode / opencode 的路径前缀。也没有 new-api 的 `advancedcustom` 等价物（`relay/channel` 目录不存在，只有 `relay/adaptor`）。

**5. 非标准陷阱 — ❌（并且反向制造问题）**

- 无 `reasoning_content` 处理。
- **明确破坏 anthropic-beta 透传**：`relay/adaptor/anthropic/adaptor.go:35` 硬写 `req.Header.Set("anthropic-beta", "messages-2023-12-15")`，随后 `:39-41` 在模型名以 `claude-3-5-sonnet` 开头时**覆盖**成 `max-tokens-3-5-sonnet-2024-07-15`。这是 `Set` 不是 `Add`/合并——**客户端发来的 `anthropic-beta` 会被直接丢弃**。这与需求 1 的"原样转发 anthropic-beta"直接冲突。（`anthropic-version` 的透传倒是有：`:30-34`。）
- keep-alive / usage 落位 / Qwen signature：无证据。

**6. 四别名 + 候选与轮换 — ⚠️**

有 group / model / channel 机制（one-api 的原始设计），但无 priority/weight 分层与 `ability` 表的复杂化版本。

**7. Codex catalog — ❌** 无 responses，无 catalog 概念。

**8. 客户端配置写入 — ❌** 零。

**9. 账号池 + 粘性 — ⚠️（只有池，没有粘性）**

one-api 的通道选择是**每请求在 group+model 的合格 channel 集合里随机/加权挑一个**，没有任何 session affinity 概念（`service/channel_select.go` 那套在 new-api 才有）。失败重试会按 retry 次数重新选，**必然**在流中断后换通道——这正是需求 9 要禁止的行为。

**10. 桌面应用 — ❌** 无 Electron，无托盘，无 DPAPI（这是 new-api 后加的）。

**11. 用量与成本 — ✅** 这是 one-api 的原始卖点：token 计费、quota、日志、多用户。但面向"多用户 SaaS 计费"，不是"单机成本看板"。

### 3.3 若以它为核心

要额外写的几乎等于**实现 new-api 相对 one-api 的全部增量**：Anthropic 入站、OpenAI Responses 入站、Gemini 原生入站、advanced custom 路径契约、session affinity、Electron 托盘、SQLite 桌面化。**这不如直接从 new-api 起步**，而 new-api 又有 AGPL 问题——所以 one-api 的 MIT 优势**不足以抵消**它需要你重写的量。

### 3.4 结论

**淘汰。** 它不是"另一个候选"，它是被 fork 出来的上游旧版本。选它 = 选 new-api 但要自己补齐 new-api 的所有增量，只换回一个 MIT 许可。

---

## 4. claude-code-router (ccr) v3.1.3

### 4.1 语言 / 依赖 / 部署形态 / License

**架构**（pnpm workspace，`packages/`）：`core`（网关与配置引擎）、`cli`（`@musistudio/claude-code-router@3.1.3`）、`electron`（桌面壳）、`ui`（React + recharts + dnd-kit 的管理界面）。Node ≥ 20（`@the-next-ai/ai-gateway` 的 `engines`）。依赖含 `better-sqlite3`（**原生模块，Electron 打包需要 rebuild**）、`node-forge`、`pm2`、`undici`。

**License**：`LICENSE` = `MIT License / Copyright (c) 2025 musistudio`。

### 4.2 关键发现：真正的 codec 不在这个仓库里

`packages/core/src/gateway/internal/shared.ts:309`：
```ts
export const gatewayPackageCandidates = ["@the-next-ai/ai-gateway", "gateway"];
```
`packages/core/package.json:19` 与 `packages/cli/package.json:44` 都依赖 `@the-next-ai/ai-gateway": "^1.0.21"`；`packages/core/src/gateway/core-runtime/supervisor.ts:589` 还有针对该包版本行为的兼容注释。

我查了该包的 registry 元数据（`https://registry.npmjs.org/@the-next-ai/ai-gateway/latest`）：

```json
{"name":"@the-next-ai/ai-gateway","version":"1.0.21","license":"MIT",
 "description":"TypeScript Fastify AI protocol gateway for OpenAI, Anthropic, Gemini, MCP and agent workflows.",
 "dependencies":{"ws":"^8.19.0","diff":"^8.0.3","glob":"^13.0.6","openai":"^6.27.0","undici":"^6.28.0","fastify":"^5.8.2"},
 "unpackedSize":4923935,"fileCount":238,"maintainers":[{"name":"musistudio"}]}
```

**含义（对"是否二次封装"这个问题是决定性的）**：

1. ccr 把 **Anthropic↔OpenAI↔Gemini↔MCP 的协议转换主体放在一个独立 npm 包**里（238 文件、约 4.9 MB 解包体积），本仓库只保留配置、生命周期、桌面壳、调度、亲和、日志。
2. 好消息：该包也是 **MIT**，且作者相同（musistudio）——**可以合法依赖**。
3. 坏消息：协议转换的**真实实现细节在这个仓库里读不到**（本次深度只到包边界），所以"ccr 能否满足需求 4/5 的路径契约与怪癖"**必须把 `@the-next-ai/ai-gateway` 也克隆下来单独评估**。本报告据仓库内可见证据只能给出保守判定。

### 4.3 需求 1–11 逐条判定

**1. Anthropic Messages 入站（含 SSE 顺序 / ping / header 透传 / count_tokens）— ⚠️（入口在，细节在外部包）**

仓库内能证明的：
- 有专门的 Anthropic 响应特性模块：`packages/core/src/gateway/features/anthropic-reasoning-response.ts`、`anthropic-response-model.ts`。
- 有上游 header 清洗：`packages/core/src/gateway/core-runtime/upstream-header-sanitizer.ts`。
- 有请求管线与执行器：`packages/core/src/gateway/request/pipeline.ts`、`packages/core/src/gateway/upstream/executor.ts`、`retry-policy.ts`、`packages/core/src/gateway/http/request-handler.ts`。
- 有协议适配（**但只处理 Gemini 的 model-in-path**）：`packages/core/src/routing/protocol-adapter.ts` 全文 53 行，只做 `generateContent` 路径里的 model 改写（`geminiGenerateContentPathPattern`）。
→ SSE 事件顺序、ping 转发、`anthropic-version`/`anthropic-beta` 原样透传、count_tokens **在本仓库内无证据**，标未核实；需评估 `@the-next-ai/ai-gateway`。

**2. OpenAI Chat + Responses（Codex wire_api=responses）— ⚠️**

- Responses 会话亲和有实现：`packages/core/src/gateway/core-runtime/responses-session-affinity.ts`（文件名即证据）。
- Responses 工具严格性处理：`packages/core/src/gateway/core-runtime/responses-tool-strictness.ts`。
- Codex 侧有大量专门代码：`packages/core/src/agents/codex/`（`app-launch.ts`、`app-net-responder.ts`、`cli-middleware-runtime.ts`、`base-instructions.ts`、`media-preview-bridge.ts`、`model-catalog.ts`）、`packages/core/src/gateway/features/codex-patch-bridge.ts`、`codex-multi-agent-bridge.ts`。
- 但 `/v1/responses` 的 inbound 处理同样依赖外部包。
→ 判定：**Codex 集成是它的强项之一**，但缺"响应侧 wire 细节"的自证。

**3. Gemini 原生 / Ollama — ⚠️** 有 Gemini 路径适配与 preset（`providers/presets/gemini/`），Ollama 未核实。

**4. 三种 wire + 每家路径契约 — ⚠️（preset 体系很好，但 DeepSeek preset 只声明了 OpenAI 协议）**

provider preset 体系很完整，共 30+ 家（`packages/core/src/providers/presets/`）：`anthropic`、`deepseek`、`gemini`、`openai`、`opencode-go`、`bailian`、`claudeapi`、`zai-global-coding`、`zhipu-cn-coding`、`kimi-coding`、`minimax`、`moonshot`、`siliconflow`、`openrouter`、`xiaomi`、`y-api`、`nvidia`、`mistral`、`runapi`、`unity2`、`code0`、`fenno`、`infistar-ai`、`qiniu-ai`、`teamorouter` …

preset 的形状（`providers/presets/types.ts` + 各 `index.ts`）是 `{id, name, aliases, endpoints: [{baseUrl, protocols: [...]}], account: {...}}`。以 DeepSeek 为例（`providers/presets/deepseek/index.ts:40-52`）：
```ts
export const deepSeekProviderPreset: ProviderPreset = {
  account: deepSeekProviderAccountConfig,           // 余额查询：/user/balance + JSONPath 映射
  aliases: ["deepseek"],
  endpoints: [{ baseUrl: "https://api.deepseek.com", protocols: ["openai_chat_completions"] }],
  id: "deepseek", name: "DeepSeek", websiteUrl: "https://www.deepseek.com/"
};
```
→ 关键缺口：**DeepSeek preset 只声明了 `openai_chat_completions`，没有 `/anthropic`**。即 ccr 的 DeepSeek 走的是 OpenAI 协议，不是需求 4 要求的 `https://api.deepseek.com/anthropic`。要满足需求 4 得**改 preset 或加一段自定义 base_url + protocol 配置**。
→ 好消息：`account` 里的余额查询（`$.balance_infos[0].total_balance` 等）正是**额度感知轮换**需要的数据源，且是声明式配置，可复制到其它反代上游。

**5. 非标准陷阱 — ❌（本仓库内几乎全无）**

- 全 `packages/core/src` 搜 `reasoning_content` **只有一处且不是回填**：`observability/stream-experience.ts:492`（把 delta 里的 `reasoning_content` 读出来做 UI 展示分片）。→ **DeepSeek+tools 必须回传历史 `reasoning_content` 的逻辑在这里没有。**
- keep-alive 空行/SSE 注释容忍：未找到专门处理（有 `gateway/features/hosted-web-search/sse.ts`，是另一件事）。
- Qwen `signature` 恒空 / `stop_sequence` 恒 null：未找到。
→ 判定：❌。这几条恰恰是本项目最容易踩坑的地方，ccr 不是答案来源。

**6. 固定 4 别名 — ✅（全球最贴，因为 `fable` 别名就是它发明的）**

`packages/core/src/agents/claude-code/environment.ts:3-12`：
```ts
export const CLAUDE_CODE_DEFAULT_FABLE_MODEL_ENV = "ANTHROPIC_DEFAULT_FABLE_MODEL";
export const CLAUDE_CODE_DEFAULT_OPUS_MODEL_ENV = "ANTHROPIC_DEFAULT_OPUS_MODEL";
export const CLAUDE_CODE_DEFAULT_SONNET_MODEL_ENV = "ANTHROPIC_DEFAULT_SONNET_MODEL";
export const CLAUDE_CODE_DEFAULT_HAIKU_MODEL_ENV = "ANTHROPIC_DEFAULT_HAIKU_MODEL";
```
以及 `:16-24` 的 `CLAUDE_CODE_MANAGED_MODEL_ENV_KEYS` 汇总、`:26-33` 的 `ClaudeCodeModelSelection{fableModel, haikuModel, model, opusModel, smallFastModel, sonnetModel}`。**`claude-fable` 正是本需求清单里的四个固定别名之一**——这不是巧合，是本需求清单从 ccr 的实践里长出来的。
→ 需求 6 的"四别名绑定不同上游"在 ccr 里就是四个 env 分别指不同 provider 模型。
→ 候选与轮换：见 `credentials-pool` 与 `routing/model-registry.ts`（⚠️ 见第 9 条，池很薄）。

**7. Codex `model_catalog_json` — ✅（有专门模块）**

`packages/core/src/agents/codex/model-catalog.ts` + `packages/core/src/gateway/model-catalog.ts` + `packages/core/src/providers/model-catalog.ts` + `packages/core/src/models/catalog-file.ts`。这是**五个候选里唯一在仓库内能看到"Codex catalog 文件"专门实现的**（new-api/LiteLLM/AIClient-2-API 都没有对应命名模块）。

**8. 客户端配置写入/备份/幂等/还原 — ✅（除注册表外几乎全中）**

- Claude Code：`packages/core/src/config/config.ts:3674`（`settingsFile: "~/.claude/settings.json"`）、`packages/core/src/config/default-config.ts:137` 与 `:168`（两份 profile 都默认该路径）、`packages/core/src/profiles/launch-core.ts:267`。
- Codex：`config.ts:3893`（`"~/.codex/config.toml"`）、`default-config.ts:146` 与 `:179`、`launch-core.ts:349`。
- **Claude Desktop 3p（关键）**：`packages/core/src/agents/claude-app/vm-storage.ts:131/136/142/151`（`LOCALAPPDATA` → `Claude-3p`），`gateway-service.ts:339-347`（`getClaudeApp3pDataDir()`，Windows 走 `LOCALAPPDATA\Claude-3p`）。写入逻辑 `gateway-service.ts:469-488`：
  - `applyClaudeAppGatewayLibraryConfig(file, gatewayConfig)`（写 `configLibrary` 类 JSON）
  - `applyClaudeAppConfigMeta(metaFile)`（写 `appliedId`）
  - `applyClaudeAppDeploymentMode(rootConfigFile)`（把 `deploymentMode` 写成 **`"3p"`**）
- **备份 / 一键还原 / 幂等**：`gateway-service.ts:22`（`CLAUDE_APP_GATEWAY_BACKUP_FILE = path.join(CONFIGDIR, "claude-app-gateway-backup.json")`）、`:168-169`（apply 前自动 backup）、`:211-221`（`restoreClaudeAppGatewayConfig()` 还原并删除备份）、`:354-366`（`backupClaudeAppGatewayConfig` 快照）、`:393-428`（`readFileSnapshot` / `restoreFileSnapshot` 做**逐文件快照**）、`:438-467`（`restoreClaudeAppOwnedKey` **只还原自己拥有的字段**——这就是幂等重写的正确做法：不覆盖用户改的其它键）。
- ⚠️ 唯一缺口：**注册表策略键**。全仓 `reg.exe` 只出现在 `packages/core/src/proxy/system-proxy.ts:92/926/953/967/981`，操作的是 `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`（系统代理），**不是 Claude Desktop 的策略键**。
→ 需求 8 的注册表部分需另找实现（cc-switch，见 §6）。

**9. 账号池 + 冷却 + 粘性 — ⚠️/❌（最弱的一环）**

- `packages/core/src/providers/credential-pool.ts` **只有 112 行**，导出三个函数：`providerCredentialLimitState`（`:19`）、`recordProviderCredentialOutcome`（`:39`）、`readProviderCredentialCooldown`（`:67`）。→ 有"记结果 + 读冷却"，**没有**配额感知轮换、没有粘性绑定、没有已吐字保护。
- 有账户服务：`packages/core/src/providers/account-service.ts`、`account-webcontent.ts`、`oauth-plugin.ts`、`probe.ts`、`runtime-topology.ts`、`manifest-service.ts`。
- 有 Responses 会话亲和：`gateway/core-runtime/responses-session-affinity.ts`（仅 Responses 路径）。
- → 判定：**多凭据/冷却：⚠️ 部分；粘性会话（已吐字不许切）：❌ 无证据。**

**10. 桌面应用 + 托盘 + SQLite + DPAPI + JSON 导入导出 — ✅/⚠️**

- Electron：`packages/electron` + 根 `electron-builder.json`（打包配置现成）。托盘能力在 electron 包内（未逐行核实）。
- SQLite：`better-sqlite3` 在 `packages/core/package.json` 与 `packages/electron/package.json` 的 dependencies 里。
- 观测/日志/回放：**很强**——`packages/core/src/observability/`（`request-log-store.ts`、`request-log-runtime.ts`、`request-log-body.ts`、`request-log-limits.ts`、`request-log-model.ts`、`raw-trace-sync.ts`、`request-log-worker.ts`），配 `gateway/context-archive/`（`protocol.ts`、`store.ts`）+ `features/context-archive-continuation.ts`。`packages/cli/package.json` 依赖里还出现 `@the-next-ai/bot-gateway-sdk`。
- 成本：`packages/core/src/models/pricing-service.ts`。
- DPAPI：**❌ 无证据**（Node 侧加解密通常走 `node-forge`/safeStorage；仓库内未见 Windows DPAPI 调用）。
- JSON 导出/导入：配置本身是 JSON（`config/config-repository.ts`），⚠️ 但非终端用户一键导入导出。

**11. 用量与成本 + 日志 + 调试回放 — ✅（最强）**

见上：`observability/` 一整套 request-log + raw-trace 同步，`context-archive` 做上下文归档与续接，`models/pricing-service.ts` 做成本。**调试回放是它的招牌**（raw trace 落盘 + `rawTraceSyncPath = "/__ccr/raw-trace-sync"`，`gateway/internal/shared.ts:302`）。

### 4.4 最值得移植的部分（MIT，可合法搬运）

| 要搬运的东西 | 路径 | 价值 |
|---|---|---|
| **Claude Desktop 3p 配置写入 + 逐文件快照备份/还原/幂等** | `packages/core/src/agents/claude-app/gateway-service.ts:22,168-169,211-221,354-428,438-488`、`vm-storage.ts:131-151` | 需求 8 的直接答案；`:438-467` 的"只还自己拥有的键"是幂等的正确姿势 |
| **Codex model catalog 生成** | `packages/core/src/agents/codex/model-catalog.ts`、`packages/core/src/models/catalog-file.ts` | 需求 7 的直接答案 |
| **四别名 env 映射** | `packages/core/src/agents/claude-code/environment.ts:3-33` | 需求 6，且与客户端真实 env 变量名对齐 |
| **Provider preset 声明式结构（含余额查询 JSONPath）** | `packages/core/src/providers/presets/types.ts` + `deepseek/index.ts` + 30 家 preset | 需求 4/9 的"声明式供应商配置 + 额度数据源"骨架 |
| **Codex/Claude CLI header 透传** | 参见 new-api 的同一清单（更完整、带一手 commit 引用） | 需求 9 的粘性键来源 |
| **raw trace / request log 分层** | `packages/core/src/observability/*` | 需求 11 的调试回放 |

⚠️ 移植成本提示：core 的体积与耦合不低（`config/config.ts` 已是数千行级，见 `:3674` 行号），**整块搬会拖入 pm2 / better-sqlite3 原生模块 / bot-gateway 等你不想要的东西**。建议**只抄具体函数级实现与数据形状，不抄模块**。

### 4.5 账号池评级

**弱**：`credential-pool.ts` 112 行，只有 limit-state / outcome / cooldown 三件事。真正的"多账号按额度轮换 + 失败冷却 + 粘性"要么在 `account-service.ts`（未深读），要么在外部 `ai-gateway` 包。**不要指望从 ccr 拿到需求 9 的完整实现。**

---

## 5. AIClient-2-API v3.5.8

### 5.1 语言 / 依赖 / 部署形态 / License

**语言栈**：Node.js ESM（`package.json` `"type": "module"` 风格，源码全 `.js`）。依赖（`package.json`）：`ai@^6`、`@ai-sdk/openai`、`openai@^6.36`、`@anthropic-ai/tokenizer@^0.0.4`、`@github/copilot-sdk@1.0.16`、`axios`、`undici`、`ws`、`google-auth-library`、`http-proxy-agent`/`https-proxy-agent`/`socks-proxy-agent`、`busboy`/`multer`/`adm-zip`。**另有 Go 写的 TLS sidecar**：`tls-sidecar/main.go` + `tls-sidecar/go.mod`。

**部署形态**：`node src/api-server.js`（`src/services/api-server.js`，纯 `http.createServer`，`:310`），自带静态 web UI（`src/ui-modules/` 12 个模块 + `static/`）。有 `install-and-run.ps1` / `.bat` / `.sh` 与 Dockerfile。

**定位（必须先说清楚，否则会误判）**：这不是通用协议网关，而是**"订阅/OAuth 客户端 → OpenAI/Claude 兼容端点"的适配器集合**。它把 Gemini CLI / Qwen Code / Kiro / Codex / Grok / GitHub Copilot 这些**用订阅凭据（OAuth token、cookie）而非 API key** 的官方客户端，翻转成标准 API。它的护城河是**凭据池与 OAuth 生命周期**，不是路径契约。

**License 与再分发限制——重点**：

`LICENSE` 是 **GPL-3.0**（非 AGPL，但这更糟——**GPL-3.0 的"分发即传染"比 AGPL 更直接地覆盖桌面软件分发**）。README 的法律章节原文：
> This project operates under the [GNU General Public License v3 (GPLv3)](https://www.gnu.org/licenses/gpl-3.0).
> ### Usage Risk Warning
> This project (AIClient-2-API) is for learning and research purposes only. Users assume all risks when using this project.
> ### Third-Party Service Responsibility Statement
> This project is an API proxy tool and does not provide any AI model services. … The author is not responsible for the availability, quality, security, or legality of third-party services.

要点：
1. **GPL-3.0 + 无 CLA + 无商业双许可**（仓库内无 `COMMERCIAL`/`CLA` 文件）。把它的代码链接进你的闭源桌面应用并分发，你需要整体按 GPL-3.0 开源。
2. TLS sidecar 是**独立 Go 进程**。理论上"独立进程 + 进程边界"能弱化传染范围（若只通过命令行/网络交互，可能被视作 separate program），但**这个判断需要法务**，且你必须完全不链接、不复制其代码进主程序。
3. README 自己写的"for learning and research purposes only"是**额外的使用范围声明**，即便你愿意开源，也要考虑这条。
4. 另外它明确"inspired by / incorporated some code from Cline 3.18.0's `gemini-cli.ts`"——**上游代码来源本身的许可状态需要独立核查**（Cline 是 Apache-2.0，但它是否保留声明/是否合规，是本项目自己的事，你引入就承接了风险）。

**结论**：**GPL-3.0 基本上把它从"作为核心"的选项里排除**（除非你接受整机开源）。它的价值退化为**思路参考 + OAuth/凭据池设计借鉴**（而且借鉴时要重写，不能复制）。

### 5.2 需求 1–11 逐条判定

**1. Anthropic Messages 入站 — ✅（含 count_tokens，是本组除 LiteLLM 外唯一真实现的）**

- `/v1/messages` 路由在：`src/services/api-manager.js:72-75`（`path === '/v1/messages'` → `ENDPOINT_TYPE.CLAUDE_MESSAGE`）。
- count_tokens **本地实现且启用**：`src/handlers/request-handler.js:227-249`，`if (path.includes('/count_tokens') && method === 'POST')` → `countTokensAnthropic(body)`，异常时降级返回 `{input_tokens: 0}`（`:238-243`）。注意它对**任何**含 `count_tokens` 的 POST 路径生效（宽松匹配）。
- 转换器分层清晰：`src/converters/ConverterFactory.js` + `src/converters/strategies/{ClaudeConverter,OpenAIConverter,CodexConverter,GeminiConverter,GrokConverter,OpenAIResponsesConverter}.js` + `register-converters.js:22` 注册 CLAUDE。`convert/convert.js:101-227` 是各向转换的调用面。
- ⚠️ 未核实：SSE 事件顺序是否严格到 `message_delta` → `message_stop`；ping 是否转发；`anthropic-beta` 是否原样透传（只在 `src/providers/claude/claude-core.js:40` 见到硬写 `'anthropic-version': '2023-06-01'`）。

**2. OpenAI Chat + Responses（Codex）— ✅**

- 路由：`api-manager.js:59-61`（`/v1/chat/completions`）、`:63-65`（`/v1/responses` → `ENDPOINT_TYPE.OPENAI_RESPONSES`）。
- 专门的 Responses 实现族：`src/providers/openai/openai-responses-core.js` **和** `openai-responses-core.mjs`、`openai-responses-strategy.js`、`codex-core.js`、`codex-request-utils.js`、`codex-responses-strategy.js`；转换侧 `src/converters/strategies/OpenAIResponsesConverter.js`。`convert/convert.js` 里 `toOpenAIResponsesFromClaude` / `toOpenAIResponsesStreamChunkFromClaude` / `toOpenAIStreamChunkFromClaude` 都有（`convert-old.js:213-227`），说明**四向转换都有**。
- README 明确 Codex 场景走 `openaiResponses-custom` provider。
- ⚠️ 未核实：`response.completed` 终结 / `previous_response_id` 回放 / `function_call`+`function_call_output` 配对的完整度。

**3. Gemini 原生 — ✅**

`api-manager.js:67-71` 用正则接 `/v1beta/models/{model}:generateContent|streamGenerateContent`；`:41-44` 接 `GET /v1beta/models`；provider `gemini-cli-oauth` / `gemini-antigravity`（`src/providers/gemini/`）。Ollama 入站未核实。

**4. 三种 wire + 每家路径契约 — ❌（这是它的硬伤）**

- 能做的：自定义上游——`--claude-base-url`（README:284-285）、`--openai-base-url`（:277-278）、`--model-provider claude-custom|openai-custom|openaiResponses-custom`（:205-211）。**这就是"换 base_url 满足 DeepSeek `/anthropic` 与 Qwen 不带 `/v1`"的量级。**
- **不能做的**：`src/providers/claude/claude-core.js` 硬编码官方 Anthropic 路径（`anthropic-version: '2023-06-01'` 附近），**没有 `IncomingPath → UpstreamPath` 映射表**（对比 new-api 的 `advancedcustom`），也没有 opencode 那种 family 前缀机制。→ commandcode `/provider/v1/messages`、opencode `/openai` `/anthropic` **无法表达**。
- 它的 provider 切换机制是**路径首段即 provider 名**（`request-handler.js:184-206`：`firstSegment = pathSegments[0]`，若是注册 provider 就改写 `currentConfig.MODEL_PROVIDER` 并剥掉该段），或 `model-provider` header（`:172-182`）。这是**provider 选择**，不是**上游路径前缀**——两者不同。

**5. 非标准陷阱 — ❌/⚠️**

- DeepSeek 只作为**模型名**出现（`src/providers/provider-models.js:130-133`：`deepseek-v3.2`/`deepseek-r1`/`deepseek-v3`；`src/providers/openai/iflow-core.js:45` 的 `THINKING_MODEL_PREFIXES` 含 `deepseek-r1`）——是"哪些模型会 thinking"的判断，**不是**"历史 assistant 消息必须回传 `reasoning_content`"的回填。
- keep-alive 空行 / SSE 注释容忍、Qwen signature 恒空 / stop_sequence 恒 null：未找到。
- → 判定 ❌（与 ccr 同样缺这一层）。

**6. 四别名 + 轮换 — ⚠️**

有 `configs/custom_models.json.example`（自定义模型表）与 `src/ui-modules/custom-models-api.js`，provider 内有 `getCustomModelAliasesForProvider` / `getCustomModelIdsForProvider`（`src/providers/provider-pool-manager.js:17-45`）。→ 模型别名机制有，但**没有"四个固定别名"这种客户端语义**，也没有把别名绑到不同上游的概念。

**7. Codex `model_catalog_json` — ❌**

未见 catalog 文件生成。它的 Codex 支持是**协议层**（把 Codex OAuth 后端翻成 `/v1/responses`），不是**客户端 catalog 替换**。需求 7 要求的 `model_catalog_json` 是 Codex CLI 的 `config.toml` 字段——这块它不做（对比 ccr 的 `agents/codex/model-catalog.ts`）。

**8. 客户端配置写入 — ❌**

全仓搜 `.claude/settings.json` / `.codex/config.toml` / `Claude-3p` 无命中。它只写自己的 `configs/*.json`（`src/core/config-manager.js`、`configs/config.json.example`、`configs/provider_pools.json.example`）。→ 需求 8 完全自研。

**9. 账号池 + 冷却 + 粘性 — ⚠️（池很强，粘性没有）**

这是它**最强的部分**，值得认真读：

- `src/providers/provider-pool-manager.js` **2,480 行**，`export class ProviderPoolManager`（`:47`）。凭据节点结构（从 `configs/provider_pools.json.example` 可见）：
  ```json
  { "customName": "AtlasCloud节点1", "OPENAI_API_KEY": "sk-...", "OPENAI_BASE_URL": "https://api.openai.com/v1",
    "checkModelName": null, "checkHealth": false, "notSupportedModels": [],
    "uuid": "...", "isHealthy": true, "isDisabled": false,
    "lastUsed": null, "usageCount": 0, "errorCount": 0, "lastErrorTime": null }
  ```
  即：**每个节点自带健康位、错误计数、最后错误时间、使用计数、最后使用时间、不支持模型黑名单**——需求 9 要的"多凭据 + 失败冷却 + 额度感知"的数据模型就在这里，且是**声明式 JSON**。
- 能力证据：
  - 令牌过期检查与自动刷新：`:194`（"Failed to check expiry"）、`:368`、`:384-405`（队列推进）、`:536-548`（刷新失败 5 次后 `markProviderUnhealthyImmediately`）。
  - 错误阈值：`:1749`、`:1787`（`provider.config.errorCount = this.maxErrorCount` 标记确定性失败）；`MAX_ERROR_COUNT` 在 config 里可配（`config.json.example`）。
  - 健康检查：`:2125-2137`（单点）、`:2194-2230`（`[ScheduledHealthCheck]` 批量，统计 `successCount`/`failCount`/总耗时）、`:2387-2388`（全失败时返回 `All health check attempts failed`）。
  - 额度/配额：`RATE_LIMIT_COOLDOWN_ENABLED` / `RATE_LIMIT_COOLDOWN_MS` / `..._JITTER_MS` / `..._MAX_MS`（`config.json.example`）→ **带抖动的指数退避冷却**；另有 `src/utils/rate-tracker.js`。
  - 降级链：`providerFallbackChain`（按 provider 类型）与 `modelFallbackMapping`（按模型名跨 provider 映射到 targetProvider+targetModel）——**这正是需求 6 的"别名 + 候选 + 轮换"的现成形状**。
  - 健康对外暴露：`GET /provider_health`，带 `unhealthRatioThreshold` 与 `summaryHealth`（`request-handler.js:140-167`）。
  - 代理解析：`PROXY_URL` + `PROXY_ENABLED_PROVIDERS`（按 provider 走代理，反代场景常见需求）。
- **粘性（已吐字不许切）：❌ 无证据。** 没有 session→credential 绑定，没有"流已开始"状态机。`lastUsed`/`usageCount` 是轮换依据，不是亲和要求。
- ⚠️ 另一个反向信号：`isHealthy`/`isDisabled`/`errorCount` 这套状态**直接落在声明式 pools JSON 里**（`provider-pool-manager.js:2475` 有 `Failed to write provider_pools.json`）——状态与配置混写同一文件，对桌面应用的并发/一致性不友好。

**10. 桌面应用 + 托盘 + SQLite + DPAPI + JSON 导入导出 — ⚠️/❌**

- 桌面壳 + 托盘：**❌ 没有**（纯 Node 服务 + 浏览器 UI + 4 个平台启动脚本）。
- SQLite：**❌ 没有**（配置是 JSON 文件，日志是文件，`LOG_DIR`/`LOG_MAX_FILES` 滚动）。
- DPAPI：**❌ 没有**（凭据以明文 key 存在 `provider_pools.json` 里——示例文件里就是明文 `sk-...`）。
- JSON 导入导出：**✅ 事实上就是**（`configs/*.json` 即配置，# `src/ui-modules/upload-config-api.js` / `config-scanner.js` 支持上传与扫描）。
- TLS sidecar（`tls-sidecar/main.go`）：用于 TLS 指纹伪装类需求（`TLS_SIDECAR_ENABLED`/`TLS_SIDECAR_PORT` 可配，`src/utils/tls-sidecar.js` 调用）。**在 Windows 本机场景意义有限**，但如果你要访问反代上游的 Cloudflare 防护端点会有用。它也是**独立的 Go 组件**，是你唯一可能"干净复用"的部分（但 GPL 传染性判断仍需法务）。

**11. 用量与成本 + 日志 + 调试回放 — ⚠️/✅**

- 有 `src/plugins/model-usage-stats/`（`stats-manager.js` + `api-routes.js`）、`src/services/usage-service.js`、`src/ui-modules/{usage-api,usage-cache}.js`。
- 日志：`src/utils/logger.js`，`LOG_ENABLED/LOG_LEVEL/LOG_OUTPUT_MODE/LOG_DIR/LOG_MAX_FILE_SIZE/LOG_MAX_FILES`。
- Prompt 日志：`PROMPT_LOG_FILENAME` / `PROMPT_LOG_MODE`（`console|file|none`）+ `--log-prompts`（README:325-328）；`src/example/{claude,gemini,openai,openaiResponses}/` 里有**真实的请求/响应/流式分片样本 JSON**（`originalRequestBody*.json` / `oldResponseChunk*.json` / `responseChunk*.json`）——这些**样本对写转换层的回归测试极有价值**（可作为 fixture 参考，注意不要复制 GPL 代码本体）。
- ⚠️ 成本统计（单价 × token）弱于 LiteLLM/new-api：有使用量统计，未见定价表。

### 5.3 若以它为核心

**不建议**（GPL-3.0）。若忽略许可，工程上要额外写：

1. 桌面壳 + 托盘 + 单实例 + 开机自启（全无）。
2. SQLite 迁移（现在全是 JSON 文件，且状态与配置混写）。
3. DPAPI 加密（现在明文存 key）。
4. 路径契约层（`IncomingPath → UpstreamPath`，参照 new-api `advancedcustom`）——它没有。
5. DeepSeek/keep-alive/Qwen 那一层怪癖处理——它没有。
6. 客户端配置写入引擎——它没有。
7. Node 运行时打包（node_modules 含 `@github/copilot-sdk`、`google-auth-library`、`undici`、`axios`、`adm-zip`、`multer`、`busboy`；体积中等偏大），Electron 打包或 pkg/SEA。
8. 删掉大量 SaaS 面：`src/plugins/api-potluck/*`（多个 key 分发/用户数据管理）、`default-auth`、OAuth 体系、市场 `configs/market.json`、更新检查 `update-api.js`。

**打包分发成本：中，但被许可成本淹没。**

### 5.4 账号池评级

**强（数据模型 + 健康检查 + 退避冷却 + 降级链），但缺粘性、且明文落盘。**
真正值得学的是它的**节点状态字段设计**（`isHealthy` / `isDisabled` / `errorCount` / `lastErrorTime` / `lastUsed` / `usageCount` / `notSupportedModels`）+ **调度退避参数**（`RATE_LIMIT_COOLDOWN_MS/JITTER/MAX`）+ **`providerFallbackChain` / `modelFallbackMapping` 的跨 provider 降级图**。这三样是需求 9 的骨架，而 new-api 的 affinity 和 LiteLLM 的 cooldown 也各自给了另一半。

---

## 6. 横向矩阵与关键结论

### 6.1 需求 1–11 汇总（✅ / ⚠️ / ❌）

| # | 需求 | LiteLLM | new-api | one-api | claude-code-router | AIClient-2-API |
|---|---|---|---|---|---|---|
| 1 | Anthropic 入站（SSE 顺序/ping/header 透传/count_tokens） | ✅ | ⚠️ count_tokens 被注释掉 | ❌ 无 /v1/messages | ⚠️ 入口在，细节在外部 npm 包 | ✅（count_tokens 本地实现） |
| 2 | OpenAI Chat + Responses（Codex 语义） | ✅ | ⚠️ responses 走插件路由，未实机核实 | ⚠️ 仅 Chat | ⚠️ 有 Codex 专项模块 | ✅ |
| 3 | Gemini 原生 / Ollama 入站 | ✅ | ✅ | ⚠️ | ⚠️ Gemini 有 | ✅ Gemini |
| 4 | 三种 wire + 每家路径契约 | ✅ native 直通抽象；⚠️ commandcode/opencode 需加一小段 | ✅ `advancedcustom` 路径映射表 | ❌ 硬编码 `/v1/messages` | ⚠️ preset 好，但 DeepSeek preset 只声明 OpenAI 协议 | ❌ 只能换 base_url |
| 5 | DeepSeek reasoning_content / usage 落位 / keep-alive / Qwen 怪癖 | ✅（reasoning 回填 + keepalive 有代码；Qwen signature ⚠️） | ❌ 未见 reasoning 回填 | ❌ 还硬覆盖 anthropic-beta | ❌ 几乎全无 | ❌ 几乎全无 |
| 6 | 四别名 + 候选轮换 | ✅ | ✅ | ⚠️ | ✅（四别名 env 即其发明） | ⚠️ |
| 7 | Codex model_catalog_json | ⚠️ 网关侧齐、客户端侧无 | ⚠️ 且声明不受理 | ❌ | ✅ 有专门模块 | ❌ |
| 8 | 客户端配置写入/备份/幂等/还原 | ❌ | ❌ | ❌ | ✅ 除注册表外全中 | ❌ |
| 9 | 账号池 + 冷却 + 粘性（已吐字不许切） | ✅ 冷却最强 + mid-stream 续写 + session 亲和 | ✅ affinity `off/prefer/strict`，为 CLI 场景长出来 | ⚠️ 有池无粘性 | ❌ 池 112 行，无粘性 | ⚠️ 池最强，无粘性 |
| 10 | 桌面 + 托盘 + SQLite + DPAPI + 导出导入 | ⚠️ 有 SQLite，无桌面/DPAPI | ⚠️ 有 Electron 托盘 + SQLite，无 DPAPI | ❌ | ✅ 有 Electron + SQLite；DPAPI ❌ | ❌ |
| 11 | 用量成本 + 日志 + 调试回放 | ✅ | ✅ | ✅ | ✅ raw trace 最强 | ⚠️ 有统计，成本弱 |

### 6.2 许可风险排序（对"闭源商用桌面分发"）

| 候选 | 许可 | 能否闭源分发 | 硬约束 |
|---|---|---|---|
| one-api | MIT | ✅ 能 | 无（但能力太弱） |
| claude-code-router | MIT | ✅ 能 | 真正的 codec 在另一个 **MIT** npm 包 `@the-next-ai/ai-gateway`，可合法依赖 |
| LiteLLM | MIT + `enterprise/` 商业 | ⚠️ 有条件 | 必须剔除 `enterprise/` 与 PyPI `litellm-enterprise`（proprietary，`LicenseRef-Proprietary`）；MIT 部分无义务 |
| new-api | **AGPL-3.0 + §7 附加条款** | ❌ 基本不能 | 必须在 UI 保留"Frontend design and development by New API contributors."+ 原项目可见链接；`go-epay` 是 maintainer proprietary 生产依赖 |
| AIClient-2-API | **GPL-3.0** | ❌ 基本不能 | 无 CLA、无商业双许可；README 另称"for learning and research only"；且自述参考了 Cline 代码 |

**两个"能闭源"的（one-api / ccr）恰好能力互补**：one-api 许可干净但能力是 new-api 的子集（无 Anthropic/Responses 入站、无粘性）；ccr 许可干净、客户端配置引擎与 Codex catalog 是一手答案，但**协议转换主体不在仓库内**（在同样 MIT 的 `@the-next-ai/ai-gateway`）。

### 6.3 三个不可回避的事实

1. **没有任何一个候选同时满足需求 1–11。** 最好的组合也要写东西：LiteLLM 缺整块需求 8 + 桌面/DPAPI；new-api 缺需求 8 + DPAPI + reasoning 回填 + 被 AGPL 卡住；ccr 缺需求 5/9 的实质实现。
2. **需求 8（客户端配置写入引擎）是唯一一个"有现成正确答案、且许可证干净"的模块**——它正好落在 ccr（MIT，`agents/claude-app/gateway-service.ts` + `agents/claude-code/environment.ts` + `agents/codex/model-catalog.ts`）和 cc-switch（MIT，见 `similar-gateways.md` 与 §7 备注）手里。**这是最有价值的移植点。**
3. **需求 5（上游怪癖）是唯一一个"有现成正确答案、但在一个许可有污染的候选里"的模块**——正确答案在 LiteLLM（`llms/deepseek/chat/transformation.py:64-102`），而 LiteLLM 的 MIT 部分**可以**用。→ 这一条的结论是"照它的**行为规格**重写，必要时按 MIT 条件复制那一小段逻辑并保留版权声明"。

---

## 7. 最终结论：混合（自研转换层 + 移植配置写入引擎 + 借鉴怪癖处理）

### 7.1 明确结论

**纯自研 / 基于 X 二次封装 / 混合 → 选【混合】，具体是：**

> **自研一个薄的 TypeScript 转换层（inbound 四协议 × outbound 三 wire）+ 移植 claude-code-router 的客户端配置写入引擎（MIT）+ 按 LiteLLM 的行为规格重写上游怪癖处理 + 参照 new-api 的 affinity 语义自研账号池。**

**不选"基于 LiteLLM 二次封装"，也不选"基于 new-api 二次封装"。**

### 7.2 理由

**为什么不基于 LiteLLM 二次封装**
它是**能力最强**的候选（需求 1/2/4/5/6/9/11 都✅或接近✅），但它是"团队级 LLM 网关"，与"Windows 单机桌面应用"的形态**根本错配**：
- 分发形态错配：Python + 39 项 proxy 依赖 + Prisma + 二进制轮子（tiktoken/tokenizers/cryptography/polars）→ 安装包 300–600 MB，杀软误报，启动慢。桌面应用的用户不会接受。
- 架构错配：它的多实例一致性设计（Redis 缓存、按 worker 的 `allowed_fails` 回退计数，`cooldown_handlers.py:682-708`）在单机单用户场景是纯开销。
- 许可边界需要清理：剔除 `enterprise/` + `litellm-enterprise`（proprietary）——可做，但它是**持续维护负担**（每次升级都要核对目录边界）。
- 需求 8/10 完全缺失，且这两块（客户端配置注入 + 托盘/DPAPI）**恰恰是桌面产品体验的主体**，等于核心能力要另写，却先背上了 210 MB 的依赖债。

**为什么不基于 new-api 二次封装**
工程上它是**最合适的形态**（单 Go 二进制 + 内嵌前端 + 已有 Electron 托盘 + SQLite + `advancedcustom` 路径契约 + 为 CLI 场景而生的 session affinity）。它的失分**只在许可**：
- AGPL-3.0 ⇒ 桌面分发即触发源码提供义务 ⇒ 无法闭源。
- §7 附加条款 ⇒ 你的 UI 必须保留项目方署名文案与可见链接（产品品牌受限于此）。
- `go-epay` proprietary ⇒ 需要额外清理。
如果本项目**本来就是开源**或**只自用不分发**，那么"基于 new-api + 放回 count_tokens + 加 reasoning 回填 + 加 DPAPI + 抄 ccr 的客户端配置引擎"会是最省力路径。**但需求里有"配置导出/导入"、桌面安装器、以及"商业/再分发限制"被明确要求评估，说明分发是既定前提。** 因此结论仍是不基于它。

**为什么不基于 ccr 二次封装**
它的**客户端半边**（配置写入、备份/还原/幂等、Codex catalog、四别名 env、Claude Desktop 3p）是五个候选里最好且 MIT 干净；但它的**网关半边**是空壳——`gatewayPackageCandidates = ["@the-next-ai/ai-gateway", "gateway"]`（`gateway/internal/shared.ts:309`）意味着 codec 是一个外部包。基于它封装 = 依赖一个**你无法逐步审计其转换正确性**的黑盒 + 继承它的插件/agent/MCP/media 全家桶（含 `pm2`、`better-sqlite3` 原生模块、bot-gateway SDK）。对一个"需求清单里有 20+ 条精确协议约束"的项目，这是错的选择。

**为什么 AIClient-2-API 直接出局**
GPL-3.0 + 无 CLA + 无商业许可 + README "learning and research only"。工程上它的 pool 很好，但**许可一票否决**。（如果它是 Apache-2.0/MIT，它会是很强的候选——它的 `/v1/messages` + `/v1/responses` + `/v1beta/models` 入站齐全，count_tokens 也是真实现。）

### 7.3 最小可行路径（MVP）

按"最小完整改动"排序，先做能跑通 Claude Code + Codex 的骨架：

**阶段 0：先验证再动手（1–2 天）**
- 用 `providers.md` 的路径契约 + `client-integration.md` 的客户端要求，写一个**只做 pass-through 的 30 行反向代理**，分别在 DeepSeek `/anthropic`、Qwen(无 /v1)、commandcode `/provider/v1/messages`、opencode `/anthropic` 上打一发 `POST /v1/messages` 流式请求，**把原始 SSE 帧抓下来**（用 AIClient-2-API 的 `src/example/*/responseChunk*.json` 作样本积形状参照）。**先证明你能看见 `message_delta`/`message_stop`/ping 的真实形态**，再谈映射。这一步能省掉后面 80% 的返工。

**阶段 1：转换层（MVP，只做必需格）**
- inbound：`POST /v1/messages`（Anthropic，含 SSE 顺序 + ping 转发 + `anthropic-version`/`anthropic-beta` 强制透传）、`POST /v1/chat/completions`、`POST /v1/responses`。**先不做 Gemini/Ollama。**
- outbound：**不做通用三 wire。** 只做两种：(a) Anthropic-native（直通，`base_url` + `path_template` 可配）；(b) OpenAI-chat（转换）。这两种覆盖 DeepSeek/Qwen/commandcode/opencode/Anthropic 官方。**Responses outbound 推迟。**
- 供应商模型就用一张声明式表，字段仿 ccr preset + new-api advancedcustom 的并集：
  ```
  { id, baseUrl, protocol: "anthropic" | "openai-chat",
    pathTemplate: "/v1/messages" | "/provider/v1/messages" | "/anthropic/v1/messages",
    headerPassthrough: ["anthropic-version","anthropic-beta"],
    quirks: { echoReasoningContent: true, usageOnLastChunk: true, stripCustomToolType: true } }
  ```
  `pathTemplate` 是需求 4 的唯一答案——**别把它做成一堆 if-else 的特例**。
- count_tokens：本地估算即可（`@anthropic-ai/tokenizer` 或简化计数），**不要**依赖上游（Qwen 那个端点没有 `/v1/models` 的现实说明上游能力不可假定）。

**阶段 2：怪癖层（按 LiteLLM 的行为规格重写）**
- `echoReasoningContent`：手工维护「assistant 消息 → 其原始 reasoning_content」的映射（**不能**像 LiteLLM 那样在缺失时注入单空格——LiteLLM 自己都 warning 这会静默降低多轮质量，见 `llms/deepseek/chat/transformation.py:88-98`；你要做的是**在收到响应时就把 reasoning 存下来**）。
- `usageOnLastChunk`：解析时维护"最后一块"的 usage 归属。
- keep-alive / SSE 注释：解析器必须**跳过** `:` 开头行与空 data 帧，不报错、不结束流。
- Qwen `signature` 空 / `stop_sequence` null：在出站响应里补默认值，避免客户端 schema 校验炸。

**阶段 3：账号池 + 粘性（自研，语义抄 new-api）**
- 数据模型抄 AIClient-2-API 的节点字段（`isHealthy`/`isDisabled`/`errorCount`/`lastErrorTime`/`lastUsed`/`usageCount`/`notSupportedModels`）+ 冷却参数（`RATE_LIMIT_COOLDOWN_MS` / `JITTER` / `MAX`）。**但状态不要写回配置文件**——进 SQLite。
- 选择语义抄 new-api 的三级：`pinned → affinity(仅首轮) → weighted-random`，并实现 `http_error`/`quota_exhausted`/`rate_limited` 的可配置允许失败次数。
- **粘性 = 两个机制，缺一不可**：(a) session→credential 绑定（TTL + `strict` 模式下绑定不可用直接返错，别悄悄换）；(b) **流已开始后禁止切换**的状态机——一旦有内容字节写给客户端，该请求的 credential 就锁死，失败只能以错误事件结束，**不能重试到另一个账号**（`function_call` 中途换号会让客户端拿到自相矛盾的 ID 序列）。

**阶段 4：客户端配置引擎（移植 ccr，MIT）**
直接移植/改写以下（保留 MIT 版权声明）：
- `agents/claude-app/gateway-service.ts`：`backup → apply → restore` 三段 + 逐文件快照 + "只还自己拥有的键"（幂等正确姿势）。
- `agents/claude-code/environment.ts:3-33`：四别名 env（含 `claude-fable`）。注意 Windows 上写 `settings.json` 也要写 `env` 块（`ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN`）。
- `agents/codex/model-catalog.ts` + `models/catalog-file.ts`：`model_catalog_json` 生成。
- 注册表策略键部分 ccr 没有 → 用已有的 **cc-switch**（MIT，本仓库 `vendor/cc-switch`，见 `similar-gateways.md`）作参考实现。

**阶段 5：桌面壳 + 平台能力**
- Tauri 或 Electron 都行；**倾向 Tauri**（体积小、无 Node 运行时），但前提是阶段 1–3 用 Rust 或你接受 Tauri 调 sidecar。若阶段 1–3 已经用 TypeScript 写完，Electron 复用成本更低（ccr 的 `electron-builder.json` 可参考）。
- SQLite 存配置/日志/池状态；日志表加 TTL 与体积上限（避免单机无限增长）。
- DPAPI：Node 侧可用 `win-dpapi` 类方案或 Rust `CryptProtectData`；**密钥永不落明文**（AIClient-2-API 明文存 `provider_pools.json` 是反面教材）。
- 导出/导入 JSON：**导出时排除密钥或对密钥单独加密**，否则"导出配置"会变成密钥泄露通道。

**阶段 6：可观测性**
- 请求日志（含 SSE 原始帧可选留存 + 体积上限）、用量/成本表、调试回放（可参照 ccr 的 `observability/` 分层，但别搬它的模块）。

### 7.4 明确不做（写在纸面上，避免范围蔓延）

- 不做多用户/计费/订阅/支付/第三方登录/OAuth 服务端（本机单用户）。
- 不做 30+ provider preset（只做需求里点名的：DeepSeek、Qwen、commandcode、opencode、Anthropic 官方，其余按需加表行）。
- 不做 `/v1/responses` 的 outbound（Codex 只需要 inbound responses + 上游 chat/anthropic；Responses outbound 是 LiteLLM 那种团队网关的负担）。
- 不做 Responses WebSocket（Codex 用 HTTP 增量 SSE 即可；new-api 的 WS 是另一个语义）。
- 不做插件系统 / MCP 代理 / 媒体工具（ccr 的 `mcp/`、`media/` 就是"别做"的示范）。

### 7.5 风险清单

| 风险 | 说明 | 缓解 |
|---|---|---|
| **协议细节靠推测** | 本报告未能实机验证四个上游的真实 SSE 帧形态（阶段 0 的存在理由） | 阶段 0 先抓真实流量；把 frame 变成 fixture 进回归测试 |
| **移植 MIT 代码的合规** | ccr（MIT）与 cc-switch（MIT）可复制，但**必须保留版权与许可声明**；LiteLLM 只能搬 MIT 部分（**绝不能**搬 `enterprise/`） | 建 `THIRD_PARTY_NOTICES`，逐文件记来源 commit；LiteLLM 那段函数保留原版权头 |
| **AGPL/GPL 诱惑** | new-api 的 affinity、AIClient-2-API 的 pool 都很香；**读它的设计、重写代码**，不要复制 | 从 new-api/AIClient-2-API 只取"行为规格"（字段名语义、状态机步骤），代码自己写；不要把它们作为依赖 |
| **上游路径契约变化** | 四家的路径契约是"当前事实"，会变（`providers.md` 已注明部分条目的确认方式） | `pathTemplate` 全部走配置 + 每条加一个"契约探针"测试；不要在代码里硬编码 |
| **DeepSeek reasoning 回填的隐性质量损失** | 若你像 LiteLLM 一样在缺失时注入空占位，多轮质量会静默下降 | 强制在**响应侧**保存 reasoning 并在下一轮回填；缺失时记 warning 并可选直接报错 |
| **粘性的两条机制只做一条** | 只做 session 绑定不做"已吐字锁号"，仍会在流中途换号，客户端拿到矛盾的工具调用序列 | 两条都做，且 `strict` 模式下宁可返错也不换号 |
| **Tauri vs Electron 选错** | 若转换层是 TS，选 Tauri 会引入 sidecar/IPC 额外复杂度 | 按阶段 1 的语言决定，别为了体积牺牲可维护性 |
| **单机日志无限增长** | SQLite 里的请求日志 + SSE 原始帧会长得很快 | 建表即设 TTL 与上限；原始帧默认关闭、按需开启 |

### 7.6 一句话总结

**需求 8 抄 ccr（MIT），需求 5 照 LiteLLM 的行为规格重写（MIT 边界内），需求 9 抄 new-api 的语义自己实现，需求 1–4/6/7 自己写一个薄的声明式转换层（`pathTemplate` + 协议族），需求 10/11 自己搭桌面壳与观测。** 不要把任何一个候选当核心——它们要么形态错配（LiteLLM 的 Python 分发、ccr 的黑盒网关），要么许可不通（new-api 的 AGPL、AIClient-2-API 的 GPL、one-api 的能力缺位）。

---

## 8. 未核实项（诚实清单）

1. **ccr 的协议转换细节与需求 1/2/4/5 的真实满足度**——codec 在 npm 包 `@the-next-ai/ai-gateway@1.0.21`（MIT，238 文件，约 4.9 MB 解包）内，**未克隆未读**。这是本报告最大的取证空洞，且它同时是"ccr 能否作为核心"的决定性变量。
2. **new-api 的 `POST /v1/responses` 端到端行为**（`response.completed` 终结、`previous_response_id` 回放、`function_call`/`function_call_output` 配对）——仅从路由与测试名推断，未实机。
3. **new-api 是否对 channel key 做落库加密**——`model/channel.go` 的 `Key` 字段未见加密钩子，但未穷尽搜索 `setting/` 与自定义 GORM serializer。
4. **new-api 的 DeepSeek 流式 usage 落位 / keep-alive 空行容忍**——Go 侧流式解析未逐行追。
5. **AIClient-2-API 的 SSE 事件顺序、ping 转发、`anthropic-beta` 透传**——只在 `claude-core.js:40` 见到硬写 `anthropic-version`。
6. **LiteLLM 对 Qwen thinking signature 恒空 / `stop_sequence` 恒 null 的规范化**——未找到，但 native passthrough 是"原样转发"，可能压根不需要处理（取决于客户端容错）。
7. **ccr 的 Electron 托盘实现细节**——`packages/electron` 未逐文件读。
8. **各家的打包实测体积**——均为量级估计，未实际构建安装包。
9. **`providers.md` 里各家路径契约的一手确认程度**——本报告直接引用该文件，若其中有未确认条目，本节结论随之打折。

> 未核实的条目里，**第 1 条优先级最高**：在决定"是否基于 ccr"之前，应先克隆 `@the-next-ai/ai-gateway` 单独评估一次。

---

## 9. 更正（2026-10-14，本报告完成后补做）

第 8 节第 1 条、第 7 节（「你无法逐步审计其转换正确性的黑盒」）与第 7.6 节里的「ccr 的黑盒网关」措辞已过时。补做核实的结果：

- `@the-next-ai/ai-gateway` **有公开 MIT 源码**：<https://github.com/The-NeXT-AI/ai-gateway>（已克隆到 `vendor/ai-gateway`）。npm `gitHead = 3fd93f8…` 与仓库里的提交 `3fd93f8 chore(release): bump version to 1.0.21` 对齐，本地 `package.json` 为 `1.0.21` / MIT。
- 「黑盒」的正确说法是：**转换代码不在 claude-code-router 仓库里**（`packages/core/src/gateway/internal/shared.ts:309` 的 `gatewayPackageCandidates` 指向该外部包），但它在公开仓库里。
- 因此 ccr 出局的理由从「黑盒」改为**形态与保真缺口**：175 个 TS 文件 / 118,371 行、Node/Fastify 服务器（Redis、Docker、计费、MCP、插件）；`count_tokens` 全仓 0 处；`cache_control` 仅 1 处。
- 本报告第 7.6 节的最终组合结论**不变**，但把 ccr 从「只抄客户端半边」升级为「协议编解码的一手移植参照」。

详情：`.scratch/model-gateway/research/ccr-gateway-package.md` 与票据 `21-research-ccr-gateway-package-source.md`。

---

## 10. 追加说明（2026-10-14，范围调整）

本报告需求清单里的**需求 9（反代类上游的账号池）已整体移出本项目**。本报告是历史取证，正文不再改动；判读时请忽略需求 9 相关的评价（含 AIClient-2-API 的 provider-pool-manager.js、new-api 的会话粘性等）。已归档材料在 .scratch/model-gateway/research/archive/。
