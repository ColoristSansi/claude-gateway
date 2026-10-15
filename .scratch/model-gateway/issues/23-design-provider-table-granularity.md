# 供应商表：认证与可用 wire 的粒度

Type: grilling
Status: resolved
Blocked by: —

## Question

供应商表的 `auth` 与「这个模型能在哪些 wire 上用」这两件事，粒度定在哪一层——上游、上游×wire，还是模型？

票 14 原型用穿了两个假设：

1. **「一个上游一份凭据」不成立。** opencode Go 的同一个 Bearer key：`/chat/completions` 正常 200，`/messages` 返回 401 `Go requires a credential`。即**同一上游按 wire 用的凭据不同**。现在表里 `auth` 挂在上游上，改用 Anthropic 直通就必须换模型或换凭据。
2. **「一个上游一种 wire 集合」不成立。** commandcode 每个模型自带 `supported_endpoints`（如 `deepseek/deepseek-v4.1-flash` = `/chat/completions` + `/responses`，排除 `/messages`），opencode Go 则是「每个模型只由一个 API family 提供服务」，发错端点 400 `ModelProtocolUnsupported`。票 09 已定「协议记在虚拟模型上」，但表里没有任何字段能**校验**这个绑定对不对，只能等上游报错。

要定的：表结构怎么表达这两件事；`/v1/models` 的 `supported_endpoints` 与 models.dev 的先验怎么灌进来；校验在配置期做还是请求期做；`quirks` 里那条 `ClaudeModelsOnAnthropicOnly` 是不是就该被这个字段取代。

## 背景

- 实测记录见 `.scratch/model-gateway/prototype/README.md` §2（必须进代码的差异）与 §3（上游事实）。
- 相关已解析票据：[协议策略与声明式供应商表](09-design-protocol-and-provider-table.md)、[原型：协议转换核心 + mock 上游](14-prototype-conversion-core.md)。

## Answer

分两层：**认证按「上游 × 出站 wire」各存一份**；**逐模型的可用 wire 按「上游 × 真实模型」一条目录记录**，虚拟模型引用它。校验三层落地，旧的 `ClaudeModelsOnAnthropicOnly` quirk 删除。

### 1. 认证：按「上游 × wire」各存一份

上游仍是一张卡（一个 `baseUrl` + 一套路径模板），但 `auth` 从单值变成按 wire 各存一份（`scheme` + 凭据引用）；某条 wire 没填就**继承上游默认**。理由是实测过的：opencode Go 同一把 Bearer key 打 Chat 面 200、打 Anthropic 面 401 `Go requires a credential`，一上游一凭据不成立。

- **不拆成两个上游实体**（`opencode-chat` / `opencode-anthropic`）。拆了确实与术语表原句「一个地址、一种协议、一份凭据」吻合，但引用计数、上游卡片、四步向导、换 key 都要按面重复一遍，而用户在界面上认的是「一家上游」。术语表那句话按此**更正**（见第 7 节）。
- **不建独立凭据表**。现在没有「两个上游共用一把 key」的真实场景，属于为将来加的表。
- 前端交互：上游向导第二步只问一份凭据；当勾选的模型或绑定的 wire 涉及另一种面时，才展开第二个 key 输入框（并注明该面凭据与上面不同，opencode Go 实测如此）。

### 2. 可用 wire：`(上游, 真实模型)` 一条目录记录

新建**上游模型目录**（`upstream_models`）：`upstream_id` + `model_id` + `supported_wires` + `source`。

- 取值域**三个值**：`anthropic` / `openai-chat` / `openai-responses`。原型现在只有两个值（`Wire::Anthropic` / `Wire::OpenAi`）与 `path_anthropic` / `path_openai` 两个路径模板，要拆成三个——否则表达不了「一个模型只答 `/chat/completions` 不答 `/responses`」（commandcode 有 8 个这样的模型），Codex 链路要的 Responses 出站也没处落。
- `source` 取值 `manual` / `upstream` / `doc`（见第 5 节）；**空集合表示「未声明」**，是一个正常状态，不是错误。
- **虚拟模型绑定引用目录条目**，再在条目声明的集合里选一个 wire。目录里没有的模型，允许「手工新增一条目录条目」再绑——这样模型 ID 打错字不会变成「未声明」而被静默放行。

### 3. 校验：配置期主拦 + 请求期兜底 + 自检为准

1. **配置期主拦**：绑定时只允许选 `supported_wires` 里的值（下拉里不出现的选不了）；未声明的模型可自由选，旁边标「未声明，靠自检」。
2. **请求期兜底**：保留原型现在的位置与形状——错配早拒 400 并记日志（`prototype/src/upstream.rs:41`）。上游的 400 不如本地这句清楚。
3. **自检为准**：声明拿不到时（opencode、千问的 Anthropic 面）只有自检说了算。

票 17 §10 挂起的「协议列能否在表内手改」就此结案：**可改，但只限声明的值**。票 17 的两个挂起项（第 45、90 行）指向本票，到此关闭。

### 4. 旧 quirk 删除

`quirk::ClaudeModelsOnAnthropicOnly` 从枚举里删掉，由 `supported_wires` 接管：**声明式表里不再有 quirk 表达「哪些端点面存在」**。连带要改：

- 票 19 的 `quirk::all()` 双向守卫（枚举值 ⇄ fixture 标签）去掉这一项。
- 原型负例 3 `cc-opus-on-chat` 的判据从「quirk 早拒」改成「`supported_wires` 不含 `openai-chat` → 配置期就不许绑，请求期早拒」。

### 5. 灌入方式与来源优先级

**拉取时机**：上游向导「拉模型」那一步拉一次 `/v1/models` 存库（含 `supported_endpoints`），之后可手动重拉；**请求期不实时查**（opencode 学不到东西，千问的 Anthropic 面根本没有该端点）。

**来源优先级**：`manual`（锁定）> 上游 `/v1/models` > 随包内置文档表（`doc`）> 未声明。

- 上游声明的是**路径**（`/chat/completions`、`/responses`、`/messages`），按 `pathTemplates` **反查**映射成 wire，不新增映射表。查不到的取值**忽略并在拉模型结果里记一条警告**（例：上游报 `/v1/messages` 而我们的模板是 `/messages`）。
- **models.dev 不参与 wire 判定**。它只到 provider 级（`npm` / `api` / `env`），没有端点级字段，仍只用来预填 baseUrl 与协议倾向（票 09 已定）。
- **opencode 的逐模型 wire 来自随包内置的文档表**（照 [research/providers.md](../research/providers.md) §3 的 Go 官方表）+ 手工覆盖。**不猜名字**：实测 ID 前缀与 `-free` / `-contributor` 后缀都推不出 family。
- **重拉时手改过的条目锁定**（`source = manual`），只新增与更新其余条目；界面标「已锁定」并可解锁。
- **opencode「拉模型」两份清单合并**：带凭据那份（19）做默认勾选，无凭据那份（85）合并进列表、标「仅免费面可见」、默认不勾选；到底能不能调用交给自检。

### 6. 本次实测事实（子代理，2026-10-14；key 全程掩码）

| 端点 | 结果 |
| --- | --- |
| commandcode `GET /provider/v1/models` | 200，89 个模型；**89/89 带 `supported_endpoints`**，值是协议路径数组，全部取值仅 3 种（`/chat/completions`、`/responses`、`/messages`），70/89 是多值 |
| opencode Go `GET /inference/go/v1/models` | 200，40 个；字段只有 `id` / `object` / `created` / `owned_by`，**无任何端点字段**；环境里的 `/zen/go/v1/models` 返回同一份 ID 列表 |
| opencode Console `GET /inference/v1/models` | 带 key 200 / 19 个，不带 key 200 / 85 个（19 是 85 的子集）；同样无端点字段 |
| opencode `GET /inference/anthropic/v1/models` | 404，body 为空（该协议族不提供模型发现） |

样例（原样、已裁剪）：

```json
{"id":"claude-sonnet-5-5","object":"model","owned_by":"command-code","name":"Claude Sonnet 5.5","context_length":1000000,"supported_endpoints":["/messages"]}
{"id":"gpt-6-astra","object":"model","owned_by":"command-code","name":"GPT-6 Astra","context_length":1050000,"supported_endpoints":["/chat/completions","/responses"]}
{"id":"deepseek-v4-flash","object":"model","owned_by":"opencode"}
```

分布：`chat+responses` 70 个、仅 `/messages` 11 个、仅 `/chat/completions` 8 个。

→ 一句话：**commandcode 能从上游自动学到逐模型可用 wire；opencode（Go 与 Console 两面）不能**，只能靠内置文档表与手工维护。

### 7. 术语与文档同步

- `GLOSSARY.md`：「上游」定义由「一个地址、一种协议、一份凭据」更正为按出站协议分凭据的现状；新增「上游模型目录」与「可用 wire 集合」两个词。
- `spec.md`：§6 的声明式表样例（认证按 wire、删掉 Claude 模型那条 quirk）、§7 的绑定与目录关系、§9 的表清单（加 `upstream_models`）、§10 的协议列结论。
- `docs/adr/0004-provider-table-granularity.md`：本票的粒度定案按仓库 ADR 形状归档（三条判据都满足）；票 27 的测试策略 ADR 顺延为 `0005`，该票正文已同步改号。
- `map.md`：Decisions so far 追加本票；Not yet specified 的「模型自动发现的缓存与失败降级」补上 opencode 带/不带凭据清单不一致这条待查事实。

### 8. 遗留（未进本票，留给后续）

- **为什么 opencode 带凭据的清单（19）比不带凭据（85）少**，本次没探明；它对「默认勾选哪份」只是外观层面的影响，合并策略两种情形都成立。已记进地图的 Not yet specified。
- 内置文档表怎么随版本更新（上游上新模型后）暂不动手，等实现时再定。
