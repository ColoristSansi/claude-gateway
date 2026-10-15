# Ollama 兼容面的端点范围与「装载/卸载」特权语义

Type: grilling
Status: resolved
Blocked by: —

## Question

[票 34](34-research-ollama-inbound.md) 把 `/api/chat` 与 `/api/tags` 的形状查清了，顺带发现**只做这两个端点是半个 Ollama 面**，而且有几处语义在网关里无处安放。要定：

1. **端点范围**：本机工具与客户端常用到的还有 `GET /api/version`、`POST /api/show`（模型详情）、`GET /api/ps`（常驻模型）。补哪些、不补哪些？`/api/version` 是纯常量（不碰上游）成本最低；`/api/show` 要编 modelfile / 参数 / capabilities，内容多半只能编；`/api/ps` 在网关里没有「常驻模型」这个概念（网关不装载模型）。
2. **「装载 / 卸载」特权语义**：`POST /api/chat` 传空 `messages` 是「装载模型」，`keep_alive: 0` 是「卸载」，官方回 200 + `done_reason: "load" / "unload"`。网关不持有模型，这一支本地直接回假终态，还是当普通请求打上游，还是回错？
3. **`/api/tags` 的字段细节**：官方未说明 `name` 与 `model` 何时不同、是否有查询参数（[票 34](34-research-ollama-inbound.md) 已记为未证实）；本网关要不要只填一份值。
4. **`/v1/*` 兼容面是否跟进**：Ollama 自己也提供 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`（调研 §3）。网关本来就有这三条入站（§4.1–§4.3），本票只需确认**不再为 Ollama 另开一套**。

## 背景

- [票 34](34-research-ollama-inbound.md) 的答案与 [research/ollama-inbound.md](../research/ollama-inbound.md)（§2、§3、§5.6、§6 缺口 6）。
- spec §4.4 已按票 34 铺开形状，并把本票要定的两处标成「待决」。
- 结论要回写 spec §4.4；若定了端点范围，spec §4.5「辅助端点」可能也要加一行。

## Answer

**八条定案（HITL，两轮弹窗，用户全部采纳建议；第三轮收尾确认「前沿已空」）。**

1. **端点范围：三个辅助端点都补，但都做「诚实最小版」**——`/api/version` 回常量串、`/api/ps` 回空 `models`、`/api/show` 只填能诚实填的键。理由：三者都不打上游、成本近零，而缺它们会让本机工具判定网关不是可用的 Ollama 端点。
2. **装载 / 卸载：本地回假终态，不打上游**——`messages: []` → 200 `{model, created_at, message:{role:"assistant",content:""}, done_reason:"load", done:true}`；`messages: []` + `keep_alive: 0` → 同形但 `done_reason:"unload"`。统计字段一律不填（官方也因 `omitempty` 不填）。`stream:false` 时是同一个对象的单个 JSON。**`messages` 非空时 `keep_alive` 一律忽略**（它只影响驻留，不影响推理）。
3. **`/api/tags` 的 `name` 与 `model` 填同一个值**：都填**裸虚拟模型名**（不带 `:latest`），与 `GET /v1/models` 逐字一致；入站解析额外**接受并剥掉 `:latest`**（官方文档：tag 缺省即 latest）。`modified_at` 填该绑定的最后修改时间，无则网关启动时间（RFC3339 UTC）。
4. **`details.*` 按「没有权重文件」填空值**（票 34 的填法不变：`parent_model`/`format`/`parameter_size`/`quantization_level` 空串、`families` 空数组；家族槽位填槽位名），**`capabilities` 在 `/api/tags` 与 `/api/show` 都填 `["completion","tools"]`**（四条出站链路都支持工具调用，成本为零且客户端真会读）。
5. **不为 Ollama 另开一套 `/v1/*`**：`/api/chat` 与 `/v1/chat/completions` 是**两个入站解码器共用同一条 IR 与出站链路**，只有解码器与错误信封不同（`/api/*` 用 `{"error":"<字符串>"}`，`/v1/*` 用 OpenAI 嵌套形状）。
6. **`/api/version` 回真实存在的 Ollama 版本串 `"0.13.3"`**（`/v1/responses` 引入的那一版，语义上覆盖我们实现的端点集）。诚实性不靠这个串，靠 `details` 不谎报权重。事实修正：该值**不是常量**（`version.Version` 默认 `"0.0.0"`，构建期 `-X` 注入），除展示外还参与 registry User-Agent 与 `requires` 版本门槛（值为 `"0.0.0"` 时门槛被跳过）——回 `0.13.3` 等于声明「实现了到 0.13.3 为止的 API 契约」。`HEAD /api/version` 一并实现（官方有，未文档化）。
7. **其余 `/api/*` 统一 404、不实现**：`pull` / `create` / `copy` / `delete` / `blobs/*` / `generate` / `embed` / `embeddings`。它们是「管理本地权重文件」与「非对话推理」两个面；已写进地图 **Out of scope**。
8. **模型名解析与未知名**：`/api/show` 与 `/api/ps` 走**同一条绑定层解析**（虚拟模型名 / 四家族槽位名，接受并剥掉 `:latest`）；解析不到统一 **404 `model '<name>' not found`**（逐字复用官方文案，工具的 show-then-pull 流程依赖它）；`/api/show` 另保留 400 `missing request body` / `model is required`（请求体用 **`model`** 字段，`name` 只是已废弃的兜底），`verbose` 接受并忽略。

**三个端点的最小字段清单（照此实现，多填即撒谎）**

- `GET /api/version` → 200 `{"version":"0.13.3"}`（唯一键；恒 200，无错误分支）。
- `POST /api/show` → 200 **必给**：`details`（六个子键恒在）+ `model_info`（至少 `{}`，**源码点名 nil 会让集成崩溃**）+ `modified_at`（`time.Time`，`omitempty` 对 struct 无效 → 键恒在；取绑定时间优于零值，零值会被界面显示成 0001 年）；**其余整键省略**（`modelfile` / `parameters` / `template` / `license` / `system` / `renderer` / `parser` / `thinking` / `tensors` / `projector_info` / `requires` / `manifests`）。
- `GET /api/ps` → 200 `{"models":[]}`（**空数组，不是 `null`**；官方源码在什么都没装载时就是这个形状），恒 200，不支持 `HEAD`。

**本票新增的一手事实**（完整见 [research/ollama-aux-endpoints.md](../research/ollama-aux-endpoints.md)，来源为 Ollama main 分支源码与官方文档）：`ShowResponse` 的 `modified_at` 与 `model_info` 也是**必给键**（票 34 只覆盖了 `/api/tags`）；`details` 自身的 `omitempty` **对 struct 值类型无效**，判定「哪些键必给」不能按 tag 通用推断，须按显式清单；`/api/show` 的请求体字段是 `model`（`name` 已废弃）、`verbose` 只影响 tokenizer 大数组与 `tensors`；`/api/ps` 非空条目的 `context_length` 无 `omitempty`（docs/api.md 的示例已过时）；`/api/version` 有未文档化的 `HEAD`、`/api/ps` 没有 `HEAD`。

**落点**：spec §4.4 已按此改写（新增四条 + 待决/待实测各收窄一条），未动 §4.5——这三个端点是 Ollama 面自身的一部分，与 §4.5 的 `/v1/*` 辅助端点不是同一组；`GLOSSARY.md` 新增词条「装载 / 卸载（Load / Unload）」。

**未证实（实现阶段实测）**：`/api/show` 404 的精确字节与 `modified_at` 序列化精度、`/api/ps` 空列表的字节级形状、各客户端对缺键（`modalities` 除外）的容忍度。

**不留新票，无新雾**：本票只定端点范围与语义；`format`（结构化输出槽位）仍归[票 37](37-grilling-structured-output-slot.md)（本图前沿上的下一张）。
