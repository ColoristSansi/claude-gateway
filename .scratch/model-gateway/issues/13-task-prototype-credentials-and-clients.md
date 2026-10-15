# 交付原型凭据并装好三个客户端

Type: task
Status: resolved
Blocked by: —

## Question

原型要跑真实上游，也需要真实客户端联调；这两样只有用户能提供。做完这张票据，[14](14-prototype-conversion-core.md) 与 [15](15-prototype-desktop-3p-e2e.md) 才有输入。

## 用户侧清单

1. **装好三个客户端并登录**（用户已答复「我装好告诉你」）：
   - Claude Desktop（Windows 版），并开启 Developer Mode（Help → Troubleshooting），确认 Developer 菜单里出现 Configure Third-Party Inference。
   - Claude Code（`claude` 命令）。
   - Codex CLI（`codex` 命令）。
2. **填好上游凭据文件**：把 `.scratch/model-gateway/prototype-upstream.env.example` 复制成 `prototype-upstream.env` 并填值（opencode 与 commandcode 两家都接）。**不要在对话里贴任何 key。**
3. 告诉 AI「装好了」并确认两个上游各自至少有一个可用模型。

## 完成判据

- 日志/配置目录能被检测到：`%USERPROFILE%\.claude\`、`%USERPROFILE%\.codex\config.toml`、`%LOCALAPPDATA%\Claude-3p\`。
- `prototype-upstream.env` 填好且未进入对话记录。

## Answer

### 客户端（实测）

- **Claude Desktop**：Microsoft Store / MSIX 包 `Claude_2.37337.1.0`（`C:\Program Files\WindowsApps\Claude_2.37337.1.0_x64__pzs8sxrjxfjjc`），已在运行，1P 模式已登录，开发者模式已开（`developer_settings.json` = `{"allowDevTools": true}`）。
  - 认领时的探测只看到 staged 未注册的 `Claude_2.31226.0.0`（无 `%APPDATA%\Claude`、无 Appx 注册、无开始菜单入口）；本次为用户从 Store 更新后的注册版本。**上一轮的「未装」结论作废。**
- **Claude Code**：`claude` 2.1.296（`C:\Users\ZHANQUAN\.local\bin\claude.exe`），`%USERPROFILE%\.claude\` 存在。**未登录**：用户无套餐。
- **Codex**：两套并存——(1) Store 应用 `OpenAI.Codex 26.1007.2314.0`（即 ChatGPT/Codex 桌面应用，CLI 二进制在包内 `app\resources\codex.exe`，**未暴露 `codex` 别名**）；(2) 本次由 agent 安装官方 CLI `@openai/codex` → `codex-cli 0.162.1`（`%APPDATA%\npm\codex.ps1`），`codex` 命令可用。`%USERPROFILE%\.codex\config.toml` 存在（Store 桌面应用写入的 `[desktop]` / `[marketplaces.*]` / `[plugins.*]` 段）。**未登录**：用户无 OpenAI 账号。

### 上游凭据（实测，值未进入对话记录）

- `GET https://api.commandcode.ai/provider/v1/models` → **HTTP 200，89 个模型**（`claude-sonnet-5-5`、`claude-sonnet-5`、`claude-sonnet-4-6`、`claude-fable-5-1/-5`、`claude-opus-5-5/-5`、`claude-opus-4-8` …）。
- `GET https://opencode.ai/zen/go/v1/models`（env 里的 `OPENCODE_BASE_URL`）→ **HTTP 200，39 个模型**（`deepseek-v4-flash`、`deepseek-v4-pro`、`deepseek-v4.1-flash`、`glm-5.x` …）。
- **别名等价（新事实）**：`opencode.ai/zen/go/v1` ≡ `opencode.ai/inference/go/v1`（同 39 个模型）；`opencode.ai/zen/v1` ≡ `opencode.ai/inference/v1`（20 个模型）。`providers.md` 只记了 `/inference/...` 一种写法，两种都能用。
- `GATEWAY_PORT=8787`。

### 对票 15 的关键事实（修正一条路径假设）

- `%LOCALAPPDATA%\Claude-3p\` **真实存在**（当前为空；`configLibrary` 尚未生成，本机任何位置都还没有）。包声明了 `rescap:Capability unvirtualizedResources`，且 `Packages\...\LocalCache\Local\Claude-3p` **不存在** → 该路径未被 MSIX 重定向，票 15 的写入假设成立。
- 但 **`%APPDATA%\Claude` 不存在**：Store/MSIX 安装的 Roaming 被重定向到 `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\`，`claude_desktop_config.json`、`config.json`、`logs\`、`claude-code-sessions\` 都在那里。→ 票 15「写入前备份原文件」「一键还原 1P」必须走这个 LocalCache 路径，不能假设 `%APPDATA%\Claude\`。

### 完成判据核对

- `%USERPROFILE%\.claude\` ✅、`%USERPROFILE%\.codex\config.toml` ✅、`%LOCALAPPDATA%\Claude-3p\` ✅（空）。
- `prototype-upstream.env` 填好、未进入对话记录 ✅。
- 两上游各至少一个可用模型 ✅（89 / 39）。

### 遗留（不影响票 14 / 15）

- Claude Code 与 Codex 未登录。票 15 只验 Desktop；将来联调 Claude Code 时把它指向网关（`ANTHROPIC_BASE_URL` + 任意 key）即可，不需要官方登录态。
- 桌面版 Codex 自带的 `codex.exe` 不在 PATH，本次改用 npm 版；两条安装并存，`~/.codex/config.toml` 由桌面应用写（见 map 的 Not yet specified）。
