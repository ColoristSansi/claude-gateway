# Ollama 辅助端点契约：`GET /api/version`、`POST /api/show`、`GET /api/ps`

调研日期：2026-10-15（AFK）
范围：给一个**不持有权重文件、不装载模型**的兼容网关实现用。只查形状与「哪些字段必须给 / 只能编 / 应留空」的落点，不写实现。
证据分级：**【官方文档】**（`docs/api.md`、`docs/openapi.yaml`）/ **【官方源码】**（`api/types.go`、`server/*.go`、`types/*`、`version/version.go`）/ **【推断】**（从源码行为必然推出、但源码没有一句话直说）/ **【未证实】**（未实测）。
冲突裁决：**docs 与源码冲突时以源码为准，并在当节单独指出**。

上游快照：`https://raw.githubusercontent.com/ollama/ollama/main/`（main 分支，2026-10-15 抓取）。行号引用的是本地抓取缓存，非仓库稳定行号。

---

## 0. 结论摘要（先看这个）

| 端点 | 必须给（Go encoder 一定会写出的键） | 只能编（无权重时没有真值） | 应当留空 / 整键省略 |
| --- | --- | --- | --- |
| `GET /api/version` | `version`（string） | 版本串本身 | 没有别的键 |
| `POST /api/show` | `details`（对象，含 6 个无 omitempty 子键）、`model_info`、`modified_at` | `details.family`、`details.parameter_size`、`details.quantization_level`、`model_info.*` 的真实值 | `modelfile` / `parameters` / `template` / `license` / `system` / `renderer` / `parser` / `capabilities` / `thinking` / `messages` / `tensors` / `projector_info` / `requires` / `manifests` |
| `GET /api/ps` | `models`（数组，本网关取 `[]`） | 列表为空时**没有任何** per-model 字段要编 | 空列表合法，`models: []` 即可；非空时 `details.*` 同理留空串 |

一句话：**只有 `show` 的 `details` / `model_info` / `modified_at` 是「必须出现的键」**，其余全是可选键，网关可以整键省略而不破坏形状；`ps` 空列表是最诚实的形态。

---

## 1. 来源清单表

| # | 来源 | 用途 | 等级 |
| --- | --- | --- | --- |
| S1 | `docs/api.md`（Show Model Information / List Running Models / Version 三节） | 逐字示例、文档侧字段说明 | 【官方文档】 |
| S2 | `docs/openapi.yaml`（`ShowRequest`/`ShowResponse`/`Ps`/`PsResponse`/`VersionResponse`/`ErrorResponse` + `/api/show`、`/api/ps`、`/api/version` paths） | 必填标注、schema、示例 | 【官方文档】 |
| S3 | `api/types.go` | `ShowRequest`/`ShowResponse`/`ProcessModelResponse`/`ProcessResponse`/`ModelDetails`/`StatusError` 的 json tag 与 omitempty | 【官方源码】 |
| S4 | `server/routes.go` | 路由注册（含 HEAD）、`ShowHandler`、`GetModelInfo`、`PsHandler`、`getExistingName`、`/api/version` 内联 handler | 【官方源码】 |
| S5 | `server/show_manifest.go` | `writeShowError`（show 的状态码映射）、`GetAllManifestsInfo` | 【官方源码】 |
| S6 | `version/version.go` | `var Version string = "0.0.0"` | 【官方源码】 |
| S7 | `scripts/env.sh` + `.github/workflows/release.yaml` | `-X ...version.Version=$VERSION` 的注入方式与取值来源 | 【官方源码】 |
| S8 | `types/model/capability.go` | capability 取值域（9 个） | 【官方源码】 |
| S9 | `types/model/thinking.go` | `Thinking{values,default}` 形状；`default` 无 omitempty | 【官方源码】 |
| S10 | `server/images.go` | `publicCapabilities()`、`Model.Thinking()` 的调用面（辅助理解 show 字段来源） | 【官方源码】 |
| S11 | `types/errtypes/errtypes.go` | `InvalidModelNameErrMsg = "invalid model name"` | 【官方源码】 |

无二手来源（无博客、无第三方 SDK 文档）。未实测任何请求。

---

## 2. `GET /api/version`

### 2.1 请求形状

| 项 | 值 | 证据 |
| --- | --- | --- |
| 方法 / 路径 | `GET /api/version` | 【官方源码】S4 |
| 额外方法 | **`HEAD /api/version` 也存在**，同一 handler | 【官方源码】S4（`r.HEAD("/api/version", ...)` / `r.GET("/api/version", ...)`） |
| 查询参数 | 无 | 【官方源码】S4 |
| 请求体 | 无（未解析 body） | 【官方源码】S4 |
| 认证 | 无 | 【官方源码】S4（无中间件） |
| 405 行为 | 其他方法（如 `POST`）→ 405，`gin.HandleMethodNotAllowed = true` | 【官方源码】S4 |

源码原文（逐字）：

```go
r.HEAD("/api/version", func(c *gin.Context) { c.JSON(http.StatusOK, gin.H{"version": version.Version}) })
r.GET("/api/version", func(c *gin.Context) { c.JSON(http.StatusOK, gin.H{"version": version.Version}) })
```

### 2.2 响应逐字示例（官方）

`docs/api.md` 原文：

```json
{
  "version": "0.5.1"
}
```
【官方文档】S1

`docs/openapi.yaml` 的 `example`（同一个端点的另一个示例值）：

```yaml
example:
  version: "0.12.6"
```
【官方文档】S2

两个示例值不同 → 示例只是示例，**不是契约值**。

### 2.3 字段级表

| 字段 | 类型 | omitempty | 必填 | 说明 |
| --- | --- | --- | --- | --- |
| `version` | string | 无（`gin.H` 手写 map，永远写出） | 必须给 | 值为 `version.Version` |

响应体是 `gin.H{"version": version.Version}`，**唯一键**；不存在第二个键。【官方源码】S4
OpenAPI schema 一致：`VersionResponse: {version: string}`（无 `required` 标注，但实现恒有该键）。【官方文档】S2

### 2.4 非 200

源码里该 handler **没有任何错误分支**：恒 200 + JSON。【官方源码】S4
唯一非 200 来源是路由层：方法不允许 → 405（body 由 gin 生成，非 `{"error":...}` 形状）；路径不存在 → 404。错误体形状参见 §5。【推断】

### 2.5 落点判断：`version` 值从哪来？是常量吗？

**不是常量，是构建期注入的包级变量。**

```go
package version

var Version string = "0.0.0"
```
【官方源码】S6

注入方式（两处，一致）：

- `scripts/env.sh`：`export VERSION=${VERSION:-$(git describe --tags --first-parent --abbrev=7 --long --dirty --always | sed -e "s/^v//g")}`，随后 `GOFLAGS="'-ldflags=-w -s "-X=github.com/ollama/ollama/version.Version=$VERSION" ...'"`【官方源码】S7
- `.github/workflows/release.yaml`：`-X=github.com/ollama/ollama/version.Version=${GITHUB_REF_NAME#v}`，即 **release tag 去掉 `v`**（如 tag `v0.13.3` → `0.13.3`）【官方源码】S7

所以官方二进制的 `version` = 「这个二进制的 git tag」，**本地 `go build` 不注入时是 `"0.0.0"`**。【官方源码】S6+S7

该值的真实消费点（决定网关怎么填）：

1. `server/images.go` 的 `checkPullRequires(requires, version.Version)`：模型 config 里的 `requires` 会与该值做 semver 比较；**`clientVersion == "0.0.0"` 时直接跳过检查**（源码注释/分支原文：`if clientVersion == "0.0.0" { return nil }`）。【官方源码】S10
2. registry 请求的 `User-Agent: ollama/<version> (...)`。【官方源码】S10
3. 服务启动日志 `Listening on %s (version %s)`。【官方源码】S4

对网关的含义：

- 用户已定案回 `"0.13.3"`，格式合法（`v`+该值可被 `semver.Canonical` 解析）。【推断，基于 S10 的 `semver.Canonical("v"+clientVersion)` 用法】
- 必须意识到**这是在声称自己等价于 Ollama 0.13.3 的 API 面**：一旦回 `0.13.3`，走 `/api/pull`/`/api/create` 的客户端若拿到带 `requires` 的模型，会真的按 0.13.3 做门槛判断（本网关不持有权重，通常不走这条链，故影响有限）。回 `0.0.0` 反而会**关掉**该门槛，但那不是「更像 Ollama」，而是「更像未注入版本的本地构建」。【推断，基于 S10 分支】
- 诚实性上无「更诚实的替代值」可选：该字段语义就是「服务端版本」。方案是要么固定回一个与所实现 API 面对齐的 Ollama 版本号（并把它当作「我实现了到 0.13.3 为止的契约」的声明），要么明说网关版本（如 `"0.0.0"` 或自定义串），后者会让按版本 gating 的客户端走「未知版本」路径。**这是一条策略选择，不是形状问题。**

---

## 3. `POST /api/show`

### 3.1 请求形状

| 项 | 值 | 证据 |
| --- | --- | --- |
| 方法 / 路径 | `POST /api/show`（仅 POST；`GET /v1/models/:model` 通过 `middleware.RetrieveMiddleware()` 复用同一 handler） | 【官方源码】S4 |
| 查询参数 | **无**（`verbose` 是 body 字段，不是 query） | 【官方源码】S4 |
| 请求体 | 必需。空 body → 400 `{"error":"missing request body"}`；JSON 语法/类型错 → 400 `{"error":"<bind error>"}` | 【官方源码】S4 |

`ShowRequest`（`api/types.go` 逐字摘录，含 tag）：

```go
type ShowRequest struct {
	Model        string `json:"model"`
	Runner       string `json:"runner,omitempty"`
	AllManifests bool   `json:"all_manifests,omitempty"`
	System       string `json:"system"`

	// Template is deprecated
	Template string `json:"template"`
	Verbose  bool   `json:"verbose"`

	Options map[string]any `json:"options"`

	// Deprecated: set the model name with Model instead
	Name string `json:"name"`
}
```
【官方源码】S3

逐字段：

| 字段 | 类型 | omitempty | 必填 | 实际作用（源码判定） |
| --- | --- | --- | --- | --- |
| `model` | string | 无 | **是（`model` 或 `name` 二选一）** | 主字段。`req.Model != ""` 直接用；否则若 `req.Name != ""` → `req.Model = req.Name`；两者都空 → 400 `{"error":"model is required"}`。【官方源码】S4 |
| `name` | string | 无 | 否（**已废弃的别名**） | 只作为 `model` 的兜底。注释原文：`Deprecated: set the model name with Model instead`。【官方源码】S3+S4 |
| `verbose` | bool | 无 | 否，**默认 `false`** | 见 3.4 的作用说明。【官方源码】S3+S4 |
| `runner` | string | 有 | 否 | 选择 manifest list 子项；取值经 `normalizeRunner`，非法 → 400 `unknown runner "x"`；`all_manifests` 与 `runner` 同时给 → 400 `runner cannot be used with all_manifests`。【官方源码】S4 |
| `all_manifests` | bool | 有 | 否 | 改走 `GetAllManifestsInfo`，返回**另一套**响应形状 `ShowManifestsResponse{manifests:[...],license?}` **而不是** `ShowResponse`。【官方源码】S5。⚠️ 本网关若不实现多 runner 变体，应拒绝或忽略该字段（不返回该形状）。 |
| `system` | string | 无 | 否 | 非空时**覆盖**响应用的 system（`m.System = req.System`）。【官方源码】S4 |
| `options` | object | 无 | 否 | 合并进 `m.Options`，影响 `parameters` 与 `modelfile` 文本。【官方源码】S4 |
| `template` | string | 无 | 否 | **请求侧已废弃且 handler 不再读取**（字段仍在，`GetModelInfo` 无引用）。【官方源码】S3+S4 |

**关键回答（父 agent 明确问的）**：请求体用 **`model`**；`name` 只是被接受的废弃别名（同一 handler 里显式兜底）。二者都给时 `model` 胜出（`if req.Model != ""` 分支在前）。OpenAPI 也只把 `model` 标为 `required: [model]`。【官方文档】S2

`docs/api.md` 的 Parameters 只写：

```
- `model`: name of the model to show
- `verbose`: (optional) if set to `true`, returns full data for verbose response fields
```
【官方文档】S1 —— **文档没把 `model` 标为必填**，源码里缺它是 400。以源码为准。【官方源码】S4

### 3.2 响应逐字示例（官方）

`docs/api.md`「Show Model Information」响应原文（**注意：文档这里是 JSON5，键未加引号、带尾逗号**）：

```json5
{
  modelfile: '# Modelfile generated by "ollama show"\n# To build a new Modelfile based on this one, replace the FROM line with:\n# FROM llava:latest\n\nFROM /Users/matt/.ollama/models/blobs/sha256:200765e1283640ffbd013184bf496e261032fa75b99498a9613be4e94d63ad52\nTEMPLATE """{{ .System }}\nUSER: {{ .Prompt }}\nASSISTANT: """\nPARAMETER num_ctx 4096\nPARAMETER stop "\u003c/s\u003e"\nPARAMETER stop "USER:"\nPARAMETER stop "ASSISTANT:"',
  parameters: 'num_keep                       24\nstop                           "<|start_header_id|>"\nstop                           "<|end_header_id|>"\nstop                           "<|eot_id|>"',
  template: "{{ if .System }}<|start_header_id|>system<|end_header_id|>\n\n{{ .System }}<|eot_id|>{{ end }}{{ if .Prompt }}<|start_header_id|>user<|end_header_id|>\n\n{{ .Prompt }}<|eot_id|>{{ end }}<|start_header_id|>assistant<|end_header_id|>\n\n{{ .Response }}<|eot_id|>",
  details: {
    parent_model: "",
    format: "gguf",
    family: "llama",
    families: ["llama"],
    parameter_size: "8.0B",
    quantization_level: "Q4_0",
  },
  model_info: {
    "general.architecture": "llama",
    "general.file_type": 2,
    "general.parameter_count": 8030261248,
    "general.quantization_version": 2,
    "llama.attention.head_count": 32,
    "llama.attention.head_count_kv": 8,
    "llama.attention.layer_norm_rms_epsilon": 0.00001,
    "llama.block_count": 32,
    "llama.context_length": 8192,
    "llama.embedding_length": 4096,
    "llama.feed_forward_length": 14336,
    "llama.rope.dimension_count": 128,
    "llama.rope.freq_base": 500000,
    "llama.vocab_size": 128256,
    "tokenizer.ggml.bos_token_id": 128000,
    "tokenizer.ggml.eos_token_id": 128009,
    "tokenizer.ggml.merges": [], // populates if `verbose=true`
    "tokenizer.ggml.model": "gpt2",
    "tokenizer.ggml.pre": "llama-bpe",
    "tokenizer.ggml.token_type": [], // populates if `verbose=true`
    "tokenizer.ggml.tokens": [], // populates if `verbose=true`
  },
  capabilities: ["completion", "vision"],
}
```
【官方文档】S1

`docs/openapi.yaml` 的同端点 200 示例（**不同字段集**，含 `thinking` / `modified_at`，无 `modelfile` / `template` / `system` / `messages`）：

```yaml
example:
  parameters: "temperature 0.7\nnum_ctx 2048"
  license: "Gemma Terms of Use \n\nLast modified: February 21, 2024..."
  capabilities:
    - "completion"
    - "thinking"
    - "vision"
  thinking:
    values:
      - false
      - true
    default: true
  modified_at: "2025-08-14T15:49:43.634137516-07:00"
  details:
    parent_model: ""
    format: "gguf"
    family: "gemma4"
    families:
      - "gemma4"
    parameter_size: "8.0B"
    quantization_level: "Q4_K_M"
  model_info:
    gemma4.attention.head_count: 8
    # …（下略 30+ 个 kv）
```
【官方文档】S2

两个官方示例的键集不同，**只有 `details` / `model_info` 在两个示例里都出现** —— 与源码的「无 omitempty」判定完全吻合（见 3.3）。

### 3.3 字段级表（以源码为准）

`ShowResponse` 逐字（`api/types.go`）：

```go
type ShowResponse struct {
	Thinking      *model.Thinking    `json:"thinking,omitempty"`
	License       string             `json:"license,omitempty"`
	Modelfile     string             `json:"modelfile,omitempty"`
	Parameters    string             `json:"parameters,omitempty"`
	Template      string             `json:"template,omitempty"`
	System        string             `json:"system,omitempty"`
	Renderer      string             `json:"renderer,omitempty"`
	Parser        string             `json:"parser,omitempty"`
	Details       ModelDetails       `json:"details,omitempty"`
	Messages      []Message          `json:"messages,omitempty"`
	RemoteModel   string             `json:"remote_model,omitempty"`
	RemoteHost    string             `json:"remote_host,omitempty"`
	ModelInfo     map[string]any     `json:"model_info"`
	ProjectorInfo map[string]any     `json:"projector_info,omitempty"`
	Tensors       []Tensor           `json:"tensors,omitempty"`
	Capabilities  []model.Capability `json:"capabilities,omitempty"`
	Manifests     []ManifestSummary  `json:"manifests,omitempty"`
	ModifiedAt    time.Time          `json:"modified_at,omitempty"`
	Requires      string             `json:"requires,omitempty"`
}
```
【官方源码】S3

`ModelDetails` 逐字：

```go
type ModelDetails struct {
	ParentModel       string   `json:"parent_model"`
	Format            string   `json:"format"`
	Family            string   `json:"family"`
	Families          []string `json:"families"`
	ParameterSize     string   `json:"parameter_size"`
	QuantizationLevel string   `json:"quantization_level"`
	ContextLength     int      `json:"context_length,omitempty"`
	EmbeddingLength   int      `json:"embedding_length,omitempty"`
	Runner            string   `json:"runner,omitempty"`
}
```
【官方源码】S3

| 字段 | 类型 | omitempty | 实际必填 | 无权重时怎么填最诚实 |
| --- | --- | --- | --- | --- |
| `modelfile` | string | **有** | 否（可整键省略） | **省略**。真实值是服务端拼出来的多行文本（`# Modelfile generated by "ollama show"` + `FROM <blob 路径>` + `TEMPLATE`/`PARAMETER` 行），必然引用本机 blob 路径与真实模板；网关没有这些，编出来就是伪造。 |
| `parameters` | string | **有** | 否 | **省略**（无真实参数层时；真实服务端此处也可能是空串→整键消失）。若确要表达默认采样参数，宁可返回 `""` 也别写假 `num_ctx`。 |
| `template` | string | **有** | 否 | **省略**。真实值是 GGUF `tokenizer.chat_template` 或模板层内容；网关若不真正用该模板渲染，写出来会让客户端据此自行渲染并向出站模型提交错误 prompt。 |
| `details` | object | 有（**但对 struct 无效**） | **是（一定出现）** | 必须给对象；子键见下。 |
| `details.parent_model` | string | 无 | **是** | `""`（真实服务端在非派生模型上也是 `""`，所以空串是**真值**而非撒谎） |
| `details.format` | string | 无 | **是** | 建议 `""`。写 `"gguf"` 是谎报（网关不持 GGUF 权重）。空串在形状上合法，客户端一般只做展示。 |
| `details.family` | string | 无 | **是** | 能确定上游模型家族时填真值（如 `"llama"`）；不确定就 `""`。不要用 `""` 之外的占位串。 |
| `details.families` | []string | 无 | **是** | `[]` 或 `null`。Go 对 nil slice 输出 `null`（真服务端在 config 缺 families 时也确实会输出 `null`）。二者都是官方可能产生的形状。 |
| `details.parameter_size` | string | 无 | **是** | 只有上游确实给了参数规模才填（`"8.0B"`）；否则 `""`。 |
| `details.quantization_level` | string | 无 | **是** | 同上，否则 `""`（真服务端也可能因 `unknown` 而未填充）。 |
| `details.context_length` | int | **有** | 否 | 真实值未知则省略（注意：给 `0` 也会被编码出来，因为 int 的 omitempty 会丢掉 0 —— `0` 与省略等价）。 |
| `details.embedding_length` | int | 有 | 否 | 同上 |
| `details.runner` | string | 有 | 否 | **省略**（`ggml`/`llama.cpp`/`mlx` 是 Ollama 的运行器身份，网关没有） |
| `model_info` | object | **无** | **是（一定出现）** | 至少 `{}`。源码显式保证非 nil，注释原文：`// Several integrations crash on a nil/omitempty+empty ModelInfo, so by default we return an empty map.` → `resp.ModelInfo = make(map[string]any)`。【官方源码】S4 这是**唯一被源码点名的兼容性硬要求**：空对象即可，别给 `null`，也别编造 kv（`general.architecture` 之类会被客户端当作真元数据）。 |
| `license` | string | **有** | 否 | **省略**。许可证文本是法律声明，编造即撒谎。 |
| `system` | string | 有 | 否 | 只有上游真有 system prompt 才填；否则省略。 |
| `renderer` / `parser` | string | 有 | 否 | **省略**（Ollama 内部渲染器/解析器名，网关无对应实现） |
| `capabilities` | []string | **有** | 否 | **省略**（整键消失）比填 `["completion"]` 更诚实：网关不装载模型，无法保证任何能力。注意官方合法取值只有 9 个：`completion`/`tools`/`insert`/`vision`/`embedding`/`thinking`/`image`/`audio`/`decision`（`types/model/capability.go`）。【官方源码】S8 ⚠️ 若某个客户端「没有 capabilities 就认为模型不可用」，那也应由产品决定是否按上游真实能力填写，而不是默认填 `completion`。 |
| `thinking` | object \| 缺省 | 有 | 否 | **省略**。形状为 `{"values":[...],"default":...}`，`values`/`default` 均无 omitempty（`types/model/thinking.go`）→**一旦给这个键，两个子键都必须给**，且 `values` 只能是 bool/string 混合。网关无真实 thinking 契约时给这个键等于宣称支持某些档位。另：源码在 `capabilities` 含 `decision` 时不返回 `thinking`。【官方源码】S9+S4 |
| `messages` | []Message | 有 | 否 | 省略（空 slice 也会因 omitempty 消失） |
| `tensors` | []Tensor | 有 | 否 | 省略；仅在 `verbose=true` 且能读到权重时才有 |
| `projector_info` | object | 有 | 否 | 省略（多模态投影器元数据） |
| `manifests` | []ManifestSummary | 有 | 否 | 省略；源码仅在子项 ≥2 时返回（单子项 manifest list 与普通模型同形）【官方源码】S5 |
| `modified_at` | time.Time | 有（**但对 struct 无效**） | **是（一定出现）** | 真实值为 manifest 文件 mtime。**网关必须给一个值**：给零值会编码成 `"0001-01-01T00:00:00Z"`（形状合法、语义「未知」），或给网关虚拟 manifest 的 mtime。注意这就是「必须编」的字段——没有真实文件时不存在「真值」。【官方源码】S3+S4 |
| `requires` | string | 有 | 否 | 省略（那是模型声明的最低 Ollama 版本） |
| `remote_model` / `remote_host` | string | 有 | 否 | 省略（网关自身不是 Ollama 的 remote 模型机制） |

**Go `encoding/json` 语义提醒（本节的判定基础）**：`omitempty` 对 **struct 值类型无效**（`ModelDetails`、`time.Time` 均如此），对 nil slice/map 有效但输出 `null` 而非省略（当该键没有 omitempty 时）。所以「必须给」的判定不能只看 tag。【官方源码】S3

### 3.4 `verbose` 的默认与作用

- 默认值：`false`（`bool` + 无 omitempty，缺省即 false）。【官方源码】S3
- 作用一：GGUF 路径把 `getModelData(m.modelPaths(), req.Verbose)` 的 `maxArraySize` 从 `0` 改成 `-1`，从而**把 tokenizer 大数组读出来**（`tokenizer.ggml.tokens`/`merges`/`token_type` 等；默认时这些数组是 `[]`/`null`）。【官方源码】S4（`maxArraySize := 0; if verbose { maxArraySize = -1 }`）
- 作用二：`verbose=true` 时填充 `tensors`（`resp.Tensors = [...]`，逐个张量的名/类型/形状）。【官方源码】S4
- 它**不影响** `details` 的存在性（`details` 恒在），也不影响 `model_info` 的存在性。【官方源码】S3+S4
- 对网关的含义：既然不读权重，`verbose` 只应影响「是否有 `tensors`」，而网关永远没有 `tensors` → **永远不因 verbose 改变输出形状**（可安全接受并忽略）。【推断】

### 3.5 非 200 状态码与错误体

错误体形状统一为 `{"error": "<字符串>"}`（`api.StatusError.ErrorMessage` 的 tag 是 `json:"error"`；handler 全部用 `gin.H{"error": ...}`）。【官方源码】S3+S4

`ShowHandler` / `writeShowError` 的完整映射：

| 触发条件 | 状态码 | body | 证据 |
| --- | --- | --- | --- |
| 空 body（`io.EOF`） | 400 | `{"error":"missing request body"}` | 【官方源码】S4 |
| body 不是合法 JSON | 400 | `{"error":"<gin bind 错误文本>"}` | 【官方源码】S4 |
| `model` 与 `name` 都缺 | 400 | `{"error":"model is required"}` | 【官方源码】S4 |
| 模型名不合法（未限定名） | 400 | `{"error":"invalid model name"}`（`errtypes.InvalidModelNameErrMsg`） | 【官方源码】S4+S11 |
| 模型不存在 | **404** | `{"error":"model '<请求里的名字>' not found"}` | 【官方源码】S5（`writeShowError` 的 `os.IsNotExist` 分支）+【推断】 |
| `manifest.ErrNoCompatibleManifest`（runner 变体缺失） | 404 | `{"error":"<err.Error()>"}`（源码注释：客户端 show-then-pull 把 404 当「去拉取」） | 【官方源码】S5 |
| 非法 `runner` | 400 | `{"error":"unknown runner \"x\""}` | 【官方源码】S4 |
| `all_manifests` + `runner` 同时给 | 400 | `{"error":"runner cannot be used with all_manifests"}` | 【官方源码】S4 |
| 其他（读 manifest/权重的内部错误） | 500 | `{"error":"<err.Error()>"}` | 【官方源码】S5 |

关于「模型不存在 → 404」的**推断链**（源码里没写死这一步）：
`ShowHandler` 先调 `getExistingName(name)`；该函数**从不返回 `os.ErrNotExist`**（源码注释：*"If no parts are found, the input name is returned as is"*，只有非 NotExist 错误才向上抛）。所以不存在的模型会**穿过** 3.1 那个 400 分支，落到 `GetModelInfo` → `manifest.ParseNamedManifestForRunner` / `getModelData` 抛 `os.ErrNotExist` → `writeShowError` 的 `os.IsNotExist` 分支 → **404 `model '<name>' not found`**。【官方源码】S4（`getExistingName` 实现）+【推断】

> ⚠️ 源码里另有一条**看起来会误伤**的路径：`ShowHandler` 对 `getExistingName` 的返回值做了 `c.JSON(400, {"error": err.Error()})`。由于该函数只在遇到**非 NotExist** 的错误时才失败，「模型不存在」不会命中它。网关实现若自行做名称解析，**别照抄这个 400**，应保持 404（否则 `ollama show`/SDK 的 show-then-pull 流程会错判成「请求错」而不是「没有这个模型」）。【推断】

`docs/openapi.yaml` 的 `/api/show` **只声明了 200**，没有 4xx/5xx 响应。【官方文档】S2 —— 测试时不要以 openapi 为准，以源码映射表为准。【官方源码】

---

## 4. `GET /api/ps`

### 4.1 请求形状

| 项 | 值 | 证据 |
| --- | --- | --- |
| 方法 / 路径 | `GET /api/ps` | 【官方源码】S4 |
| 查询参数 | 无 | 【官方源码】S4 |
| 请求体 | 无（不解析 body） | 【官方源码】S4 |
| HEAD | **不支持**（只有 `r.GET("/api/ps", ...)`；对比 `/api/version`、`/api/tags` 有 HEAD） | 【官方源码】S4 |
| 认证 | 无 | 【官方源码】S4 |

### 4.2 响应逐字示例（官方）

`docs/api.md`「List Running Models」响应原文：

```json
{
  "models": [
    {
      "name": "mistral:latest",
      "model": "mistral:latest",
      "size": 5137025024,
      "digest": "2ae6f6dd7a3dd734790bbbf58b8909a606e0e7e97e94b7604e0aa7ae4490e6d8",
      "details": {
        "parent_model": "",
        "format": "gguf",
        "family": "llama",
        "families": ["llama"],
        "parameter_size": "7.2B",
        "quantization_level": "Q4_0"
      },
      "expires_at": "2024-06-04T14:38:31.83753-07:00",
      "size_vram": 5137025024
    }
  ]
}
```
【官方文档】S1

`docs/openapi.yaml`（`/api/ps` 200 example，**同一端点、字段集不同**）：

```yaml
example:
  models:
    - name: "gemma4"
      model: "gemma4"
      size: 6591830464
      digest: "c6eb396dbd5992bbe3f5cdb947e8bbc0ee413d7c17e2beaae69f5d569cf982eb"
      details:
        parent_model: ""
        format: "gguf"
        family: "gemma4"
        families:
          - "gemma4"
        parameter_size: "8.0B"
        quantization_level: "Q4_K_M"
      expires_at: "2025-10-17T16:47:07.93355-07:00"
      size_vram: 5333539264
      context_length: 4096
```
【官方文档】S2

**冲突**：`docs/api.md` 的示例**没有 `context_length`**，`docs/openapi.yaml` 有；源码 `ProcessModelResponse.ContextLength` **无 omitempty → 恒出现**。→ **以源码为准：`context_length` 存在**；`docs/api.md` 该示例已过时。【官方源码】S3+S4 / 【官方文档】S1 冲突

### 4.3 字段级表

`ProcessResponse` / `ProcessModelResponse` 逐字：

```go
type ProcessResponse struct {
	Models []ProcessModelResponse `json:"models"`
}

type ProcessModelResponse struct {
	Name          string       `json:"name"`
	Model         string       `json:"model"`
	Size          int64        `json:"size"`
	Digest        string       `json:"digest"`
	Details       ModelDetails `json:"details,omitempty"`
	ExpiresAt     time.Time    `json:"expires_at"`
	SizeVRAM      int64        `json:"size_vram"`
	ContextLength int          `json:"context_length"`
	Runner        string       `json:"runner,omitempty"`
}
```
【官方源码】S3

| 字段 | 类型 | omitempty | 必填 | 说明 / 无装载时怎么填 |
| --- | --- | --- | --- | --- |
| `models` | []object | **无** | **是** | 空列表时源码显式初始化为**空 slice**（`models := []api.ProcessModelResponse{}`）→ 序列化为 `[]`，**不是 `null`**。所以 `{"models": []}` 是官方真实可能产生的响应（无任何模型被装载时）。【官方源码】S4 |
| `models[].name` | string | 无 | 是（非空项） | 显示名，源码：`model.ParseName(m.ShortName).DisplayShortest()` |
| `models[].model` | string | 无 | 是 | 与 `name` **同值**（源码给两者赋同一个 `displayName`） |
| `models[].size` | int64 | 无 | 是 | 装载的模型总字节 |
| `models[].digest` | string | 无 | 是 | `ManifestDigest`，否则 `Digest`（**裸 hex，无 `sha256:` 前缀**） |
| `models[].details` | object | 有（对 struct 无效） | 是 | 恒出现；6 个子键同 §3.3（`parent_model`/`format`/`family`/`families`/`parameter_size`/`quantization_level`），另 `context_length`/`embedding_length`/`runner` 带 omitempty。源码此处 `parent_model` **不填** → `""`；`format`/`family`/`families`/`parameter_size`/`quantization_level` 来自 config。【官方源码】S4 |
| `models[].expires_at` | string(RFC3339) | 无 | 是 | 调度器的过期时间；排序键（剩余时间长的在前）。**无装载时不存在此项** |
| `models[].size_vram` | int64 | 无 | 是 | 占用显存字节 |
| `models[].context_length` | int | **无** | 是 | 已装载实例的上下文长度（**docs/api.md 示例缺此项，见 4.2 冲突**） |
| `models[].runner` | string | 有 | 否 | 归一化后的 runner（`ggml`/`llama.cpp`/`mlx`）；legacy manifest 会回落到配置推导的默认值 |

**空列表是否合法？** 合法，而且是**源码里的确定形状**：`models := []api.ProcessModelResponse{}` + `c.JSON(200, api.ProcessModelResponse{Models: models})` → `{"models":[]}`；`Models` 字段本身无 omitempty，键恒在。【官方源码】S4 / 【推断】（对「无模型装载时为 `[]`」这一步，源码 initialize 已足够直接）

### 4.4 非 200

`PsHandler` **没有任何错误分支**（不读 body、不查模型、只有一次 `c.JSON(http.StatusOK, ...)`）→ 恒 200。【官方源码】S4
唯二的非 200：方法不对（`POST /api/ps`）→ 405；路径不存在 → 404。【推断，基于 S4 的路由与 `HandleMethodNotAllowed = true`】

### 4.5 落点判断

- 网关不装载模型 → **`{"models": []}` 是唯一诚实且形状正确的答案**；官方在「什么都没装载」时输出完全一致的 bytes（除 JSON 空白）。【官方源码】S4
- 不要为了「看起来有东西」塞伪条目：`size`/`size_vram`/`digest`/`expires_at`/`context_length` **全都没有 omitempty**，一旦给出条目就必须把 7 个键都给全，而这些全是「只存在于真实装载」的量。
- 若未来网关仍有「逻辑会话」概念，**不要**把它映射到 `/api/ps`（语义是「已装进内存的模型」），否则 `expires_at`/`size_vram` 必然编造。【推断】

---

## 5. 通用：错误体与非 200 形状

- 错误体统一为**单层** `{"error":"<字符串>"}`。【官方源码】S3（`StatusError.ErrorMessage string \`json:"error"\``）+ S4 全部 `gin.H{"error": ...}`
- 例外（不遵循 `{"error":...}`）：gin 自己的 404/405（`HandleMethodNotAllowed = true`）；以及 `AuthorizationError` 的 `{"error":"unauthorized","signin_url":"..."}`（仅云路径，本网关无关）。【官方源码】S3+S4
- OpenAPI 一致：`ErrorResponse: {error: string}`（无 required 标注）。【官方文档】S2
- 票 34 已记结论「错误体 `{"error":"<字符串>"}`，不是 OpenAI 嵌套形状」 → **本次调研未发现冲突**，并补齐了 `/api/show` 的 400/404 细分。

---

## 6. 与票 34（`issues/34-research-ollama-inbound.md`）的冲突核对

| 票 34 已记 | 本次结果 | 判定 |
| --- | --- | --- |
| 错误体是 `{"error":"<字符串>"}`（非 OpenAI 嵌套形状） | 一致；show 额外给出 400/404 细分与文案 | **无冲突**，已补细 |
| `/api/tags` 各项键「都没有 omitempty → 必须给值」 | 一致（`ListModelResponse` 的 `name`/`model`/`modified_at`/`size`/`digest` 无 omitempty；`details` 的 omitempty 对 struct 无效） | **无冲突** |
| `/api/tags` 建议 `size:0`、`digest` 用确定性 sha256、`details.*` 填空串、不谎报 `format:"gguf"` | 同一套结论在 `/api/show` 的 `details` 与 `/api/ps` 的 `details` 上复用 | **无冲突**，可统一成一条「虚拟模型 details 约定」 |
| 未证实项：`done_reason` 值域、`content` parts 数组、`/api/tags` 的 `name` vs `model` 与查询参数、流式帧 `thinking` 位置 | 本票未涉及这些；但顺带证实：`/api/version` **有未文档化的 HEAD**、`/api/ps` **没有 HEAD**（对比见 §2.1/§4.1） | 无冲突，新增事实 |
| —（票 34 未记） | **新增**：`ShowResponse` 的 `modified_at`（`time.Time`，struct 使 omitempty 失效）也是**必给键**；`model_info` 必给（源码显式保证 `{}`，注释点名 nil 会让集成崩溃） | 票 34 只提了 `/api/tags`，未覆盖 show 的必给键 → **建议 spec 补一句** |

**唯一需要提请注意的「措辞级」差异**：票 34 §2 说 `/api/tags` 的 `details.*`「都没有 omitempty → 必须给值」。更准确的原文级表述是：`details` 本身带 `omitempty` 但对 **struct 值类型无效**，因此**键恒在**；其 6 个子键（`parent_model`/`format`/`family`/`families`/`parameter_size`/`quantization_level`）才真正无 omitempty。结论（必须给值）不变，实现时若按「tag 是否含 omitempty」做通用判断会得到相反答案——**不要写这种通用判断**，按显式清单来。【官方源码】S3

---

## 7. 给网关的最小实现清单（可直接抄）

已定案对齐（本次调研的落点确认，均与官方形状兼容）：
① `/api/version` 回 `"0.13.3"`（真实存在的 Ollama 版本串，semver 合法；它同时等价于声明「实现了 0.13.3 为止的 API 面」，见 §2.5）；
② 三端点都做「诚实最小版」：`ps` 回空 `models`；`show` 只填能诚实填的键（必给三件套 `details`/`model_info`/`modified_at`，其余整键省略）。

```
GET  /api/version   -> 200 {"version": "0.13.3"}               # 唯一键；HEAD 也建议实现
POST /api/show      -> 200 必给: details{parent_model,format,family,families,
                                         parameter_size,quantization_level},
                              model_info: {},
                              modified_at: <RFC3339>
                       其余键按「有真值才给」：system / license / capabilities / thinking …
                       错误: 400 {"error":"missing request body"} / {"error":"model is required"}
                             / {"error":"invalid model name"}
                             404 {"error":"model '<请求名>' not found"}
GET  /api/ps        -> 200 {"models": []}                       # 空数组，非 null
                       错误: 405（其他方法）
```

`modified_at` 的取值建议：网关维护虚拟 manifest 的 mtime（稳定、可复现）优于零值 `"0001-01-01T00:00:00Z"`；两者都合法，但零值会被 UI 显示成 0001 年。【推断】

---

## 8. 未证实 / 待实测

1. **【未证实】** 未起真实 Ollama 服务验证下方字节：`/api/show` 404 的实际 body 文案与 `modified_at` 的实际序列化精度（`time.Time` 的 RFC3339Nano 输出）；`/api/ps` 空列表的精确字节（`{"models":[]}` 已由源码构造方式确定为 `[]`，但未实测）。
2. **【未证实】** 各客户端（`ollama` CLI、ollama-python/js、Open WebUI 等）对缺键的容忍度：源码只点名了 `model_info` 为 nil 会让集成崩溃（`server/routes.go` 注释原文），其余「省略是否安全」是形状层面的判断，未做客户端实测。
3. **【未证实】** `details.families` 为 `null` 与 `[]` 在客户端侧是否等价（Go 侧两者都可能产生，未逐一实测各客户端）。
4. **【未证实】** `verbose=true` 在**远程模型**路径下是否完全无效（源码对 `RemoteHost != ""` 提前 return，未见 verbose 分支；未实测）。
5. **【未证实】** gin 的 405 响应体形状（是否为默认 `404 page not found` 风格的纯文本）未实测；实现时建议自测一次。
