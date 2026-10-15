//! 票 51 验收单测：家族槽位 / 虚拟模型解析边界、客户端档案覆盖、绑定只能从目录里选、
//! wire 未声明早拒 400、自检探针（`max_tokens:1` + 只看状态码 + 超时 504 + 落库）。
//!
//! 测试面 = `gateway_core::binding` 的公开接口 + `egress::provider` 的目录读写 + 票 42 的
//! `Store`；另外九、十条走**真实 HTTP**（`ingress::proxy::router()` 在本进程里挂起来），
//! 验的是「接进真实路由」之后的入站状态码与错误信封。一律临时目录，不碰真实 `%APPDATA%`。

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use gateway_core::binding::{
    self, default_profiles, Binding, BindingTable, Profile, ProfileRow, FAMILY_SLOTS,
};
use gateway_core::egress::provider::{
    self, save_manual_model, save_upstream, AuthScheme, Source, Upstream, UpstreamModel, Wire,
    WireChoice,
};
use gateway_core::ingress::proxy::Gateway;
use gateway_core::store::Store;
use serde_json::{json, Value};

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg51-binding-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

async fn store(tag: &str) -> Store {
    Store::open(&tmp_root(tag)).expect("开库")
}

fn upstream(id: &str, base: &str) -> Upstream {
    let mut up = Upstream::new(id, id, base);
    up.set_auth("default", AuthScheme::Bearer);
    up.set_path(Wire::Anthropic, "/v1/messages");
    up.set_path(Wire::OpenAiChat, "/v1/chat/completions");
    up
}

/// 一张纯内存的绑定表（不给库）：global 一条家族槽位 + 一条普通虚拟模型，codex 档案覆盖同一槽位。
fn table() -> BindingTable {
    BindingTable::new(
        vec![
            Binding::new(
                Profile::Global,
                "claude-haiku",
                "cc",
                "claude-haiku-5-5",
                Wire::Anthropic,
            ),
            Binding::new(
                Profile::Global,
                "my-model",
                "oc",
                "deepseek-v4.1-flash",
                Wire::OpenAiChat,
            ),
            Binding::new(
                Profile::Codex,
                "claude-haiku",
                "oc",
                "gpt-5.1-codex",
                Wire::OpenAiChat,
            ),
        ],
        default_profiles(),
    )
}

// ───────────────────────── ① 模型名四条边界 ─────────────────────────

#[test]
fn model_names_trim_are_case_sensitive_and_slots_are_plain_rows() {
    let t = table();

    // 去首尾空白后命中（票 30 第 4 条）。
    assert_eq!(
        t.resolve("  claude-haiku  ", "").unwrap().real_model,
        "claude-haiku-5-5"
    );
    // 大小写敏感：换了大小写就是未知模型。
    assert!(t.resolve("CLAUDE-HAIKU", "").is_err());
    assert!(t.resolve("Claude-Haiku", "").is_err());
    // 未知（含将来的家族名）→ 404 `not_found_error`。
    for name in ["claude-opus-6", "根本没有这个模型", ""] {
        let e = t.resolve(name, "").expect_err("未命中必须报错");
        assert_eq!(e.status(), 404, "{name}");
        assert_eq!(e.error_type(), "not_found_error", "{name}");
        assert!(e.message().contains("未知虚拟模型"), "{name}");
    }
    // 空串（模型名缺失）也是未命中，不当成通配。
    assert!(t.resolve("   ", "").is_err());
    // 槽位名就是普通的一行绑定，没有特殊命中路径。
    let slot = t.resolve("claude-haiku", "").unwrap();
    assert_eq!(slot.name, "claude-haiku");
    assert!(binding::is_family(&slot.name) && FAMILY_SLOTS.contains(&"claude-haiku"));
}

// ───────────────────────── ② 客户端档案覆盖 ─────────────────────────

#[test]
fn profile_override_wins_and_global_is_the_fallback() {
    let t = table();
    // 同一槽位：codex 走另一条上游 + 另一个真实模型 + 另一个 wire。
    let codex = t.resolve("claude-haiku", "codex_exec/0.162.1").unwrap();
    assert_eq!(codex.profile, Profile::Codex);
    assert_eq!(codex.upstream_id, "oc");
    assert_eq!(codex.real_model, "gpt-5.1-codex");
    assert_eq!(codex.wire, Wire::OpenAiChat);

    // 别的客户端不受影响：落回 global。
    let cc = t
        .resolve("claude-haiku", "claude-cli/2.1.299 (external, sdk-cli)")
        .unwrap();
    assert_eq!(cc.profile, Profile::Global);
    assert_eq!(cc.upstream_id, "cc");
    assert_eq!(cc.wire, Wire::Anthropic);

    // 档案里没有的模型名照样落回 global（覆盖是逐模型的，不是整档案替换）。
    assert_eq!(
        t.resolve("my-model", "codex_exec/0.162.1")
            .unwrap()
            .upstream_id,
        "oc"
    );

    // 覆盖只对「该档案 + 该模型」成立。
    let t2 = BindingTable::new(
        vec![Binding::new(
            Profile::Codex,
            "codex-only",
            "oc",
            "gpt-5.1-codex",
            Wire::OpenAiChat,
        )],
        default_profiles(),
    );
    assert!(
        t2.resolve("codex-only", "").is_err(),
        "别的客户端不该看到 codex 的私有绑定"
    );
    assert!(t2.resolve("codex-only", "codex_exec/0.162.1").is_ok());
}

#[test]
fn measured_user_agents_map_to_their_profiles() {
    let t = table();
    // 三台客户端的实测 UA（Claude Desktop 票 15；Claude Code / Codex 本机实测，见票据 Answer）。
    let desktop =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
                   Claude/2.37337.1 Chrome/142.0.7444.175 Electron/44.7.0 Safari/537.36 MSIX";
    let code = "claude-cli/2.1.299 (external, sdk-cli)";
    let codex = "codex_exec/0.162.1 (Windows 10.0.26200; x86_64) unknown (codex_exec; 0.162.1)";
    assert_eq!(t.profile_for(desktop), Profile::ClaudeDesktop);
    assert_eq!(t.profile_for(code), Profile::ClaudeCode);
    assert_eq!(t.profile_for(codex), Profile::Codex);
    // Codex 桌面应用：UA 未知，但必带 `x-codex-*` 头（票 32 的真录入站体）。
    assert_eq!(t.profile_for("x-codex-installation-id"), Profile::Codex);
    // 认不出就是全局。
    assert_eq!(t.profile_for("client-a/1.0"), Profile::Global);
    assert_eq!(t.profile_for(""), Profile::Global);
    // 大小写不敏感（UA 串的大小写不归我们管）。
    assert_eq!(t.profile_for("CLAUDE-CLI/9.9.9"), Profile::ClaudeCode);
    // 档案行可以被库里那份覆盖/追加。
    let custom = BindingTable::new(
        Vec::new(),
        vec![ProfileRow {
            profile: Profile::Codex,
            match_ua: "my-agent/".into(),
        }],
    );
    assert_eq!(custom.profile_for("my-agent/2.0"), Profile::Codex);
    assert_eq!(
        custom.profile_for("codex_exec/0.162.1"),
        Profile::Global,
        "库里那份是唯一真相"
    );
}

// ───────────────────────── ③ /v1/models 的暴露集合 ─────────────────────────

#[test]
fn exposed_models_list_family_slots_first_then_normal_models() {
    let t = table();
    let slots: Vec<String> = FAMILY_SLOTS.iter().map(|s| s.to_string()).collect();
    let names: Vec<String> = t.exposed().into_iter().map(|m| m.name).collect();
    // 四个家族槽位恒在、顺序固定，且排在最前（票 31：Codex 目录与 /v1/models 同一份清单）。
    assert_eq!(&names[..4], &slots[..]);
    assert_eq!(
        names,
        vec![
            "claude-fable",
            "claude-opus",
            "claude-sonnet",
            "claude-haiku",
            "my-model"
        ]
    );
    let flags: Vec<bool> = t.exposed().into_iter().map(|m| m.is_family).collect();
    assert_eq!(flags, vec![true, true, true, true, false]);
    // 每条带的三元组默认按 `global` 解析；按档案看用 `exposed_for`（Codex 目录写描述用）。
    let haiku = t
        .exposed()
        .into_iter()
        .find(|m| m.name == "claude-haiku")
        .unwrap()
        .binding
        .expect("夹具里绑好了");
    assert_eq!(haiku.upstream_id, "cc");
    let haiku_codex = t
        .exposed_for(Profile::Codex)
        .into_iter()
        .find(|m| m.name == "claude-haiku")
        .unwrap()
        .binding
        .expect("codex 档案里有覆盖");
    assert_eq!(haiku_codex.real_model, "gpt-5.1-codex");
    // 还没绑的家族槽位照样暴露名字，只是没有三元组（请求期 404）。
    let bare = BindingTable::new(Vec::new(), default_profiles());
    let unbound = bare
        .exposed()
        .into_iter()
        .find(|m| m.name == "claude-opus")
        .unwrap();
    assert!(unbound.binding.is_none());
    // 同名跨档案只出一条（含 Codex 的覆盖名）。
    let t2 = BindingTable::new(
        vec![
            Binding::new(Profile::Global, "claude-haiku", "cc", "m", Wire::Anthropic),
            Binding::new(Profile::Codex, "claude-haiku", "oc", "m2", Wire::OpenAiChat),
        ],
        default_profiles(),
    );
    let names2: Vec<String> = t2.exposed().into_iter().map(|m| m.name).collect();
    assert_eq!(names2, slots);
}

// ───────────────────────── ④ 落库：绑定只能从目录里选 ─────────────────────────

#[tokio::test]
async fn bindings_roundtrip_through_the_store() {
    let store = store("roundtrip").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:9"))
        .await
        .unwrap();
    save_upstream(&store, &upstream("oc", "http://127.0.0.1:9"))
        .await
        .unwrap();
    save_manual_model(&store, "cc", "claude-haiku-5-5", &[Wire::Anthropic])
        .await
        .unwrap();
    save_manual_model(&store, "oc", "gpt-5.1-codex", &[Wire::OpenAiChat])
        .await
        .unwrap();

    let global = Binding::new(
        Profile::Global,
        "claude-haiku",
        "cc",
        "claude-haiku-5-5",
        Wire::Anthropic,
    );
    let codex = Binding::new(
        Profile::Codex,
        "claude-haiku",
        "oc",
        "gpt-5.1-codex",
        Wire::OpenAiChat,
    );
    assert_eq!(
        binding::save(&store, &global).await.unwrap(),
        WireChoice::Declared
    );
    assert_eq!(
        binding::save(&store, &codex).await.unwrap(),
        WireChoice::Declared
    );

    let t = binding::load(&store).await.unwrap();
    assert_eq!(t.len(), 2);
    // 覆盖生效、别的客户端落回 global —— 从库里读出来之后仍然成立。
    assert_eq!(
        t.resolve("claude-haiku", "codex_exec/0.162.1")
            .unwrap()
            .upstream_id,
        "oc"
    );
    assert_eq!(
        t.resolve("claude-haiku", "claude-cli/2.1.299")
            .unwrap()
            .upstream_id,
        "cc"
    );
    // 库里的 match_ua 是那份实测 UA（v4 迁移填的），不是代码里的默认值。
    assert_eq!(t.profile_for("claude-cli/2.1.299"), Profile::ClaudeCode);
    assert_eq!(
        t.profile_for("Mozilla/5.0 ... Claude/2.37337.1 ..."),
        Profile::ClaudeDesktop
    );
    assert_eq!(t.profile_for("x-codex-installation-id"), Profile::Codex);
    // v4 迁移的字面量与 `binding::DEFAULT_MATCH_UA` 必须逐字一致（两处漂了这条会红）。
    let from_db: Vec<(String, String)> = store
        .read(|c| {
            let mut st = c.prepare("SELECT id,match_ua FROM client_profiles ORDER BY id")?;
            let rows =
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
        .unwrap();
    let mut want: Vec<(String, String)> = binding::DEFAULT_MATCH_UA
        .iter()
        .map(|(id, ua)| ((*id).to_string(), (*ua).to_string()))
        .collect();
    want.push(("global".to_string(), String::new()));
    want.sort();
    assert_eq!(from_db, want);
    // 幂等重写：再存一次不炸，还是两条。
    binding::save(&store, &global).await.unwrap();
    assert_eq!(binding::load(&store).await.unwrap().len(), 2);
}

#[tokio::test]
async fn saving_needs_a_catalog_entry_first() {
    let store = store("needs-catalog").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:9"))
        .await
        .unwrap();
    let b = Binding::new(
        Profile::Global,
        "claude-opus",
        "cc",
        "claude-opus-5",
        Wire::Anthropic,
    );
    match binding::save(&store, &b).await {
        Err(binding::SaveError::NoCatalogEntry {
            upstream_id,
            real_model,
        }) => {
            assert_eq!(upstream_id, "cc");
            assert_eq!(real_model, "claude-opus-5");
        }
        other => panic!("目录没有这条时不许绑：{other:?}"),
    }
    // 手工新增一条目录条目（`manual` 锁定）之后就能绑（spec §7）。
    save_manual_model(&store, "cc", "claude-opus-5", &[Wire::Anthropic])
        .await
        .unwrap();
    assert_eq!(
        binding::save(&store, &b).await.unwrap(),
        WireChoice::Declared
    );
}

#[tokio::test]
async fn saving_a_wire_the_catalog_does_not_declare_is_refused() {
    let store = store("wire-declared").await;
    save_upstream(&store, &upstream("cc", "http://127.0.0.1:9"))
        .await
        .unwrap();
    save_manual_model(&store, "cc", "claude-sonnet-5-5", &[Wire::Anthropic])
        .await
        .unwrap();
    let b = Binding::new(
        Profile::Global,
        "claude-sonnet",
        "cc",
        "claude-sonnet-5-5",
        Wire::OpenAiChat,
    );
    match binding::save(&store, &b).await {
        Err(binding::SaveError::WireNotDeclared(rej)) => {
            assert_eq!(rej.status, 400);
            assert_eq!(rej.error_type, "invalid_request_error");
            assert!(rej.message.contains("openai-chat"));
        }
        other => panic!("声明了 anthropic 却绑 openai-chat，配置期就得拦：{other:?}"),
    }
}

#[tokio::test]
async fn undeclared_catalog_entry_allows_any_wire_with_the_note() {
    let store = store("wire-undeclared").await;
    save_upstream(&store, &upstream("oc", "http://127.0.0.1:9"))
        .await
        .unwrap();
    // 空集合 = 未声明（正常态）：可以自由选，但要标注「未声明，靠自检」。
    save_manual_model(&store, "oc", "glm-5.1", &[])
        .await
        .unwrap();
    let b = Binding::new(Profile::Global, "oc-glm", "oc", "glm-5.1", Wire::OpenAiChat);
    let choice = binding::save(&store, &b).await.unwrap();
    assert_eq!(choice, WireChoice::Undeclared);
    assert_eq!(choice.note(), Some(provider::UNDECLARED_NOTE));
    assert_eq!(provider::UNDECLARED_NOTE, "未声明，靠自检");
}

// ───────────────────────── ⑤ 接进真实路由：404 / 400 ─────────────────────────

/// 把转发面挂在本进程里，回 (base_url, Gateway)。
async fn spawn_router(app: Gateway) -> String {
    let state = Arc::new(app);
    let router = gateway_core::ingress::proxy::router().with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

async fn post_messages(base: &str, body: &Value, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut rb = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .json(body);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let resp = rb.send().await.expect("POST /v1/messages 失败");
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn unknown_model_names_are_404_on_the_real_route() {
    let store = store("route-404").await;
    let app = Gateway::new(store, HashMap::new(), table());
    let base = spawn_router(app).await;

    for name in ["claude-opus-6", "CLAUDE-HAIKU", "", "   ", "my-model2"] {
        let (status, v) = post_messages(
            &base,
            &json!({"model": name, "max_tokens": 256, "stream": true,
                    "messages": [{"role":"user","content":"."}]}),
            &[],
        )
        .await;
        assert_eq!(status, 404, "{name:?} → {v}");
        assert_eq!(v["error"]["type"], "not_found_error", "{name:?} → {v}");
    }

    // 去首尾空白之后是有这条绑定的（后面缺凭据 → 502，但**不是** 404）。
    let (status, v) = post_messages(
        &base,
        &json!({"model":"  claude-haiku  ", "max_tokens": 256, "stream": true,
                "messages": [{"role":"user","content":"."}]}),
        &[],
    )
    .await;
    assert_ne!(status, 404, "首尾空白该被去掉后命中：{v}");
    assert_ne!(v["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn a_binding_whose_wire_the_catalog_does_not_declare_is_rejected_with_400() {
    let store = store("route-400").await;
    // 目录条目只声明 anthropic；绑定表里那条却是 openai-chat（模拟老库 / 手工改库绕过了配置期主拦）。
    let mut upstreams = HashMap::new();
    upstreams.insert("cc".to_string(), upstream("cc", "http://127.0.0.1:9"));
    let app = Gateway::new(
        store,
        upstreams,
        BindingTable::new(
            vec![Binding::new(
                Profile::Global,
                "claude-sonnet",
                "cc",
                "claude-sonnet-5-5",
                Wire::OpenAiChat,
            )],
            default_profiles(),
        ),
    );
    app.set_catalog_entry(
        "cc",
        "claude-sonnet-5-5",
        UpstreamModel {
            upstream_id: "cc".into(),
            model_id: "claude-sonnet-5-5".into(),
            supported_wires: BTreeSet::from([Wire::Anthropic]),
            source: Source::Upstream,
        },
    );
    let base = spawn_router(app).await;

    let (status, v) = post_messages(
        &base,
        &json!({"model":"claude-sonnet", "max_tokens": 256, "stream": true,
                "messages": [{"role":"user","content":"."}]}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "{v}");
    assert_eq!(v["error"]["type"], "invalid_request_error");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("anthropic"));
}

// ───────────────────────── ⑥ 自检探针 ─────────────────────────

/// 起一个记请求体的 mock 上游：`/v1/messages` 按给定状态码回答（`sleep` 非零就拖住）。
async fn mock_upstream(status: u16, sleep: Duration) -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                move |State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(v): Json<Value>| async move {
                    seen.lock().unwrap().push(v);
                    if !sleep.is_zero() {
                        tokio::time::sleep(sleep).await;
                    }
                    (
                        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                        Json(json!({"type": "message"})),
                    )
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

#[tokio::test]
async fn probe_is_non_streaming_max_tokens_one_and_records_the_status_code() {
    let store = store("probe-ok").await;
    let (base, seen) = mock_upstream(200, Duration::ZERO).await;
    save_upstream(&store, &upstream("mock", &base))
        .await
        .unwrap();
    store
        .put_credential("mock", "default", "probe-key")
        .await
        .unwrap();
    let up = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();

    let r = binding::selfcheck(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "mock-model",
        Duration::from_secs(5),
    )
    .await;
    assert!(r.ok, "{r:?}");
    assert_eq!(r.status, 200);

    // 探针形状：非流式 `/v1/messages` + `max_tokens:1`，且**不看 body**（只认状态码）。
    let sent = seen
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("上游该收到一次探针");
    assert_eq!(sent["model"], "mock-model");
    assert_eq!(sent["max_tokens"], 1);
    assert!(sent.get("stream").is_none(), "探针是非流式的：{sent}");
    assert!(sent["messages"].is_array(), "{sent}");

    // 落库：就绪判据只看这一列。
    let after = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    let sc = after.last_selfcheck.expect("自检结果必须落库");
    assert!(sc.ok && sc.at > 0);
}

#[tokio::test]
async fn probe_sees_a_non_2xx_status_as_a_failure_even_with_a_body() {
    let store = store("probe-500").await;
    let (base, _seen) = mock_upstream(500, Duration::ZERO).await;
    save_upstream(&store, &upstream("mock", &base))
        .await
        .unwrap();
    store
        .put_credential("mock", "default", "probe-key")
        .await
        .unwrap();
    let up = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    let r = binding::selfcheck(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "mock-model",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(r.status, 500, "{r:?}");
    assert!(!r.ok);
    assert!(r.detail.contains("500"), "{r:?}");
    let after = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    assert!(!after.last_selfcheck.expect("失败也要落库").ok);
}

#[tokio::test]
async fn probe_timeout_is_504_and_records_a_failure() {
    let store = store("probe-timeout").await;
    let (base, _seen) = mock_upstream(200, Duration::from_secs(30)).await;
    save_upstream(&store, &upstream("mock", &base))
        .await
        .unwrap();
    store
        .put_credential("mock", "default", "probe-key")
        .await
        .unwrap();
    let up = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    let r = binding::selfcheck(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "mock-model",
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(r.status, 504, "{r:?}");
    assert!(!r.ok);
    assert!(r.detail.contains("超时"), "{r:?}");
    let after = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    assert!(!after.last_selfcheck.expect("失败也要落库").ok);
}

#[tokio::test]
async fn probe_connect_failure_is_502() {
    let store = store("probe-502").await;
    // 127.0.0.1:1 上没人听。
    save_upstream(&store, &upstream("mock", "http://127.0.0.1:1"))
        .await
        .unwrap();
    let up = provider::load_upstream(&store, "mock")
        .await
        .unwrap()
        .unwrap();
    let r = binding::selfcheck(
        &reqwest::Client::new(),
        &store,
        &up,
        Wire::Anthropic,
        "mock-model",
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(r.status, 502, "{r:?}");
    assert!(!r.ok);
}
