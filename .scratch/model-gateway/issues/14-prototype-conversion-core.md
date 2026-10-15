# 原型：协议转换核心 + mock 上游

Type: prototype
Status: resolved
Blocked by: —

## Question

IR、四个（至少两个）编解码与流式转发能不能立起来？先拿 mock 上游验证，避免被外部网络与上游怪癖干扰。

## 范围

1. Rust 骨架（tokio + axum），固定端口 8787，SQLite 建表，家族槽位四个。
2. IR + Anthropic Messages 编解码（入站 `/v1/messages` ↔ 出站 Anthropic 直通）+ SSE 逐帧转发（含 `ping` 转发与事件顺序到 `message_stop`）。
3. **mock 上游**：本地假上游回放录制的 SSE 帧，覆盖五类场景——纯文本、工具调用、thinking、usage 统计、异常中断。
4. 真实上游各接一家：opencode 与 commandcode（`quirks` 声明式表达差异）。
5. 代码放 `.scratch/model-gateway/prototype/`，粗糙优先，能跑就行。

## 产出

- 可执行的最小网关 + 一组 fixture + 一段能复现的启动命令。
- 一段简短记录：哪些上游差异被 `quirks` 吸收了，哪些必须写进代码。

## Answer

### 结论：立起来了

IR + 两套编解码（Anthropic Messages 直通、OpenAI Chat）+ SSE 逐帧转发，全部跑通；`quirks` 与声明式表吸收掉的上游差异与「必须写进代码」的差异已经分家，后者有 7 条（见产物 §2）。

产物全部在 `.scratch/model-gateway/prototype/`：

- 启动：`cd .scratch\model-gateway\prototype; cargo run`（网关 8787，mock 上游 8788，凭据读 `../prototype-upstream.env`）。
- 自检：`pwsh -File .\verify.ps1` → **10/10 合格**（5 场景 × 2 wire；断言首帧 `message_start`、非 abort 场景以 `message_stop` 收尾且 usage 非零、abort 场景必须以 `upstream_abort` 的 `error` 帧收尾）。
- 代码 9 个模块 + 10 份 fixture + `README.md`（结论记录全文）；运行时状态看 `GET /api/routes`、`GET /api/logs`（SQLite `gateway.db`）。
- 启动时打印整张路由表；四个家族槽位、两个真实上游、三条负例都在表里。

### 实测证据

| 项 | 结果 |
|---|---|
| mock 五场景 × 两 wire | 10/10 通过（纯文本 / 工具调用 / thinking + 签名 / usage 与缓存字段 / 异常中断） |
| opencode（真实上游） | `deepseek-v4.1-flash`、`deepseek-v4-pro`、`glm-5.3`、`kimi-k3`、`qwen3.8-max`、`minimax-m3` 走 Chat 端点全部 200 |
| commandcode（真实上游） | `claude-sonnet-5-5` 走 `/messages` 200；`deepseek/deepseek-v4.1-flash` 走 `/chat/completions` 200 |
| 四个家族槽位 | 全部 200 流式到 `end_turn`，`/api/logs` 证明四个槽位按表分别落到两个上游（fable/opus → opencode，sonnet/haiku → commandcode） |
| 负例（错误信封与状态码就地转换） | opencode Anthropic 端点 401 `authentication_error`；commandcode opus 403 `permission_error`；wire 错配由 quirk 早拒 400 |

### 家族槽位绑定（用户本次决定）

**四个槽位都用 `deepseek-v4.1-flash`，两家上游各两个**（用户原话「均使用两家供应商的deepseek 4.1 flash」）：

| 槽位 | 上游 | 真实模型 | wire |
|---|---|---|---|
| `claude-fable` | opencode | `deepseek-v4.1-flash` | openai |
| `claude-opus` | opencode | `deepseek-v4.1-flash` | openai |
| `claude-sonnet` | commandcode | `deepseek/deepseek-v4.1-flash` | openai |
| `claude-haiku` | commandcode | `deepseek/deepseek-v4.1-flash` | openai |

理由（实测）：commandcode 本计划只有 `claude-sonnet-5-5` 在档（`claude-opus-5` / `claude-fable-5` 一律 403 `MODEL_NOT_IN_PLAN`），opencode 的 Anthropic 端点要另一种凭据（401）、Chat 端点正常；而两家的 deepseek 4.1 flash 都验证 200。

### 新增事实（已同步进 `research/providers.md` §3）

- opencode Go **凭据按端点面分家**：同 key 打 `/chat/completions` 200、打 `/messages` 401 `Go requires a credential`、打 Console 面 `/zen/v1/messages` 401 `Missing API key`。
- commandcode 的 `/v1/models` 89 个模型 ≠ 可用模型：plan 门在**调用时**才拦（403），模型自带 `supported_endpoints` 声明只在哪几个端点存在。

### 由此新开的票据

[供应商表：认证与可用 wire 的粒度](23-design-provider-table-granularity.md)——原型把「一上游一凭据」和「一上游一种 wire 集合」两个假设都用穿了，表结构要按 wire / 逐模型重新定粒度。

### 未做 / 取舍（原型层面）

非流式请求直接 400；入站只做 Anthropic Messages（Chat / Responses / Ollama / `count_tokens` 未做）；只解码再编码、不做字节直通；mock 的「异常中断」用「流在终态前结束」模拟，不是 TCP 硬切；无入站鉴权、密钥明文在内存；头透传只验到代码路径（未对上游回显核对）。
