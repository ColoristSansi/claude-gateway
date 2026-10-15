//! 票 53 管理面（`/api/logs*` 与 `/api/usage/summary`）的验收。
//!
//! 面是**真实 HTTP**：自起一份 axum 挂 `ingress::router`（管理面 + 转发面一起，与产品同一条
//! 装配路径），断言落在前端真正拿到的那份 JSON 上——字段名、状态码、筛选语义、脱敏。
//! 不碰真实 `%APPDATA%`，库一律在临时目录。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gateway_core::ingress::api::ApiState;
use gateway_core::ingress::proxy::Gateway as ProxyGateway;
use gateway_core::obs::{self, RequestLog};
use gateway_core::store::Store;
use serde_json::{json, Value};

/// 反查用的已知密钥串（明文只在内存；入库走 DPAPI 密文，与真实装库路径一样）。
const SECRET: &str = "gw-test-secret-53-known-plaintext-9f3a";

struct Api {
    base: String,
    store: Store,
    dir: PathBuf,
}

async fn start(tag: &str) -> Api {
    let dir = std::env::temp_dir().join("cg53-logs-api").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir).expect("开库");
    // 凭据与手工价都对上游有外键：先有一行上游，后面才种得进去。
    gateway_core::egress::provider::save_upstream(
        &store,
        &gateway_core::egress::provider::Upstream::new("u1", "u1", "http://127.0.0.1:1"),
    )
    .await
    .expect("种上游");
    store
        .put_credential("u1", "default", SECRET)
        .await
        .expect("种一份已知密钥");
    let bindings = gateway_core::binding::load(&store).await.expect("读绑定表");
    let gw = Arc::new(ProxyGateway::new(store.clone(), HashMap::new(), bindings));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑测试端口");
    let addr = listener.local_addr().expect("读端口");
    let state = ApiState::new(
        "0.1.0-test",
        dir.join("gateway.db").to_string_lossy().into_owned(),
        addr.port(),
    );
    let app = gateway_core::ingress::router(state, Some(gw));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Api {
        base: format!("http://{addr}"),
        store,
        dir,
    }
}

impl Api {
    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("GET 失败");
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .expect("POST 失败");
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    /// 库文件的原始字节（密钥反查用；`-wal` 也算）。
    fn db_bytes(&self) -> Vec<u8> {
        let mut all = Vec::new();
        for entry in std::fs::read_dir(&self.dir).expect("读数据目录") {
            let p = entry.expect("目录项").path();
            if p.is_file() {
                all.extend(std::fs::read(&p).unwrap_or_default());
            }
        }
        all
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row(ts: i64, vm: &str, client: &str, upstream: &str, status: i64) -> RequestLog {
    RequestLog {
        ts,
        virtual_model: vm.into(),
        client: Some(client.into()),
        upstream_id: upstream.into(),
        real_model: format!("{vm}-real"),
        wire: "anthropic".into(),
        status,
        first_byte_ms: 12,
        total_ms: 34,
        input_tokens: Some(1000),
        output_tokens: Some(500),
        cache_read: None,
        ..Default::default()
    }
}

/// 表格本身：时间倒序、缺省 50 条、四个筛选维度、模型名 / 请求 ID 搜索、游标翻页。
#[tokio::test]
async fn logs_table_sorts_filters_searches_and_pages() {
    let api = start("table").await;
    let t = now();

    for i in 0..60 {
        let mut r = row(t - 60 + i, "claude-haiku", "claude-code", "u1", 200);
        if i == 59 {
            r.virtual_model = "claude-opus".into();
            r.client = Some("codex".into());
            r.status = 502;
            r.aborted = true;
            r.request_id = Some("req-known-0001".into());
            // 票 58：缓存写这一列也要从 `/api/logs` 出得来。
            r.cache_write = Some(3819);
        }
        obs::log(&api.store, &r).await.expect("写日志");
    }

    // 缺省 50 条 + 游标（最新那条在最前，就是我们专门造的那条）。
    let (status, v) = api.get("/api/logs").await;
    assert_eq!(status, 200, "{v}");
    let rows = v["logs"].as_array().expect("logs 是数组");
    assert_eq!(rows.len(), 50, "默认一页 50 条");
    assert_eq!(rows[0]["virtual_model"], "claude-opus");
    assert_eq!(rows[0]["client"], "codex");
    assert_eq!(rows[0]["request_id"], "req-known-0001");
    assert_eq!(rows[0]["aborted"], true);
    assert_eq!(
        rows[0]["cache_write"], 3819,
        "缓存写这一列要从 /api/logs 出得来（票 58）：{rows:?}"
    );
    assert_eq!(v["limit"], 50);
    assert!(v["next_cursor"].is_i64(), "满了就该给游标：{v}");
    assert_eq!(v["retention_days"], 7);
    assert_eq!(v["rate"], 7.10, "库里的汇率初值");
    // 时间倒序
    let ts: Vec<i64> = rows.iter().map(|r| r["ts"].as_i64().unwrap()).collect();
    let mut sorted = ts.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(ts, sorted, "时间倒序");

    // 第二页：游标之后还剩 10 条。
    let cursor = v["next_cursor"].as_i64().unwrap();
    let (_, v2) = api.get(&format!("/api/logs?cursor={cursor}")).await;
    assert_eq!(v2["logs"].as_array().unwrap().len(), 10);
    assert!(v2["next_cursor"].is_null(), "不满一页就不该再给游标：{v2}");

    // 四个筛选维度各来一次。
    for (q, want) in [
        ("client=codex", 1),
        ("client=claude-code", 59),
        ("upstream=u1", 60),
        ("virtual_model=claude-opus", 1),
        ("status=ok", 59),
        ("status=5xx", 1),
        ("status=4xx", 0),
        ("status=error", 1),
        ("q=claude-opus", 1),
        ("q=req-known-0001", 1),
        ("q=%25", 0),
    ] {
        let (status, v) = api.get(&format!("/api/logs?{q}&limit=100")).await;
        assert_eq!(status, 200, "{q}：{v}");
        assert_eq!(
            v["logs"].as_array().unwrap().len(),
            want,
            "{q} 该回 {want} 条：{v}"
        );
    }

    // 筛选下拉的候选值来自库。
    let (status, f) = api.get("/api/logs/filters").await;
    assert_eq!(status, 200, "{f}");
    assert_eq!(f["clients"], json!(["claude-code", "codex"]));
    assert_eq!(f["upstreams"], json!(["u1"]));
    assert_eq!(f["virtual_models"], json!(["claude-haiku", "claude-opus"]));
    assert_eq!(f["statuses"], json!(["ok", "4xx", "5xx", "error"]));
}

/// 脱敏（票 53 第 20 行 / 票 19 §10 ③）：拿已知密钥串在**管理面响应**与**库文件**里都查不到；
/// 反查带**正对照**（同一份响应 / 同一份库字节里，必须找得到那个我们明知存在的值），
/// 否则「找不到」可能只是因为查错了地方。
#[tokio::test]
async fn logs_api_and_db_never_contain_the_known_secret() {
    let api = start("redaction").await;
    let mut r = row(now() - 5, "claude-haiku", "claude-code", "u1", 200);
    r.error = Some("上游 502：bad gateway".into());
    r.body = Some(r#"{"model":"claude-haiku-real","note":"出站请求体"}"#.into());
    obs::log(&api.store, &r).await.expect("写日志");

    let mut surfaces: Vec<(String, String)> = Vec::new();
    for path in [
        "/api/status",
        "/api/logs",
        "/api/logs/filters",
        "/api/logs/body-recording",
        "/api/usage/summary?range=today&dim=virtual_model",
        "/api/usage/summary?range=7d&dim=upstream",
    ] {
        let (status, v) = api.get(path).await;
        assert_eq!(status, 200, "{path}：{v}");
        surfaces.push((path.to_string(), v.to_string()));
    }
    // body 开关开到底再读一遍：开着时也不许漏（票 53 第 20 行的括注）。
    let (status, on) = api
        .post(
            "/api/logs/body-recording",
            &json!({"enabled": true, "consent": true}),
        )
        .await;
    assert_eq!(status, 200, "{on}");
    let (_, v) = api.get("/api/logs").await;
    surfaces.push(("/api/logs（body 开着）".into(), v.to_string()));

    let db = api.db_bytes();
    let has = |hay: &[u8], needle: &str| hay.windows(needle.len()).any(|w| w == needle.as_bytes());

    // 正对照：库与 API 里都该有我们明知存在的东西。
    assert!(
        has(&db, "claude-haiku-real"),
        "正对照失败：库字节里连已知的出站模型名都没有，反查方法本身可疑"
    );
    assert!(
        surfaces.iter().any(|(_, t)| t.contains("claude-haiku")),
        "正对照失败：管理面响应里连虚拟模型名都没有"
    );

    // 反查：密钥明文一处都不许有。
    assert!(!has(&db, SECRET), "密钥明文出现在库文件里");
    for (path, text) in &surfaces {
        assert!(!text.contains(SECRET), "密钥明文出现在 {path} 的响应里");
    }
    // 掩码也不该是明文（掩码只给上游页看，这里连掩码都不出现）。
    assert!(
        !db.windows(8).any(|w| w == b"gw-test-"),
        "库里不该有密钥串的片段"
    );
}

/// 用量基础：概览要的今日 + 近 7 天 × 两维，无价 / token 未知各自另计、不计入总额。
#[tokio::test]
async fn usage_summary_serves_today_and_7d_for_the_overview() {
    let api = start("usage").await;
    let t = now();
    // 有价的一条（手工价 $3 / $15 每百万）。
    gateway_core::egress::provider::save_manual_model(&api.store, "u1", "m-priced", &[])
        .await
        .unwrap();
    gateway_core::egress::provider::set_manual_prices(
        &api.store,
        "u1",
        "m-priced",
        &gateway_core::egress::provider::Prices {
            input: Some(3.0),
            output: Some(15.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut priced = row(t - 60, "claude-haiku", "claude-code", "u1", 200);
    priced.real_model = "m-priced".into();
    obs::log(&api.store, &priced).await.unwrap();
    // 无价的一条（目录里没有这个模型 → 未知价）。
    let mut unpriced = row(t - 120, "claude-opus", "codex", "u1", 200);
    unpriced.real_model = "m-unknown".into();
    obs::log(&api.store, &unpriced).await.unwrap();
    // 3 天前的一条有价（只进近 7 天）。
    let mut older = row(t - 3 * 86_400, "claude-haiku", "claude-code", "u1", 200);
    older.real_model = "m-priced".into();
    obs::log(&api.store, &older).await.unwrap();

    let (status, v) = api
        .get("/api/usage/summary?range=today&dim=virtual_model")
        .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["range"], "today");
    assert_eq!(v["dim"], "virtual_model");
    assert_eq!(v["totals"]["requests"], 2);
    assert_eq!(v["totals"]["no_price"], 1);
    assert_eq!(v["totals"]["token_unknown"], 0);
    // 票 58：汇总四列 token（含缓存写）都在；这两个计数把缓存写也算进「token 未知」的判据。
    assert_eq!(v["totals"]["cache_write"], 0, "{v}");
    let cost = v["totals"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 0.0105).abs() < 1e-12, "今日只算那一条有价的：{v}");
    assert_eq!(v["rate"], 7.10);
    let haiku = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "claude-haiku")
        .expect("按虚拟模型分组");
    assert_eq!(haiku["requests"], 1);
    assert_eq!(haiku["cache_write"], 0, "分组行也带这一列：{haiku}");
    let opus = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "claude-opus")
        .expect("无价那条也单独成行");
    assert_eq!(opus["cost_usd"], Value::Null, "无价不显示 0");

    let (status, w) = api.get("/api/usage/summary?range=7d&dim=upstream").await;
    assert_eq!(status, 200, "{w}");
    assert_eq!(w["totals"]["requests"], 3);
    assert!(
        (w["totals"]["cost_usd"].as_f64().unwrap() - 0.021).abs() < 1e-12,
        "{w}"
    );
    assert_eq!(w["rows"].as_array().unwrap().len(), 1);

    // 票 53 只做 today / 7d，别的范围给 400 而不是悄悄回一份错的（完整范围归票 59）。
    let (status, e) = api
        .get("/api/usage/summary?range=30d&dim=virtual_model")
        .await;
    assert_eq!(status, 400, "{e}");
    assert_eq!(e["error"]["type"], "invalid_request_error");
}

/// 转发面没挂上（库开不了那一档）时日志面回 503，不装成一张空表。
#[tokio::test]
async fn logs_are_unavailable_when_forwarding_is_not_mounted() {
    let dir = std::env::temp_dir()
        .join("cg53-logs-api")
        .join("no-forwarding");
    let _ = std::fs::remove_dir_all(&dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = ApiState::new(
        "0.1.0-test",
        dir.join("gateway.db").to_string_lossy().into_owned(),
        addr.port(),
    );
    let app = gateway_core::ingress::router(state, None);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    for path in [
        "/api/logs",
        "/api/logs/filters",
        "/api/logs/body-recording",
        "/api/usage/summary?range=today&dim=virtual_model",
    ] {
        let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(resp.status().as_u16(), 503, "{path}");
    }
    // 管理面本身还活着（票 43 的 `/api/status` 不依赖转发面）。
    let status: Value = reqwest::get(format!("http://{addr}/api/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["running"], true);
}
