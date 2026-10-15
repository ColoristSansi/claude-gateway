# 供应商表按「上游 × 协议」分凭据，按「上游 × 模型」记可用协议

一张供应商表要表达两件事，而这两件事的粒度不同，混在一层就会得到两处错误的真相。

实证：**同一家上游按协议用的凭据可能不同**——opencode Go 的同一把 Bearer key 打 `/chat/completions` 返回 200，打 `/messages` 返回 401 `Go requires a credential`；**「这个模型能走哪些协议」是逐模型的**——commandcode 的 `/v1/models` 每个模型自带 `supported_endpoints`（89/89 齐全，取值是协议路径，70/89 是多值），opencode 则是每个模型只由一个 API family 提供服务（但它不声明，`/models` 只有 `id/object/created/owned_by`，Anthropic 面 `/models` 甚至 404，ID 前缀与后缀都推不出 family）。

决定：

1. **凭据按「上游 × 出站协议」各存一份**，某条协议未填则继承上游默认。上游对外仍是一个实体：一个 baseUrl、一张卡、一次自检。
2. **可用协议按「(上游, 真实模型)」一条目录记录**存（表 `upstream_models`：`supported_wires` + `source`）。取值域三值 `anthropic` / `openai-chat` / `openai-responses`；**空集合表示未声明，是正常态，不等于不支持**。
3. **虚拟模型绑定引用目录条目**，再在条目声明的集合里选协议；目录里没有的模型必须先手工新增一条条目才能绑。
4. 校验三层：**配置期主拦**（只许选声明过的值，未声明的可手选并标注）→ **请求期早拒 400 兜底** → **自检为准**。
5. 「哪些端点面存在」不再由 `quirks` 表达，`ClaudeModelsOnAnthropicOnly` 删除（票 19 的 `quirk::all()` 双向守卫与原型负例同步改）。

理由：凭据回答的是「同一份订阅在不同协议面上的准入」，可用协议回答的是「模型与端点分派的对应关系」；两者随上游变化各变各的，一个挂在供应商上的单值字段加一个 quirks 枚举表达不了。

**Status**: accepted（2026-10-14，由 [票 23](../../.scratch/model-gateway/issues/23-design-provider-table-granularity.md) 定案；依据是票 14 原型的实测与票 23 期间对两家上游 `/v1/models` 的实测）

**Considered Options**:

- **把上游拆成两个实体**（`opencode-chat` / `opencode-anthropic`，每个仍是「一个地址、一种协议、一份凭据」）：与术语表原句吻合，但引用计数、上游卡片、四步向导、换 key 都要按面重复一遍，而用户在界面上认的是「一家上游」。否决。
- **建独立凭据表，多个上游共用一份**：现在没有「两个上游共用一把 key」的真实场景，属于为将来加的表。否决。
- **可用协议挂在上游上**（一个上游一种协议集合）：被两家上游的逐模型分派直接证伪。否决。
- **不建字段，错配只靠自检发现**：用户要点到错配那一次才知道，客户端报的错也不如本地一句红字清楚。否决。
- **取值域只留两个值**（`anthropic` / `openai`，把 Responses 并进 openai 面）：表达不了「只答 `/chat/completions` 不答 `/responses`」（commandcode 有 8 个这样的模型），Codex 链路要的 Responses 出站也没处落。否决。

**Consequences**: 上游模型目录成为独立一张表，拉取时机（上游向导「拉模型」抓一次存库、可手动重拉、请求期不实时查）与来源优先级（手改锁定 > 上游 `/v1/models` > 随包内置文档表 > 未声明）都归它。票 17 挂起的「协议列能否在表内手改」随之结案：可改，但候选取值只限该模型声明过的值。models.dev 不参与协议判定（它只到 provider 级），仍只用来预填 baseUrl 与协议倾向。术语表同步更正「上游」的定义，并新增「可用 wire 集合」与「上游模型目录」两个词。代价：每个上游多一张目录表要维护；opencode 这类不声明协议的上游靠随包内置文档表兜底，该表不随上游更新就会过期。
