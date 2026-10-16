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
use gateway_core::egress::discovery::*;
use gateway_core::egress::provider::*;
use gateway_core::egress::structured;
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

/// 一发自检探针：打哪条上游的哪个面、真实模型、上限与期望状态码
/// （`want: None` = 只验契约：这一档要么答了、要么被探针档砍在 504）。
struct Probe<'a> {
    id: &'a str,
    up: &'a Upstream,
    wire: Wire,
    model: &'a str,
    deadline: Duration,
    want: Option<u16>,
}

/// 票 51 验收 2：自检探针（非流式 `/v1/messages` + `max_tokens:1`，**只看状态码**）
/// 在两条真上游各跑一次，外加两条**已知形状**的样本。
///
/// - opencode + `deepseek-v4.1-flash` + Chat wire：探针档 20 秒一次、放宽到 90 秒一次。
///   上游的 Go 面要求自称 UA + `x-opencode-session`（票 02 / 19 / 49），不带就是 400
///   `MissingSessionID`；这一面还**慢**（实测 2～29 秒），所以 20 秒那发可能是 504。
/// - commandcode + `claude-sonnet-5-5` + Anthropic wire：健康样本（实测 2.8～4.9 秒 200）。
/// - commandcode + `deepseek/deepseek-v4.1-flash` + **Chat wire**：票 26 / 票 51:24 记的那条
///   `max_tokens:1` 挂住（直连 curl 25 秒也无响应）→ 探针档 20 秒上限 → **504**；
///   但这条形状**是间歇的**（同一天实测：20.003 秒 → 504，另两次 1.4 / 2.2 秒 → 200），
///   所以断言的是契约（200 或 504）并把实际观测打出来，不钉死 504。
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
    let probes = [
        Probe {
            id: "opencode",
            up: &oc,
            wire: Wire::OpenAiChat,
            model: "deepseek-v4.1-flash",
            deadline: binding::PROBE_DEADLINE,
            want: None,
        },
        Probe {
            id: "opencode",
            up: &oc,
            wire: Wire::OpenAiChat,
            model: "deepseek-v4.1-flash",
            deadline: Duration::from_secs(90),
            want: Some(200),
        },
        Probe {
            id: "commandcode",
            up: &cc,
            wire: Wire::Anthropic,
            model: "claude-sonnet-5-5",
            deadline: binding::PROBE_DEADLINE,
            want: Some(200),
        },
        Probe {
            id: "commandcode",
            up: &cc,
            wire: Wire::OpenAiChat,
            model: "deepseek/deepseek-v4.1-flash",
            deadline: binding::PROBE_DEADLINE,
            want: None,
        },
    ];
    println!("\n===== 自检探针（非流式 /v1/messages + max_tokens:1，只看状态码）=====");
    let mut fails: Vec<String> = Vec::new();
    for p in probes {
        let (id, model, wire, deadline, want) = (p.id, p.model, p.wire, p.deadline, p.want);
        let r = binding::selfcheck(&http, &store, p.up, wire, model, deadline).await;
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

// ───────────────── 票 50 验收 3：走「拉模型」整条流程（discovery::run_pull）打真上游 ─────────────────

/// commandcode：带凭据一路成功，拉到 89～90 条且每一条都带 `supported_endpoints`（→ 逐条有 wire 判定）。
#[tokio::test]
#[ignore = "打真上游：cargo test --test upstream_live -- --ignored --nocapture"]
async fn ticket50_commandcode_pull_flow_lands_89_to_90_with_endpoints() {
    let env = EnvFile::load();
    let base = env.get("COMMANDCODE_BASE_URL");
    let key = env.get("COMMANDCODE_API_KEY");
    let root = tmp_root("t50-commandcode");
    let store = Store::open(&root).expect("开库");

    let mut up = live_upstream("commandcode", &base);
    up.quirks = vec![]; // 尽力而为：有 default 就用它，不退匿名
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("commandcode", "default", &key)
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let report = run_pull(&store, &http, &up, 1_760_000_300).await.unwrap();
    println!(
        "\n[t50] commandcode：结局 {:?} / 凭据 {:?} / 拉到 {} 条（新增 {} 更新 {} 清掉 {} 手改锁定 {}）/ 反查警告 {} 条",
        report.disposition,
        report.credential,
        report.applied.fetched,
        report.applied.added,
        report.applied.updated,
        report.applied.removed,
        report.applied.kept_manual,
        report.warnings.len()
    );
    assert_eq!(report.disposition, PullDisposition::Fetched, "{report:?}");
    assert_eq!(
        report.credential,
        PullCredential::Credentialed,
        "有 default 就该带凭据拉，不该退匿名"
    );
    assert!(
        report.warnings.is_empty(),
        "端点全对得上 pathTemplates：{:?}",
        report.warnings
    );
    assert_eq!(report.applied.undeclared, 0, "每条都有端点 → 不该有未声明");
    let n = report.applied.fetched;
    assert!(
        (89..=90).contains(&n),
        "票 50 验收 3 观测 89～90 条带 supported_endpoints，这次 {n} 条：上游清单变了先确认真实变化，再改这条断言"
    );
    let models = list_models(&store, "commandcode").await.unwrap();
    assert_eq!(models.len(), n);
    for m in &models {
        assert!(
            !m.supported_wires.is_empty(),
            "{} 没有 wire 判定：说明这条没带 supported_endpoints",
            m.model_id
        );
        assert_eq!(m.source, Source::Upstream);
    }
    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

/// opencode：只拉带凭据那份（Console 面 19 / 匿名 85 是权限过滤）；wire 只来自随包文档表。
#[tokio::test]
#[ignore = "打真上游：cargo test --test upstream_live -- --ignored --nocapture"]
async fn ticket50_opencode_pull_flow_uses_the_credentialed_list() {
    let env = EnvFile::load();
    let base = env.get("OPENCODE_BASE_URL");
    let key = env.get("OPENCODE_API_KEY");
    let root = tmp_root("t50-opencode");
    let store = Store::open(&root).expect("开库");

    // 向导的 opencode 预设要带这条 quirk（没凭据不拉 / 带凭据 404·零条才退匿名）。
    let mut up = live_upstream("opencode", &base);
    up.quirks = vec![Quirk::ModelsListNeedsCredential];
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("opencode", "default", &key)
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let report = run_pull(&store, &http, &up, 1_760_000_400).await.unwrap();
    println!(
        "\n[t50] opencode：结局 {:?} / 凭据 {:?} / 拉到 {} 条（未声明 {}）/ 反查警告 {} 条",
        report.disposition,
        report.credential,
        report.applied.fetched,
        report.applied.undeclared,
        report.warnings.len()
    );
    assert_eq!(report.disposition, PullDisposition::Fetched, "{report:?}");
    assert_eq!(
        report.credential,
        PullCredential::Credentialed,
        "带凭据拉得到就不退匿名（票 35 第 8 条）"
    );
    assert!(report.applied.fetched > 0);
    assert_eq!(report.credential.note(), None, "带凭据那份没有任何额外提示");

    // 逐条 wire 只能来自随包文档表；文档表不认的保持未声明（不猜名字）。
    let models = list_models(&store, "opencode").await.unwrap();
    let from_doc = models
        .iter()
        .filter(|m| m.source == Source::Doc && !m.supported_wires.is_empty())
        .count();
    println!(
        "[t50] opencode 目录 {} 条，其中文档表判定 {from_doc} 条",
        models.len()
    );
    assert!(from_doc > 0, "文档表至少要认出几条（票 23）");
    for m in &models {
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
                "{} 是未声明的上游条目",
                m.model_id
            );
        } else {
            assert_eq!(m.source, Source::Doc, "{} 的来源应是 doc", m.model_id);
        }
    }

    // 同一条上游匿名拉一次：只打印条数（Go 面与凭据无关；Console 面 19/85 是权限过滤），
    // 证明「带凭据优先、404/空才退匿名」里那条兜底不会被走到。
    match pull_models(&http, &up, MODELS_PATH, None).await {
        PullOutcome::Fetched(v) => println!(
            "[t50] opencode 匿名那份 {} 条（带凭据那次 {} 条）",
            v.len(),
            report.applied.fetched
        ),
        other => println!("[t50] opencode 匿名那条：{other:?}"),
    }
    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

// ───────────────── 票 58 验收 2：真打一发带缓存写的请求，看它落库并计价 ─────────────────

/// 票 58 验收 2（**联网**，默认不跑）：真打 commandcode 的 Anthropic 面一发带
/// `cache_control: ephemeral` 的请求，走**产品侧同一条转发路径**（`forward_buffered`）——
/// 断言 ① 上游报的缓存写 token 落进了 `requests.cache_write`；② 四项价（含缓存写）
/// 算出 `cost_usd`；③ 随后把四项价全抬 10 倍 + 换快照，**这一行的数值一动不动**（不追古）。
///
/// 每次跑都换前缀：Anthropic 的 prompt cache 按前缀命中，同一个前缀第二次只会报 `cache_read`，
/// 换一个才有「缓存创建」可测（真打的成本是几十个 token 的钱）。
#[tokio::test]
#[ignore = "打真上游：cargo test -p gateway-core --test upstream_live -- --ignored --nocapture"]
async fn ticket58_a_real_cache_write_request_is_logged_and_priced() {
    use gateway_core::binding::BindingTable;
    use gateway_core::egress::provider::AuthScheme;
    use gateway_core::egress::upstream::{forward_buffered, Inbound};
    use gateway_core::ingress::proxy::Gateway;
    use gateway_core::obs::{self, LogQuery};
    use std::collections::HashMap;
    use std::sync::Arc;

    let env = EnvFile::load();
    let base = env.get("COMMANDCODE_BASE_URL");
    let key = env.get("COMMANDCODE_API_KEY");
    let root = tmp_root("t58");
    let store = Store::open(&root).expect("开库");

    let up = live_upstream("commandcode", &base);
    save_upstream(&store, &up).await.unwrap();
    save_manual_model(
        &store,
        "commandcode",
        "claude-sonnet-5-5",
        &[Wire::Anthropic],
    )
    .await
    .unwrap();
    // 手工四项价都手填（含第四项缓存写）——本票要证的就是第四项真的参与计价。
    let prices = Prices {
        input: Some(3.0),
        output: Some(15.0),
        cache_read: Some(0.30),
        cache_write: Some(3.75),
    };
    set_manual_prices(&store, "commandcode", "claude-sonnet-5-5", &prices)
        .await
        .unwrap();

    let salt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let filler: Vec<String> = (0..1200).map(|i| format!("token{i}")).collect();
    let prefix = format!("You are a careful assistant ({salt}). {}", filler.join(" "));
    let ir = gateway_core::anthropic::decode_request(&serde_json::json!({
        "model": "claude-sonnet-5-5",
        "max_tokens": 16,
        "stream": true,
        "system": [{"type": "text", "text": prefix, "cache_control": {"type": "ephemeral"}}],
        "messages": [{"role": "user", "content": "say hi"}],
    }))
    .unwrap();

    let app = Arc::new(Gateway::new(
        store.clone(),
        HashMap::new(),
        BindingTable::default(),
    ));
    let resp = forward_buffered(
        app,
        &up,
        Wire::Anthropic,
        AuthScheme::Bearer,
        &key,
        ir,
        "claude-haiku".into(),
        "claude-code".into(),
        Vec::new(),
        Inbound::Anthropic,
    )
    .await;
    assert_eq!(resp.status(), 200, "真上游这一发必须是 200");

    let rows = obs::list(&store, &LogQuery::default()).await.unwrap();
    let row = rows.first().expect("这一发必须落库").clone();
    println!("\n[t58] 真打一发：{row}");
    let cache_write = row["cache_write"]
        .as_i64()
        .unwrap_or_else(|| panic!("cache_write 必须落库（不是 NULL）：{row}"));
    assert!(cache_write > 0, "带 cache_control 的一发该有缓存写：{row}");
    let cache_read = row["cache_read"].as_i64().unwrap_or(0);
    let input = row["input_tokens"].as_i64().unwrap_or(0);
    let output = row["output_tokens"].as_i64().unwrap_or(0);

    // 独立手算：行里的四项 token × 手填的四项价（每百万 token 美元）。
    let per_mtok = |t: i64, p: f64| t as f64 * p / 1_000_000.0;
    let want = per_mtok(input, 3.0)
        + per_mtok(output, 15.0)
        + per_mtok(cache_read, 0.30)
        + per_mtok(cache_write, 3.75);
    let cost = row["cost_usd"]
        .as_f64()
        .unwrap_or_else(|| panic!("四项价都有 → 成本必须算得出来：{row}"));
    assert!(
        (cost - want).abs() < 1e-12,
        "成本 {cost} 与手算 {want} 不一致：{row}"
    );

    // 历史不重算：四项价全抬 10 倍 + 换一份快照，这一行一动不动。
    set_manual_prices(
        &store,
        "commandcode",
        "claude-sonnet-5-5",
        &Prices {
            input: Some(30.0),
            output: Some(150.0),
            cache_read: Some(3.0),
            cache_write: Some(37.5),
        },
    )
    .await
    .unwrap();
    import_catalog_snapshot(
        &store,
        r#"{"commandcode":{"models":{"claude-sonnet-5-5":{"name":"x","cost":{"input":99,"output":99,"cache_write":99}}}}}"#,
    )
    .await
    .unwrap();
    let after = obs::list(&store, &LogQuery::default()).await.unwrap();
    assert_eq!(
        after[0]["cost_usd"].as_f64().unwrap(),
        cost,
        "改价 / 刷新快照不许追古"
    );
    assert_eq!(after[0]["cache_write"].as_i64().unwrap(), cache_write);

    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
    println!(
        "[t58] cache_write={cache_write} cache_read={cache_read} in={input} out={output} → cost_usd=${cost:.6}（改价后仍是 ${cost:.6}）"
    );
}

// ───────────── 票 57 验收 3：结构化输出能力位探针打两条真上游 ─────────────

/// 带敌意提示词的探针在**两条真上游**的各协议面上实测，并给出能力位结论。
///
/// 判据不是状态码：`research/structured-output-wires.md` §3.3 实测 commandcode 的 Anthropic
/// 兼容端点在**6 个变体上全 200 且逐字静默忽略**——所以本测试把**探针原文（提示词 + 上游回答）**
/// 逐条打出来，那才是「ignore」的证据。
///
/// 硬断言只有三条（其余只打印，因为它们的结论取决于上游今天认不认这个面）：
/// - commandcode 的 Anthropic 面判出 `ignore`（验收要求）；
/// - 两家的 **Chat 面**判出 `native`（研究 §3.2 实测：敌意提示词被 schema 压过）。
#[tokio::test]
#[ignore = "打真上游：cargo test -p gateway-core --test upstream_live -- --ignored --nocapture"]
async fn ticket57_structured_output_probe_on_live_upstreams() {
    let env = EnvFile::load();
    let root = tmp_root("t57-structured");
    let store = Store::open(&root).expect("开库");
    let http = reqwest::Client::new();

    let mut oc = live_upstream("opencode", &env.get("OPENCODE_BASE_URL"));
    // opencode 的 Go 面要求自称 UA + 会话头（票 02 / 19 / 49），不带就是 400 `MissingSessionID`。
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

    // 面上限给宽：opencode 的 Chat 面实测 2～29 秒（票 51 验收 2 记过）。
    struct Case<'a> {
        id: &'a str,
        up: &'a Upstream,
        wire: Wire,
        model: &'a str,
        deadline: Duration,
        /// `None` = 只打印结论（这个面今天认不认取决于上游）。
        want: Option<StructuredOutput>,
    }
    let cases = [
        Case {
            id: "commandcode",
            up: &cc,
            wire: Wire::Anthropic,
            model: "claude-sonnet-5-5",
            deadline: Duration::from_secs(60),
            want: Some(StructuredOutput::Ignore),
        },
        Case {
            id: "commandcode",
            up: &cc,
            wire: Wire::OpenAiChat,
            model: "deepseek/deepseek-v4.1-flash",
            deadline: Duration::from_secs(60),
            want: Some(StructuredOutput::Native),
        },
        Case {
            id: "opencode",
            up: &oc,
            wire: Wire::OpenAiChat,
            model: "deepseek-v4.1-flash",
            deadline: Duration::from_secs(120),
            // 只打印，不硬断言：opencode 的 Go 面**间歇**回 400
            // `This response_format type is unavailable now`（2026-10-15 22:38 这次实测就撞上）；
            // 同一份 body 用 curl 直连连发 4 个变体全是 200 且 content = `{"a":"hello"}`
            // （见票据 Answer 的现场比对），所以结论是 native、那一发 400 是上游自己的调度门。
            want: None,
        },
        // 这两个面只打印：opencode 的 Anthropic 面按研究 §3.3 拿现有凭据是 401（未测出），
        // commandcode 的 Responses 面认不认 `text.format` 也没实测过。
        Case {
            id: "opencode",
            up: &oc,
            wire: Wire::Anthropic,
            model: "claude-haiku-5-5",
            deadline: Duration::from_secs(60),
            want: None,
        },
        Case {
            id: "commandcode",
            up: &cc,
            wire: Wire::OpenAiResponses,
            model: "claude-sonnet-5-5",
            deadline: Duration::from_secs(60),
            want: None,
        },
    ];

    println!("\n===== 票 57：结构化输出能力位探针（带 schema 的最短请求 + 敌意提示词）=====");
    let mut fails: Vec<String> = Vec::new();
    for Case {
        id,
        up,
        wire,
        model,
        deadline,
        want,
    } in cases
    {
        let r = structured::probe(&http, &store, up, wire, model, deadline).await;
        println!(
            "\n--- {id} / {model} / {} / 上限 {}s ---",
            wire.as_str(),
            deadline.as_secs()
        );
        println!("提示词原文：{}", r.prompt);
        println!(
            "上游回答（status={} / {}ms）：{:?}",
            r.status, r.elapsed_ms, r.answer
        );
        println!(
            "结论：能力位 = {:?}（{}）",
            r.mode.map(|m| m.as_str()),
            r.detail
        );
        if let Some(want) = want {
            if r.mode != Some(want) {
                fails.push(format!(
                    "{id} / {model} / {}：期望 {}，实际 {:?}（status={} answer={:?}）",
                    wire.as_str(),
                    want.as_str(),
                    r.mode.map(|m| m.as_str()),
                    r.status,
                    r.answer
                ));
            }
        }
    }

    // 结论落库：能力位按「上游 × 出站协议」各存一份。
    for id in ["commandcode", "opencode"] {
        let up = load_upstream(&store, id).await.unwrap().unwrap();
        for wire in Wire::ALL {
            println!(
                "[t57] 库里 {id} / {} → {:?}",
                wire.as_str(),
                up.structured_capability(wire)
                    .map(|c| (c.mode.as_str(), c.source.as_str()))
            );
        }
    }
    assert!(
        fails.is_empty(),
        "结构化输出能力位探针不合格：\n{}",
        fails.join("\n")
    );
    assert!(
        !dir_contains_bytes(&root, env.get("OPENCODE_API_KEY").as_bytes())
            && !dir_contains_bytes(&root, env.get("COMMANDCODE_API_KEY").as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

// ───────────── 票 65 验收 2：真打上游的 Responses 出站（流式 + 非流式） ─────────────

/// 上游 **Responses 出站**的真上游验收（**联网**，默认不跑）：从 commandcode 的 `/models` 里
/// 挑一条**声明了 `/responses`** 的模型（票 49 的反查把 `supported_endpoints` 映成 wire），
/// 走产品侧那条 HTTP 路由（客户端面 = **Anthropic**）真打流式 + 非流式各一发。
///
/// 请求体与帧序列从 obs 库（body 开关）读回来打印：`request_bodies.request_body` 是
/// **发给上游的逐字请求体**，`response_body` 是上游回的原始 SSE 帧。
/// **绝不打印密钥与 baseUrl**（只打模型 id / 帧数 / 词表 / usage）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "打真上游：cargo test -p gateway-core --test upstream_live -- --ignored --nocapture"]
async fn ticket65_responses_outbound_streams_and_buffers_against_a_live_upstream() {
    use gateway_core::binding::{default_profiles, Binding, BindingTable, Profile};
    use gateway_core::ingress::proxy::{Gateway, BODY_LOG_WINDOW};
    use gateway_core::obs::{self, LogQuery};
    use std::collections::HashMap;
    use std::sync::Arc;

    let env = EnvFile::load();
    let base = env.get("COMMANDCODE_BASE_URL");
    let key = env.get("COMMANDCODE_API_KEY");
    let root = tmp_root("t65-responses");
    let store = Store::open(&root).expect("开库");
    let http = reqwest::Client::new();

    let up = live_upstream("commandcode", &base);
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("commandcode", "default", &key)
        .await
        .unwrap();

    // 目录按真清单反查 wire（票 49 的口径），再挑一条声明了 `openai-responses` 的。
    let outcome = pull_models(&http, &up, MODELS_PATH, Some(&key)).await;
    assert!(matches!(outcome, PullOutcome::Fetched(_)), "{outcome:?}");
    apply_pull(&store, &up, outcome, 1_760_000_500)
        .await
        .unwrap();
    let models = list_models(&store, "commandcode").await.unwrap();
    let declares = models
        .iter()
        .filter(|m| m.supported_wires.contains(&Wire::OpenAiResponses))
        .count();
    let picked = models
        .iter()
        .find(|m| m.model_id == "gpt-6-luna" && m.supported_wires.contains(&Wire::OpenAiResponses))
        .or_else(|| {
            models
                .iter()
                .find(|m| m.supported_wires.contains(&Wire::OpenAiResponses))
        })
        .expect("真清单里该有声明 openai-responses 的模型")
        .model_id
        .clone();
    println!(
        "\n===== 票 65：真上游 Responses 出站（commandcode / {picked} / openai-responses）====="
    );
    println!(
        "（清单 {} 条，其中声明 /responses 的 {declares} 条）",
        models.len()
    );

    let mut upstreams = HashMap::new();
    upstreams.insert("commandcode".to_string(), up);
    let table = BindingTable::new(
        vec![Binding::new(
            Profile::Global,
            "live-responses",
            "commandcode",
            &picked,
            Wire::OpenAiResponses,
        )],
        default_profiles(),
    );
    let app = Arc::new(Gateway::new(store.clone(), upstreams, table));
    app.set_body_log(BODY_LOG_WINDOW.as_secs());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = gateway_core::ingress::proxy::router().with_state(app);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let ask = |stream: bool| {
        serde_json::json!({
            "model": "live-responses",
            "max_tokens": 512,
            "stream": stream,
            "system": "你是简洁的助手。",
            "messages": [{"role":"user","content":"只回四个字：收到收到"}],
        })
    };
    let post = |stream: bool| {
        let http = http.clone();
        async move {
            http.post(format!("http://{addr}/v1/messages"))
                .json(&ask(stream))
                .send()
                .await
                .expect("POST /v1/messages 失败")
        }
    };

    // ① 流式：客户端面（Anthropic）的帧要打到最后。
    let r = post(true).await;
    let status = r.status().as_u16();
    let stream_text = r.text().await.unwrap_or_default();
    assert_eq!(status, 200, "真上游流式这一发必须 200：{stream_text}");
    let events: Vec<&str> = stream_text
        .lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .collect();
    assert_eq!(
        events.last().copied(),
        Some("message_stop"),
        "要打到 message_stop：{stream_text}"
    );
    let answer: String = stream_text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d).ok())
        .filter_map(|v| {
            v.pointer("/delta/text")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
        .collect();
    println!("\n[流式] 客户端面帧序列：{}", events.join(" → "));
    println!("[流式] 正文：{answer:?}");

    // ② 非流式：同一条流缓到终态再组装成一条完整 message。
    let r = post(false).await;
    let status = r.status().as_u16();
    let body_text = r.text().await.unwrap_or_default();
    assert_eq!(status, 200, "真上游非流式这一发必须 200：{body_text}");
    let v: serde_json::Value = serde_json::from_str(&body_text).expect("非流式体是 JSON");
    println!(
        "[非流式] content={:?} stop_reason={} usage={}",
        v["content"][0]["text"], v["stop_reason"], v["usage"]
    );
    assert!(
        !v["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "非流式该有正文：{body_text}"
    );
    assert!(
        v["usage"]["input_tokens"].as_u64().unwrap_or(0) > 0,
        "usage 要从上游终态帧里解出来：{body_text}"
    );

    // ③ 落库的请求体与上游原始帧（body 开关那一份）。
    let rows = obs::list(&store, &LogQuery::default()).await.unwrap();
    assert!(rows.len() >= 2, "两发都该落库：{rows:?}");
    for row in rows.iter().take(2) {
        assert_eq!(row["wire"], "openai-responses", "{row}");
        assert_eq!(row["status"], 200, "{row}");
        let id = row["id"].as_i64().unwrap_or(0).to_string();
        let detail = obs::log_detail(&store, &id)
            .await
            .unwrap()
            .unwrap_or_default();
        let sent = detail["bodies"]["request"].as_str().unwrap_or_default();
        let raw = detail["bodies"]["response"].as_str().unwrap_or_default();
        println!(
            "\n[日志行 {}] 发给上游的请求体（非流式那发也一样是 stream:true）：",
            row["id"]
        );
        println!("{}", &sent[..sent.len().min(900)]);
        let vocab: Vec<&str> = raw
            .lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect();
        println!("[上游帧词表] {}", vocab.join(" → "));
        assert!(
            sent.contains("\"max_output_tokens\""),
            "请求体该是 Responses 形状：{sent}"
        );
        assert!(raw.contains("response.completed"), "上游该发到终态：{raw}");
    }

    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}

// ───────────── 票 64 验收 3（可选）：真上游的一条 Chat 入站 ─────────────

/// `POST /v1/chat/completions` 打一条**真上游**：解 Chat 请求 → IR → 出站 Chat wire →
/// 把上游帧编回 Chat SSE。走的是产品侧那条 HTTP 路由（`proxy::router()`，本进程里挂起来）。
///
/// 硬断言只有形状：HTTP 200、`data:` 帧、末帧 `[DONE]`、有正文、usage 报得出来，
/// 且**没有**「`choices: []` + usage」的单独 usage chunk（DeepSeek 的怪癖，spec §4.3）。
/// **绝不打印密钥与 baseUrl**：报告里只有模型 id、帧数、正文字数与 usage。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "打真上游：cargo test -p gateway-core --test upstream_live -- --ignored --nocapture"]
async fn ticket64_chat_inbound_against_a_live_upstream() {
    use gateway_core::binding::{default_profiles, Binding, BindingTable, Profile};
    use gateway_core::ingress::proxy::Gateway;
    use std::collections::HashMap;
    use std::sync::Arc;

    let env = EnvFile::load();
    let base_url = env.get("OPENCODE_BASE_URL");
    let key = env.get("OPENCODE_API_KEY");
    let root = tmp_root("t64-chat-inbound");
    let store = Store::open(&root).expect("开库");

    let up = {
        let mut up = live_upstream("opencode", &base_url);
        // opencode 的 Go 计划要求自称 UA + 带会话头（票 02 / 19 / 49）：不带就是 400
        // `MissingSessionID`。这是上游的硬要求，不是入站面的问题。
        up.quirks = vec![
            Quirk::ClientUserAgent("claude-gateway-prototype/0.0.1".into()),
            Quirk::SessionIdHeader("x-opencode-session".into()),
        ];
        up
    };
    save_upstream(&store, &up).await.unwrap();
    store
        .put_credential("opencode", "default", &key)
        .await
        .unwrap();

    let mut upstreams = HashMap::new();
    upstreams.insert("opencode".to_string(), up);
    let table = BindingTable::new(
        vec![Binding::new(
            Profile::Global,
            "live-chat",
            "opencode",
            "deepseek-v4.1-flash",
            Wire::OpenAiChat,
        )],
        default_profiles(),
    );
    let app = Arc::new(Gateway::new(store.clone(), upstreams, table));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = gateway_core::ingress::proxy::router().with_state(app);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "live-chat",
            "stream": true,
            "max_tokens": 128,
            "messages": [{"role":"user","content":"只回四个字：收到收到"}],
        }))
        .send()
        .await
        .expect("POST /v1/chat/completions 失败");
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();

    assert_eq!(status, 200, "真上游这一发必须 200：{text}");
    let frames: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|d| *d != "[DONE]")
        .filter_map(|d| serde_json::from_str(d).ok())
        .collect();
    assert!(
        text.trim_end().ends_with("data: [DONE]"),
        "Chat 流必须以 [DONE] 收尾：{text}"
    );

    let body: String = frames
        .iter()
        .filter_map(|f| f["choices"][0]["delta"]["content"].as_str())
        .collect();
    // deepseek-v4.1-flash 是推理模型：正文之前先发 `reasoning_content`（我们原样转发，
    // 与出站解码器读的是同一个键）。两者任一不为空就说明帧编对了。
    let reasoning: String = frames
        .iter()
        .filter_map(|f| f["choices"][0]["delta"]["reasoning_content"].as_str())
        .collect();
    let finish: Vec<&str> = frames
        .iter()
        .filter_map(|f| f["choices"][0]["finish_reason"].as_str())
        .collect();
    let usage = frames
        .iter()
        .find_map(|f| f.get("usage").cloned())
        .unwrap_or(serde_json::Value::Null);

    println!(
        "\n===== 票 64：真上游（opencode / deepseek-v4.1-flash / chat wire）的 Chat 入站 ====="
    );
    println!(
        "帧数 {}（正文 {} 字 / 推理 {} 字）；finish_reason={:?}；usage={usage}",
        frames.len(),
        body.chars().count(),
        reasoning.chars().count(),
        finish
    );

    assert!(
        !body.is_empty() || !reasoning.is_empty(),
        "真上游这一发该有 delta 载荷：{text}"
    );
    assert!(!finish.is_empty(), "要有 finish_reason：{text}");
    assert!(usage["prompt_tokens"].as_u64().unwrap_or(0) > 0, "{text}");
    assert!(
        !frames.iter().any(|f| f["choices"]
            .as_array()
            .map(|c| c.is_empty())
            .unwrap_or(false)),
        "没有单独的 usage chunk（DeepSeek 怪癖）：{text}"
    );
    assert!(
        !dir_contains_bytes(&root, key.as_bytes()),
        "库文件里不该出现明文密钥"
    );
}
