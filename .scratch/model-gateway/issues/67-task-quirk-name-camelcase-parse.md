# quirk 名的 camelCase 只认一半：`temperatureMax2` / `emptyThinkingSignature` / `silentTruncate` 被静默丢弃

Type: task
Status: open
Blocked by: —

## Question

`Quirk::parse`（`crates/gateway-core/src/egress/provider.rs:204`）的注释写明本意是「规范名是
`Quirk::name` 的 PascalCase；**spec §6 样例与手写 JSON 里的 camelCase 也认**，免得把『写对了意思、
写错了大小写』的配置静默丢掉」。

但 7 个值里只有 4 个真认 camelCase（`ClientUserAgent` / `SessionIdHeader` / `NoModelsEndpoint` /
`ModelsListNeedsCredential`，见 `provider.rs:206-211`）；**`TemperatureMax2` /
`EmptyThinkingSignature` / `SilentTruncate` 三条只列了 PascalCase**（`provider.rs:212-214`）。

入站入口在 `crates/gateway-core/src/ingress/api/upstreams.rs:642-651`：`filter_map` 把解析不出的
名字**直接丢掉**——不报错、不警告、不计数。所以这不是「响亮地拒绝」，是**静默降级**。

后果（spec §6:211 的示例 JSON 正好是踩雷写法）：把示例那一串
`["noModelsEndpoint", "temperatureMax2", "emptyThinkingSignature", "silentTruncate"]` 原样 POST 给
`/api/upstreams`，落库只剩 `noModelsEndpoint` —— 另外三条消失，而用户以为自己配齐了。

旁证（同文件里的相邻实现）：`Wire::parse`（`provider.rs:84-91`）明确容忍 camelCase；`providers.json`
的预设用 PascalCase（`["ModelsListNeedsCredential", ""]`）→ **向导路径不中招**，中招的是手写 JSON /
直发 API / 照抄 spec 示例。票 66 刚把这三个 quirk 的执法点做实，认不出来就等于执法点对这部分配置
不生效。

**证据等级：读码认定（行号已核），未执行验证** —— 本票第一件事就是把它变红。

## 交付

1. `Quirk::parse` 给 `TemperatureMax2` / `EmptyThinkingSignature` / `SilentTruncate` 补 camelCase
   分支（与现有四条同款 `"Pascal" | "camel" => ...` 写法）。规范名不变，写出去仍是 PascalCase。
2. 一条能变红的用例：三条 camelCase 名各自能解析出对应 `Quirk`；顺带钉住「7 个值的 camelCase
   全认」（拿 `Quirk::all()` 逐个过一遍 `parse(q.name().to_lowercase-首字母小写)` 之类，别写成 4 个
   硬编码断言，免得下次加 quirk 又漏）。
3. 可选（**要用户拍，不许顺手改**）：`upstreams.rs` 那条 `filter_map` 要不要把丢掉的名字**说出来**
   （响应里带 `ignoredQuirks`，或记一条日志）。它现在是静默的，所以上面这个 bug 才藏了这么久；
   但改成 400 会打断既有导入/还原路径，**本票默认不动语义**。

## 收口验收

- 新用例先红后绿（贴前后）；把补上的分支改回单名 → 对应用例变红 → 改回变绿；
- `pwsh -File ./check.ps1 -Quick -Replay` 全绿（含前端类型检查步）；
- 落库形状不变：`Quirk::name()` 仍是 PascalCase，`providers.json` 预设逐字未动；
- 若选择做第 3 条（`ignoredQuirks`），补一条 API 层用例：混入一个不存在的 quirk 名 → 其余照常生效
  且被丢的名字可观测。

## 不做

- 不改 spec / ADR（§6 的示例表与 §12.1 已于 2026-10-16 收尾同步，无需再动）；
- 不动另外四个 quirk 的解析与任何执法点；
- 不动票 64/65/66 的实现。
