# quirk 名的 camelCase 只认一半：`temperatureMax2` / `emptyThinkingSignature` / `silentTruncate` 被静默丢弃

Type: task
Status: resolved
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

## Answer

**执行人**：收尾第二轮（2026-10-16）。**第 3 条按编排者当场拍板「做」**，实现方式是**响应字段
`ignoredQuirks`**（不是记日志：日志面在管理面里没有现成落点，而回执是调用方当场看得到的地方）。

### 交付 1：`Quirk::parse` 补三条 camelCase（`egress/provider.rs:212-217`）

与现有四条同款写法：`"TemperatureMax2" | "temperatureMax2"` / `"EmptyThinkingSignature" |
"emptyThinkingSignature"` / `"SilentTruncate" | "silentTruncate"`。规范名与写出形状不变
（`Quirk::name()` 仍是 PascalCase）。**一处改，三个调用点同时受益**：`provider.rs:853`（库 → 表）、
`provider.rs:1218`（`providers.json` 预填）、`upstreams.rs:648`（管理面入口）。

### 交付 2 + 3：一条单测 + 一条 API 层用例

1. `provider.rs` 测试模块新增 `every_quirk_parses_in_pascal_and_camel_case`：camelCase 从
   `Quirk::all()` **现派生**（`pascal[..1].to_ascii_lowercase() + &pascal[1..]`），七个值两写各过一遍，
   所以下次加 quirk 忘写 camelCase 分支这条也会红；另钉 spec §6:211 的三个名字，并留一条反例
   （`temperaturemax2` 必须解析不出来，别让 parse 变成什么都收）。
2. `tests/admin_api.rs` 新增 `quirk_names_parse_in_camel_case_and_unknown_ones_are_reported`：
   照 spec §6:211 的示例逐字 POST（全 camelCase + 末尾混一个 `thisQuirkDoesNotExist`）→
   ① `upstream.quirks` 逐条等于四个 PascalCase 名；② `ignoredQuirks == ["thisQuirkDoesNotExist"]`；
   PUT 走同一条路径再验一遍（两个 handler 都接了，不是只接 create）。
3. 实现：`upstream_from_body` 的第二个返回值就是被丢掉的名字（`(Upstream, Vec<String>)`），
   create / update 的回执都带 `"ignoredQuirks"`。字符串按名字原样回；**非字符串元素（数字之类的脏
   元素）也照实回一句**，不再当它没写。**没改任何语义**：认不出的名字仍然只是「不生效」，不是 400，
   既有导入 / 还原路径的行为一字不变，只是现在说得出来。

### 收口验收

**① 新用例先红后绿（原文）**：

- 单测红：``assertion `left == right` failed: `temperatureMax2` 应当解析成 TemperatureMax2 / left: None / right: Some("TemperatureMax2")``（`provider.rs:2016`）；补分支后 `test egress::provider::tests::every_quirk_parses_in_pascal_and_camel_case ... ok`。
- API 红（**正是本票描述的那一幕**）：POST spec 示例后回执是 `"quirks":["NoModelsEndpoint"]` —— 四条只落一条；
  补完后 `"quirks":["NoModelsEndpoint","TemperatureMax2","EmptyThinkingSignature","SilentTruncate"]`、
  `"ignoredQuirks":["thisQuirkDoesNotExist"]`，`test quirk_names_parse_in_camel_case_and_unknown_ones_are_reported ... ok`。
- 反向验证：上面那条「红」就是**未补分支时的同一份用例**跑出来的（单名分支 → 对应用例红），改回后绿。
- `cargo test -p gateway-core --test admin_api`：`7 passed; 0 failed`（新旧用例都没被带红）。

**② 门禁**：`pwsh -File ./check.ps1 -Quick -Replay`（`CARGO_TARGET_DIR` 用全新目录，避免票 41 第 11 条那类假绿）→ 末行 `全部通过。`、exit 0；日志 `cg-notes\67-check.log`。

**③ 落库形状不变**：`Quirk::name()` / 库里的 `quirks_json` 仍是 PascalCase；`providers.json` 预填逐字未动
（`git diff` 只有三个文件 + 一条 TS 类型）。

**④ 收尾自查**：`git grep -n "filter_map(|q|" crates/gateway-core/src/ingress/api/upstreams.rs` 已无命中——
那条静默 `filter_map` 没了。

### 留下的坑

- `ignoredQuirks` 目前**只有回执**：前端 `app/src/api.ts` 只补了类型（`ignoredQuirks?: string[]`），
  向导页**没有**弹提示。要让用户当场看见，得在「上游」页保存后的提示里加一行——不在本票范围。
- 非流式的 `Message` 里带 `signature: ""` 仍在**流式路径**原样出去（票 66 已记，与本票无关）。

