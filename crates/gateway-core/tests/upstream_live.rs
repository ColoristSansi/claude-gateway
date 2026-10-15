//! 票 49 验收 2 的真上游验收（**联网**，默认不跑）：
//!
//! ```text
//! $env:CG_UPSTREAM_ENV='C:\...\prototype-upstream.env'
//! cargo test -p gateway-core --test upstream_live -- --ignored --nocapture
//! ```
//!
//! 干的事：两条真上游各建一条上游记录 → 用真凭据（DPAPI 入库）拉一次 `/models` →
//! 灌进上游模型目录 → 打印条数与抽样 wire 判定。
//!
//! **绝不打印密钥与 baseUrl**：报告里只有条数、模型 id、wire 判定与来源。
//! 跑完还要在数据目录的字节里反查密钥明文（上一轮验收的硬判据）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gateway_core::binding;
use gateway_core::egress::provider::*;
use gateway_core::store::Store;

const DEFAULT_ENV: &str =
    r"C:\Users\ZHANQUAN\AIGC\Project\Claude Gateway\.scratch\model-gateway\prototype-upstream.env";

struct EnvFile {
    vars: Vec<(String, String)>,
}

impl EnvFile {
    fn load() -> EnvFile {
        let path = std::env::var("CG_UPSTREAM_ENV").unwrap_or_else(|_| DEFAULT_ENV.to_string());
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到凭据文件（设 CG_UPSTREAM_ENV 指它）：{path} / {e}"));
        let mut vars = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                vars.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        EnvFile { vars }
    }

    fn get(&self, k: &str) -> String {
        self.vars
            .iter()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("凭据文件里没有 {k}"))
    }
}

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg49-live").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// 真实数据目录里的字节反查（库文件 + `-wal` / `-shm`）。
fn dir_contains_bytes(dir: &Path, needle: &[u8]) -> bool {
    for e in std::fs::read_dir(dir).expect("读数据目录") {
        let p = e.expect("目录项").path();
        if p.is_file() {
            let bytes = std::fs::read(&p).expect("读文件");
            if bytes.windows(needle.len()).any(|w| w == needle) {
                return true;
            }
        }
    }
    false
}

/// 两条真上游的声明式表：baseUrl 由凭据文件给（**不落进代码**），路径模板相对它。
/// 实测两家的 baseUrl 都已带 `/v1`，所以模板不带 `/v1`。
fn live_upstream(id: &str, base_url: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base_url);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/messages");
    up.set_path(Wire::OpenAiChat, "/chat/completions");
    up.set_path(Wire::OpenAiResponses, "/responses");
    up
}

fn report(name: &str, models: &[UpstreamModel], applied: &PullApplied) {
    println!("\n===== {name} =====");
    println!(
        "成员数 = {}（新增 {} / 更新 {} / 清掉 {} / 手工留着 {}）；未声明 {}；反查警告 {} 条",
        models.len(),
        applied.added,
        applied.updated,
        applied.removed,
        applied.kept_manual,
        applied.undeclared,
        applied.warnings.len()
    );
    for w in &applied.warnings {
        println!("  警告：{w}");
    }
    let mut by_wire: std::collections::BTreeMap<String, usize> = Default::default();
    for m in models {
        let key = if m.supported_wires.is_empty() {
            "未声明".to_string()
        } else {
            m.supported_wires
                .iter()
                .map(Wire::as_str)
                .collect::<Vec<_>>()
                .join("+")
        };
        *by_wire.entry(key).or_default() += 1;
    }
    let mut dist: Vec<_> = by_wire.into_iter().collect();
    dist.sort_by_key(|a| std::cmp::Reverse(a.1));
    println!("wire 分布：{dist:?}");
    println!("抽样：");
    for m in models.iter().take(6) {
        let wires: Vec<&str> = m.supported_wires.iter().map(Wire::as_str).collect();
        println!(
            "  {} -> {:?}（来源 {}）",
            m.model_id,
            wires,
            m.source.as_str()
        );
    }
}

#[tokio::test]
#[ignore = "打真上游：cargo test --test upstream_live -- --ignored --nocapture"]
async fn commandcode_declares_endpoints_for_every_model() {
    let env = EnvFile::load();
    let base = env.get("COMMANDCODE_BASE_URL");
    let key = env.get("COMMANDCODE_API_KEY");
    let root = tmp_root("commandcode");
    let store = Store::open(&root).expect("开库");

    let up = live_upstream("commandcode", &base);
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("commandcode", "default", &key)
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let credential = pull_credential(&store, "commandcode").await.unwrap();
    assert!(credential.is_some(), "default 凭据应当取得到");
    let outcome = pull_models(&http, &up, MODELS_PATH, credential.as_deref()).await;
    assert!(
        matches!(outcome, PullOutcome::Fetched(_)),
        "commandcode 应当能拉到清单：{outcome:?}"
    );
    let remote = match &outcome {
        PullOutcome::Fetched(v) => v.clone(),
        _ => unreachable!(),
    };
    let with_endpoints = remote.iter().filter(|m| !m.endpoints.is_empty()).count();
    println!(
        "commandcode 拉到 {} 条，其中带 supported_endpoints 的 {with_endpoints} 条",
        remote.len()
    );
    assert_eq!(
        with_endpoints,
        remote.len(),
        "实测 89/89 → 现在应仍是全部带端点（票 23 表）"
    );

    let applied = apply_pull(&store, &up, outcome, 1_760_000_100)
        .await
        .unwrap();
    let models = list_models(&store, "commandcode").await.unwrap();
    report("commandcode", &models, &applied);

    // supported_wires 与观测一致：逐条等于按 pathTemplates 反查的结果，且没有反查警告。
    assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
    assert_eq!(applied.undeclared, 0, "每条都有端点 → 不该有未声明");
    for m in &models {
        let observed: BTreeSet<Wire> = remote
            .iter()
            .find(|r| r.id == m.model_id)
            .map(|r| reverse_lookup(&up, &r.endpoints).0)
            .expect("目录里的模型应来自这次拉取");
        assert_eq!(
            m.supported_wires, observed,
            "{} 的 wire 判定不一致",
            m.model_id
        );
        assert!(!m.supported_wires.is_empty());
        assert_eq!(m.source, Source::Upstream);
    }

    // 密钥明文不许落在库里（上一轮验收的硬判据）。
    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

#[tokio::test]
#[ignore = "打真上游：cargo test --test upstream_live -- --ignored --nocapture"]
async fn opencode_declares_no_endpoints_so_wires_come_from_doc_table() {
    let env = EnvFile::load();
    let base = env.get("OPENCODE_BASE_URL");
    let key = env.get("OPENCODE_API_KEY");
    let root = tmp_root("opencode");
    let store = Store::open(&root).expect("开库");

    let up = live_upstream("opencode", &base);
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("opencode", "default", &key)
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let credential = pull_credential(&store, "opencode").await.unwrap();
    assert!(credential.is_some(), "default 凭据应当取得到");
    let outcome = pull_models(&http, &up, MODELS_PATH, credential.as_deref()).await;
    assert!(
        matches!(outcome, PullOutcome::Fetched(_)),
        "opencode 应当能拉到清单：{outcome:?}"
    );
    let remote = match &outcome {
        PullOutcome::Fetched(v) => v.clone(),
        _ => unreachable!(),
    };
    println!(
        "opencode 拉到 {} 条；带 supported_endpoints 的 {} 条（预期 0：两面 /models 都不报端点）",
        remote.len(),
        remote.iter().filter(|m| !m.endpoints.is_empty()).count()
    );
    assert!(
        remote.iter().all(|m| m.endpoints.is_empty()),
        "opencode 不该报端点；报了说明上游改了，得回来改判据"
    );

    let applied = apply_pull(&store, &up, outcome, 1_760_000_200)
        .await
        .unwrap();
    let models = list_models(&store, "opencode").await.unwrap();
    report("opencode", &models, &applied);

    let from_doc = models
        .iter()
        .filter(|m| m.source == Source::Doc && !m.supported_wires.is_empty())
        .count();
    let undeclared = models
        .iter()
        .filter(|m| m.supported_wires.is_empty())
        .count();
    println!("内置文档表判定 {from_doc} 条，未声明 {undeclared} 条");
    assert!(from_doc > 0, "文档表至少要认出几条（票 23：opencode 靠它）");
    for m in &models {
        // wire 只能来自随包文档表；文档表不认的保持未声明（不猜名字）。
        assert_eq!(
            m.supported_wires,
            doc_wires(&m.model_id),
            "{} 的 wire 只能来自文档表",
            m.model_id
        );
        if m.supported_wires.is_empty() {
            assert_eq!(
                m.source,
                Source::Upstream,
                "{} 应是未声明的上游条目",
                m.model_id
            );
        } else {
            assert_eq!(m.source, Source::Doc, "{} 的来源应是 doc", m.model_id);
        }
    }
    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

/// 票 51 验收 2：自检探针（非流式 `/v1/messages` + `max_tokens:1`，**只看状态码**）
/// 在两条真上游各跑一次，外加票据点名的那条**已知挂住**的组合。
///
/// 三个形状：
/// - opencode + `deepseek-v4.1-flash` + Chat wire（票 26 实测 1.9 秒 200）
/// - commandcode + `claude-sonnet-5-5` + Anthropic wire（票 26 实测 2.5 秒 200）
/// - commandcode + `deepseek/deepseek-v4.1-flash` + **Chat wire**：票 26 / 票 51:24 记的那条
///   `max_tokens:1` 挂住（直连 curl 25 秒也无响应）→ 探针档 20 秒上限 → **504**
///
/// 顺带在真数据上验一次「绑定只能从上游模型目录里选」：目录里有的能绑、没有的绑不上。
#[tokio::test]
#[ignore = "打真上游：cargo test -p gateway-core --test upstream_live -- --ignored --nocapture"]
async fn selfcheck_probe_on_both_live_upstreams() {
    let env = EnvFile::load();
    let root = tmp_root("selfcheck");
    let store = Store::open(&root).expect("开库");
    let http = reqwest::Client::new();

    let mut oc = live_upstream("opencode", &env.get("OPENCODE_BASE_URL"));
    // opencode 的 Go 计划要求客户端自称（票 02 / 19）：没有 UA 与 `x-opencode-session`，
    // Chat 面直接 400 `MissingSessionID`——这是上游的硬要求，不是探针的问题。
    oc.quirks = vec![
        Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1".into()),
        Quirk::SessionIdHeader("x-opencode-session".into()),
    ];
    let cc = live_upstream("commandcode", &env.get("COMMANDCODE_BASE_URL"));
    save_upstream(&store, &oc).await.unwrap();
    save_upstream(&store, &cc).await.unwrap();
    store
        .put_credential("opencode", "default", &env.get("OPENCODE_API_KEY"))
        .await
        .unwrap();
    store
        .put_credential("commandcode", "default", &env.get("COMMANDCODE_API_KEY"))
        .await
        .unwrap();

    // 目录条目（手工新增，`manual` 锁定）：绑定只能从目录里选。
    for (id, model, wire) in [
        ("opencode", "deepseek-v4.1-flash", Wire::OpenAiChat),
        ("commandcode", "claude-sonnet-5-5", Wire::Anthropic),
        (
            "commandcode",
            "deepseek/deepseek-v4.1-flash",
            Wire::OpenAiChat,
        ),
    ] {
        save_manual_model(&store, id, model, &[wire]).await.unwrap();
    }

    // 真数据上的配置期主拦：目录里有的能绑（家族槽位走真实目录条目），没有的绑不上。
    let slot = binding::Binding::new(
        binding::Profile::Global,
        "claude-haiku",
        "commandcode",
        "claude-sonnet-5-5",
        Wire::Anthropic,
    );
    match binding::save(&store, &slot).await {
        Ok(WireChoice::Declared) => {
            println!("\nclaude-haiku → commandcode / claude-sonnet-5-5 / anthropic：已绑")
        }
        other => panic!("真目录条目该能绑上：{other:?}"),
    }
    let bogus = binding::Binding::new(
        binding::Profile::Codex,
        "codex-不存在",
        "commandcode",
        "根本没有这个模型",
        Wire::Anthropic,
    );
    match binding::save(&store, &bogus).await {
        Err(binding::SaveError::NoCatalogEntry { .. }) => {
            println!("目录里没有的模型：绑不上（符合 spec §7）")
        }
        other => panic!("目录里没有的模型必须绑不上：{other:?}"),
    }
    let table = binding::load(&store).await.unwrap();
    assert_eq!(
        table.resolve("claude-haiku", "").unwrap().real_model,
        "claude-sonnet-5-5"
    );

    // 探针四发：两条真上游各一发（opencode 的 Chat 面按票 02 / 19 / 49 必须自称 UA + 带会话头，
    // 不带就是 400 `MissingSessionID`），外加两条**已知形状**的样本。
    //
    // 期望值给 `None` 的那两发是「上游看心情」的组合：同一个三元组今天实测既见过
    // 20.0 秒 → 504（挂住），也见过 1.4 秒 → 200。所以那两发断言的是**契约**而不是具体码：
    // 2xx = 通过、504 = 探针档超时；两者都要如实落库。
    let probes: [(&str, &Upstream, Wire, &str, Duration, Option<u16>); 4] = [
        (
            "opencode",
            &oc,
            Wire::OpenAiChat,
            "deepseek-v4.1-flash",
            binding::PROBE_DEADLINE,
            None,
        ),
        (
            "opencode",
            &oc,
            Wire::OpenAiChat,
            "deepseek-v4.1-flash",
            Duration::from_secs(90),
            Some(200),
        ),
        (
            "commandcode",
            &cc,
            Wire::Anthropic,
            "claude-sonnet-5-5",
            binding::PROBE_DEADLINE,
            Some(200),
        ),
        (
            "commandcode",
            &cc,
            Wire::OpenAiChat,
            "deepseek/deepseek-v4.1-flash",
            binding::PROBE_DEADLINE,
            None,
        ),
    ];
    println!("\n===== 自检探针（非流式 /v1/messages + max_tokens:1，只看状态码）=====");
    let mut fails: Vec<String> = Vec::new();
    for (id, up, wire, model, deadline, want) in probes {
        let r = binding::selfcheck(&http, &store, up, wire, model, deadline).await;
        println!(
            "  {id} / {model} / {} / 上限 {}s → status={} ok={} {}ms  {}",
            wire.as_str(),
            deadline.as_secs(),
            r.status,
            r.ok,
            r.elapsed_ms,
            r.detail
        );
        match want {
            Some(want) if r.status != want => fails.push(format!(
                "{id} / {model} / {}（上限 {}s）：期望 {want}，实际 {}",
                wire.as_str(),
                deadline.as_secs(),
                r.status
            )),
            // 契约：这一档要么答了（2xx / 上游自己的 4xx/5xx），要么被探针档砍在 504。
            None if r.status != 200 && r.status != 504 => fails.push(format!(
                "{id} / {model} / {}（上限 {}s）：只该是 200 或 504，实际 {}",
                wire.as_str(),
                deadline.as_secs(),
                r.status
            )),
            _ => {}
        }
        if r.status == 504 && !r.detail.contains("超时") {
            fails.push(format!("504 的文案该说超时：{}", r.detail));
        }
        // 落库：就绪判据只看 `upstreams.last_selfcheck_*` 这一列。
        let after = load_upstream(&store, id).await.unwrap().unwrap();
        let sc = after.last_selfcheck.expect("自检结果必须落库");
        if sc.ok != r.ok {
            fails.push(format!(
                "{id} 的自检结果没落对：{sc:?} vs status={}",
                r.status
            ));
        }
    }
    assert!(fails.is_empty(), "自检探针不合格：\n{}", fails.join("\n"));

    assert!(
        !dir_contains_bytes(&root, env.get("OPENCODE_API_KEY").as_bytes())
            && !dir_contains_bytes(&root, env.get("COMMANDCODE_API_KEY").as_bytes()),
        "库文件里不该出现明文密钥"
    );
}
