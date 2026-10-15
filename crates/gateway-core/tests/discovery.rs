//! 票 50 验收：模型发现的**分流与降级处置**（三种结局 / 重拉失败保留旧条目 / 告警角标与清除时机 /
//! 就绪判据不含拉取 / opencode 的凭据分支 / `noModelsEndpoint` 不必尝试）。
//!
//! 测试面 = `gateway_core::egress::discovery` + `egress::provider` 的公开接口 + `store::Store`。
//! 一律用临时目录，不碰真实 `%APPDATA%`；mock 上游 = 本进程内的 axum。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use gateway_core::egress::discovery::*;
use gateway_core::egress::provider::*;
use gateway_core::store::Store;
use serde_json::{json, Value};

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg50-discovery-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

async fn store(tag: &str) -> Store {
    Store::open(&tmp_root(tag)).expect("开库")
}

/// 声明式表：baseUrl 带版本前缀，路径模板相对它（与票 49 的用例同一形状）。
fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/messages");
    up.set_path(Wire::OpenAiChat, "/chat/completions");
    up.set_path(Wire::OpenAiResponses, "/responses");
    up
}

/// 起一个只会答 `path` 的 mock 上游，返回 base URL。
async fn mock(path: &'static str, status: u16, body: Value) -> String {
    let app = Router::new().route(
        path,
        get(move || {
            let body = body.clone();
            async move { (StatusCode::from_u16(status).unwrap(), Json(body)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[derive(Clone)]
struct AuthMock {
    hits: Arc<AtomicUsize>,
    /// 带凭据时的答复（status, body）
    credentialed: (u16, Value),
    anonymous: (u16, Value),
}

/// mock 上游：**按有没有带凭据**给两份不同的答复（opencode 19/85 那种权限过滤）。
async fn mock_by_auth(
    credentialed: (u16, Value),
    anonymous: (u16, Value),
) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let state = AuthMock {
        hits: hits.clone(),
        credentialed,
        anonymous,
    };
    let app = Router::new()
        .route(
            "/models",
            get(
                |State(st): State<AuthMock>, headers: HeaderMap| async move {
                    st.hits.fetch_add(1, Ordering::SeqCst);
                    let authed =
                        headers.contains_key("authorization") || headers.contains_key("x-api-key");
                    let (status, body) = if authed {
                        st.credentialed.clone()
                    } else {
                        st.anonymous.clone()
                    };
                    (StatusCode::from_u16(status).unwrap(), Json(body))
                },
            ),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), hits)
}

async fn pull_state(s: &Store, id: &str) -> PullState {
    load_upstream(s, id)
        .await
        .unwrap()
        .unwrap()
        .last_pull
        .expect("拉取过后上游记录上应有上次拉取状态")
}

// ───────────────── ① 三种结局各一条用例（票 50 验收 1） ─────────────────

#[tokio::test]
async fn three_outcomes_are_branched_apart() {
    let http = reqwest::Client::new();

    // ① 成功：上游报端点 → 反查成 wire，目录入库。
    let s = store("outcome-ok").await;
    let up = upstream(
        "cc",
        &mock(
            "/models",
            200,
            json!({ "data": [
                { "id": "claude-sonnet-5-5", "supported_endpoints": ["/messages"] },
                { "id": "gpt-6-astra", "supported_endpoints": ["/chat/completions"] }
            ]}),
        )
        .await,
    );
    let r = run_pull(&s, &http, &up, 100).await.expect("拉取不该报错");
    assert_eq!(r.disposition, PullDisposition::Fetched, "{r:?}");
    assert_eq!(r.credential, PullCredential::Anonymous, "没凭据就尽力而为");
    assert_eq!(r.applied.added, 2);
    assert!(r.message.contains('2'), "结果行要带条数：{}", r.message);
    assert!(r.offers(PullAction::AddManualModel), "目录随时可手改");
    assert!(!r.offers(PullAction::Retry), "成功后不给重试按钮");
    assert_eq!(r.reuse_at, None, "成功就不叫沿用上次结果");

    let models = list_models(&s, "cc").await.unwrap();
    assert_eq!(models.len(), 2);
    let sonnet = models
        .iter()
        .find(|m| m.model_id == "claude-sonnet-5-5")
        .unwrap();
    assert_eq!(sonnet.supported_wires, [Wire::Anthropic].into());
    assert_eq!(sonnet.source, Source::Upstream);

    // ② 该上游不提供模型发现：404 / 405 / 200 零条 → 软提示 + 手填入口，不报红。
    for (tag, status, body) in [
        ("outcome-404", 404u16, json!({ "error": "not found" })),
        ("outcome-405", 405u16, json!({})),
        ("outcome-empty", 200u16, json!({ "data": [] })),
    ] {
        let s = store(tag).await;
        let up = upstream("oc", &mock("/models", status, body.clone()).await);
        let r = run_pull(&s, &http, &up, 200).await.expect("软提示不是错误");
        assert_eq!(r.disposition, PullDisposition::NoDiscovery, "{tag}: {r:?}");
        assert!(r.offers(PullAction::AddManualModel), "{tag}: 要手填入口");
        assert!(!r.offers(PullAction::Retry), "{tag}: 不是失败，不给重试");
        assert!(r.message.contains("不提供模型发现"), "{tag}: {}", r.message);
        assert!(r.message.contains("手填模型 ID"), "{tag}: {}", r.message);
        assert!(
            load_upstream(&s, "oc").await.unwrap().is_some(),
            "{tag}: 上游仍可保存"
        );
        assert_eq!(
            pull_state(&s, "oc").await.status,
            PullStatus::NoDiscovery,
            "{tag}"
        );
    }

    // ③ 失败：其他非 2xx → 失败提示 + 内联「手工添加模型」+「重试」；上游仍可保存。
    let s = store("outcome-503").await;
    let up = upstream("cc", &mock("/models", 503, json!({})).await);
    let r = run_pull(&s, &http, &up, 300)
        .await
        .expect("失败也要回执，不 Err");
    assert_eq!(r.disposition, PullDisposition::Failed, "{r:?}");
    assert!(r.message.contains("拉取失败"), "{}", r.message);
    assert!(
        r.offers(PullAction::AddManualModel),
        "失败提示里内联手工添加"
    );
    assert!(r.offers(PullAction::Retry), "失败提示里给重试按钮");
    assert!(load_upstream(&s, "cc").await.unwrap().is_some());
    assert_eq!(pull_state(&s, "cc").await.status, PullStatus::Failed);
    let logged: i64 = s
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(
        logged, 0,
        "拉取失败不进概览「最近错误」（那是请求错误的地盘）"
    );
}

// ───────────────── ② 重拉失败保留旧条目 + 角标清除时机（票 50 验收 2） ─────────────────

#[tokio::test]
async fn repull_failure_keeps_old_entries_and_the_badge_clears_only_on_success() {
    let http = reqwest::Client::new();
    let s = store("repull").await;

    // 先成功一次，顺手留一条 wire 反查警告（/embeddings 对不上任何模板）。
    let up = upstream(
        "cc",
        &mock(
            "/models",
            200,
            json!({ "data": [
                { "id": "a-1", "supported_endpoints": ["/messages"] },
                { "id": "weird-1", "supported_endpoints": ["/embeddings"] }
            ]}),
        )
        .await,
    );
    let ok = run_pull(&s, &http, &up, 1_000).await.unwrap();
    assert_eq!(ok.disposition, PullDisposition::Fetched);
    // 用户在这一步手改了一条（声明 Anthropic 面）：此后重拉不许覆盖它。
    save_manual_model(&s, "cc", "my-handmade", &[Wire::Anthropic])
        .await
        .unwrap();
    assert_eq!(ok.warnings.len(), 1, "查不到的端点内联一条警告");
    assert!(ok.warnings[0].contains("/embeddings"), "{}", ok.warnings[0]);
    let st = pull_state(&s, "cc").await;
    assert_eq!(st.status, PullStatus::Ok);
    assert_eq!(st.ok_at, Some(1_000));
    assert!(
        st.warning.as_deref().unwrap().contains("/embeddings"),
        "警告落进「上次拉取警告」，卡片/列表据此亮角标：{:?}",
        st.warning
    );
    assert_eq!(list_models(&s, "cc").await.unwrap().len(), 3);

    // 重拉失败：旧条目一条不少（含手改锁定那条），角标换成失败原因，提示沿用上次结果（1000）。
    let down = upstream("cc", &mock("/models", 503, json!({})).await);
    let r = run_pull(&s, &http, &down, 2_000).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Failed);
    assert_eq!(r.reuse_at, Some(1_000), "只提示「沿用上次结果（时间）」");
    assert_eq!(
        list_models(&s, "cc").await.unwrap().len(),
        3,
        "一次抖动不清库"
    );
    let st = pull_state(&s, "cc").await;
    assert_eq!(st.status, PullStatus::Failed);
    assert_eq!(st.at, 2_000, "上次尝试时间");
    assert_eq!(st.ok_at, Some(1_000), "上次成功时间不因失败而丢");
    assert!(
        st.warning.as_deref().unwrap().contains("拉取失败"),
        "角标换成失败原因：{:?}",
        st.warning
    );

    // 「不提供模型发现」也不是成功：不清角标、不动目录。
    let nd = upstream("cc", &mock("/models", 404, json!({})).await);
    let r = run_pull(&s, &http, &nd, 3_000).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::NoDiscovery);
    assert_eq!(r.reuse_at, Some(1_000));
    assert_eq!(list_models(&s, "cc").await.unwrap().len(), 3);
    let st = pull_state(&s, "cc").await;
    assert_eq!(st.ok_at, Some(1_000));
    assert!(st.warning.is_some(), "只有成功拉取才清角标");

    // 下次成功：角标清除、成功时间前进；手改锁定条目**在拉取清单里也不被覆盖**。
    let fine = upstream(
        "cc",
        &mock(
            "/models",
            200,
            json!({ "data": [
                { "id": "a-1", "supported_endpoints": ["/messages"] },
                { "id": "my-handmade", "supported_endpoints": ["/chat/completions"] }
            ]}),
        )
        .await,
    );
    let r = run_pull(&s, &http, &fine, 4_000).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Fetched);
    assert!(r.warnings.is_empty());
    assert_eq!(r.applied.kept_manual, 1, "手改过的条目锁定不覆盖");
    assert_eq!(r.applied.removed, 1, "上游不再报的 weird-1 清掉");
    let st = pull_state(&s, "cc").await;
    assert_eq!(st.status, PullStatus::Ok);
    assert_eq!(st.warning, None, "下次成功拉取清除角标");
    assert_eq!(st.ok_at, Some(4_000));
    let models = list_models(&s, "cc").await.unwrap();
    let handmade = models
        .iter()
        .find(|m| m.model_id == "my-handmade")
        .expect("手改条目不该被重拉清掉");
    assert_eq!(handmade.source, Source::Manual);
    assert_eq!(
        handmade.supported_wires,
        [Wire::Anthropic].into(),
        "上游这次报的是 openai-chat，手改条目仍按手填的 anthropic"
    );
    assert_eq!(models.len(), 2, "上游报的 a-1 + 手改的 my-handmade");
}

// ───────────────── ③ 角标与拉取失败不计入侧栏 / 就绪（票 50 验收） ─────────────────

#[tokio::test]
async fn pull_failure_and_warnings_never_count_toward_unready() {
    let http = reqwest::Client::new();
    let s = store("tally").await;

    // 两条上游都从未自检；拉取一次失败（角标亮）。
    for id in ["a", "b"] {
        let up = upstream(id, &mock("/models", 503, json!({})).await);
        run_pull(&s, &http, &up, 1).await.unwrap();
    }
    let t = upstream_tally(&s).await.unwrap();
    assert_eq!(t.total, 2);
    assert_eq!(t.selfcheck_passed, 0);
    assert_eq!(t.selfcheck_failed(), 2, "没自检过 = 未通过自检");
    assert_eq!(t.with_pull_warning, 2, "角标是另一维事实");
    assert!(!t.ready(), "拉取失败不算就绪");

    // 一条自检通过：侧栏少一个未通过、概览第一步就绪。
    set_selfcheck_result(&s, "a", true, 9).await.unwrap();
    let t = upstream_tally(&s).await.unwrap();
    assert_eq!(t.selfcheck_failed(), 1, "判据只看自检");
    assert_eq!(t.with_pull_warning, 2, "角标不影响这个数");
    assert!(t.ready(), "就绪 = 至少一条已保存且自检通过");

    // 自检失败的上游照样算未通过；拉取成功也不改这个数。
    set_selfcheck_result(&s, "b", false, 9).await.unwrap();
    let ok = upstream(
        "b",
        &mock(
            "/models",
            200,
            json!({ "data": [{ "id": "x-1", "supported_endpoints": ["/messages"] }] }),
        )
        .await,
    );
    run_pull(&s, &http, &ok, 2).await.unwrap();
    let t = upstream_tally(&s).await.unwrap();
    assert_eq!(t.selfcheck_failed(), 1);
    assert_eq!(t.with_pull_warning, 1, "b 这次成功拉取清了自己的角标");
}

// ───────────────── ④ `noModelsEndpoint`：不必尝试（spec §6 / 票 35 第 2 条） ─────────────────

#[tokio::test]
async fn no_models_endpoint_quirk_skips_the_request_and_asks_for_a_hand_written_id() {
    let http = reqwest::Client::new();
    let s = store("quirk-skip").await;
    // 上游答得出来，但它声明了「不提供模型发现」：这样一次网络都不该打。
    let (base, hits) =
        mock_by_auth((200, json!({ "data": [] })), (200, json!({ "data": [] }))).await;
    let mut up = upstream("qwen", &base);
    up.quirks = vec![Quirk::NoModelsEndpoint];

    let r = run_pull(&s, &http, &up, 7).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::NoDiscovery);
    assert_eq!(r.credential, PullCredential::NotAttempted, "压根没打");
    assert!(r.message.contains("noModelsEndpoint"), "{}", r.message);
    assert!(r.offers(PullAction::AddManualModel));
    assert_eq!(pull_state(&s, "qwen").await.status, PullStatus::NoDiscovery);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "noModelsEndpoint 的含义就是「不必尝试」（spec §6 / 票 35 第 2 条）"
    );
}

// ───────────────── ⑤ opencode 的凭据分支（票 35 第 8 条 / 票 50 验收 3） ─────────────────

/// 声明了「模型清单只认带凭据那份」的上游，没有凭据：**一次网络都不打**。
#[tokio::test]
async fn credential_scoped_upstream_without_a_key_does_not_pull() {
    let http = reqwest::Client::new();
    let s = store("no-key").await;
    let (base, hits) = mock_by_auth(
        (404, json!({})),
        (
            200,
            json!({ "data": [{ "id": "unauthed-1", "supported_endpoints": ["/messages"] }] }),
        ),
    )
    .await;
    let mut up = upstream("oc", &base);
    up.quirks = vec![Quirk::ModelsListNeedsCredential];

    let r = run_pull(&s, &http, &up, 11).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::NoCredential, "{r:?}");
    assert_eq!(r.credential, PullCredential::NotAttempted);
    assert!(r.message.contains("key"), "{}", r.message);
    assert!(r.offers(PullAction::FillCredential));
    assert!(r.offers(PullAction::AddManualModel));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "没凭据就不拉：匿名那份是未授权的全量目录（票 35 第 8 条）"
    );
    assert_eq!(list_models(&s, "oc").await.unwrap().len(), 0, "不动目录");
}

/// 没声明那条 quirk 的上游没有凭据时：尽力而为地匿名拉（不是所有上游都要 key）。
#[tokio::test]
async fn plain_upstream_without_a_key_still_tries_anonymously() {
    let http = reqwest::Client::new();
    let s = store("plain-anon").await;
    let (base, hits) = mock_by_auth(
        (404, json!({})),
        (
            200,
            json!({ "data": [{ "id": "x-1", "supported_endpoints": ["/messages"] }] }),
        ),
    )
    .await;
    let up = upstream("local", &base);

    let r = run_pull(&s, &http, &up, 12).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Fetched);
    assert_eq!(r.credential, PullCredential::Anonymous);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "只打一次");
    assert!(r.credential.note().unwrap().contains("匿名"));
}

#[cfg(windows)]
#[tokio::test]
async fn credentialed_pull_is_tried_first_and_only_falls_back_on_no_discovery() {
    let http = reqwest::Client::new();

    // ① 带凭据就拉得到：只打一次，不退匿名。
    let s = store("keyed-ok").await;
    let (base, hits) = mock_by_auth(
        (
            200,
            json!({ "data": [{ "id": "authorized-1", "supported_endpoints": ["/messages"] }] }),
        ),
        (
            200,
            json!({ "data": [{ "id": "anon-1", "supported_endpoints": ["/messages"] }] }),
        ),
    )
    .await;
    let mut up = upstream("oc", &base);
    up.quirks = vec![Quirk::ModelsListNeedsCredential];
    // 凭据表对上游有外键：向导也是先存了上游那行才能填 key（票 52 的顺序）。
    save_upstream(&s, &up).await.unwrap();
    s.put_credential("oc", "default", "sk-test-000000")
        .await
        .unwrap();

    let r = run_pull(&s, &http, &up, 20).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Fetched);
    assert_eq!(r.credential, PullCredential::Credentialed, "{r:?}");
    assert_eq!(r.credential.note(), None, "带了凭据就没有额外提示");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "带凭据拉得到就不退匿名");
    let ids: Vec<String> = list_models(&s, "oc")
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.model_id)
        .collect();
    assert_eq!(ids, ["authorized-1"], "入库的是带凭据那份目录");

    // ② 带凭据 404 / 零条：才退匿名一次，并如实标注这份目录是未授权的。
    let s = store("keyed-fallback").await;
    let (base, hits) = mock_by_auth(
        (404, json!({})),
        (
            200,
            json!({ "data": [
                { "id": "anon-1", "supported_endpoints": ["/messages"] },
                { "id": "anon-2", "supported_endpoints": ["/chat/completions"] }
            ]}),
        ),
    )
    .await;
    let mut up = upstream("oc", &base);
    up.quirks = vec![Quirk::ModelsListNeedsCredential];
    save_upstream(&s, &up).await.unwrap();
    s.put_credential("oc", "default", "sk-test-000000")
        .await
        .unwrap();

    let r = run_pull(&s, &http, &up, 21).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Fetched);
    assert_eq!(r.credential, PullCredential::AnonymousFallback, "{r:?}");
    assert!(
        r.credential.note().unwrap().contains("403"),
        "界面要说明这份目录是未授权的：{:?}",
        r.credential.note()
    );
    assert_eq!(r.applied.added, 2);
    assert_eq!(hits.load(Ordering::SeqCst), 2, "带凭据一次 + 退匿名一次");

    // ③ 带凭据 404 且匿名也 404：不提供模型发现，且不谎称「目录是匿名的」。
    let s = store("keyed-both-404").await;
    let (base, _) = mock_by_auth((404, json!({})), (404, json!({}))).await;
    let mut up = upstream("oc", &base);
    up.quirks = vec![Quirk::ModelsListNeedsCredential];
    save_upstream(&s, &up).await.unwrap();
    s.put_credential("oc", "default", "sk-test-000000")
        .await
        .unwrap();
    let r = run_pull(&s, &http, &up, 22).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::NoDiscovery);
    assert_eq!(r.credential, PullCredential::NotAttempted, "没拉到目录");
}

#[cfg(windows)]
#[tokio::test]
async fn several_credentials_without_a_default_ask_the_ui_to_pick_one() {
    let http = reqwest::Client::new();
    let s = store("pick-wire").await;
    let (base, hits) = mock_by_auth((404, json!({})), (404, json!({}))).await;
    let up = upstream("oc", &base);
    save_upstream(&s, &up).await.unwrap();
    s.put_credential("oc", "anthropic", "sk-test-000000")
        .await
        .unwrap();
    s.put_credential("oc", "openai-chat", "sk-test-111111")
        .await
        .unwrap();

    let r = run_pull(&s, &http, &up, 30).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::CredentialChoice, "{r:?}");
    assert_eq!(r.choose_from, ["anthropic", "openai-chat"], "给下拉的取值");
    assert!(r.offers(PullAction::ChooseCredential));
    assert!(r.offers(PullAction::AddManualModel));
    assert_eq!(hits.load(Ordering::SeqCst), 0, "不瞎猜，等界面选");

    // 补一份 default：从此 default 优先，不再问。
    s.put_credential("oc", "default", "sk-test-222222")
        .await
        .unwrap();
    let (base, hits) = mock_by_auth(
        (
            200,
            json!({ "data": [{ "id": "authorized-1", "supported_endpoints": ["/messages"] }] }),
        ),
        (200, json!({})),
    )
    .await;
    let up = upstream("oc", &base);
    let r = run_pull(&s, &http, &up, 31).await.unwrap();
    assert_eq!(r.disposition, PullDisposition::Fetched);
    assert_eq!(r.credential, PullCredential::Credentialed);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
