//! 票 59 验收：用量与成本页 / 日志详情四段与帧时间轴 / 诊断包与导出脱敏 / 设置页。
//!
//! 面是**真实 HTTP**（网关自起一份、假上游在自己的端口上），装配路径与产品一致
//! （`service::start` + `ingress::router`）。库、数据目录、客户端配置根全在临时目录里：
//! 真实 `%APPDATA%` / `~/.claude` 一个字节都不碰；自启用带进程号的临时 Run 值名。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::Router;
use gateway_core::binding::{self, Binding, Profile};
use gateway_core::clients::ClientEnv;
use gateway_core::egress::provider::{self, Prices, Upstream, Wire};
use gateway_core::ingress::api::{AdminCtx, Maintenance};
use gateway_core::ingress::proxy::Gateway as ProxyGateway;
use gateway_core::service::{self, GatewayConfig};
use gateway_core::store::Store;
use serde_json::{json, Value};

/// 反查用的已知密钥（明文只在内存；入库走 DPAPI 密文，与真实装库路径一样）。
const SECRET: &str = "gw-test-secret-59-known-plaintext-7c1e";
/// 出站请求体 / 入站提示词的标记串：诊断包里**必须**一个都找不到（不含 body）。
const INBOUND_MARKER: &str = "M59-INBOUND-BODY-MARKER";
const DELTA_MARKER: &str = "M59-DELTA-MARKER";

#[derive(Default)]
struct TestControl {
    calls: Mutex<Vec<String>>,
}

impl gateway_core::ingress::api::GatewayControl for TestControl {
    fn start(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("start".into());
        Ok(())
    }
    fn restart(&self, port: u16) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("restart:{port}"));
        Ok(())
    }
}

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cg59-api")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 假上游：`/v1/messages` 回一条**六帧**的 SSE（流式与非流式都走它——网关自己决定缓不缓）。
async fn mock_upstream() -> String {
    async fn messages() -> axum::response::Response {
        use axum::response::IntoResponse;
        let body = format!(
            concat!(
                "event: message_start\n",
                "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"mock-fast\",\"content\":[],\"stop_reason\":null,\"usage\":{{\"input_tokens\":3,\"output_tokens\":0,\"cache_read_input_tokens\":0}}}}}}\n\n",
                "event: content_block_start\n",
                "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
                "event: content_block_delta\n",
                "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{delta}\"}}}}\n\n",
                "event: content_block_stop\n",
                "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
                "event: message_delta\n",
                "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":1}}}}\n\n",
                "event: message_stop\n",
                "data: {{\"type\":\"message_stop\"}}\n\n",
            ),
            delta = DELTA_MARKER
        );
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            body,
        )
            .into_response()
    }
    let app = Router::new().route("/v1/messages", post(messages));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{}", addr.port())
}

struct Harness {
    base: String,
    store: Store,
    gateway: service::Gateway,
    proxy: Arc<ProxyGateway>,
    root: PathBuf,
    mock: String,
}

impl Harness {
    /// 起一份网关（管理面 + 维护面 + 转发面都挂上），并把一条上游 / 目录 / 绑定种进库。
    async fn start(tag: &str) -> Harness {
        let root = tmp(tag);
        let mock = mock_upstream().await;
        let store = Store::open(&root).expect("开库");
        let env = ClientEnv::under(root.join("clients"));
        let control = Arc::new(TestControl::default());
        let proxy = Arc::new(ProxyGateway::new(
            store.clone(),
            std::collections::HashMap::new(),
            binding::BindingTable::default(),
        ));
        let admin = Arc::new(
            AdminCtx::new(
                store.clone(),
                root.clone(),
                env,
                control,
                "0.1.0-test",
                root.join("gateway.db").to_string_lossy().into_owned(),
            )
            .with_proxy(proxy.clone()),
        );
        let config = GatewayConfig {
            port: 0,
            db_path: root.join("gateway.db"),
            version: "0.1.0-test".into(),
            forwarding: Some(proxy.clone()),
            admin: Some(admin),
            maintenance: Some(Maintenance::with_autostart_value(
                store.clone(),
                root.clone(),
                format!("Claude Gateway (t59 api {}-{tag})", std::process::id()),
            )),
        };
        let gateway = service::start(config).await.expect("起网关");
        let h = Harness {
            base: format!("http://127.0.0.1:{}", gateway.port()),
            store,
            gateway,
            proxy,
            root,
            mock,
        };
        h.seed().await;
        h
    }

    /// 种上游 / 目录条目 / 手工价 / 绑定 / 凭据，再让转发面热重载（与界面改完配置同一条路）。
    async fn seed(&self) {
        let mut up = Upstream::new("u1", "u1", self.mock.clone());
        up.set_path(Wire::Anthropic, "/v1/messages");
        provider::save_upstream(&self.store, &up)
            .await
            .expect("种上游");
        provider::save_manual_model(&self.store, "u1", "mock-fast", &[Wire::Anthropic])
            .await
            .expect("种目录条目");
        provider::set_manual_prices(
            &self.store,
            "u1",
            "mock-fast",
            &Prices {
                input: Some(3.0),
                output: Some(15.0),
                ..Default::default()
            },
        )
        .await
        .expect("种手工价");
        binding::save(
            &self.store,
            &Binding::new(
                Profile::Global,
                "claude-haiku",
                "u1",
                "mock-fast",
                Wire::Anthropic,
            ),
        )
        .await
        .expect("种绑定");
        self.store
            .put_credential("u1", "default", SECRET)
            .await
            .expect("种凭据");
        self.proxy.reload().await.expect("热重载");
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = reqwest::get(format!("{}{path}", self.base)).await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .put(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    /// 走网关发一条 claude-haiku 请求（`stream` 决定走 Pump 还是 forward_buffered）。
    async fn ask(&self, stream: bool) -> u16 {
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/messages", self.base))
            .json(&json!({
                "model": "claude-haiku",
                "max_tokens": 8,
                "stream": stream,
                "messages": [{"role": "user", "content": INBOUND_MARKER}],
            }))
            .send()
            .await
            .expect("POST /v1/messages 失败");
        let status = resp.status().as_u16();
        let _ = resp.text().await;
        status
    }

    /// 等第一行日志出现，回它的行 id。
    async fn first_log_id(&self) -> i64 {
        for _ in 0..100 {
            let (status, v) = self.get("/api/logs?limit=5").await;
            if status == 200 {
                if let Some(id) = v["logs"]
                    .as_array()
                    .and_then(|l| l.first())
                    .and_then(|r| r["id"].as_i64())
                {
                    return id;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("10 秒内没有请求日志落库");
    }

    async fn stop(self) {
        self.gateway.stop().await;
    }
}

/// 用量页：五个范围 × 三个维度 + 按天明细；无价 / token 未知单独计数、不计入总额。
#[tokio::test]
async fn usage_report_gives_five_ranges_three_dimensions_and_a_daily_table() {
    let h = Harness::start("usage").await;
    for stream in [true, false] {
        assert_eq!(h.ask(stream).await, 200, "haiku 请求该通");
    }
    h.first_log_id().await;

    // 三维度：同一份数据换分组键。
    for (dim, key) in [
        ("virtual_model", "claude-haiku"),
        ("upstream", "u1"),
        ("client", "global"),
    ] {
        let (status, v) = h.get(&format!("/api/usage?range=today&dim={dim}")).await;
        assert_eq!(status, 200, "{dim}: {v}");
        assert_eq!(v["totals"]["requests"], 2, "{dim}: {v}");
        assert!(
            v["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["key"] == key),
            "维度 {dim} 该有分组键 {key}：{v}"
        );
    }

    // 花费与 token：3 入 + 1 出，每百万 $3 / $15 → 两发共 $0.0001 级别（两位小数下界由界面管）。
    let (_, today) = h.get("/api/usage?range=today&dim=virtual_model").await;
    assert_eq!(today["totals"]["input_tokens"], 6, "{today}");
    assert_eq!(today["totals"]["output_tokens"], 2, "{today}");
    assert_eq!(today["totals"]["no_price"], 0, "{today}");
    assert_eq!(today["totals"]["token_unknown"], 0, "{today}");
    let cost = today["totals"]["cost_usd"].as_f64().unwrap();
    assert!(
        (cost - 0.000048).abs() < 1e-12,
        "两发 3×$3 + 1×$15：{today}"
    );

    // 按天明细：只有今天一天，日期是**本机本地**的自然日（界面直接显示这个串）。
    // 本地日边界的精确口径钉在 `obs::tests::range_windows_use_local_natural_days`（固定时区 +
    // .NET 独立算过的字面量）；这里验的是接线：那个串得能当自定义区间的入参用回来。
    let daily = today["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 1, "{today}");
    let day = daily[0]["day"].as_str().unwrap().to_string();
    assert_eq!(day.len(), 10, "日期串要是 YYYY-MM-DD：{today}");
    assert_eq!(&day[4..5], "-", "{today}");
    assert_eq!(&day[7..8], "-", "{today}");
    assert_eq!(daily[0]["requests"], 2, "{today}");

    // 其余四个范围与自定义区间都认；坏范围 / 坏日期是 400，不悄悄给一份错的。
    for range in ["7d", "30d", "all"] {
        let (status, v) = h
            .get(&format!("/api/usage?range={range}&dim=upstream"))
            .await;
        assert_eq!(status, 200, "{range}: {v}");
        assert_eq!(v["totals"]["requests"], 2, "{range}: {v}");
    }
    let (status, v) = h
        .get(&format!(
            "/api/usage?range=custom&from={day}&to={day}&dim=client"
        ))
        .await;
    assert_eq!(status, 200, "自定义区间：{v}");
    assert_eq!(v["totals"]["requests"], 2, "{v}");
    let (status, e) = h.get("/api/usage?range=yesterday&dim=client").await;
    assert_eq!(status, 400, "{e}");
    let (status, e) = h
        .get("/api/usage?range=custom&from=2026-10-15&to=2026-10-01&dim=client")
        .await;
    assert_eq!(status, 400, "{e}");
    // 概览那份走的还是同一个端点（两处一份实现）。
    let (status, v) = h.get("/api/usage/summary?range=7d&dim=upstream").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["totals"]["requests"], 2, "{v}");

    h.stop().await;
}

/// 日志详情四段：概要 / 请求与响应体 / 帧时间轴（可切原始 SSE 文本）/ 错误信息。
#[tokio::test]
async fn log_detail_opens_four_sections_with_frames_and_raw_sse() {
    let h = Harness::start("detail").await;
    // body 开关：默认关 → 帧与 body 都不记（响应内容也是内容，同一个闸门）。
    assert_eq!(h.ask(true).await, 200);
    let id = h.first_log_id().await;
    let (status, closed) = h.get(&format!("/api/logs/{id}")).await;
    assert_eq!(status, 200, "{closed}");
    assert_eq!(closed["frames"]["frames"].as_array().unwrap().len(), 0);
    assert_eq!(closed["frames"]["sse"], Value::Null);
    assert_eq!(closed["bodies"]["recorded"], false);
    assert_eq!(closed["summary"]["virtual_model"], "claude-haiku");

    // 开启（要一次隐私确认）后再发一发：四段都该有东西。
    let (status, on) = h
        .post(
            "/api/logs/body-recording",
            json!({"enabled": true, "consent": true}),
        )
        .await;
    assert_eq!(status, 200, "{on}");
    assert_eq!(on["on"], true, "{on}");
    assert_eq!(h.ask(true).await, 200);
    let id = h.first_log_id().await;
    let (status, d) = h.get(&format!("/api/logs/{id}")).await;
    assert_eq!(status, 200, "{d}");

    // ① 概要
    assert_eq!(d["summary"]["upstream"], "u1");
    assert_eq!(d["summary"]["real_model"], "mock-fast");
    assert_eq!(d["summary"]["wire"], "anthropic");
    assert_eq!(d["summary"]["status"], 200);
    assert!(d["summary"]["total_ms"].as_i64().unwrap() >= 0);
    assert_eq!(d["summary"]["input_tokens"], 3, "{d}");

    // ② 请求与响应体（只在 body 记录开启时有）
    assert_eq!(d["bodies"]["recorded"], true, "{d}");
    assert!(
        d["bodies"]["request"]
            .as_str()
            .unwrap()
            .contains("mock-fast"),
        "请求体该是发给上游的那份：{d}"
    );
    let sse = d["bodies"]["response"].as_str().unwrap();
    assert!(sse.contains("event: message_start"), "{d}");
    assert!(sse.contains(DELTA_MARKER), "原始 SSE 文本要字节原样：{d}");

    // ③ 帧时间轴：逐帧时间戳 + 事件类型 + 摘要，可切原始 SSE 文本
    let frames = d["frames"]["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 6, "六帧一个不少：{d}");
    let events: Vec<&str> = frames
        .iter()
        .map(|f| f["event"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        events,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ],
        "{d}"
    );
    assert_eq!(frames[0]["seq"], 0);
    assert!(frames[2]["summary"]
        .as_str()
        .unwrap()
        .contains(DELTA_MARKER));
    assert!(frames[5]["raw"].as_str().unwrap().contains("message_stop"));
    assert!(frames
        .windows(2)
        .all(|w| w[1]["at_ms"].as_i64() >= w[0]["at_ms"].as_i64()));
    // 「切原始 SSE 文本」= 同一份上游原样字节（不是重建的帧拼起来）。
    assert_eq!(d["frames"]["sse"], d["bodies"]["response"]);

    // ④ 错误信息：成功那一发没有模板也没有原文。
    assert_eq!(d["error"]["raw"], Value::Null, "{d}");
    assert_eq!(d["error"]["template"], Value::Null, "{d}");

    // 按 request_id 也取得到（日志列表里那把钥匙）。
    let rid = d["summary"]["request_id"].as_str().unwrap().to_string();
    let (status, by_rid) = h.get(&format!("/api/logs/{rid}")).await;
    assert_eq!(status, 200, "{by_rid}");
    assert_eq!(by_rid["summary"]["id"], d["summary"]["id"]);

    // 查不到是 404，不是 200 + 空详情。
    let (status, e) = h.get("/api/logs/999999").await;
    assert_eq!(status, 404, "{e}");

    // 非流式出口（forward_buffered）也记帧。
    // 先把开关重拨一次（前面的窗口还在），再发一发非流式。
    assert_eq!(h.ask(false).await, 200);
    let id = h.first_log_id().await;
    let (_, buffered) = h.get(&format!("/api/logs/{id}")).await;
    assert_eq!(
        buffered["frames"]["frames"].as_array().unwrap().len(),
        6,
        "{buffered}"
    );

    // 清空日志：行、body、帧一起走。
    let (status, cleared) = h.post("/api/logs/clear", json!({})).await;
    assert_eq!(status, 200, "{cleared}");
    assert!(cleared["deleted"].as_i64().unwrap() >= 2, "{cleared}");
    println!(
        "[t59] 详情四段：{} 帧 / SSE {} 字节 / 清空 {} 行",
        frames.len(),
        sse.len(),
        cleared["deleted"]
    );
    let (status, _) = h.get(&format!("/api/logs/{id}")).await;
    assert_eq!(status, 404, "清空后再查该是 404");

    h.stop().await;
}

/// 诊断包与导出文件：**不含密钥明文，也不含 body**（两个面都带正对照）。
#[tokio::test]
async fn diagnostics_and_export_leak_neither_secret_nor_body() {
    let h = Harness::start("redaction").await;
    let (status, on) = h
        .post(
            "/api/logs/body-recording",
            json!({"enabled": true, "consent": true}),
        )
        .await;
    assert_eq!(status, 200, "{on}");
    assert_eq!(h.ask(true).await, 200);
    let id = h.first_log_id().await;
    let (_, d) = h.get(&format!("/api/logs/{id}")).await;
    let rid = d["summary"]["request_id"].as_str().unwrap().to_string();
    // 前提：这一步真的记下了 body 与帧（否则「查不到」是废话）。
    assert!(
        d["bodies"]["request"]
            .as_str()
            .unwrap()
            .contains("mock-fast"),
        "{d}"
    );
    assert!(
        d["bodies"]["response"]
            .as_str()
            .unwrap()
            .contains(DELTA_MARKER),
        "{d}"
    );

    // ── 诊断包 ──
    let (status, diag) = h
        .post(
            "/api/diagnostics/copy",
            json!({"requestId": rid, "error": "上游 502：bad gateway"}),
        )
        .await;
    assert_eq!(status, 200, "{diag}");
    let text = diag["text"].as_str().unwrap();
    // 正对照：诊断包里必须找得到我们明知存在的东西，否则「查不到」可能只是查错了地方。
    assert!(text.contains("0.1.0-test"), "正对照：版本：{text}");
    assert!(text.contains("\"u1\""), "正对照：上游配置：{text}");
    assert!(
        text.contains("上游 502：bad gateway"),
        "正对照：错误原文：{text}"
    );
    assert!(text.contains(&rid), "正对照：相关请求元数据：{text}");
    // 诊断包那几格：端口与**运行状态**都得如实（不是写死的 true）。
    assert!(text.contains("\"running\": true"), "运行状态：{text}");
    assert!(text.contains("\"port\":"), "端口：{text}");
    // 反查：密钥明文、请求体、响应体、入站提示词一处都不许有。
    for banned in [
        SECRET,
        "sk-",
        INBOUND_MARKER,
        DELTA_MARKER,
        "event: message_start",
    ] {
        assert!(!text.contains(banned), "诊断包里出现了 {banned}：{text}");
    }

    // ── 导出文件（票 62 的后端，票 59 的验收要在这里反查）──
    let export = h.root.join("export.json");
    let (status, rep) = h
        .post(
            "/api/export",
            json!({"path": export.to_string_lossy(), "includeSecrets": false}),
        )
        .await;
    assert_eq!(status, 200, "{rep}");
    assert_eq!(rep["secrets"], false, "{rep}");
    assert_eq!(rep["encrypted"], false, "{rep}");
    let file = std::fs::read_to_string(&export).expect("读导出文件");
    assert!(!file.contains(SECRET), "导出文件里有密钥明文");
    // 正对照：导出的是**这份配置**（上游、目录条目、绑定都在），密钥只留掩码。
    assert!(file.contains("\"u1\""), "正对照：上游 id：{file}");
    assert!(file.contains("mock-fast"), "正对照：目录条目：{file}");
    assert!(file.contains("claude-haiku"), "正对照：绑定：{file}");
    assert!(
        file.contains("gw-"),
        "正对照：掩码在（前 3 位），明文不在：{file}"
    );
    assert!(file.contains("\"secrets\": false"), "{file}");

    // 真跑一回的摘录（`--nocapture` 看得到）：反查与正对照各几处命中。
    let hits = |needle: &str| -> usize { text.matches(needle).count() };
    println!(
        "[t59] 诊断包 {} 字节：密钥明文 {} / 掩码片段 sk- {} / 入站提示词 {} / 响应体标记 {}；\
         正对照 版本 {} / 上游 {}",
        text.len(),
        hits(SECRET),
        hits("sk-"),
        hits(INBOUND_MARKER),
        hits(DELTA_MARKER),
        hits("0.1.0-test"),
        hits("\"u1\"")
    );
    println!(
        "[t59] 导出 {} 字节：密钥明文 {} / 上游 id {} / 目录条目 {} / 绑定 {} / 掩码前缀 {}",
        file.len(),
        file.matches(SECRET).count(),
        file.matches("\"u1\"").count(),
        file.matches("mock-fast").count(),
        file.matches("claude-haiku").count(),
        file.matches("gw-").count()
    );

    h.stop().await;
}

/// 设置页：读得到全部格子、写回保留期 / 汇率 / 托盘行为；about 给版本与回退指引。
#[tokio::test]
async fn settings_page_reads_and_writes_its_fields() {
    let h = Harness::start("settings").await;

    let (status, s) = h.get("/api/settings").await;
    assert_eq!(status, 200, "{s}");
    assert_eq!(s["version"], "0.1.0-test");
    assert!(s["port"].as_u64().unwrap() > 0);
    assert!(s["dbPath"].as_str().unwrap().ends_with("gateway.db"));
    assert!(s["dataDir"].as_str().unwrap().contains("cg59-api"));
    assert_eq!(s["logRetentionDays"], 7, "保留期初值 7 天：{s}");
    assert_eq!(s["usdCnyRate"], 7.10, "汇率随包初值：{s}");
    assert_eq!(s["catalogSnapshotAt"], Value::Null, "{s}");
    assert_eq!(s["trayCloseAction"], "hide", "托盘行为默认关窗隐藏：{s}");
    assert_eq!(s["bodyRecording"]["on"], false);
    assert_eq!(s["bodyRecording"]["window_seconds"], 3600);
    assert_eq!(s["bodyRecording"]["retention_days"], 7);
    assert_eq!(s["autostart"]["enabled"], false, "默认关：{s}");
    // 凭据不放设置页（票 17 第 66 条）：整份响应里不许出现密钥相关字段。
    assert!(!s.to_string().contains("credential"), "{s}");
    assert!(!s.to_string().contains("sk-"), "{s}");

    // 写三项（body 开关另有自己的端点，这里不碰）。
    let (status, w) = h
        .put(
            "/api/settings",
            json!({"logRetentionDays": 30, "usdCnyRate": 7.35, "trayCloseAction": "quit"}),
        )
        .await;
    assert_eq!(status, 200, "{w}");
    assert_eq!(w["logRetentionDays"], 30, "{w}");
    assert_eq!(w["usdCnyRate"], 7.35, "{w}");
    assert_eq!(w["trayCloseAction"], "quit", "{w}");
    // 读回来还是这份（不是只改了回执）。
    let (_, again) = h.get("/api/settings").await;
    assert_eq!(again["logRetentionDays"], 30, "{again}");
    assert_eq!(again["trayCloseAction"], "quit", "{again}");

    // 越界一律 400，不静默截断。
    for bad in [
        json!({"logRetentionDays": -1}),
        json!({"logRetentionDays": 100000}),
        json!({"usdCnyRate": 0}),
        json!({"usdCnyRate": -3}),
        json!({"trayCloseAction": "sleep"}),
    ] {
        let (status, e) = h.put("/api/settings", bad.clone()).await;
        assert_eq!(status, 400, "{bad} 该被拒：{e}");
    }
    // 打开目录只认 data / logs（写错名字不许起资源管理器）。
    let (status, e) = h
        .post("/api/settings/db/open", json!({"which": "elsewhere"}))
        .await;
    assert_eq!(status, 400, "{e}");

    // 关于与版本：回退指引 + 免安装提示 + 检查更新的现状。
    let (status, a) = h.get("/api/about").await;
    assert_eq!(status, 200, "{a}");
    assert_eq!(a["version"], "0.1.0-test");
    assert!(a["rollback"]["guidance"]
        .as_str()
        .unwrap()
        .contains("updates"));
    assert!(a["updatesDir"].as_str().unwrap().contains("updates"));
    assert_eq!(a["rollback"]["hasPrevious"], false);
    assert!(a["portable"]["isPortable"].is_boolean());
    assert!(
        !a["portable"]["note"].as_str().unwrap().is_empty(),
        "免安装提示不能空着：{a}"
    );
    assert_eq!(a["update"]["supported"], false);

    // 端口那一项走的是 `/api/gateway/port` 同一段 `apply_port_change`（写库 + 同步已生效客户端）。
    // 这一档没有外壳，按新端口重启那一半不做（真实应用里由外壳做）。
    let free = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("探空闲端口");
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        port
    };
    let (status, moved) = h.put("/api/settings", json!({"port": free})).await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["port"], free, "端口写进去了：{moved}");
    let (_, back) = h.get("/api/settings").await;
    assert_eq!(back["port"], free, "{back}");
    // 已被占用的端口不许写进去（409）。
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("占一个端口");
    let held_port = held.local_addr().unwrap().port();
    let (status, e) = h.put("/api/settings", json!({"port": held_port})).await;
    assert_eq!(status, 409, "被占的端口该 409：{e}");

    h.stop().await;
}

/// 联网那一发（`#[ignore]`，只在手动跑 `--ignored` 时打真实 models.dev）：
/// 设置页的「刷新快照」端点回什么——成功与「失败沿用旧快照」两种都算通过（口径如此）。
#[tokio::test]
#[ignore = "打真实网络（models.dev）；手动 cargo test -- --ignored 时跑"]
async fn live_catalog_refresh_reports_what_it_did() {
    let h = Harness::start("catalog-live").await;
    let (status, r) = h.post("/api/catalog/refresh", json!({})).await;
    assert_eq!(status, 200, "{r}");
    println!("[t59] /api/catalog/refresh -> {r}");
    assert!(r["message"].as_str().unwrap().contains("快照"), "{r}");
    if r["refreshed"] == true {
        assert!(r["imported"].as_u64().unwrap() > 0, "{r}");
    } else {
        assert_eq!(r["reusedOldSnapshot"], true, "{r}");
    }
    h.stop().await;
}
