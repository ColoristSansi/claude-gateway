# 原型：Codex 端到端（/v1/responses + SSE + 模型目录）

Type: prototype
Status: resolved (2026-10-15)
Blocked by: —（原为 31，2026-10-15 已结）

## Question

Claude Desktop 已有端到端验收（票 15），Codex 侧还没有。网关要做到什么程度，才算「Codex 用上了第三方模型」？

要定：

1. 验收判据：Codex CLI（非交互 `codex exec`）与 Codex 桌面应用各跑通什么算过？（桌面应用已证请求会到网关，见票 25）
2. `/v1/responses` 入站的保真下限：**必须按 SSE 回**（回普通 JSON 会被判 `stream disconnected before completion`）；请求头里出现 `x-openai-internal-codex-responses-lite: true` 说明这一版走 responses-lite——要不要按它降级？`response.completed` 结束、`function_call`/`function_call_output` 成对（spec §4.2 已写契约）。
3. 覆盖到哪一层：流式与非流式、`previous_response_id` 续轮、工具调用（实测请求体里带 `functions` 命名空间与自定义 `exec` 工具，body 118KB）、`reasoning.encrypted_content`。
4. 真实配置怎么落：模型目录（票 31）写进真实 `~/.codex/config.toml`（首写备份 + 一键还原），跑完怎么还原；`[desktop]` / `[plugins.*]` / `[mcp_servers.*]` 段必须原样保留。

## 背景

- 已解析：[票 15](15-prototype-desktop-3p-e2e.md)（同形状的先例）、[票 25](25-grilling-codex-desktop-card-split.md)（桌面应用实测证据）、[票 14](14-prototype-conversion-core.md)（现有原型只有 Anthropic + OpenAI Chat 入站，**没有** `/v1/responses`）。
- 已解析 [票 31](31-grilling-codex-model-catalog.md)（模型目录形状与并存规则，8 条定案）——本票按它落地。
- 顺带结掉票 24 的 **U4**（Codex CLI 真实发 `POST /v1/responses`）——本机目前只验过桌面应用。

## Answer

**过了。** 四个家族名在 Codex CLI 与桌面应用里都能用上第三方模型，四条要定的事都定了。实现在 `.scratch/model-gateway/prototype/`：`src/responses.rs`（Responses ⇄ IR）、`src/upstream.rs`（入站编码器两态 + 按入站协议给错误信封）、`codex-gateway.ps1`（模型目录 + 配置写入）、`tests/replay.rs` 第 ⑫ 格。细节与实测记录见 `prototype/README.md` §10。

### 1. 验收判据（实测结果）

| 路径 | 判据 | 结果 |
| --- | --- | --- |
| Codex CLI 0.162.1 `codex exec` | 抬头 `provider: claudegateway` + 指定模型，答对，`/api/logs` 有记录 | 过（`claude-sonnet → commandcode / deepseek/deepseek-v4.1-flash / openai / 200`） |
| Codex CLI 工具往返 | 模型发 `exec_command` → Codex 执行 → 结果回灌 → 模型接着答完 | 过（上游侧 `messages` = `[system, user, assistant(tool_calls), tool]`） |
| Codex 桌面应用（真实 `~/.codex`） | 选择器里看到四个家族名；流式作答；无报错 | 过（网关日志 4 条 `Codex Desktop/0.162.0-alpha.17.2` 的 `/v1/responses`，分落 opencode / commandcode，全 200） |

### 2. `/v1/responses` 的保真下限

- **必须按 SSE 回，且以 `response.completed` 收尾**——已实测确认（回普通 JSON 会被判 `stream disconnected before completion`，见票 25）。
- **`x-openai-internal-codex-responses-lite` 不分流**：网关不读这个头，两端一律回全量 Responses SSE，CLI 与桌面应用都过。「按它降级」没有证据支持，不做。
- `function_call` 与 `function_call_output` 靠 **`call_id`** 配对（上游 `tool_use` 的 id 直接当 `call_id`）；自由格式 `custom_tool_call` 走同一套配对。已实测。
- **错误信封按入站协议给形状**：Responses 用 OpenAI 的 `{"error":{"message","type","code"}}`。原先一律回 Anthropic 那份 `{"type":"error",...}`，Codex 读不懂就只能抛无语境的断流文案。

### 3. 覆盖到哪一层

- **流式与非流式都做**：非流式出站照样 `stream:true`，网关缓冲到终态再组装完整 response 对象（与票 26 对 `/v1/messages` 的处理同构）。
- **`previous_response_id` 不实现**：Codex 固定 `store:false`，续轮靠回放 `input`（第 ⑫ 格就是这么发的）；收到该字段当没看见。
- **工具调用覆盖**：`function` 与 `custom` 两种工具定义都收；自由格式工具对上游声明成「一个 `input` 字符串参数」，回来时拆出来还原成 `custom_tool_call`。实测 Codex 用中性目录模板时**根本不发**自由格式工具，所以这条是兜底而非主路径。
- **`reasoning` 项**：输入侧一律丢弃（拿不到可验证的 `encrypted_content`）；输出侧只回 summary、**不伪造** `encrypted_content`。实测 Codex 接受并按 summary 渲染。
- **系统提示不在 `instructions` 里**：Codex 把它当 `input` 的 `role:"developer"` 消息发，所以 `developer`/`system` 必须归一化进 IR 的 system —— 这是本票挖出来的第一条硬事实。

### 4. 真实配置怎么落

`prototype/codex-gateway.ps1`，模式 `gateway` / `show` / `restore` / `rollback` / `selftest`：

- 接管范围 = **逐行认领**：顶层 `model_provider` / `model` / `model_catalog_json` + `[model_providers.claudegateway]` 这张表 + 两对哨兵行。别的行一个字节不动。
- 首写快照 `config.toml.bak-claudegateway`；**还原默认是摘键**（不是整份回滚），`rollback` 只在「两边摘掉网关内容后仍一致」时才允许。
- **桌面应用会改写 `config.toml`，而且会插进我们的范围里**：实测它补了 `[plugins."computer-use@openai-bundled"]` 与 `[windows] sandbox = "elevated"`，后者正好落在我最初那版「哨兵区域」之间 —— 按区域删就会把它一起删掉。**已改成逐行认领**，并把这个场景做成 `-Mode selftest` 的回归（插入 → 外部写手插表 → 摘除，必须回到原样且保住那张表，且重写幂等）。这条同时把票 28「显式接管键清单」的由来说实了。
- 还原干净有 diff 证据：摘键后 `~/.codex/config.toml` 与「写入前」逐字节比，差异**只剩桌面应用自己写的那三处**，`claudegateway` 零残留，目录文件已删。

### 顺带结掉的事

- **票 24 的 U4 结案**：Codex CLI 真实发 `POST /v1/responses`（UA `codex_exec/0.162.1`）。
- **修正票 31 的一条措辞**：模型目录行模板按**入站协议**分，不是按出站 wire 分。判据是「网关在 Responses 上说不说得清自由格式工具」，与出站是 anthropic 还是 openai-chat 无关；按 cc-switch 给 native-`/responses` 网关用的中性模板（`shell_type="shell_command"`、无 freeform）。spec §7 已改。

### 没做 / 留给实现阶段

- `namespace`（多智能体）与 `web_search` 工具：丢弃。
- 接管键的**冲突检测**（票 28 的「磁盘值非网关所写即放弃并提示」）：没实现，这里是直接覆盖 + 首写快照保退路。
- 客户端档案覆盖、配置写入引擎本体：按地图 **Out of scope**，归实现阶段。
- Codex 自己的 Windows 沙箱会拦起进程（`blocked by policy`），所以 CLI 那条工具用例拿到的是沙箱拒绝文案 —— 工具**往返**照样通了；这不是网关的问题。

回归门禁：`check.ps1` 全绿（单测 29、集成 25）。**本票不留新票。**
