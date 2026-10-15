//! 集成测试的共用底座：把网关与它的 mock 上游**两个进程**拉起来，再收拾干净。
//! 测试自己起环境（票 19 §1）：一条命令就能从零跑，不依赖「先手工启动网关」。
//!
//! 票 30 改了两处：① 端口按「进程 id + 递增序号」现算，多个 `#[tokio::test]` 才能并行各起一份；
//! ② 凭据文件里放两个**已知密钥字符串**，供「诊断/API/库里查不到密钥」那一格反查。
//!
//! 票 41 改了两处（都很关键）：
//! ① 网关与 mock 是**两个独立进程**（`gateway-harness` + `mock-upstream`），mock 不再挂在
//!    网关的 router 上；
//! ② 日志不再经 `GET /api/logs` 读（`/api/*` 归票 43），改成测试直接开票 42 的 `Store`
//!    读 `requests` 表——这本来就是那条断言的实质（库里有几行、body 写没写）。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

pub mod golden;

/// 反查用的已知密钥（票 19 §10 的日志与脱敏③）。真上游地址故意指向打不通的端口：
/// 集成测试只碰 mock，绝不打真实上游。
pub const SECRET_OPENCODE: &str = "gw-test-secret-opencode-2f9c";
pub const SECRET_COMMANDCODE: &str = "gw-test-secret-commandcode-71ab";

static NEXT: AtomicU32 = AtomicU32::new(0);

pub struct Gateway {
    pub base: String,
    pub mock: String,
    pub fixtures: PathBuf,
    dir: PathBuf,
    gateway: Child,
    mock_child: Child,
}

impl Gateway {
    /// 默认环境：保活与探针档都用产品默认值。
    pub async fn start() -> Gateway {
        Gateway::start_with(&[]).await
    }

    /// 带环境覆盖的启动（`GATEWAY_PING_SECS` / `GATEWAY_PROBE_SECS` / `GATEWAY_LOG_BODY`）。
    pub async fn start_with(extra: &[(&str, &str)]) -> Gateway {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures = manifest.join("tests").join("fixtures");
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let seed = (std::process::id() % 977) * 7 + n;
        let port: u16 = 19000 + ((seed % 4000) * 2) as u16;
        let mock_port: u16 = port + 1;
        let dir = std::env::temp_dir().join(format!("gw-replay-{}-{port}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let env_file = dir.join("upstreams.env");
        std::fs::write(
            &env_file,
            format!(
                "OPENCODE_BASE_URL=http://127.0.0.1:1\nOPENCODE_API_KEY={SECRET_OPENCODE}\n\
                 COMMANDCODE_BASE_URL=http://127.0.0.1:1\nCOMMANDCODE_API_KEY={SECRET_COMMANDCODE}\n"
            ),
        )
        .expect("写测试用凭据文件失败");

        // ① mock 上游：独立进程，回放 fixtures（回显与切法是它自己的状态）。
        let mock_child = Command::new(env!("CARGO_BIN_EXE_mock-upstream"))
            .env("GATEWAY_FIXTURES", &fixtures)
            .env("GATEWAY_MOCK_PORT", mock_port.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("起不来 mock-upstream 二进制");

        // ② 网关：同一份 `/v1/*` 路由（`ingress::proxy::router()`），只按 base_url 打 mock。
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gateway-harness"));
        cmd.env("GATEWAY_ENV", &env_file)
            .env("GATEWAY_DATA_DIR", dir.join("data"))
            .env("GATEWAY_PORT", port.to_string())
            .env("GATEWAY_MOCK_PORT", mock_port.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let gateway = cmd.spawn().expect("起不来 gateway-harness 二进制");

        let gw = Gateway {
            base: format!("http://127.0.0.1:{port}"),
            mock: format!("http://127.0.0.1:{mock_port}"),
            fixtures,
            dir,
            gateway,
            mock_child,
        };

        let client = reqwest::Client::new();
        for _ in 0..100 {
            if let Ok(r) = client.get(format!("{}/healthz", gw.base)).send().await {
                if r.status().is_success() {
                    return gw;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("网关 10 秒内没起来（{}/healthz 不通）", gw.base);
    }

    /// 网关自己的 GET（`/v1/models` 这类转发面端点）。
    pub async fn get_json(&self, path: &str) -> Value {
        reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("GET 失败")
            .json()
            .await
            .expect("响应不是 JSON")
    }

    /// mock 的请求回显（`GET <mock>/api/echo`）：头类 quirk / 路径模板 / auth 头靠它断言。
    pub async fn echo(&self) -> Value {
        reqwest::Client::new()
            .get(format!("{}/api/echo", self.mock))
            .send()
            .await
            .expect("GET /api/echo 失败")
            .json()
            .await
            .expect("回显不是 JSON")
    }

    /// 请求日志：直接读库（票 42 的 `Store` + `obs::recent`）。
    /// 日志是异步落库的（转发侧不 `await` 到底），所以等到至少 `min_rows` 行再回。
    pub async fn logs(&self, min_rows: usize) -> Vec<Value> {
        use gateway_core::store::Store;
        let Ok(store) = Store::open(&self.dir.join("data")) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for _ in 0..100 {
            rows = gateway_core::obs::recent(&store, 50)
                .await
                .unwrap_or_default();
            if rows.len() >= min_rows {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        rows
    }

    /// 拨 mock 的回放切法（`frame` / `half` / `byte` / `whole` / `crlf`）。
    pub async fn set_chunks(&self, scheme: &str) {
        let resp = reqwest::Client::new()
            .post(format!("{}/__chunks", self.mock))
            .json(&serde_json::json!({ "scheme": scheme }))
            .send()
            .await
            .expect("POST /__chunks 失败");
        assert!(
            resp.status().is_success(),
            "切法 {scheme} 被 mock 拒绝：{}",
            resp.status()
        );
    }

    /// 拨 body 日志开关的窗口（秒；0 = 立即关）。
    pub async fn set_body_log(&self, seconds: u64) {
        let (status, _) = self
            .post_json("/__bodylog", &serde_json::json!({ "seconds": seconds }))
            .await;
        assert_eq!(status, 200, "POST /__bodylog 没通");
    }

    /// POST 一个 JSON 端点（`count_tokens` / `__bodylog`），返回 (状态码, JSON)。
    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .expect("POST 失败");
        let status = resp.status().as_u16();
        let v = resp.json().await.unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    /// 走网关发一次流式请求，返回 (HTTP 状态码, 原始响应字节)。
    pub async fn post_messages(&self, body: &serde_json::Value) -> (u16, Vec<u8>) {
        self.post_messages_with(body, &[]).await
    }

    /// 带自定义入站头（`anthropic-version` / `anthropic-beta` / `user-agent` 这类）的请求。
    pub async fn post_messages_with(
        &self,
        body: &serde_json::Value,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<u8>) {
        let mut rb = reqwest::Client::new()
            .post(format!("{}/v1/messages", self.base))
            .json(body);
        for (k, v) in headers {
            rb = rb.header(*k, *v);
        }
        let resp = rb.send().await.expect("POST /v1/messages 失败");
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.expect("读响应体失败").to_vec();
        (status, bytes)
    }

    /// 走网关发一次 Responses 请求（票 32），返回 (HTTP 状态码, 原始响应字节)。
    pub async fn post_responses(&self, body: &serde_json::Value) -> (u16, Vec<u8>) {
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/responses", self.base))
            .json(body)
            .send()
            .await
            .expect("POST /v1/responses 失败");
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.expect("读响应体失败").to_vec();
        (status, bytes)
    }

    /// 逐块计时版：返回 (状态码, 字节, 首块毫秒, 读完毫秒)。
    /// 首帧延迟不变式（票 19 §10）靠它——整包缓冲的话首块会等到流末尾。
    pub async fn post_messages_timed(
        &self,
        body: &serde_json::Value,
    ) -> (u16, Vec<u8>, u128, u128) {
        use futures_util::StreamExt;
        let t0 = Instant::now();
        let resp = reqwest::Client::new()
            .post(format!("{}/v1/messages", self.base))
            .json(body)
            .send()
            .await
            .expect("POST /v1/messages 失败");
        let status = resp.status().as_u16();
        let mut stream = resp.bytes_stream();
        let mut first: Option<u128> = None;
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap_or_else(|e| panic!("读响应流失败：{e}"));
            if first.is_none() {
                first = Some(t0.elapsed().as_millis());
            }
            bytes.extend_from_slice(&chunk);
        }
        (status, bytes, first.unwrap_or(0), t0.elapsed().as_millis())
    }

    /// 测试库的原始字节（密钥反查用）。
    pub fn db_bytes(&self) -> Vec<u8> {
        std::fs::read(self.dir.join("data").join("gateway.db")).unwrap_or_default()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
        let _ = self.mock_child.kill();
        let _ = self.mock_child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
