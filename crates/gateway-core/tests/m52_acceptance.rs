//  票 52 的**真实上游**验收（`#[ignore]`，联网、手动跑）：三步主路径从零走一遍。
//
//  「加上游 → 拉模型 → 强制自检 → 绑四个槽位 → 探针 → 切客户端 → 端到端验收」全部走真实
//  HTTP（`/api/*`），上游是 `.scratch/model-gateway/prototype-upstream.env` 里那两家之一。
//  库与三个客户端的配置根都在临时目录：真实 `%APPDATA%` / `%USERPROFILE%\.claude` 一个字节不碰。
//
//  ```text
//  $env:CARGO_TARGET_DIR='C:/Users/ZHANQUAN/AIGC/Project/cg-target/52'
//  cargo test -p gateway-core --test m52_acceptance -- --ignored --nocapture
//  ```
//
//  凭据只从上面那个文件现读，**不进代码、不进库、不进输出**（断言里会真查一遍）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gateway_core::clients::ClientEnv;
use gateway_core::egress::provider::Env;
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

fn env_file() -> String {
    std::env::var("CG_UPSTREAM_ENV").unwrap_or_else(|_| {
        r"C:\Users\ZHANQUAN\AIGC\Project\Claude Gateway\.scratch\model-gateway\prototype-upstream.env"
            .to_string()
    })
}

struct H {
    base: String,
    gateway: service::Gateway,
}

impl H {
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
}

#[tokio::test]
#[ignore = "联网：真实上游 + 临时数据目录，手动跑（cargo test -- --ignored --nocapture）"]
async fn three_step_path_with_a_real_upstream() {
    let env = Env::load(&env_file());
    let (provider, base_url, key) =
        if let (Some(b), Some(k)) = (env.get("OPENCODE_BASE_URL"), env.get("OPENCODE_API_KEY")) {
            ("opencode", b, k)
        } else if let (Some(b), Some(k)) = (
            env.get("COMMANDCODE_BASE_URL"),
            env.get("COMMANDCODE_API_KEY"),
        ) {
            ("commandcode", b, k)
        } else {
            panic!("env 文件里没有可用的上游（OPENCODE_* / COMMANDCODE_*）");
        };
    println!(
        "→ 用真实上游 {provider}（{base_url}），key 长度 {}（不打印内容）",
        key.len()
    );

    let root = std::env::temp_dir().join("cg52-real-upstream");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::open(&root).unwrap();
    let clients_root = root.join("clients");
    let control = Arc::new(TestControl::default());
    // 转发面只造一份：`AdminCtx` 的热重载与 `/v1/*` 必须看同一张表（否则验收会 404）。
    let forwarding = Arc::new(gateway_core::ingress::proxy::Gateway::new(
        store.clone(),
        HashMap::new(),
        gateway_core::binding::BindingTable::default(),
    ));
    let admin = Arc::new(
        AdminCtx::new(
            store.clone(),
            root.clone(),
            ClientEnv::under(&clients_root),
            control,
            "0.2.1-acceptance",
            root.join("gateway.db").to_string_lossy().into_owned(),
        )
        .with_proxy(forwarding.clone()),
    );
    let gateway = service::start(GatewayConfig {
        port: 0,
        db_path: root.join("gateway.db"),
        version: "0.2.1-acceptance".into(),
        forwarding: Some(forwarding),
        admin: Some(admin.clone()),
    })
    .await
    .unwrap();
    let h = H {
        base: format!("http://127.0.0.1:{}", gateway.port()),
        gateway,
    };

    // ── 起手：三步全未就绪 ──
    let (s, badges) = h.get("/api/sidebar/badges").await;
    println!("① 起手徽标 {s} {badges}");
    assert_eq!(badges["clientsNotEffective"], 3, "{badges}");

    // ── 第①步：加上游 + 拉模型 + 强制自检 ──
    let (s, created) = h
        .send(
            "POST",
            "/api/upstreams",
            json!({
                "id": provider,
                "name": provider,
                "baseUrl": base_url,
                // env 里的 baseUrl 自带 /v1（Go 面就是 /zen/go/v1）：路径模板相应去掉前缀。
                "paths": {
                    "anthropic": "/messages",
                    "openai-chat": "/chat/completions",
                    "openai-responses": "/responses"
                },
                // opencode 的 Go 面要求专属 UA 与会话头（quirk 声明，网关侧自动带上）。
                "quirks": [
                    "ClientUserAgent:claude-gateway/0.1",
                    "SessionIdHeader:x-opencode-session"
                ],
                "credentials": [{"wire": "default", "key": key}],
            }),
        )
        .await;
    assert_eq!(s, 201, "{created}");
    println!("② 建上游 {s}：{}", created["upstream"]["baseUrl"]);
    assert!(
        !created.to_string().contains(&key),
        "回执里出现了密钥明文（必须只有掩码）"
    );

    let (s, pull) = h
        .send(
            "POST",
            &format!("/api/upstreams/{provider}/discovery"),
            json!({}),
        )
        .await;
    assert_eq!(s, 200, "{pull}");
    println!(
        "③ 拉模型 {s}：{}；拉到 {} 条；警告 {} 条",
        pull["disposition"],
        pull["models"].as_array().map(|v| v.len()).unwrap_or(0),
        pull["warnings"].as_array().map(|v| v.len()).unwrap_or(0)
    );
    assert_eq!(pull["disposition"], "fetched", "{pull}");

    // 目录里按「有声明 wire 的模型」逐个试自检，第一个过的用它（真人挑模型也是这么试的）。
    let models = pull["models"].as_array().cloned().unwrap_or_default();
    let candidates: Vec<(String, String)> = models
        .iter()
        .filter_map(|m| {
            let id = m["modelId"].as_str()?.to_string();
            let wire = m["supportedWires"]
                .as_array()?
                .first()?
                .as_str()?
                .to_string();
            Some((id, wire))
        })
        .take(6)
        .collect();
    assert!(
        !candidates.is_empty(),
        "目录里没有一条声明过 wire 的模型：{pull}"
    );

    let mut chosen: Option<(String, String, Value)> = None;
    for (model, wire) in &candidates {
        let (s, check) = h
            .send(
                "POST",
                &format!("/api/upstreams/{provider}/selftest"),
                json!({"model": model, "wire": wire}),
            )
            .await;
        println!("④ 自检候选 {model} / {wire}：{s} {}", check["status"]);
        if check["ok"] == true {
            chosen = Some((model.clone(), wire.clone(), check));
            break;
        }
    }
    let (model, wire, check) = chosen.unwrap_or_else(|| {
        panic!(
            "前 {} 个候选模型自检全没过（这家上游当前都不给用？）",
            candidates.len()
        )
    });
    println!("⑤ 强制自检通过：{}", check);

    // ── 第②步：绑四个家族槽位 + 探针 ──
    for slot in [
        "claude-fable",
        "claude-opus",
        "claude-sonnet",
        "claude-haiku",
    ] {
        let (s, body) = h
            .send(
                "PUT",
                &format!("/api/bindings/{slot}"),
                json!({"upstreamId": provider, "realModel": model, "wire": wire}),
            )
            .await;
        assert_eq!(s, 200, "{slot}: {body}");
        println!("⑥ 绑 {slot} → {provider}/{model}/{wire}（{s}）");
    }
    let (s, probe) = h
        .send("POST", "/api/bindings/claude-haiku/probe", json!({}))
        .await;
    println!("⑦ haiku 槽位探针 {s}：{probe}");
    assert_eq!(probe["ok"], true, "{probe}");

    // ── 第③步：切 Claude Code（真实写入走临时配置根）──
    let (s, applied) = h
        .send("POST", "/api/clients/claude-code/apply", json!({}))
        .await;
    assert_eq!(s, 200, "{applied}");
    println!(
        "⑧ 切 Claude Code {s}：状态 {}；文件 {}",
        applied["client"]["state"], applied["report"]["files"][0]["path"]
    );
    assert_eq!(applied["client"]["state"], "effective", "{applied}");

    let (_, clients) = h.get("/api/clients").await;
    println!("⑨ 三张卡：{}", clients["clients"]);

    // ── 端到端验收（haiku 槽位，走真实入站）──
    let (s, acc) = h.send("POST", "/api/acceptance/run", json!({})).await;
    println!("⑩ 端到端验收 {s}：{acc}");
    assert_eq!(acc["ok"], true, "{acc}");
    assert_eq!(acc["upstream"], provider, "{acc}");

    // 用量与诊断（脱敏）。
    let (_, usage) = h
        .get("/api/usage/summary?range=today&dim=virtual_model")
        .await;
    println!("⑪ 今日用量：{usage}");
    let (_, diag) = h
        .send(
            "POST",
            "/api/diagnostics/copy",
            json!({"error": "验收用例"}),
        )
        .await;
    let text = diag["text"].as_str().unwrap_or_default();
    assert!(!text.contains(&key), "诊断包里出现了密钥明文");
    println!("⑫ 诊断包 {} 字节，密钥明文命中 0 次", text.len());

    // ── 收尾：配置还原（临时根里，也照做一遍证明可还原）──
    let (s, restored) = h
        .send("POST", "/api/clients/claude-code/restore", json!({}))
        .await;
    println!(
        "⑬ 还原 Claude Code {s}：状态 {}",
        restored["client"]["state"]
    );
    assert_eq!(restored["client"]["state"], "official");

    // 临时目录里也不许出现明文 key。
    let mut hits = 0;
    for entry in walk(&root) {
        if let Ok(bytes) = std::fs::read(&entry) {
            if bytes.windows(key.len()).any(|w| w == key.as_bytes()) {
                hits += 1;
                println!("！！ 明文命中：{}", entry.display());
            }
        }
    }
    assert_eq!(hits, 0, "临时数据目录里出现了明文密钥");

    h.gateway.stop().await;
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
//  追加到 `tests/m52_acceptance.rs` 末尾：**真实文件**的四态验收（`#[ignore]`）。
//  走产品的真实布局：`ClientEnv::production`（`~/.claude` 等）+ 真实备份根
//  `%APPDATA%\com.claudegateway.desktop\backups`；只有库落在临时目录。
//  跑完必须把配置还原（用例自己就做了，失败时也能拿备份路径手动还原）。
//
//  ```text
//  cargo test -p gateway-core --test m52_acceptance -- --ignored --nocapture real_client_file
//  ```

#[tokio::test]
#[ignore = "动真实客户端配置文件（~/.claude/settings.json）：只在人工验收时跑，跑完自动还原"]
async fn real_client_file_shows_mismatch_after_a_hand_edit() {
    use gateway_core::clients::{self, ClientId};

    let data_dir = std::env::temp_dir().join("cg52-real-file-db");
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    let store = Store::open(&data_dir).unwrap();
    // 真实配置根 + 真实备份根（产品布局）；库在临时目录。
    let env_root = gateway_core::store::default_data_dir().unwrap();
    let env = ClientEnv::production(&env_root).unwrap();
    println!(
        "真实 Claude Code 配置根：{}",
        env.claude_code_root.display()
    );
    println!("真实备份根：{}", env.backup_root.display());

    let file = env.claude_code_root.join("settings.json");
    let before = std::fs::read_to_string(&file).unwrap_or_default();
    println!("改动前：{before}");

    let control = Arc::new(TestControl::default());
    let admin = Arc::new(AdminCtx::new(
        store.clone(),
        env_root.clone(),
        env.clone(),
        control,
        "0.2.1-acceptance",
        data_dir.join("gateway.db").to_string_lossy().into_owned(),
    ));
    let values = admin.gateway_values().await.unwrap();

    // ① 起手态（这份文件里没有网关的键）。
    let card = clients::status(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    println!("① 起手：{}（{:?}）", card.state_label, card.state);
    assert_ne!(card.state, clients::ClientState::Effective);

    // ② 一键切换 → 已生效（真实写入，先备份）。
    let report = clients::apply(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    println!("② 切换：{}", report.message);
    let after_apply = std::fs::read_to_string(&file).unwrap();
    assert!(after_apply.contains("ANTHROPIC_BASE_URL"), "{after_apply}");
    assert!(
        after_apply.contains("autoUpdatesChannel"),
        "非接管键必须原样保住：{after_apply}"
    );
    let card = clients::status(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    assert_eq!(
        card.state,
        clients::ClientState::Effective,
        "{:?}",
        card.files
    );

    // ③ 手工改坏地址 → 如实报「值不匹配」（实读，不缓存）。
    let mut doc: Value = serde_json::from_str(&after_apply).unwrap();
    doc["env"]["ANTHROPIC_BASE_URL"] = json!("http://127.0.0.1:1/hand-edited");
    std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let card = clients::status(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    println!("③ 手改后：{}", card.state_label);
    assert_eq!(card.state, clients::ClientState::Mismatch);
    let diff = card.files[0]
        .fields
        .iter()
        .find(|f| f.key == "env.ANTHROPIC_BASE_URL")
        .unwrap();
    assert!(!diff.matches);
    assert_eq!(diff.current, json!("http://127.0.0.1:1/hand-edited"));
    println!(
        "   对照表那一行：当前 {} vs 期望 {}",
        diff.current, diff.expected
    );

    // ④ 一键还原 → 已回官方（摘网关键，非接管键照旧）。
    let report = clients::restore(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    println!("④ 还原：{}", report.message);
    let card = clients::status(&env, &store, ClientId::ClaudeCode, &values)
        .await
        .unwrap();
    assert_eq!(card.state, clients::ClientState::Official);
    let restored = std::fs::read_to_string(&file).unwrap();
    println!("还原后：{restored}");
    assert!(!restored.contains("ANTHROPIC_BASE_URL"), "{restored}");
    assert!(restored.contains("autoUpdatesChannel"), "{restored}");
    // 逐字节可比：原文件就那么一行键。
    assert_eq!(
        serde_json::from_str::<Value>(&restored).unwrap(),
        serde_json::from_str::<Value>(&before).unwrap(),
        "还原后应当与改动前语义一致"
    );
    println!("✅ 真实文件四态走通，配置已还原");
}
