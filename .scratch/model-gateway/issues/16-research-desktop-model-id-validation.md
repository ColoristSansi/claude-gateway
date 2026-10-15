# 核实 Claude Desktop 3P 的模型 ID 校验规则

Type: research
Status: resolved
Blocked by: 13

## Question

Claude Desktop 3P 到底校验不校验模型 ID？两份证据互相矛盾：

- 官方文档：自动发现（`/v1/models`）只展示「ID 可识别为 Claude」的条目，但显式 `inferenceModels` **可以填任意完整模型 ID**；全是完整 ID 时 app 会自动跳过 `/v1/models`。
- cc-switch 的说法（据称取自 app.asar 里的校验器）：只允许 `claude-{sonnet,opus,haiku,fable,mythos}-*`，非法名会触发 fail-all，**整组模型被拒收**。

## 为什么重要

它决定：四个家族名是不是唯一可行解，以及 `/v1/models` 要不要按客户端做过滤。

## 怎么做

Desktop 装好之后（票据 [13](13-task-prototype-credentials-and-clients.md)）实测：

1. `inferenceModels` 里塞一个非 Claude 风格 ID，看是整组被拒还是只忽略该条。
2. `inferenceModels` 里同时塞合法与非法 ID，观察菜单。
3. 若拒收，检查 Desktop 的日志/开发者工具里的报错原文。
4. 结论写回本票据，并同步修订 [spec 第 8 节](../spec.md)。

## Comments

- **2026-10-15（票 26 更正 §6 的归属，结论不变）**：§6 把「连接测试走 `/v1/messages/count_tokens`」记在 `provider: gateway` 名下，**归错了 provider**。逐字复核两个装机版本（`Claude_2.37337.1.0` 与 `2.31226.0.0`，`app.asar` 内该模块逐字一致）：
  - `provider: gateway` 的探针链只有 `/v1/models`（仅当 Models 列表为空）与 `POST <baseUrl>/v1/messages` + `{model, max_tokens:1, messages:[{role:"user",content:"."}]}`（头 `anthropic-version: 2023-06-01`），且 `Irn` **只看状态码、不读 body**。
  - `/v1/messages/count_tokens` 的唯一调用点是 **Azure AI Foundry provider**（label `"Azure AI Foundry"` 的那个 `probeInference`）；SDK 的 `countTokens()` 全包 **0 个调用点**。
  - body 只被 `completeOneShot`（`Rrn` + `Lrn`）解析：`content` 必须是数组（否则 `one-shot response had no content array`）、`stop_reason` 取字符串或 null。§6 里 `Orn` 的状态码分类（401/403 → auth；400/404 + `probedModel` → `model_rejected`；其余不是）复核无误。
  - 因此「两个端点都必须实现」仍然成立（`count_tokens` 对 Claude Code 是可选、缺失时退化为字符估算，对 Desktop 不是必需），只是理由不是「Desktop 的连接测试」。
  - 行为定案与实现见[票 26](26-grilling-count-tokens-and-non-streaming.md)；`spec.md` §8.1 第 3 条与 `START-HERE.md` 硬约束第 2 条已同步更正。

- 2026-10-14（票 15 端到端实测顺手拿到的观察，**只算半个答案，本票据仍未解析**）：
  - `inferenceModels` 填四个 `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`（都是完整 ID）时，Desktop 菜单**精确**显示这四项，无额外项、无拒收。四项都落在 cc-switch 说的白名单里，所以这条**两种情况都解释得通**，不构成判据。
  - **可用的正面证据**：官方「当每个条目都是完整模型 ID 时 app 自动跳过 `/v1/models` 调用」得到实测支持——整个启动与四轮问答期间，网关一次 `GET /v1/models` 都没收到。
  - **正式实验仍未做**：把非 claude-* 的 ID（例如 `mock-anthropic-text` 或 `deepseek-v4.1-flash`）混进 `inferenceModels`，看是整组 fail-all 还是只忽略该条。做这个实验只需改 `desktop-3p.ps1` 写出的 `inferenceModels` 后重启 Desktop，网关那边已经有现成的可路由条目。

## Answer

**结论：Desktop 确实校验模型 ID，但处置是「逐条剔除」，不是「整组 fail-all」；而且它认的规则比 cc-switch 的白名单宽得多。** 2026-10-14 解析，两条独立证据：装机版 `app.asar` 里的校验器本体 + 真机混合列表实验。

### 1. 校验器本体（本机装机版本）

版本：MSIX `Claude_2.37337.1.0_x64__pzs8sxrjxfjjc`，`app\version` = `44.7.0`。
位置：`…\app\resources\app.asar`（Electron asar，未压缩）→ `.vite/build/index.chunk-CHpJbW9_.js`（主 bundle；`index.pre.js` 是同一模块的另一份副本，名字不同、逻辑逐字相同）。asar 内偏移：`540113`（判据常量）、`542151`（校验器）、`701085`（剔除器）。

```js
Po  = ["sonnet","opus","haiku","fable","mythos"]
GSe = RegExp(`^(${Po.join("|")})(-[\\d.]+)?$`)                    // 裸档位名：sonnet / opus-4.5 / haiku-4
KSe = ["claude", ...Po, "anthropic"]                              // 子串包含表
qSe = /ark-code|astron|command-r|deepseek|doubao|gemini|gemma|glm|gpt|grok|hermes|hy3|kimi|lfm|
       \bling\b|llama|longcat|mimo|minimax|mistral|mixtral|moonshot|nemotron|openai|phi-|qianfan|
       qwen|tc-code|\bunic\b|yi-|stepfun|step-3|seed-|bytedance|hunyuan|granite|amazon\.nova|nova-|
       devstral|ministral|ernie|codex|arcee|trinity|abab|phi\d|\bk2\.|\bm2\.|jamba|arctic|solar|
       mercury|zamba|kat-coder|\bds-|dpsk/                         // 非 Anthropic 厂商黑名单
function Io(e){ let t=e.toLowerCase(); return qSe.test(t) ? !1 : GSe.test(t) || KSe.some(x=>t.includes(x)) }
function Lo(e,t){ if(e===void 0||GSe.test(t.toLowerCase()))return{ok:!0};
                  switch(e){ … case"gateway": case"mantle": return nCe(t) } }   // nCe = Io + 报错文案
```

- **`gateway` 与 `anthropic` 用同一个判据**：`gateway` 走 `nCe`，报错原文
  `expected a gateway model route referencing an Anthropic model (e.g. claude-sonnet-4-5, anthropic/claude-*). Name routes to match the underlying model.`
- **判定顺序**：先过黑名单（命中即非法、不可翻盘），再过「裸档位名」或「**子串**含 `claude`/`sonnet`/`opus`/`haiku`/`fable`/`mythos`/`anthropic`」。
- 推论（都实测过或由代码直接读出）：
  - `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku` → 合法（子串 `claude`）。
  - `claude-deepseek-v4-pro` → **非法**：黑名单 `deepseek` 赢过子串 `claude`（这正是 cc-switch 单测里那条 `assert!(!is_claude_safe_model_id("claude-deepseek-v4-pro"))` 的真实原因）。
  - `mock-anthropic-text`、`my-claude-thing` → **合法**（子串命中），与「必须 `claude-<角色>-*`」的第三方说法不符。
  - `deepseek-v4.1-flash` → **非法**（黑名单 `deepseek`）。

### 2. 处置：逐条剔除，没有 fail-all 分支

```js
function TMe(e,t,n){                       // e = 已解析的配置对象
  let r=e.inferenceProvider, i=e.inferenceModels;
  if(!r||!Array.isArray(i)) return e;
  let a = i.filter(entry => { let v=Lo(r,entry.name); … ; return v.ok });   // 逐条过滤
  return a.length!==i.length && a.length>0 && (e.inferenceModels=a), e      // 有存活者才回写
}
```

- 存活 ≥1 条 → 生效列表 = 存活者；被剔的那条只留一条 warn：
  `inferenceModels: "<名>" is not an Anthropic model and was removed from the list — <reason>`。
- **代码里没有任何「整组拒收 / fail-all」分支。** 全非法时 `a.length===0`，反而不回写（原样透传），到会话层才以「选中的模型被拒」处理：`validateSessionModel` → `Lo(...)` → `resolveSessionModel` 打 warn 后**回落到默认模型**（`{ok:!1}` 时 `JVe(i, void 0, …)`），也不是拒收整组。
- 写入路径也过同一个 `TMe`（`WL` 只把 `kind:"invalid"` 当阻断，模型问题归 `kind:"model"`，只做提示、不阻断保存）。
- `AllModelsRestrictedError`（"All of your configured models are currently unavailable."）来自 `resolveDefaultSessionModel` 的 `all_models_restricted`（模型存在但**全部被限制**），**与 ID 校验无关**——不要把它当成 fail-all 的证据。

### 3. 调用链：确认「写文件」这条路径也过滤（不是只在 UI 编辑时校验）

`%LOCALAPPDATA%\Claude-3p\configLibrary\<id>.json` → `UKn`（读文件）→ `Qwa`（`{channel:"local"}` 装配）→ `WLe`（解析 + 校验）→ `TMe`（剔除非法条目）→ `Vgn`（adminList → picker 条目）。`Qwa` 属于 3P 配置 handler 模块（`appliedConfigHandler` / `writeConfigHandler` 同一处）。

### 4. 真机混合列表实验（2026-10-14 20:09）

网关原型在 `127.0.0.1:8787` 上跑；profile 写入 6 条：

| # | `inferenceModels[].name` | 预测 | 实测 |
|---|---|---|---|
| 1–4 | `claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku` | 留 | 留 |
| 5 | `deepseek-v4.1-flash` | 剔（黑名单） | **剔** |
| 6 | `mock-anthropic-text` | 留（子串 `anthropic`） | **留** |

`%LOCALAPPDATA%\Claude-3p\logs\main.log`（原文）：

```
2026-10-14 20:09:33 [warn] inferenceModels: "deepseek-v4.1-flash" is not an Anthropic model and was removed
  from the list — expected a gateway model route referencing an Anthropic model (e.g. claude-sonnet-4-5,
  anthropic/claude-*). Name routes to match the underlying model.
2026-10-14 20:09:33 [info] [custom-3p] Model discovery: skipped (inferenceModels obviates discovery); picker = 5 (inferenceModels)
```

- **写 6 留 5** ⇒ 只剔掉那一条，整组未被拒收。用户当场打开模型选择器确认：`claude-fable / claude-opus / claude-sonnet / claude-haiku / mock-anthropic-text`，`deepseek-v4.1-flash` 不在。
- 全程网关**没有**收到 `GET /v1/models`（再次印证「全完整 ID ⇒ app 跳过自动发现」）。
- 实验后 profile 已按字节还原回 `{}`，Desktop 重启回 1P，网关进程已停。

### 5. 对两个待决问题的影响

1. **四个家族名不是唯一可行解，但「随意取名」也不行。** 名字必须 (a) 不含黑名单厂商词，(b) 含 `claude`/档位名/`anthropic` 之一，或本身就是裸档位名。给上游真实模型起名时仍建议「安全 route id + `labelOverride` 显示名」（cc-switch 那条做法依然成立，它的白名单只是真实规则的**保守子集**：安全，但过窄）。
2. **`/v1/models` 不需要按客户端做过滤。** 过滤发生在配置层（`TMe`），发现层用的是同一个判据（`R3t(e) → Io(e.id)`）；而我们全填完整 ID 时 app 根本不调 `/v1/models`。网关只需「四个家族名给 Claude 系、标准 `models` 数组给 Codex」。

### 6. 副产品：Desktop 的推理探针是非流式的（产品硬约束）

同一轮实验弹出的橙色横幅「Configured model not available — Your gateway couldn't serve `claude-haiku`」由这条链路产生，与模型 ID 校验无关：

- 健康探针 `Frn` → `POST <baseUrl>/v1/messages`，body 恰好是 `{"model":<探针模型>,"max_tokens":1,"messages":[一条 user 消息]}`，**没有 `stream` 字段**。
- 探针模型由 `zT(models)` 选：按 `haiku → sonnet → opus` 顺序取第一个命中的档位，再取名字最短的一条（本次即 `claude-haiku`）。
- 连接测试另走 `POST /v1/messages/count_tokens`（`probeInference`，body 只有 `{model, messages}`）。
- `Orn(status, label, detail)`：**401/403 → `auth`**；**400/404 且有 `probedModel` → `model_rejected`** → `ConfigHealth.state = config_model_rejected` → 界面横幅。
- 本原型「非流式一律 400」（`prototype/README.md` §4）正好踩中；Desktop 日志同时有
  `[title-gen] direct request failed { model: 'claude-haiku', kind: 'model_rejected', detail: 'Gateway rejected model "claude-haiku" (HTTP 400)', httpStatus: 400 }`。
- **要求**：产品网关必须实现非流式 `/v1/messages`（至少能答 `max_tokens:1` 的最小体）与 `/v1/messages/count_tokens`，否则 3P 界面会常驻这条「模型不可用」警告，即使流式聊天完全正常。已写进 [spec §8.1](../spec.md)。

### 7. 对 cc-switch 说法的更正

- 引用准确的部分：`app.asar` 里确实有 `["sonnet","opus","haiku","fable","mythos"]` 这个角色表（本机装机版逐字可见）。
- 不成立的部分：**「非法名触发 fail-all、整组被拒收」在本机装机版本（MSIX 2.37337.1.0 / app 44.7.0）里没有对应代码**——只有逐条剔除。cc-switch 引用的 `1.12603.1+` 与 MSIX 版本号体系不同，无法对齐；不排除旧构建更严，但当前版本必须以实测为准。
- 其校验函数 `is_claude_safe_model_id` 仍是**安全的保守子集**（要求 `claude-<角色>-*`），照它写入不会出问题，只是会拒掉本来合法的名字。
