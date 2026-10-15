# M3 收口：保真长尾验收

Type: task
Status: resolved
Blocked by: 56, 57, 58, 59

## Question

走 spec §12 的 M3 收口验收。**主会话 + 用户在场**。

依据：spec §12 M3。

## 交付

- 验收记录贴进本票 Answer：`check.ps1` 输出、结构化输出探针在两条上游的结论、用量与上游 usage 对账的截图或数字。

## 收口验收

- `check.ps1` 全绿（含 Ollama 面的回放断言）。
- 结构化输出探针在两条真实上游实测并给出能力位结论。
- 用量数字与上游回的 usage 对得上（抽查两条请求）。
- 用户签字。

## Answer

**M3 收口通过**（2026-10-16，主会话 + 用户在场；分支 `integration/model-gateway`）。

现场步骤卡：`C:\Users\ZHANQUAN\AIGC\Project\cg-notes\m3-live.md`（七页逐页「看什么算过」、用量页五范围 × 三维度、日志四段、设置六项、徽标、改前快照路径）；证据日志同目录 `60-*.log`（含 `60-usage-raw\` 的原始响应）；改前原文快照 `…\cg-notes\60-prewrite-backup\`。

### 一、`check.ps1` 全绿（含 Ollama 面的回放断言）

```
$ pwsh -File .\check.ps1 -Quick -Replay
=== fmt / clippy (两套特性) ===            无输出
=== unit ===                               bin 8 / lib 168 / admin_api 6 / binding_save_api 1 /
                                           binding_table 14 / discovery 8 / dist 6 / dist_api 5 /
                                           gateway_service 4 / logs_api 4 / m59_api 4 / ollama 12 /
                                           provider_table 14 / structured_output 19 / doctest 1
=== replay (--features mock) ===           22 passed
=== replay-unit ===                        5 passed
全部通过。   exit 0
```

`ollama` 那 12 条在跑（票 56 的回放/单测格），`structured_output` 19 条（票 57）。

### 二、结构化输出探针：两条真实上游实测（复核，与票 57 一字不差）

| 上游 / 出站面 | 模型 | status | 上游回答原文 | 能力位 |
| --- | --- | --- | --- | --- |
| commandcode / **anthropic** | `claude-sonnet-5-5` | **200** | `hello` | **`ignore`**（`source=selfcheck`）|
| commandcode / openai-chat | `deepseek-v4.1-flash` | 200 | `{"a":"hello"}` | `native` |
| opencode / openai-chat | `deepseek-v4.1-flash` | 200 | `{"a":"hello"}` | `native` |
| opencode / anthropic | `claude-haiku-5-5` | 401 | — | 未判出（不落库）|
| commandcode / openai-responses | — | 400 | 必须走 `/messages` | 未判出（不落库）|

敌意提示词逐字：`Reply with exactly the word hello and nothing else. Do not output JSON.` —— **commandcode 的 Anthropic 面 200 却照提示词回 `hello`**，规范 §6「状态码骗人」那条当场复现，能力位判 `ignore`。

### 三、用量与成本对账（三条请求，逐项零差异）

对账方式：开 body 开关 → 真发请求 → 用 `GET /api/logs/{id}` 的**第二段（上游原样响应体）**取**同一次响应里上游自己报的 usage**，与库里那一行逐字段比。

| 行 | 形状 | 网关库 `in / out / cache_read / cache_write` | 上游同一次响应的 usage | 成本（快照价 2/10/0.2/2.5 每 Mtok）|
| --- | --- | --- | --- | --- |
| 46 | 非流式 | `25 / 4 / 0 / 0` | `{"input_tokens":25,"output_tokens":4,"cache_read_input_tokens":0}` | `$0.00009` |
| 47 | 流式 + `cache_control` | `9 / 16 / 0 / **3845**` | `message_delta {…,"cache_read_input_tokens":0,"cache_creation_input_tokens":3845}` | `$0.0097905` |
| 48 | 同前缀再发一次 | `9 / 13 / **3845** / 0` | `message_delta {…,"cache_read_input_tokens":3845,"cache_creation_input_tokens":0}` | `$0.000917` |

**四项全等、一项不差**，成本可手算复现。缓存写（票 58）与缓存读都在真实响应上走到了。

### 四、Ollama 面（票 56）实测

`/api/chat` 流式 = 2 帧 NDJSON（无 `data:` / `event:` / `[DONE]`，末帧 `done:true` + `done_reason` + `prompt_eval_count`/`eval_count` 在帧顶层）；`stream:false` → 单个 JSON；`/api/tags` 5 条（`name == model`、`size=0`、确定性 digest）；`/api/version` → `{"version":"0.13.3"}`（GET + HEAD 200）；`/api/ps` → `{"models":[]}`（HEAD 405）；`/api/show` → `details` + `model_info:{}`、`:latest` 剥掉、未知名 404 `{"error":"model 'nope-xyz' not found"}`、空 body 400；**装载 / 卸载本地回假终态**（请求总数 51 → 51，加 0，零上游）；其余 `/api/*` 404、管理面照常。

### 五、脱敏

诊断包 1702 B：两把 key 明文 **0 / 0**、`sk-` 0，掩码 `use…nf36` / `oc_…3ZEO` **各 1**；导出 43330 B：key 明文 0；库文件（含 `-wal` / `-shm`）四种编码（前 12 / 后 12 / base64 / hex）**全 0**；正对照（`credentials` 命中、接口与诊断包里掩码命中）都在。body 不给 consent → 400；5 秒窗口 7 秒后 `on=false`（1 小时自动关同段代码）。

### 六、用户当场走界面并签字

七页逐页走过（概览三步清单 `ready: true`、上游页角标 0、模型与映射四行槽位、客户端三张卡、日志、用量、设置）；用量页五范围 × 三维度 + 切美元；日志页详情四段（含帧时间轴切原始 SSE）+ 复制诊断（无密钥、无 body）；设置页六项各存一次；托盘关窗隐藏 / 右键退出真停。

**用户在走查时抓到一个真 bug，当场修掉并复验通过**（这是本票最有价值的一段）：

> 用户原话：*模型与映射页 claude-sonnet 和 haiku 档切换模型后点击存后，页面会变成空白，只能退出程序再重启，重启后模型会恢复为之前的模型*

根因（实测，非猜测）两条同源：

1. **白屏**：`app/src/components.tsx` 的 `ErrorBlock` 把第三个 `useState(busy)` 写在 `if (!error) return null` **之后** —— 它以 `error=null` 挂载只用 2 个 hook，第一次报错时变 3 个，react-dom **生产包**在 `updateWorkInProgressHook` 里 `throw formatProdErrorMessage(310)`；顶层没有 ErrorBoundary → React 卸掉整棵根树。headless 驱动实测：`#root.childElementCount = 0`、控制台 `Minified React error #310`。
2. **存不进去**：受控 `select` 的候选里没有当前值时，浏览器把**显示**回退到第一项而 React state 仍是旧值 —— 界面显示 `openai-chat`、实际发出 `anthropic` → 网关按票 49/51 的规矩回 400（`claude-sonnet 的协议不在这条模型的声明里`）→ 触发上面那条白屏 → 库里一个字节没动 → 重启看旧值。`sonnet`/`haiku` 是唯二 `wire=anthropic` 的槽位，所以只有这两行中招。

修法（commit `8a2204d`，已合）：hook 提到 early return 之前；新增 `RenderBoundary` 包住 `App`（spec §10 的「可读错误 + 一个主按钮」，**绝不允许整页白屏**）；`ModelsPage` 换模型/上游时按 `supportedWires` 拨正 `draft.wire`，错误块接住后端 `action` 并兜底「重试存这一条」。新增回归 `crates/gateway-core/tests/binding_save_api.rs`：改 sonnet/haiku → 200 + `GET` 读回 → **关网关重开仍在** → 未声明 wire → 400 且库不动。

重装后在真实安装版界面上复验（exe sha256 `612AF5EA…`）：点存不白屏 → 读回新值 → **重启应用后仍是新值**；非法组合当场给「协议不在这条模型的声明里 + 可能原因 + 主按钮 + 复制诊断」且库不动；另外**注入**「`/api/bindings` 回复缺 `profiles`」实测边界顶出可读模板而不是白屏。用户原值已改回（sonnet `commandcode / claude-sonnet-5-5 / anthropic`、haiku `commandcode / claude-haiku-5-5 / anthropic`）。

### 遗留

1. **七页人眼之外没有浏览器自动化**：机器侧只能给端点级证据；本票的界面结论来自用户当场走查（票 60b 那次白屏是用 headless 驱动复现的，属于例外）。
2. **快照刷新的失败路径「沿用旧快照」没跑**（要断网/改 hosts，没做）；成功路径跑到 8868 条模型价。
3. **成功换端口那一路没跑**（会重写已生效客户端地址）；端口被占 → 409 已验。
4. 前 45 条历史日志永远「无价格」（快照导入前落的库，**不追古**，符合票 33 的冻结口径）。
5. 靶子进程 00:57 自己消失一次，未查清原因（无 crash dump、无退出日志）——用户当时正在走查且刚遇到白屏，最可能是他自己退出的。
6. 观察到两个槽位的绑定与 M2 步骤卡记录不同（`claude-fable` / `claude-opus` 都指 `opencode-go / deepseek-v4.1-flash / openai-chat`，四槽位探针仍全 200）——疑似用户自己在界面上改过，**未改回**。
