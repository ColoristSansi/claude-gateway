# Codex 桌面应用：模型选择器与请求路由（票 25 实测）

Status: 已实测定案（2026-10-14）
方法: 先只读扫本机产物，再做三次现场实测（写 `~/.codex/config.toml` + 完全重启桌面应用 + mock 监听）
对象: MSIX `OpenAI.Codex 26.1007.2314.0`（`C:\Program Files\WindowsApps\OpenAI.Codex_26.1007.2314.0_x64__2p2nqsd0c76g0`）、核心 `codex.exe`（`%LOCALAPPDATA%\OpenAI\Codex\bin\2e5e00daee91c61d\`，SHA256 `D13914CE…DAEC`）

## 结论

1. **模型选择器不按登录门控。** cc-switch 那条断言（「没登录就藏起 `config.toml` 里的自定义模型、官方标 not planned」）在本机这一版**不成立**。决定模型能否出现在选择器里的是**模型目录文件**（`model_catalog_json`）：
   - 只写 `model_provider` + `[model_providers.*]`、**不写目录** → 选择器里只有官方模型；
   - 补上目录文件 → 网关模型在「Codex」与「ChatGPT Work」**两个模式里都出现**，全程**零官方登录**。
2. **请求确实按 `config.toml` 的 provider 走**，不需要官方登录：mock 收到 `POST /v1/responses`（`originator: Codex Desktop`）。**必须按 SSE 回**，普通 JSON 会被判 `stream disconnected before completion`。
3. 桌面应用真正需要官方登录的只有**云线程（durable thread）**，与网关无关。

## 实测记录

### 实验 1：只写 provider，不写目录（失败）

写入（`config.toml.bak-gateway-probe` 为备份）：

```toml
model_provider = "gateway-probe"

[model_providers.gateway-probe]
name = "Gateway Probe"
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"
```

完全重启应用 → 选择器（ChatGPT Work 模式）：默认 / 6.1 Sol / 6 Astra / 6 Sol / 6 Luna / 5.6 Sol / 5.6 Terra / 5.6 Luna / 5.5，**没有自定义条目**。切到 Codex 模式同样。

### 实验 2：补上模型目录（成功）

目录文件 `~/.codex/gateway-probe-catalog.json` = 内置目录（`codex debug models` 的输出，11 条官方模型）**加一条**：

```json
{"slug":"gateway-probe-model","display_name":"Gateway Probe","description":"Gateway probe model (local test)","visibility":"list", ...}
```

配置加一行 `model_catalog_json = "gateway-probe-catalog.json"`，完全重启应用 →
**`Gateway Probe` 出现在选择器里，「Codex」与「ChatGPT Work」两个模式都有**（官方 11 条也照旧在）。

机制：**写了 `model_catalog_json` 之后 Codex 只认目录文件里的模型**，不再刷新内置列表（第三方实现的原话与教训见 `vendor/cc-switch/src-tauri/src/services/provider/codex_official_models.rs:3`、`:15-16`：目录里若没有一条 `visibility=list` 的模型，官方模型会被全部藏起来）。

### 实验 3：路由（通过）

mock（只记录请求、回普通 JSON）监听 `127.0.0.1:8787` → 应用里选中 `Gateway Probe` 发一条消息 → mock 日志（`%TEMP%\gw-mock\requests.log`）：

```
POST /v1/responses
accept: text/event-stream
originator: Codex Desktop
user-agent: Codex Desktop/0.162.0-alpha.17.2 (Windows 10.0.26200; x86_64) unknown (Codex Desktop; 26.1007.21434)
host: 127.0.0.1:8787
content-length: 118709
x-codex-turn-metadata: {… "client_type":"desktop_app", "model":"gateway-probe-model",
  "reasoning_effort":"low", "sandbox_mode":"workspace-write", "workspace_kind":"projectless" …}
BODY: {"model":"gateway-probe-model","stream":true,"input":[…]}
```

应用报 `stream disconnected before completion: stream closed before response.completed`（它等的是 SSE，我们回的是 JSON）→ 反证请求已到达网关。重复 POST 是它对失败流的重试。

## 代码层证据（解释实验 1 与 2）

- 桌面应用 = Electron 壳（`ChatGPT.exe`）+ 用 stdio 拉起同一份 `codex.exe` 作为 app-server；`CODEX_HOME` = `%USERPROFILE%\.codex`。
- **应用不覆盖 `model_provider`**：实测命令行 = `codex.exe -c features.code_mode_host=true app-server --analytics-default-enabled -c plugins.….enabled=true`（两项插件开关）——所以 `config.toml` 是被读的。
- 选择器数据源是 app-server 的 `model/list`（`app.asar\webview\assets\app-initial-*.js` 偏移≈4,812,341：`sendRequest('model/list',{includeHidden:!0,…})`）；可见性过滤 `jRi`（同文件 ≈4,809,986）对自定义 provider 走 `!o.hidden`，与登录无关；唯一「回落默认模型」的分支 `vVi`（≈4,840,100）要求 `isChatGptAuth===true`。
- 自定义模型有专门文案 `composer.mode.local.model.custom`（`defaultMessage: "Custom"`、`description: "Custom model from config"`）；选择器文案里没有任何「请登录」。
- 云线程判定：`app.asar\.vite\build\main-*.js`（≈3,245,982）`getAuthToken()` 为空就抛 `Sign in to ChatGPT to start a durable thread.`；本机日志里 `hostId=durable` 一直重连失败，`hostId=local`（stdio）正常连接。
- 官方登录态：`%USERPROFILE%\.codex\auth.json`（`cli_auth_credentials_store` 默认 `"file"`）；本机该文件当时**不存在**。

## 仍然未验证

- **Codex CLI 真实发请求**（票 24 的 U4）：本次只验了桌面应用；CLI 侧仍是 doctor 级结论。
- 桌面应用**登录后**的行为（云线程可用性、选择器是否多出其它条目）：本机从未登录过。
- 只扫了 `app.asar` 的 `webview/assets` 与 `.vite/build`；567MB asar 其余部分未全量扫描。`not planned` 字样在本机产物 0 命中，但第三方文档的结论可能对更早版本成立。
- 目录文件里的 `display_name` / 推理档位 / 上下文窗口在 UI 上的确切呈现未逐项核对（见票 31）。

## 复现步骤

1. 备份 `~/.codex/config.toml`；写入 `model_provider` + `[model_providers.<id>]`（`base_url` 指本机某个端口）+ `model_catalog_json`（指向含目标模型的目录文件）。
2. 完全退出并重开 Codex 桌面应用；「Codex」与「ChatGPT Work」两个模式都可看选择器。
3. 路由：用只记录请求的 mock 监听那个端口，选中自定义模型发一条消息，看是否收到 `POST /v1/responses`。
4. 还原：把备份拷回、删掉目录文件。
