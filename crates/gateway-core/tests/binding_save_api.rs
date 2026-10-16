//! 票 60b 回归：模型与映射页「改槽位绑定 → 点存」这条路（用户人眼走查时实测白屏的那条）。
//!
//! API 半逐字重放用户原路径：
//!   ① 改 `claude-sonnet` / `claude-haiku` 的绑定 → 200，且 `GET /api/bindings` 立刻读回新值；
//!   ② **重启**（关网关、同一个库重开）后仍是新值——用户报的「重启后恢复为之前的模型」；
//!   ③ 非法组合（wire 没在这条模型的声明里）→ 400 且错误信封可读（what + likelyCause 都在），
//!      库里这条绑定一个字节都没动。
//!
//! 界面那半（错误提示不再把整棵树渲染崩、协议下拉不再「显示一个值、发另一个值」）不走 HTTP，
//! 见 `app/src/components.tsx` 与 `app/src/pages/ModelsPage.tsx` 的注释，验证方式见票据 Answer。
//!
//! 目录用**手工条目**造（`POST /api/upstreams/{id}/models`），不需要真上游、不联网；
//! 库与客户端配置根都在临时目录，真实 `%APPDATA%` 一个字节不碰。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gateway_core::clients::ClientEnv;
use gateway_core::ingress::api::{AdminCtx, GatewayControl};
use gateway_core::service::{self, GatewayConfig};
use gateway_core::store::Store;
use serde_json::{json, Value};

#[derive(Default)]
struct TestControl {
    calls: Mutex<Vec<String>>,
}

impl GatewayControl for TestControl {
    fn start(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("start".into());
        Ok(())
    }
    fn restart(&self, port: u16) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("restart:{port}"));
        Ok(())
    }
}

struct H {
    root: PathBuf,
    gateway: service::Gateway,
    base: String,
}

impl H {
    /// 起一个网关：库与配置根都指 `root`。同一个 `root` 再起一次就是「重启」。
    async fn boot(root: PathBuf, tag: &str) -> H {
        let store = Store::open(&root).expect("开库");
        let forwarding = Arc::new(gateway_core::ingress::proxy::Gateway::new(
            store.clone(),
            std::collections::HashMap::new(),
            gateway_core::binding::BindingTable::default(),
        ));
        let admin = Arc::new(
            AdminCtx::new(
                store,
                root.clone(),
                ClientEnv::under(root.join("clients")),
                Arc::new(TestControl::default()),
                "0.1.0-test",
                root.join("gateway.db").to_string_lossy().into_owned(),
            )
            .with_proxy(forwarding.clone()),
        );
        let gateway = service::start(GatewayConfig {
            port: 0,
            db_path: root.join("gateway.db"),
            version: "0.1.0-test".into(),
            forwarding: Some(forwarding),
            admin: Some(admin),
            maintenance: None,
            // 票 61 的升级面：要 Tauri updater 插件，这里不挂（/api/update/* 是 503）。
            updater: None,
        })
        .await
        .expect(tag);
        H {
            root,
            base: format!("http://127.0.0.1:{}", gateway.port()),
            gateway,
        }
    }

    /// 关掉再在同一个根上重开：就是用户嘴里的「退出程序再重启」。
    async fn restart(self) -> H {
        let H { root, gateway, .. } = self;
        gateway.stop().await;
        H::boot(root, "重启网关").await
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let r = reqwest::get(format!("{}{path}", self.base)).await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn send(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let r = reqwest::Client::new()
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn put_binding(
        &self,
        slot: &str,
        upstream: &str,
        model: &str,
        wire: &str,
    ) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/api/bindings/{slot}?client=global"),
            json!({"upstreamId": upstream, "realModel": model, "wire": wire}),
        )
        .await
    }

    /// 某个槽位此刻的三元组（`GET /api/bindings` 读回，跟界面同一份数据）。
    async fn triple(&self, slot: &str) -> String {
        let (s, body) = self.get("/api/bindings?client=global").await;
        assert_eq!(s, 200, "{body}");
        body["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["slot"] == slot)
            .unwrap_or_else(|| panic!("槽位表里没有 {slot}：{body}"))["binding"]["triple"]
            .as_str()
            .unwrap_or_else(|| panic!("{slot} 还没绑定：{body}"))
            .to_string()
    }

    async fn stop(self) {
        self.gateway.stop().await;
    }
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("cg60b-binding-save").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn triple(upstream: &str, model: &str, wire: &str) -> String {
    format!("{upstream} / {model} / {wire}")
}

#[tokio::test]
async fn slot_binding_save_reads_back_survives_a_restart_and_rejects_an_undeclared_wire() {
    let h = H::boot(tmp("save"), "起网关").await;

    // 上游 + 手工目录：两条只声明 anthropic 的（给 sonnet / haiku 换着绑），一条只声明 openai-chat 的
    // （用户当时挑的那种「换个便宜的」模型，正是白屏那一连串的起点）。
    let (s, body) = h
        .send(
            "POST",
            "/api/upstreams",
            json!({
                "id": "cmd",
                "name": "cmd",
                "baseUrl": "http://127.0.0.1:1",
                "paths": {"anthropic": "/messages", "openai-chat": "/chat/completions"},
                "credentials": [{"wire": "default", "key": "sk-test-DO-NOT-LEAK"}],
            }),
        )
        .await;
    assert_eq!(s, 201, "{body}");
    for (model, wires) in [
        ("sonnet-one", json!(["anthropic"])),
        ("sonnet-two", json!(["anthropic"])),
        ("haiku-one", json!(["anthropic"])),
        ("chat-only", json!(["openai-chat"])),
    ] {
        let (s, body) = h
            .send(
                "POST",
                "/api/upstreams/cmd/models",
                json!({"modelId": model, "wires": wires}),
            )
            .await;
        assert_eq!(s, 200, "{model}: {body}");
    }

    // ── ① 用户原路径：把 sonnet / haiku 各改一次绑定（改成目录里已声明的另一个模型）──
    let (s, body) = h
        .put_binding("claude-sonnet", "cmd", "sonnet-one", "anthropic")
        .await;
    assert_eq!(s, 200, "改 sonnet 绑定：{body}");
    assert_eq!(body["saved"], true, "{body}");
    let (s, body) = h
        .put_binding("claude-haiku", "cmd", "haiku-one", "anthropic")
        .await;
    assert_eq!(s, 200, "改 haiku 绑定：{body}");
    assert_eq!(body["saved"], true, "{body}");

    assert_eq!(
        h.triple("claude-sonnet").await,
        triple("cmd", "sonnet-one", "anthropic")
    );
    assert_eq!(
        h.triple("claude-haiku").await,
        triple("cmd", "haiku-one", "anthropic")
    );

    // 再换一次（同一上游目录里另一个已声明的模型），读回也必须是新的。
    let (s, body) = h
        .put_binding("claude-sonnet", "cmd", "sonnet-two", "anthropic")
        .await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(
        h.triple("claude-sonnet").await,
        triple("cmd", "sonnet-two", "anthropic")
    );

    // ── ③ 非法组合：这条模型没声明 anthropic（用户当时界面显示 openai-chat、实际发出的就是它）──
    let (s, body) = h
        .put_binding("claude-haiku", "cmd", "chat-only", "anthropic")
        .await;
    assert_eq!(s, 400, "没声明的 wire 必须 400：{body}");
    let err = &body["error"];
    assert!(
        err["what"]
            .as_str()
            .unwrap_or_default()
            .contains("协议不在这条模型的声明里"),
        "错误信封要能读懂发生了什么：{body}"
    );
    assert!(
        err["likelyCause"]
            .as_str()
            .unwrap_or_default()
            .contains("openai-chat"),
        "可能原因要指出这条模型声明的是哪些 wire：{body}"
    );
    for key in ["what", "likelyCause", "action", "raw"] {
        assert!(err.get(key).is_some(), "错误信封缺 {key}：{body}");
    }
    // 没存下去 = 库里一个字节都没动（用户重启后看到旧值就是这个）。
    assert_eq!(
        h.triple("claude-haiku").await,
        triple("cmd", "haiku-one", "anthropic")
    );

    // ── ② 重启后仍是新值 ──
    let h = h.restart().await;
    assert_eq!(
        h.triple("claude-sonnet").await,
        triple("cmd", "sonnet-two", "anthropic"),
        "重启后 sonnet 该还是刚存的那条"
    );
    assert_eq!(
        h.triple("claude-haiku").await,
        triple("cmd", "haiku-one", "anthropic"),
        "重启后 haiku 该还是刚存的那条"
    );

    h.stop().await;
}

/// 票 65：`openai-responses` 的绑定**必须能存下去**（评审的阻断③已随出站 wire 落地销账）。
///
/// 上一轮给的临时措施（`unimplementedWires` + 保存拒收）随本票一起撤掉：`/api/bindings`
/// 与 `/api/providers` 都不再回那个字段，两条保存路径（槽位绑定 / 普通虚拟模型）都放行。
#[tokio::test]
async fn responses_wire_bindings_are_saved_through_both_paths() {
    let h = H::boot(tmp("responses-wire"), "起网关").await;

    // 界面拿到的候选里没有「不可选」的 wire 了（字段整个撤掉，不是空数组）。
    let (s, body) = h.get("/api/bindings?client=global").await;
    assert_eq!(s, 200, "{body}");
    println!(
        "\n[票 65] GET /api/bindings?client=global → wires={}，还带 unimplementedWires 吗：{}",
        body["wires"],
        body.get("unimplementedWires").is_some()
    );
    assert!(
        body.get("unimplementedWires").is_none(),
        "临时措施该整条撤掉：{body}"
    );
    assert!(
        body["wires"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "openai-responses"),
        "候选里照旧列全量：{body}"
    );
    let (s, body) = h.get("/api/providers").await;
    assert_eq!(s, 200, "{body}");
    assert!(body.get("unimplementedWires").is_none(), "{body}");

    // 上游 + 一条只声明 `openai-responses` 的手工模型（预设里 5 个就是这个样子）。
    let (s, body) = h
        .send(
            "POST",
            "/api/upstreams",
            json!({
                "id": "resp",
                "name": "resp",
                "baseUrl": "http://127.0.0.1:1",
                "paths": {"anthropic": "/messages", "openai-responses": "/responses"},
                "credentials": [{"wire": "default", "key": "sk-test-DO-NOT-LEAK"}],
            }),
        )
        .await;
    assert_eq!(s, 201, "{body}");
    let (s, body) = h
        .send(
            "POST",
            "/api/upstreams/resp/models",
            json!({"modelId": "gpt-5-responses", "wires": ["openai-responses"]}),
        )
        .await;
    assert_eq!(s, 200, "{body}");

    // ① 槽位绑定：**故意选一次 openai-responses，存下去要成功**（这就是界面那个下拉发的请求）。
    let (s, body) = h
        .put_binding(
            "claude-sonnet",
            "resp",
            "gpt-5-responses",
            "openai-responses",
        )
        .await;
    assert_eq!(s, 200, "openai-responses 必须能存：{body}");
    assert_eq!(body["saved"], true, "{body}");
    println!(
        "[票 65] PUT /api/bindings/claude-sonnet?client=global wire=openai-responses → {s}，saved={}，读回：{}",
        body["saved"],
        h.triple("claude-sonnet").await
    );
    assert_eq!(
        h.triple("claude-sonnet").await,
        triple("resp", "gpt-5-responses", "openai-responses"),
        "读回要与刚存的一致"
    );

    // ② 普通虚拟模型是另一条保存路径，同样放行。
    let (s, body) = h
        .send(
            "POST",
            "/api/virtual-models?client=global",
            json!({"name": "v1", "upstreamId": "resp", "realModel": "gpt-5-responses", "wire": "openai-responses"}),
        )
        .await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["saved"], true, "{body}");
    println!(
        "[票 65] POST /api/virtual-models wire=openai-responses → {s}，triple={}",
        body["binding"]["triple"]
    );

    // 重启也还在（走的还是同一个 `bindings` 表）。
    let h = h.restart().await;
    assert_eq!(
        h.triple("claude-sonnet").await,
        triple("resp", "gpt-5-responses", "openai-responses"),
        "重启后该还是刚存的那条"
    );
    let (s, body) = h.get("/api/bindings?client=global").await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(
        body["virtualModels"][0]["triple"],
        triple("resp", "gpt-5-responses", "openai-responses"),
        "{body}"
    );

    h.stop().await;
}
