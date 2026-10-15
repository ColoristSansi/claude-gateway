//! 票 49 验收单测：三层校验 / 路径反查 / 价格优先级 / 快照刷新 / 目录与拉取 / 删引用检查。
//!
//! 测试面 = `gateway_core::egress::provider` 的公开接口 + `store::Store`（票 42）。
//! 一律用临时目录，不碰真实 `%APPDATA%`；mock 上游用本进程内的 axum。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use axum::routing::get;
use axum::{Json, Router};
use gateway_core::egress::provider::*;
use gateway_core::store::Store;
use serde_json::json;

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg49-provider-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

async fn store(tag: &str) -> Store {
    Store::open(&tmp_root(tag)).expect("开库")
}

/// 一份「像真上游那样」的声明式表：baseUrl 带版本前缀，路径模板相对它。
fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/messages");
    up.set_path(Wire::OpenAiChat, "/chat/completions");
    up.set_path(Wire::OpenAiResponses, "/responses");
    up
}

/// 起一个只会答 `path` 的 mock 上游，返回 base URL。
async fn mock_upstream(path: &'static str, status: u16, body: serde_json::Value) -> String {
    let app = Router::new().route(
        path,
        get(move || {
            let body = body.clone();
            async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    Json(body),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

// ───────────────────────── ① 三层校验 ─────────────────────────

#[test]
fn config_period_only_offers_declared_wires() {
    let entry = UpstreamModel {
        upstream_id: "cc".into(),
        model_id: "gpt-6-astra".into(),
        supported_wires: [Wire::OpenAiChat, Wire::OpenAiResponses].into(),
        source: Source::Upstream,
    };
    // 配置期主拦：下拉里只给声明过的值。
    assert_eq!(
        entry.allowed_wires(),
        vec![Wire::OpenAiChat, Wire::OpenAiResponses]
    );
    assert!(entry.validate_wire(Wire::OpenAiChat).is_ok());
    assert!(
        entry.validate_wire(Wire::Anthropic).is_err(),
        "没声明 anthropic 就不许选"
    );
}

#[test]
fn request_period_rejects_undeclared_wire_with_400() {
    let entry = UpstreamModel {
        upstream_id: "cc".into(),
        model_id: "claude-sonnet-5-5".into(),
        supported_wires: [Wire::Anthropic].into(),
        source: Source::Upstream,
    };
    let rej = entry
        .validate_wire(Wire::OpenAiChat)
        .expect_err("声明里没有 openai-chat，请求期必须早拒");
    assert_eq!(rej.status, 400);
    assert_eq!(rej.error_type, "invalid_request_error");
    assert!(rej.message.contains("claude-sonnet-5-5"), "{}", rej.message);
    assert!(rej.message.contains("openai-chat"), "{}", rej.message);
    assert!(rej.message.contains("anthropic"), "{}", rej.message);
}

#[tokio::test]
async fn undeclared_is_normal_and_selfcheck_is_the_tiebreaker() {
    let s = store("undeclared").await;
    let up = upstream("oc", "https://oc.example");
    save_upstream(&s, &up).await.unwrap();
    // 空集合 = 未声明：可自由选，但标「未声明，靠自检」。
    let entry = UpstreamModel {
        upstream_id: "oc".into(),
        model_id: "deepseek-v4.1-flash".into(),
        supported_wires: BTreeSet::new(),
        source: Source::Upstream,
    };
    assert_eq!(
        entry.validate_wire(Wire::OpenAiResponses).unwrap(),
        WireChoice::Undeclared
    );
    assert_eq!(WireChoice::Undeclared.note(), Some("未声明，靠自检"));
    assert_eq!(WireChoice::Declared.note(), None);

    // 自检为准：结果落在上游记录上（第三层）。
    set_selfcheck_result(&s, "oc", false, 1_760_000_000)
        .await
        .unwrap();
    let loaded = load_upstream(&s, "oc").await.unwrap().unwrap();
    assert_eq!(
        loaded.last_selfcheck,
        Some(SelfCheck {
            at: 1_760_000_000,
            ok: false
        })
    );
}

// ───────────────────────── ② pathTemplates 反查 ─────────────────────────

#[test]
fn endpoint_paths_reverse_lookup_to_wires() {
    let up = upstream("cc", "https://cc.example/provider/v1");
    // 精确命中 + 带版本前缀的期望路径；上游报的是短路径。
    assert_eq!(
        wire_for_endpoint(&up, "/messages"),
        Some(Wire::Anthropic),
        "上游报 /messages，我们的模板是 /messages"
    );
    assert_eq!(
        wire_for_endpoint(&up, "/v1/messages"),
        Some(Wire::Anthropic),
        "上游报 /v1/messages 也要认（模板 /messages 是它的后缀）"
    );
    assert_eq!(
        wire_for_endpoint(&up, "/chat/completions"),
        Some(Wire::OpenAiChat)
    );
    assert_eq!(
        wire_for_endpoint(&up, "/responses?x=1"),
        Some(Wire::OpenAiResponses),
        "带查询串也要认"
    );

    let (wires, warnings) = reverse_lookup(
        &up,
        &[
            "/messages".into(),
            "/chat/completions".into(),
            "/embeddings".into(),
        ],
    );
    assert_eq!(
        wires,
        [Wire::Anthropic, Wire::OpenAiChat].into(),
        "命中两条"
    );
    assert_eq!(warnings.len(), 1, "查不到的那条记一条警告：{warnings:?}");
    assert!(warnings[0].contains("/embeddings"), "{}", warnings[0]);
    assert!(
        warnings[0].contains("/messages"),
        "警告要带上我们期望的路径：{}",
        warnings[0]
    );
}

// ───────────────────────── ③ 价格优先级 ─────────────────────────

#[test]
fn price_priority_is_manual_then_snapshot_then_unknown() {
    let manual = Prices {
        input: Some(9.0),
        output: None,
        cache_read: None,
        cache_write: None,
    };
    let snapshot = Prices {
        input: Some(1.0),
        output: Some(5.0),
        cache_read: Some(0.1),
        cache_write: None,
    };
    let r = resolve_prices(&manual, Some(&snapshot));
    assert_eq!(r.input.value(), Some(9.0), "手工价压过快照");
    assert_eq!(r.input.source, PriceSource::Manual);
    assert_eq!(r.output.value(), Some(5.0), "手工没填的项回退快照");
    assert_eq!(r.output.source, PriceSource::Snapshot);
    assert_eq!(r.cache_read.value(), Some(0.1));
    assert_eq!(r.cache_write.value(), None, "快照也没有就是未知，不显示 0");
    assert_eq!(r.cache_write.source, PriceSource::Unknown);

    // 连快照都没有：四项全未知。
    let only_manual = resolve_prices(&manual, None);
    assert_eq!(only_manual.output.source, PriceSource::Unknown);
    assert_eq!(only_manual.input.value(), Some(9.0));
}

#[tokio::test]
async fn snapshot_refresh_never_overwrites_locked_manual_items() {
    let s = store("prices").await;
    let up = upstream("u1", "https://u1.example");
    save_upstream(&s, &up).await.unwrap();

    let imported = import_catalog_snapshot(&s, BUNDLED_MODELS_DEV_SNAPSHOT)
        .await
        .unwrap();
    assert!(imported > 0, "随包快照要能导进 model_catalog");
    let snap = catalog_prices(&s, "anthropic", "claude-haiku-4-5")
        .await
        .unwrap()
        .expect("随包快照里有 claude-haiku-4-5");
    assert_eq!(snap.input, Some(1.0));
    assert_eq!(snap.cache_write, Some(1.25));

    // 手工只填「输入」；刷新快照后输入仍是手工值（锁定），其余跟着快照。
    set_manual_prices(
        &s,
        "u1",
        "claude-haiku-4-5",
        &Prices {
            input: Some(9.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let before = resolved_prices(&s, "u1", "claude-haiku-4-5").await.unwrap();
    assert_eq!(before.input.value(), Some(9.0));
    assert_eq!(before.input.source, PriceSource::Manual);
    assert_eq!(before.output.source, PriceSource::Snapshot);

    // 刷新换掉整张快照（模拟设置页手动刷新成功）。
    let base = mock_upstream(
        "/api.json",
        200,
        json!({ "anthropic": { "id": "anthropic", "models": {
            "claude-haiku-4-5": { "id": "claude-haiku-4-5", "name": "Claude Haiku 4.5",
                                   "cost": { "input": 2.0, "output": 7.0, "cache_read": 0.2 } } } } }),
    )
    .await;
    let http = reqwest::Client::new();
    let refreshed = refresh_catalog(&s, &http, &format!("{base}/api.json"))
        .await
        .expect("刷新成功");
    assert_eq!(refreshed.imported, 1);

    let after = resolved_prices(&s, "u1", "claude-haiku-4-5").await.unwrap();
    assert_eq!(after.input.value(), Some(9.0), "手工锁定项不该被刷新覆盖");
    assert_eq!(after.input.source, PriceSource::Manual);
    assert_eq!(after.output.value(), Some(7.0), "未锁定的项跟新快照");
    assert_eq!(
        after.cache_write.source,
        PriceSource::Unknown,
        "新快照没这项"
    );
    assert_eq!(
        catalog_snapshot_at(&s).await.unwrap(),
        Some(refreshed.at),
        "刷新时间写进 settings.catalog_snapshot_at"
    );
}

#[tokio::test]
async fn catalog_refresh_failure_keeps_the_old_snapshot() {
    let s = store("refresh-fail").await;
    import_catalog_snapshot(&s, BUNDLED_MODELS_DEV_SNAPSHOT)
        .await
        .unwrap();
    let before = catalog_prices(&s, "anthropic", "claude-haiku-4-5")
        .await
        .unwrap()
        .unwrap();

    let base = mock_upstream("/api.json", 500, json!({ "error": "boom" })).await;
    let http = reqwest::Client::new();
    let failed = refresh_catalog(&s, &http, &format!("{base}/api.json")).await;
    assert!(failed.is_err(), "非 2xx 算失败");

    let after = catalog_prices(&s, "anthropic", "claude-haiku-4-5")
        .await
        .unwrap()
        .expect("失败要沿用旧快照，不能清空");
    assert_eq!(after.input, before.input);
    assert_eq!(
        catalog_snapshot_at(&s).await.unwrap(),
        None,
        "失败不写刷新时间"
    );
}

// ───────────────────────── ④ 目录：拉取与来源 ─────────────────────────

#[tokio::test]
async fn pull_maps_endpoints_to_wires_and_records_warnings() {
    let s = store("pull").await;
    let mut up = upstream("cc", "https://cc.example/provider/v1");
    save_upstream(&s, &up).await.unwrap();
    let html = json!({ "data": [
        { "id": "claude-sonnet-5-5", "object": "model", "supported_endpoints": ["/messages"] },
        { "id": "gpt-6-astra", "object": "model",
          "supported_endpoints": ["/chat/completions", "/responses"] },
        { "id": "weird-1", "object": "model", "supported_endpoints": ["/embeddings"] },
        { "id": "silent-1", "object": "model" }
    ]});
    let base = mock_upstream("/models", 200, html).await;
    up.base_url = base;

    let http = reqwest::Client::new();
    let outcome = pull_models(&http, &up, MODELS_PATH, None).await;
    let applied = apply_pull(&s, &up, outcome, 1_760_000_001).await.unwrap();
    assert_eq!(applied.added, 4);
    assert_eq!(applied.warnings.len(), 1, "{:?}", applied.warnings);

    let models = list_models(&s, "cc").await.unwrap();
    let get = |id: &str| models.iter().find(|m| m.model_id == id).unwrap().clone();
    assert_eq!(
        get("claude-sonnet-5-5").supported_wires,
        [Wire::Anthropic].into()
    );
    assert_eq!(
        get("gpt-6-astra").supported_wires,
        [Wire::OpenAiChat, Wire::OpenAiResponses].into()
    );
    assert_eq!(get("gpt-6-astra").source, Source::Upstream);
    assert!(
        get("weird-1").supported_wires.is_empty(),
        "反查不到的路径忽略，落成未声明"
    );
    assert!(get("silent-1").supported_wires.is_empty());

    // 警告落进上游记录的「上次拉取警告」，成功拉取会写时间与状态。
    let loaded = load_upstream(&s, "cc").await.unwrap().unwrap();
    let pull = loaded.last_pull.expect("拉取结果");
    assert_eq!(pull.status, PullStatus::Ok);
    assert_eq!(pull.at, 1_760_000_001);
    assert!(pull.warning.unwrap().contains("/embeddings"));
}

#[tokio::test]
async fn repull_keeps_manual_entries_and_clears_warnings_on_success() {
    let s = store("repull").await;
    let mut up = upstream("cc", "https://cc.example/provider/v1");
    save_upstream(&s, &up).await.unwrap();

    // 手工新增一条（manual 锁定），再拉一次：不许被覆盖。
    save_manual_model(&s, "cc", "my-handmade", &[Wire::Anthropic])
        .await
        .unwrap();
    let html = json!({ "data": [
        { "id": "my-handmade", "object": "model", "supported_endpoints": ["/chat/completions"] },
        { "id": "a-1", "object": "model", "supported_endpoints": ["/messages"] }
    ]});
    let base = mock_upstream("/models", 200, html).await;
    up.base_url = base;
    let http = reqwest::Client::new();
    let applied = apply_pull(
        &s,
        &up,
        pull_models(&http, &up, MODELS_PATH, None).await,
        10,
    )
    .await
    .unwrap();
    assert_eq!(applied.added, 1, "只有 a-1 是新的");
    assert_eq!(applied.kept_manual, 1, "手工条目留着");
    let models = list_models(&s, "cc").await.unwrap();
    let handmade = models.iter().find(|m| m.model_id == "my-handmade").unwrap();
    assert_eq!(handmade.source, Source::Manual, "manual 锁定不覆盖");
    assert_eq!(
        handmade.supported_wires,
        [Wire::Anthropic].into(),
        "手工声明的 wire 也不动"
    );

    // 重拉失败：旧条目仍在，状态是 failed，警告写进去。
    let down = mock_upstream("/models", 503, json!({})).await;
    up.base_url = down;
    let failed = pull_models(&http, &up, MODELS_PATH, None).await;
    assert!(matches!(failed, PullOutcome::Failed(_)), "{failed:?}");
    let applied = apply_pull(&s, &up, failed, 11).await.unwrap();
    assert_eq!(applied.removed, 0);
    assert_eq!(list_models(&s, "cc").await.unwrap().len(), 2);
    let loaded = load_upstream(&s, "cc").await.unwrap().unwrap();
    assert_eq!(loaded.last_pull.unwrap().status, PullStatus::Failed);
    assert_eq!(list_models(&s, "cc").await.unwrap().len(), 2);

    // 下次成功拉取：警告自动清除。
    up.base_url = mock_upstream(
        "/models",
        200,
        json!({ "data": [{ "id": "a-1", "object": "model", "supported_endpoints": ["/messages"] }] }),
    )
    .await;
    apply_pull(
        &s,
        &up,
        pull_models(&http, &up, MODELS_PATH, None).await,
        12,
    )
    .await
    .unwrap();
    let loaded = load_upstream(&s, "cc").await.unwrap().unwrap();
    let pull = loaded.last_pull.unwrap();
    assert_eq!(pull.status, PullStatus::Ok);
    assert_eq!(pull.warning, None, "成功拉取清警告");
}

#[tokio::test]
async fn no_discovery_is_not_failure_and_keeps_old_entries() {
    let s = store("nodiscovery").await;
    let mut up = upstream("oc", "https://oc.example/zen/go/v1");
    save_upstream(&s, &up).await.unwrap();
    save_manual_model(&s, "oc", "hand-made", &[]).await.unwrap();

    let base = mock_upstream("/models", 404, json!({ "error": "not found" })).await;
    up.base_url = base;
    let http = reqwest::Client::new();
    let outcome = pull_models(&http, &up, MODELS_PATH, None).await;
    assert_eq!(outcome, PullOutcome::NoDiscovery, "404 = 不提供模型发现");
    let applied = apply_pull(&s, &up, outcome, 20).await.unwrap();
    assert_eq!(applied.added, 0);
    assert_eq!(list_models(&s, "oc").await.unwrap().len(), 1, "旧条目保留");
    let pull = load_upstream(&s, "oc")
        .await
        .unwrap()
        .unwrap()
        .last_pull
        .unwrap();
    assert_eq!(pull.status, PullStatus::NoDiscovery);

    // 200 但零条 = 同一支。
    up.base_url = mock_upstream("/models", 200, json!({ "data": [] })).await;
    assert_eq!(
        pull_models(&http, &up, MODELS_PATH, None).await,
        PullOutcome::NoDiscovery
    );
}

#[tokio::test]
async fn doc_table_fills_wires_when_upstream_declares_none() {
    let s = store("doc-table").await;
    let mut up = upstream("oc", "https://oc.example/zen/go/v1");
    save_upstream(&s, &up).await.unwrap();
    // opencode 两面 /models 都不报端点：wire 只能来自随包文档表 + 手工。
    let html = json!({ "data": [
        { "id": "deepseek-v4.1-flash", "object": "model", "owned_by": "opencode" },
        { "id": "claude-haiku-5-5", "object": "model", "owned_by": "opencode" },
        { "id": "gpt-6-luna", "object": "model", "owned_by": "opencode" },
        { "id": "nobody-knows-this-1", "object": "model", "owned_by": "opencode" }
    ]});
    up.base_url = mock_upstream("/models", 200, html).await;
    let http = reqwest::Client::new();
    apply_pull(
        &s,
        &up,
        pull_models(&http, &up, MODELS_PATH, None).await,
        30,
    )
    .await
    .unwrap();

    let models = list_models(&s, "oc").await.unwrap();
    let get = |id: &str| models.iter().find(|m| m.model_id == id).unwrap().clone();
    assert_eq!(
        get("deepseek-v4.1-flash").supported_wires,
        [Wire::OpenAiChat].into()
    );
    assert_eq!(get("deepseek-v4.1-flash").source, Source::Doc);
    assert_eq!(
        get("claude-haiku-5-5").supported_wires,
        [Wire::Anthropic].into()
    );
    assert_eq!(
        get("gpt-6-luna").supported_wires,
        [Wire::OpenAiResponses].into()
    );
    assert!(
        get("nobody-knows-this-1").supported_wires.is_empty(),
        "文档表里没有的模型不猜：保持未声明"
    );
    assert_eq!(get("nobody-knows-this-1").source, Source::Upstream);
}

// ───────────────────────── ⑤ 凭据 / 删引用 / 往返 ─────────────────────────

#[tokio::test]
async fn upstream_roundtrip_keeps_auth_paths_and_quirks() {
    let s = store("roundtrip").await;
    let mut up = upstream("qwen", "https://platform.qianwenai.com/apps/anthropic");
    // 按「上游 × wire」各存一份；anthropic 面用另一种认证（票 23 第 1 节）。
    up.set_auth("anthropic", AuthScheme::XApiKey);
    up.header_passthrough = vec!["anthropic-version".into(), "anthropic-beta".into()];
    up.quirks = vec![
        Quirk::ClientUserAgent("my-coding-agent/1.0".into()),
        Quirk::SessionIdHeader("x-opencode-session".into()),
        Quirk::NoModelsEndpoint,
    ];
    save_upstream(&s, &up).await.unwrap();

    let loaded = load_upstream(&s, "qwen").await.unwrap().unwrap();
    assert_eq!(loaded.base_url, up.base_url);
    assert_eq!(
        loaded.auth_for(Wire::OpenAiChat),
        AuthScheme::Bearer,
        "未填继承 default"
    );
    assert_eq!(loaded.auth_for(Wire::Anthropic), AuthScheme::XApiKey);
    assert_eq!(loaded.path(Wire::Anthropic), Some("/messages"));
    assert_eq!(
        loaded.url(Wire::OpenAiChat).unwrap(),
        "https://platform.qianwenai.com/apps/anthropic/chat/completions"
    );
    assert_eq!(loaded.header_passthrough, up.header_passthrough);
    assert_eq!(loaded.quirks.len(), 3);
    assert!(loaded.has_quirk(&Quirk::NoModelsEndpoint));
    assert!(!loaded.has_quirk(&Quirk::TemperatureMax2));

    // 覆盖写不产生第二行；能列出全部。
    save_upstream(&s, &up).await.unwrap();
    assert_eq!(list_upstreams(&s).await.unwrap().len(), 1);

    // 拉取凭据：default 优先；只有一份就用它；多份且无 default → None（由界面选）。
    #[cfg(windows)]
    {
        s.put_credential("qwen", "default", "sk-a-000000")
            .await
            .unwrap();
        assert_eq!(
            pull_credential(&s, "qwen").await.unwrap().as_deref(),
            Some("sk-a-000000")
        );
        s.delete_credential("qwen", "default").await.unwrap();
        assert_eq!(
            pull_credential(&s, "qwen").await.unwrap(),
            None,
            "一份都没有"
        );
        s.put_credential("qwen", "anthropic", "sk-b-000000")
            .await
            .unwrap();
        assert_eq!(
            pull_credential(&s, "qwen").await.unwrap().as_deref(),
            Some("sk-b-000000"),
            "没有 default 时唯一一份即用它"
        );
        s.put_credential("qwen", "openai-chat", "sk-c-000000")
            .await
            .unwrap();
        assert_eq!(
            pull_credential(&s, "qwen").await.unwrap(),
            None,
            "多份且没有 default：交给界面下拉选，不瞎猜"
        );
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 没有 DPAPI，凭据这条链走不通（本项目的目标平台是 Windows）。
        let _ = pull_credential(&s, "qwen").await;
    }
}

#[tokio::test]
async fn deleting_a_referenced_upstream_lists_the_references() {
    let s = store("refs").await;
    let up = upstream("qwen", "https://u.example");
    save_upstream(&s, &up).await.unwrap();
    s.write(|c| {
        c.execute(
            "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,real_model,wire)
             VALUES('global','claude-sonnet',1,'qwen','qwen3-max','anthropic'),
                   ('codex','gpt-x',0,'qwen','qwen3-max','openai-chat')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    match delete_upstream(&s, "qwen").await.unwrap() {
        DeleteOutcome::Referenced(refs) => {
            assert_eq!(refs.len(), 2, "{refs:?}");
            assert!(refs
                .iter()
                .any(|r| r.client_profile_id == "codex" && r.name == "gpt-x"));
            assert!(refs
                .iter()
                .any(|r| r.client_profile_id == "global" && r.name == "claude-sonnet"));
        }
        DeleteOutcome::Deleted => panic!("被引用的上游不许直接删"),
    }
    assert!(load_upstream(&s, "qwen").await.unwrap().is_some());

    s.write(|c| {
        c.execute("DELETE FROM virtual_models", [])?;
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        delete_upstream(&s, "qwen").await.unwrap(),
        DeleteOutcome::Deleted
    );
    assert!(load_upstream(&s, "qwen").await.unwrap().is_none());
    assert!(
        list_models(&s, "qwen").await.unwrap().is_empty(),
        "目录随上游一起删"
    );
}

// ───────────────────────── ⑥ 超时常量（票 49 Answer 自定口径） ─────────────────────────

#[test]
fn timeouts_are_fixed_and_no_retry() {
    assert_eq!(
        PULL_TIMEOUT,
        Duration::from_secs(20),
        "拉模型 20 秒（票 35）"
    );
    assert_eq!(
        CATALOG_REFRESH_TIMEOUT,
        Duration::from_secs(30),
        "快照刷新 30 秒：载荷是整个 api.json"
    );
}
