//! 声明式供应商表 + 上游模型目录 + 价格（spec §6 / §9；票 23 / 33 / 35 / 49）。
//!
//! 三张表都在这里读写（用票 42 的 [`Store`] 自己写 SQL，不往 store 模块里塞领域类型）：
//!
//! - `upstreams`：一个上游一行。`auth_json` 按「上游 × wire」各一份（未填继承 `default`），
//!   `path_templates_json` 按 wire 各一条路径模板，`quirks_json` 只放枚举值 + 载荷。
//! - `credentials`：按「上游 × wire」的 DPAPI 密文；读写走 [`Store`] 已有接口。
//! - `upstream_models`：上游模型目录 = `(上游, 真实模型)` 一行，`supported_wires`（空集合 =
//!   未声明，正常态）+ `source`（`manual` 锁定 / `upstream` / `doc`）+ 手工价四列。
//! - `model_catalog`：models.dev 随包快照（`cost.*` 每百万 token 美元），手工价之外的唯一
//!   自动价格来源。
//!
//! # 三层校验（ADR-0004 第 4 条）
//!
//! 1. **配置期主拦**：[`UpstreamModel::allowed_wires`] 是下拉里唯一出现的值；
//! 2. **请求期早拒 400**：[`UpstreamModel::validate_wire`] 回 [`EarlyReject`]（`invalid_request_error`）；
//! 3. **自检为准**：声明拿不到时（opencode 两面、千问的 Anthropic 面）只有自检说了算，
//!    结果落在 `upstreams.last_selfcheck_*`（v3 迁移加的列，见 [`crate::store::schema`]）。
//!
//! # 口径
//!
//! - 路径反查：上游报的端点路径按 `pathTemplates` 反查成 wire；**查不到就忽略并记一条警告**
//!   （上游报的路径 + 我们期望的路径），警告进「上次拉取警告」，下次成功拉取自动清除。
//! - 价格优先级：**手工 > 快照 > 未知**，逐项判定；手工填了就锁定，快照刷新不动它。
//! - 拉取：20 秒超时、不自动重试（票 35 第 9 条）；快照刷新 30 秒、同样不重试（票 49 自定）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::store::{settings_keys, Store};

/// 拉模型的路径（相对 `baseUrl`）。`/models` 不属于任何一个 wire，所以不进 `path_templates`。
pub const MODELS_PATH: &str = "/models";

/// 拉模型超时（票 35 第 9 条：20 秒，超时算失败，不自动重试）。
pub const PULL_TIMEOUT: Duration = Duration::from_secs(20);

/// 快照刷新超时。票 33 只写「失败静默沿用旧快照、不自动重试」，没给秒数；
/// 票 49 定 **30 秒**——载荷是整个 `api.json`（实测约 5.6 MB），比逐上游的模型清单大得多，
/// 沿票 35 的 20 秒会容易假超时。同样不自动重试。
pub const CATALOG_REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

/// models.dev 全量快照地址（设置页「刷新价格快照」打的就是它）。
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// 随包内置的 models.dev 快照（票 33：不联网也有价；设置页手动刷新才整体换掉）。
pub const BUNDLED_MODELS_DEV_SNAPSHOT: &str = include_str!("models_dev_snapshot.json");

/// 随包内置的文档表：**不声明端点的上游**（opencode 两面都是）逐模型 wire 的唯一非手工来源。
/// 依据 `research/providers.md` §3 的 Go 官方表；模型 ID 带 `*` 后缀表示前缀匹配。
pub const DOC_TABLE: &str = include_str!("doc_models.json");

/// 未声明条目在界面上的标注（票 23 第 3 节第 1 条）。
pub const UNDECLARED_NOTE: &str = "未声明，靠自检";

// ───────────────────────────── 取值域 ─────────────────────────────

/// 出站协议（spec §6）：三个值，不是两个——否则表达不了「只答 `/chat/completions` 不答
/// `/responses`」（commandcode 有 8+ 个这样的模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Wire {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
}

impl Wire {
    pub const ALL: [Wire; 3] = [Wire::Anthropic, Wire::OpenAiChat, Wire::OpenAiResponses];

    pub fn as_str(&self) -> &'static str {
        match self {
            Wire::Anthropic => "anthropic",
            Wire::OpenAiChat => "openai-chat",
            Wire::OpenAiResponses => "openai-responses",
        }
    }

    /// 读入侧容忍历史写法：`openai`（v1 原型的两值域）与 camelCase（spec §6 样例 JSON、
    /// v2 迁移写进 `path_templates_json` 的键）。写出去一律用 [`Wire::as_str`] 的规范值。
    pub fn parse(s: &str) -> Option<Wire> {
        match s.trim() {
            "anthropic" => Some(Wire::Anthropic),
            "openai-chat" | "openaiChat" | "openai" => Some(Wire::OpenAiChat),
            "openai-responses" | "openaiResponses" | "responses" => Some(Wire::OpenAiResponses),
            _ => None,
        }
    }
}

/// 认证方式（原型两个值，逐字保留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    Bearer,
    XApiKey,
}

impl AuthScheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthScheme::Bearer => "bearer",
            AuthScheme::XApiKey => "x-api-key",
        }
    }

    /// 兜底 `Bearer`（原型行为：认不出的写法当 bearer，不因为一个脏值让整条上游不可用）。
    pub fn parse(s: &str) -> AuthScheme {
        match s.trim() {
            "x-api-key" | "x-api-key-header" => AuthScheme::XApiKey,
            _ => AuthScheme::Bearer,
        }
    }
}

/// quirks 只允许枚举值（spec §6）；新增一个必须同时新增一条回归 fixture。
/// **「哪些协议面服务哪些模型」不在 quirks 里**——那是上游模型目录的职责
/// （`ClaudeModelsOnAnthropicOnly` 已按 ADR-0004 删除）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quirk {
    /// 上游要求客户端自称（形如 `my-coding-agent/1.0`），不接受通用 SDK UA。
    ClientUserAgent(String),
    /// 同一段对话必须带稳定会话 ID 的头名。
    SessionIdHeader(String),
    /// 该上游根本不提供模型发现端点：向导不必尝试，直接给手填入口。
    NoModelsEndpoint,
    /// 模型清单**只有带凭据那份可信**：不带凭据会拿到未授权的全量目录（opencode 实测 19/85，
    /// 差异是权限过滤，未授权模型调用一律 403）。含义：没凭据就不拉；带凭据 404 / 零条才退匿名兜底
    /// （票 35 第 8 条；执法点在 [`crate::egress::discovery::run_pull`]）。
    ModelsListNeedsCredential,
    TemperatureMax2,
    EmptyThinkingSignature,
    SilentTruncate,
}

impl Quirk {
    pub const NAMES: [&'static str; 7] = [
        "ClientUserAgent",
        "SessionIdHeader",
        "NoModelsEndpoint",
        "ModelsListNeedsCredential",
        "TemperatureMax2",
        "EmptyThinkingSignature",
        "SilentTruncate",
    ];

    /// 枚举值的全集（载荷留空）：票 19 §6 的双向守卫拿它跟 fixture 标签对齐。
    pub fn all() -> Vec<Quirk> {
        vec![
            Quirk::ClientUserAgent(String::new()),
            Quirk::SessionIdHeader(String::new()),
            Quirk::NoModelsEndpoint,
            Quirk::ModelsListNeedsCredential,
            Quirk::TemperatureMax2,
            Quirk::EmptyThinkingSignature,
            Quirk::SilentTruncate,
        ]
    }

    /// 票 19 §6.2：每个 quirk 必须声明它作用的出站 wire / 请求头 / 请求体。
    pub fn acts_on(&self) -> &'static str {
        match self {
            Quirk::ClientUserAgent(_) => "出站请求头 user-agent（注入网关自称）",
            Quirk::SessionIdHeader(_) => {
                "出站请求头（头名由变体载荷给出，值为按首条 user 文本派生的会话 ID）"
            }
            Quirk::NoModelsEndpoint => "上游级事实：不提供模型发现端点（向导不必尝试拉取）",
            Quirk::ModelsListNeedsCredential => {
                "上游级事实：模型清单只有带凭据那份可信（没凭据不拉，带凭据 404/零条才退匿名）"
            }
            Quirk::TemperatureMax2 => "出站请求体：temperature 上限 2",
            Quirk::EmptyThinkingSignature => "出站转换：thinking 签名可能为空串",
            Quirk::SilentTruncate => "出站响应：上游可能静默截断（不给终态）",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Quirk::ClientUserAgent(_) => "ClientUserAgent",
            Quirk::SessionIdHeader(_) => "SessionIdHeader",
            Quirk::NoModelsEndpoint => "NoModelsEndpoint",
            Quirk::ModelsListNeedsCredential => "ModelsListNeedsCredential",
            Quirk::TemperatureMax2 => "TemperatureMax2",
            Quirk::EmptyThinkingSignature => "EmptyThinkingSignature",
            Quirk::SilentTruncate => "SilentTruncate",
        }
    }

    /// 载荷（标记型 quirk 是空串）；与 [`Quirk::name`] 一起构成库里的形状。
    pub fn payload(&self) -> &str {
        match self {
            Quirk::ClientUserAgent(s) | Quirk::SessionIdHeader(s) => s,
            _ => "",
        }
    }

    pub fn parse(name: &str, arg: &str) -> Option<Quirk> {
        match name {
            "ClientUserAgent" => Some(Quirk::ClientUserAgent(arg.to_string())),
            "SessionIdHeader" => Some(Quirk::SessionIdHeader(arg.to_string())),
            "NoModelsEndpoint" => Some(Quirk::NoModelsEndpoint),
            "ModelsListNeedsCredential" => Some(Quirk::ModelsListNeedsCredential),
            "TemperatureMax2" => Some(Quirk::TemperatureMax2),
            "EmptyThinkingSignature" => Some(Quirk::EmptyThinkingSignature),
            "SilentTruncate" => Some(Quirk::SilentTruncate),
            _ => None,
        }
    }
}

/// 目录条目的来源（spec §6）：优先级 `manual` > `upstream` > `doc` > 未声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// 手工新增或手工改过：锁定，重拉不覆盖。
    Manual,
    /// 上游 `/v1/models` 声明的。
    Upstream,
    /// 随包内置文档表给的（不声明端点的上游）。
    Doc,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Upstream => "upstream",
            Source::Doc => "doc",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "manual" => Some(Source::Manual),
            "upstream" => Some(Source::Upstream),
            "doc" => Some(Source::Doc),
            _ => None,
        }
    }
}

/// 上次拉取的结局（票 35 的三支：成功 / 不提供模型发现 / 失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullStatus {
    Ok,
    NoDiscovery,
    Failed,
}

impl PullStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PullStatus::Ok => "ok",
            PullStatus::NoDiscovery => "no-discovery",
            PullStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<PullStatus> {
        match s {
            "ok" => Some(PullStatus::Ok),
            "no-discovery" => Some(PullStatus::NoDiscovery),
            "failed" => Some(PullStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullState {
    /// 上次**尝试**拉取的时间（失败也算尝试过）。
    pub at: i64,
    /// 上次**成功**拉取（真的换过目录）的时间；从没成功过 = `None`。
    /// 失败与「不提供模型发现」不动它，所以卡片能报「沿用上次结果（时间）」（票 35 第 5 条）。
    pub ok_at: Option<i64>,
    pub status: PullStatus,
    /// 「上次拉取警告」：wire 反查不到的端点 / 拉取失败原因。**只有成功拉取才清**（票 35 第 1 条）。
    pub warning: Option<String>,
}

/// 上次自检结果（就绪判据只看它，spec §6 / 票 35 第 7 条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfCheck {
    pub at: i64,
    pub ok: bool,
}

// ───────────────────────── 上游（声明式表） ─────────────────────────

/// `upstreams` 一行。四个 wire 面共用一张卡、一个 `baseUrl`、一次自检（ADR-0004 第 1 条）。
#[derive(Debug, Clone, PartialEq)]
pub struct Upstream {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// 键 = `default` 或 wire 串；某 wire 未填就继承 `default`。
    pub auth: BTreeMap<String, AuthScheme>,
    pub path_templates: BTreeMap<Wire, String>,
    pub header_passthrough: Vec<String>,
    pub quirks: Vec<Quirk>,
    /// 运行期事实，不在 [`save_upstream`] 的写入范围里。
    pub last_pull: Option<PullState>,
    pub last_selfcheck: Option<SelfCheck>,
}

impl Upstream {
    /// 新上游：`default` 用 bearer，三条路径模板照 spec §6（`baseUrl` 不带 `/v1`）。
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let mut auth = BTreeMap::new();
        auth.insert("default".to_string(), AuthScheme::Bearer);
        let mut path_templates = BTreeMap::new();
        path_templates.insert(Wire::Anthropic, "/v1/messages".to_string());
        path_templates.insert(Wire::OpenAiChat, "/v1/chat/completions".to_string());
        path_templates.insert(Wire::OpenAiResponses, "/v1/responses".to_string());
        Self {
            id: id.into(),
            name: name.into(),
            base_url: base_url.into(),
            auth,
            path_templates,
            header_passthrough: Vec::new(),
            quirks: Vec::new(),
            last_pull: None,
            last_selfcheck: None,
        }
    }

    pub fn set_auth(&mut self, key: &str, scheme: AuthScheme) {
        self.auth.insert(key.to_string(), scheme);
    }

    pub fn set_path(&mut self, wire: Wire, path: impl Into<String>) {
        self.path_templates.insert(wire, path.into());
    }

    pub fn path(&self, wire: Wire) -> Option<&str> {
        self.path_templates.get(&wire).map(String::as_str)
    }

    /// 出站 URL = `baseUrl`（去尾斜杠）+ 该 wire 的路径模板。
    pub fn url(&self, wire: Wire) -> Option<String> {
        self.path(wire)
            .map(|p| format!("{}{}", self.base_url.trim_end_matches('/'), p))
    }

    /// 拉模型打的 URL（`/models` 不属于任何一个 wire）。
    pub fn models_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), MODELS_PATH)
    }

    /// 该 wire 的认证方式：先按 wire 找，没有就继承 `default`，再没有就是 bearer。
    pub fn auth_for(&self, wire: Wire) -> AuthScheme {
        self.auth
            .get(wire.as_str())
            .or_else(|| self.auth.get("default"))
            .copied()
            .unwrap_or(AuthScheme::Bearer)
    }

    /// 上游级凭据的认证方式（`default`；拉模型用它）。
    pub fn auth_default(&self) -> AuthScheme {
        self.auth
            .get("default")
            .copied()
            .unwrap_or(AuthScheme::Bearer)
    }

    pub fn has_quirk(&self, q: &Quirk) -> bool {
        self.quirks
            .iter()
            .any(|x| std::mem::discriminant(x) == std::mem::discriminant(q))
    }

    /// `pathTemplates` 的 `wire=路径` 清单，反查警告里要报「我们期望的路径」。
    pub fn expected_paths(&self) -> String {
        self.path_templates
            .iter()
            .map(|(w, p)| format!("{}={}", w.as_str(), p))
            .collect::<Vec<_>>()
            .join("、")
    }

    /// 会话 ID：同一段对话稳定即可（上游用于路由与 prompt 缓存优化）。FNV-1a 够用。
    /// 载荷是 [`Quirk::SessionIdHeader`] 声明的头名。
    pub fn session_id(&self, seed: &str) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in seed.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        format!("gw-{h:016x}")
    }
}

// ---------- env（M1 脚手架：凭据还从 .env 来） ----------

/// 极简 KEY=VALUE 凭据文件（票 41 从原型搬入）。
///
/// 正式路径是票 42 的 [`Store`] + DPAPI（`credentials` 表）；M1 与回放 harness 先用它把
/// 两条真上游接起来，票 51 把路由表改成「按绑定种库」后这个脚手架就该退场。
#[derive(Default)]
pub struct Env {
    pub vars: HashMap<String, String>,
    pub port: Option<u16>,
}

impl Env {
    /// 极简 KEY=VALUE 解析：只要几个键，不值得为它引一个 dotenv 依赖。
    pub fn load(path: &str) -> Env {
        let mut e = Env::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("[warn] 读不到 {path}，真实上游将不可用（只有 mock 能跑）");
            return e;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                e.vars.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        e.port = e.get("GATEWAY_PORT").and_then(|v| v.parse().ok());
        e
    }

    pub fn get(&self, k: &str) -> Option<String> {
        self.vars.get(k).filter(|v| !v.is_empty()).cloned()
    }
}

// ───────────────────────── 上游模型目录 ─────────────────────────

/// `upstream_models` 一行：一个真实模型在某个上游上的可用 wire 集合与来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamModel {
    pub upstream_id: String,
    pub model_id: String,
    /// **空集合 = 未声明**（正常态，不等于不支持）。
    pub supported_wires: BTreeSet<Wire>,
    pub source: Source,
}

impl UpstreamModel {
    /// 配置期主拦：下拉里只出现这些值。
    pub fn allowed_wires(&self) -> Vec<Wire> {
        self.supported_wires.iter().copied().collect()
    }

    /// 第二 / 三层：声明过的 wire 放行；未声明的可自由选但要标注；其余早拒 400。
    pub fn validate_wire(&self, wire: Wire) -> Result<WireChoice, EarlyReject> {
        if self.supported_wires.is_empty() {
            return Ok(WireChoice::Undeclared);
        }
        if self.supported_wires.contains(&wire) {
            return Ok(WireChoice::Declared);
        }
        let declared = self
            .supported_wires
            .iter()
            .map(Wire::as_str)
            .collect::<Vec<_>>()
            .join("、");
        Err(EarlyReject {
            status: 400,
            error_type: "invalid_request_error",
            message: format!(
                "上游 {} 的模型 {} 没有声明 {} 面（声明的是：{}）。改绑定或用自检确认后再试。",
                self.upstream_id,
                self.model_id,
                wire.as_str(),
                declared
            ),
        })
    }
}

/// 校验结论：声明过的 / 未声明的（可自由选，界面标注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireChoice {
    Declared,
    Undeclared,
}

impl WireChoice {
    /// 界面标注；声明过的不需要标注。
    pub fn note(&self) -> Option<&'static str> {
        match self {
            WireChoice::Declared => None,
            WireChoice::Undeclared => Some(UNDECLARED_NOTE),
        }
    }
}

/// 请求期早拒（形状照原型 `upstream::guard`：400 + `invalid_request_error`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyReject {
    pub status: u16,
    pub error_type: &'static str,
    pub message: String,
}

// ───────────────────────── pathTemplates 反查 ─────────────────────────

/// 规范化端点路径：去查询串 / 片段、去尾斜杠、转小写、补前导斜杠。
fn normalize_path(p: &str) -> String {
    let p = p.split(['?', '#']).next().unwrap_or("");
    let p = p.trim().trim_end_matches('/');
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    p.to_ascii_lowercase()
}

/// 上游报的端点路径 → wire。
///
/// 两个方向都比：模板 `/messages` 认上游报的 `/messages` 与 `/v1/messages`，
/// 模板 `/v1/messages` 也认上游报的 `/messages`（票 23 第 5 节：按 `pathTemplates` 反查，
/// **不新增映射表**）。
pub fn wire_for_endpoint(up: &Upstream, reported: &str) -> Option<Wire> {
    let r = normalize_path(reported);
    up.path_templates.iter().find_map(|(wire, tpl)| {
        let t = normalize_path(tpl);
        (r == t || r.ends_with(&t) || t.ends_with(&r)).then_some(*wire)
    })
}

/// 反查一批端点：回（命中的 wire 集合，忽略掉的警告）。
pub fn reverse_lookup(up: &Upstream, endpoints: &[String]) -> (BTreeSet<Wire>, Vec<String>) {
    let mut wires = BTreeSet::new();
    let mut warnings = Vec::new();
    for ep in endpoints {
        match wire_for_endpoint(up, ep) {
            Some(w) => {
                wires.insert(w);
            }
            None => warnings.push(format!(
                "上游报的端点 {} 对不上任何 pathTemplates（期望 {}），已忽略",
                ep.trim(),
                up.expected_paths()
            )),
        }
    }
    (wires, warnings)
}

// ───────────────────────── 价格 ─────────────────────────

/// 手工四项价（每百万 token 美元）；`None` = 该项没填 → 回退快照。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Prices {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// 某一项价格的实际来源（界面要逐项标出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    Manual,
    Snapshot,
    Unknown,
}

impl PriceSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            PriceSource::Manual => "manual",
            PriceSource::Snapshot => "snapshot",
            PriceSource::Unknown => "unknown",
        }
    }
}

/// 一项价格：值 + 来源。未知就是 `None` + `Unknown`（不显示 0、不留空）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedPrice {
    pub usd_per_mtok: Option<f64>,
    pub source: PriceSource,
}

impl ResolvedPrice {
    pub fn value(&self) -> Option<f64> {
        self.usd_per_mtok
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedPrices {
    pub input: ResolvedPrice,
    pub output: ResolvedPrice,
    pub cache_read: ResolvedPrice,
    pub cache_write: ResolvedPrice,
}

/// 价格优先级：**手工 > 快照 > 未知**，逐项判（票 33 第 1 节）。
pub fn resolve_prices(manual: &Prices, snapshot: Option<&Prices>) -> ResolvedPrices {
    let pick = |m: Option<f64>, s: Option<f64>| match (m, s) {
        (Some(v), _) => ResolvedPrice {
            usd_per_mtok: Some(v),
            source: PriceSource::Manual,
        },
        (None, Some(v)) => ResolvedPrice {
            usd_per_mtok: Some(v),
            source: PriceSource::Snapshot,
        },
        (None, None) => ResolvedPrice {
            usd_per_mtok: None,
            source: PriceSource::Unknown,
        },
    };
    ResolvedPrices {
        input: pick(manual.input, snapshot.and_then(|s| s.input)),
        output: pick(manual.output, snapshot.and_then(|s| s.output)),
        cache_read: pick(manual.cache_read, snapshot.and_then(|s| s.cache_read)),
        cache_write: pick(manual.cache_write, snapshot.and_then(|s| s.cache_write)),
    }
}

// ───────────────────────── 文档表 / 快照解析 ─────────────────────────

/// 随包文档表给的逐模型 wire（查不到 = 空集合，即「不猜」）。
pub fn doc_wires(model_id: &str) -> BTreeSet<Wire> {
    let mut out = BTreeSet::new();
    let Ok(table) = serde_json::from_str::<Value>(DOC_TABLE) else {
        return out;
    };
    let Some(map) = table.as_object() else {
        return out;
    };
    for (wire_name, ids) in map {
        let Some(wire) = Wire::parse(wire_name) else {
            continue;
        };
        let hit = ids
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|id| match id.strip_suffix('*') {
                Some(prefix) => model_id.starts_with(prefix),
                None => model_id == id,
            });
        if hit {
            out.insert(wire);
        }
    }
    out
}

/// 从 models.dev 形状的 `cost` 对象读四项价（缺项 = `None`）。
fn prices_from_cost(cost: Option<&Value>) -> Prices {
    let get = |k: &str| cost.and_then(|c| c.get(k)).and_then(Value::as_f64);
    Prices {
        input: get("input"),
        output: get("output"),
        cache_read: get("cache_read"),
        cache_write: get("cache_write"),
    }
}

// ───────────────────────── 上游读写 ─────────────────────────

/// 写声明式字段（`auth` / 路径模板 / 透传头 / quirks）；`last_pull` 与 `last_selfcheck`
/// 是运行期事实，各有自己的写入口，不在这里被覆盖。
pub async fn save_upstream(store: &Store, up: &Upstream) -> Result<()> {
    let up = up.clone();
    store
        .write(move |c| {
            let auth = Value::Object(
                up.auth
                    .iter()
                    .map(|(k, v)| (k.clone(), json!(v.as_str())))
                    .collect(),
            );
            let paths = Value::Object(
                up.path_templates
                    .iter()
                    .map(|(w, p)| (w.as_str().to_string(), json!(p)))
                    .collect(),
            );
            let header = json!(up.header_passthrough);
            let quirks = Value::Array(
                up.quirks
                    .iter()
                    .map(|q| json!({ "name": q.name(), "arg": q.payload() }))
                    .collect(),
            );
            c.execute(
                "INSERT INTO upstreams(id,name,base_url,auth_json,path_templates_json,
                                       header_passthrough_json,quirks_json)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET
                     name=excluded.name, base_url=excluded.base_url, auth_json=excluded.auth_json,
                     path_templates_json=excluded.path_templates_json,
                     header_passthrough_json=excluded.header_passthrough_json,
                     quirks_json=excluded.quirks_json",
                params![
                    up.id,
                    up.name,
                    up.base_url,
                    auth.to_string(),
                    paths.to_string(),
                    header.to_string(),
                    quirks.to_string()
                ],
            )?;
            Ok(())
        })
        .await
}

fn upstream_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Upstream> {
    let id: String = r.get(0)?;
    let name: String = r.get(1)?;
    let base_url: String = r.get(2)?;
    let auth_json: String = r.get(3)?;
    let paths_json: String = r.get(4)?;
    let header_json: String = r.get(5)?;
    let quirks_json: String = r.get(6)?;
    let pull_at: Option<i64> = r.get(7)?;
    let pull_status: Option<String> = r.get(8)?;
    let pull_warning: Option<String> = r.get(9)?;
    let sc_at: Option<i64> = r.get(10)?;
    let sc_status: Option<String> = r.get(11)?;
    let pull_ok_at: Option<i64> = r.get(12)?;

    let mut auth = BTreeMap::new();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&auth_json) {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                auth.insert(k, AuthScheme::parse(s));
            }
        }
    }
    if auth.is_empty() {
        auth.insert("default".to_string(), AuthScheme::Bearer);
    }
    let mut path_templates = BTreeMap::new();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&paths_json) {
        for (k, v) in map {
            if let (Some(w), Some(p)) = (Wire::parse(&k), v.as_str()) {
                path_templates.insert(w, p.to_string());
            }
        }
    }
    let header_passthrough = serde_json::from_str::<Vec<String>>(&header_json).unwrap_or_default();
    let quirks = serde_json::from_str::<Value>(&quirks_json)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    let name = q.get("name")?.as_str()?;
                    let arg = q.get("arg").and_then(Value::as_str).unwrap_or("");
                    Quirk::parse(name, arg)
                })
                .collect()
        })
        .unwrap_or_default();

    let last_pull = pull_at.and_then(|at| {
        Some(PullState {
            at,
            ok_at: pull_ok_at,
            status: PullStatus::parse(pull_status.as_deref()?)?,
            warning: pull_warning.clone(),
        })
    });
    let last_selfcheck = sc_at.map(|at| SelfCheck {
        at,
        ok: sc_status.as_deref() == Some("ok"),
    });

    Ok(Upstream {
        id,
        name,
        base_url,
        auth,
        path_templates,
        header_passthrough,
        quirks,
        last_pull,
        last_selfcheck,
    })
}

const UPSTREAM_COLS: &str = "id,name,base_url,auth_json,path_templates_json,header_passthrough_json,
     quirks_json,last_pull_at,last_pull_status,last_pull_warning,last_selfcheck_at,last_selfcheck_status,
     last_pull_ok_at";

pub async fn load_upstream(store: &Store, id: &str) -> Result<Option<Upstream>> {
    let id = id.to_string();
    store
        .read(move |c| {
            let sql = format!("SELECT {UPSTREAM_COLS} FROM upstreams WHERE id=?1");
            let mut st = c.prepare(&sql)?;
            let mut rows = st.query(params![id])?;
            match rows.next()? {
                Some(r) => Ok(Some(upstream_from_row(r)?)),
                None => Ok(None),
            }
        })
        .await
}

pub async fn list_upstreams(store: &Store) -> Result<Vec<Upstream>> {
    store
        .read(move |c| {
            let sql = format!("SELECT {UPSTREAM_COLS} FROM upstreams ORDER BY rowid");
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map([], upstream_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

/// 「上次拉取时间 / 结果 / 警告」的写入点。`warning` 为 `None` 即清空（下次成功拉取自动清）。
pub async fn set_pull_result(store: &Store, id: &str, pull: &PullState) -> Result<()> {
    let (id, at, ok_at, status, warning) = (
        id.to_string(),
        pull.at,
        pull.ok_at,
        pull.status.as_str().to_string(),
        pull.warning.clone(),
    );
    store
        .write(move |c| {
            c.execute(
                "UPDATE upstreams SET last_pull_at=?2,last_pull_ok_at=?3,last_pull_status=?4,
                                      last_pull_warning=?5
                 WHERE id=?1",
                params![id, at, ok_at, status, warning],
            )?;
            Ok(())
        })
        .await
}

/// 库里现存的「上次成功拉取时间」与「上次拉取警告」。
///
/// 失败与「不提供模型发现」两支要**保留**这两个事实（票 35 第 1 / 5 条：只有成功拉取才移动成功
/// 时间、才清角标），所以它们从库里现读，而不是信调用方手里那份 `Upstream`（可能是拉取前的旧快照）。
pub async fn carried_pull_facts(store: &Store, id: &str) -> Result<(Option<i64>, Option<String>)> {
    let id = id.to_string();
    store
        .read(move |c| {
            let row = c
                .query_row(
                    "SELECT last_pull_ok_at,last_pull_warning FROM upstreams WHERE id=?1",
                    params![id],
                    |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            Ok(row.unwrap_or((None, None)))
        })
        .await
}

/// 第三层校验的结果落库（`ok` / `fail`）；侧栏「未通过自检的上游数」按它统计。
pub async fn set_selfcheck_result(store: &Store, id: &str, ok: bool, at: i64) -> Result<()> {
    let (id, status) = (id.to_string(), if ok { "ok" } else { "fail" }.to_string());
    store
        .write(move |c| {
            c.execute(
                "UPDATE upstreams SET last_selfcheck_at=?2,last_selfcheck_status=?3 WHERE id=?1",
                params![id, at, status],
            )?;
            Ok(())
        })
        .await
}

/// 绑定层的一条引用（哪个客户端档案的哪个虚拟模型绑了这个上游）——删上游前要列出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRef {
    pub client_profile_id: String,
    pub name: String,
}

impl BindingRef {
    /// 人话（界面直接显示）。
    pub fn label(&self) -> String {
        format!("{} / {}", self.client_profile_id, self.name)
    }
}

pub async fn upstream_references(store: &Store, id: &str) -> Result<Vec<BindingRef>> {
    let id = id.to_string();
    store
        .read(move |c| {
            let mut st = c.prepare(
                "SELECT client_profile_id,name FROM virtual_models WHERE upstream_id=?1
                 ORDER BY client_profile_id,name",
            )?;
            let rows = st.query_map(params![id], |r| {
                Ok(BindingRef {
                    client_profile_id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    /// 被引用：列出引用处，不删（`ON DELETE RESTRICT` 是库里那道防线，这里是给人看的那道）。
    Referenced(Vec<BindingRef>),
}

/// 删上游：被绑定引用就拒绝并回引用处；没人引用才真删（目录与凭据随 `ON DELETE CASCADE` 走）。
pub async fn delete_upstream(store: &Store, id: &str) -> Result<DeleteOutcome> {
    let refs = upstream_references(store, id).await?;
    if !refs.is_empty() {
        return Ok(DeleteOutcome::Referenced(refs));
    }
    let id = id.to_string();
    store
        .write(move |c| {
            c.execute("DELETE FROM upstreams WHERE id=?1", params![id])?;
            Ok(())
        })
        .await?;
    Ok(DeleteOutcome::Deleted)
}

/// 删上游 + **一并删掉引用它的绑定**（界面二次确认后走这条，票 17 第 39 条 / 票 49 第 20 条）。
pub async fn delete_upstream_forced(store: &Store, id: &str) -> Result<usize> {
    let refs = upstream_references(store, id).await?;
    let id = id.to_string();
    let removed = refs.len();
    store
        .write(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM virtual_models WHERE upstream_id=?1", params![id])?;
            tx.execute("DELETE FROM upstreams WHERE id=?1", params![id])?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(removed)
}

// ───────────────────────── 声明式供应商表（向导第①步的预填） ─────────────────────────

/// 随包内置的供应商预填表（`providers.json`）：models.dev 给不出 baseUrl 的那几家，
/// 由这张表补（research/providers.md 逐条有据）。
pub const BUNDLED_PROVIDERS: &str = include_str!("providers.json");

/// 一条供应商预填：向导选中它就预填 baseUrl 与协议倾向，仍可手改。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// 协议倾向（选它当默认 wire）。
    pub wire: String,
    /// 逐 wire 的路径模板（缺的用 [`Upstream::new`] 的默认值）。
    pub paths: BTreeMap<String, String>,
    /// 逐 wire 的认证方式（缺的继承 `default`）。
    pub auth: BTreeMap<String, String>,
    /// quirk 名（有载荷的写成 `名:载荷`）。
    pub quirks: Vec<String>,
    pub note: String,
    pub doc_url: String,
}

/// 解析随包预填表（坏数据就回空表：向导退化成手填，不阻断）。
pub fn provider_presets() -> Vec<ProviderPreset> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        id: String,
        name: String,
        base_url: String,
        wire: String,
        #[serde(default)]
        paths: BTreeMap<String, String>,
        #[serde(default)]
        auth: BTreeMap<String, String>,
        #[serde(default)]
        quirks: Vec<Vec<String>>,
        #[serde(default)]
        note: String,
        #[serde(default)]
        doc_url: String,
    }
    serde_json::from_str::<Vec<Raw>>(BUNDLED_PROVIDERS)
        .unwrap_or_default()
        .into_iter()
        .map(|r| ProviderPreset {
            id: r.id,
            name: r.name,
            base_url: r.base_url,
            wire: r.wire,
            paths: r.paths,
            auth: r.auth,
            quirks: r
                .quirks
                .iter()
                .map(|q| {
                    let name = q.first().cloned().unwrap_or_default();
                    match q.get(1) {
                        Some(arg) if !arg.is_empty() => format!("{name}:{arg}"),
                        _ => name,
                    }
                })
                .collect(),
            note: r.note,
            doc_url: r.doc_url,
        })
        .collect()
}

/// 把一条预填变成一张还没保存的上游表（向导第②步的起点）。
pub fn preset_to_upstream(preset: &ProviderPreset) -> Upstream {
    let mut up = Upstream::new(preset.id.clone(), preset.name.clone(), preset.base_url.clone());
    up.path_templates.clear();
    for (wire, path) in &preset.paths {
        if let Some(w) = Wire::parse(wire) {
            up.set_path(w, path.clone());
        }
    }
    if up.path_templates.is_empty() {
        // 预填表只给了一个 wire 面：另一面用 spec §6 的默认模板，用户可手改。
        up.path_templates = Upstream::new("", "", "").path_templates;
    }
    up.auth.clear();
    up.set_auth("default", AuthScheme::Bearer);
    for (wire, scheme) in &preset.auth {
        up.set_auth(wire, AuthScheme::parse(scheme));
    }
    up.quirks = preset
        .quirks
        .iter()
        .filter_map(|q| {
            let (name, arg) = q.split_once(':').unwrap_or((q.as_str(), ""));
            Quirk::parse(name, arg)
        })
        .collect();
    up
}

// ───────────────────────── 目录读写 ─────────────────────────

fn model_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UpstreamModel> {
    let wires_json: String = r.get(1)?;
    let source: String = r.get(2)?;
    let supported_wires = serde_json::from_str::<Vec<String>>(&wires_json)
        .unwrap_or_default()
        .iter()
        .filter_map(|w| Wire::parse(w))
        .collect();
    Ok(UpstreamModel {
        upstream_id: r.get(0)?,
        model_id: r.get(3)?,
        supported_wires,
        source: Source::parse(&source).unwrap_or(Source::Upstream),
    })
}

pub async fn list_models(store: &Store, upstream_id: &str) -> Result<Vec<UpstreamModel>> {
    let upstream_id = upstream_id.to_string();
    store
        .read(move |c| {
            let mut st = c.prepare(
                "SELECT upstream_id,supported_wires,source,model_id FROM upstream_models
                 WHERE upstream_id=?1 ORDER BY model_id",
            )?;
            let rows = st.query_map(params![upstream_id], model_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

/// 取某条上游在某个 wire 面上的出站凭据：先找这一面那份，没有就继承 `default`（ADR-0004）。
///
/// 绑定层与自检探针共用这一个入口——「认证方式 + key 由调用方传」的那条规矩（票 49）
/// 只在出站组装点执法，取凭据的优先级只有一份。
pub async fn wire_credential(
    store: &Store,
    upstream_id: &str,
    wire: Wire,
) -> Result<Option<String>> {
    if let Some(secret) = store.credential_secret(upstream_id, wire.as_str()).await? {
        return Ok(Some(secret));
    }
    store.credential_secret(upstream_id, "default").await
}

/// 整张上游模型目录：`(上游 id, 真实模型 id)` → 目录条目。
///
/// 请求期早拒（票 49 的三层校验第二层）与外壳装配用；键与 `Gateway.catalog` 同一套。
pub async fn catalog_map(
    store: &Store,
) -> Result<std::collections::HashMap<(String, String), UpstreamModel>> {
    let models = store
        .read(|c| {
            let mut st = c.prepare(
                "SELECT upstream_id,supported_wires,source,model_id FROM upstream_models",
            )?;
            let rows = st.query_map([], model_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await?;
    Ok(models
        .into_iter()
        .map(|m| ((m.upstream_id.clone(), m.model_id.clone()), m))
        .collect())
}

/// 手工新增 / 手工改一条目录条目：`source = manual` 锁定，重拉不覆盖（票 23 第 5 节）。
pub async fn save_manual_model(
    store: &Store,
    upstream_id: &str,
    model_id: &str,
    wires: &[Wire],
) -> Result<()> {
    let (upstream_id, model_id, wires) = (
        upstream_id.to_string(),
        model_id.to_string(),
        serde_json::to_string(&wires.iter().map(Wire::as_str).collect::<Vec<_>>())?,
    );
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source)
                 VALUES(?1,?2,?3,'manual')
                 ON CONFLICT(upstream_id,model_id) DO UPDATE SET
                     supported_wires=excluded.supported_wires, source='manual'",
                params![upstream_id, model_id, wires],
            )?;
            Ok(())
        })
        .await
}

/// 写手工四项价（`None` = 该项留空 → 回退快照）。填了的项就此锁定，快照刷新不动它。
pub async fn set_manual_prices(
    store: &Store,
    upstream_id: &str,
    model_id: &str,
    prices: &Prices,
) -> Result<()> {
    let (upstream_id, model_id, p) = (upstream_id.to_string(), model_id.to_string(), *prices);
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source,
                                             price_input_usd,price_output_usd,
                                             price_cache_read_usd,price_cache_write_usd)
                 VALUES(?1,?2,'[]','manual',?3,?4,?5,?6)
                 ON CONFLICT(upstream_id,model_id) DO UPDATE SET
                     price_input_usd=excluded.price_input_usd,
                     price_output_usd=excluded.price_output_usd,
                     price_cache_read_usd=excluded.price_cache_read_usd,
                     price_cache_write_usd=excluded.price_cache_write_usd",
                params![
                    upstream_id,
                    model_id,
                    p.input,
                    p.output,
                    p.cache_read,
                    p.cache_write
                ],
            )?;
            Ok(())
        })
        .await
}

/// 快照里这个模型的价格：先按 `(上游 id, 模型 id)` 精确找，再退回「任意 provider 的同名模型」。
///
/// 中转上游（commandcode / opencode）在 models.dev 里没有 provider 条目，而价格是按真实模型
/// 算的，所以第二跳才是常态；models.dev 不参与 wire 判定（票 23 第 5 节）。
fn snapshot_prices(c: &Connection, upstream_id: &str, model_id: &str) -> Result<Option<Prices>> {
    let sql = "SELECT cost_input_usd,cost_output_usd,cost_cache_read_usd,cost_cache_write_usd
                 FROM model_catalog WHERE provider=?1 AND model_id=?2";
    let read = |row: &rusqlite::Row<'_>| -> Result<Prices> {
        Ok(Prices {
            input: row.get(0)?,
            output: row.get(1)?,
            cache_read: row.get(2)?,
            cache_write: row.get(3)?,
        })
    };
    let mut st = c.prepare(sql)?;
    let mut rows = st.query(params![upstream_id, model_id])?;
    if let Some(r) = rows.next()? {
        return Ok(Some(read(r)?));
    }
    drop(rows);
    let mut st = c.prepare(
        "SELECT cost_input_usd,cost_output_usd,cost_cache_read_usd,cost_cache_write_usd
           FROM model_catalog WHERE model_id=?1 ORDER BY provider LIMIT 1",
    )?;
    let mut rows = st.query(params![model_id])?;
    match rows.next()? {
        Some(r) => Ok(Some(read(r)?)),
        None => Ok(None),
    }
}

/// 快照里某个模型的价格（设置页 / 用量页用）。
pub async fn catalog_prices(
    store: &Store,
    upstream_id: &str,
    model_id: &str,
) -> Result<Option<Prices>> {
    let (upstream_id, model_id) = (upstream_id.to_string(), model_id.to_string());
    store
        .read(move |c| snapshot_prices(c, &upstream_id, &model_id))
        .await
}

/// 某个目录条目**当下**生效的四项价与各自来源（手工 > 快照 > 未知）。
pub async fn resolved_prices(
    store: &Store,
    upstream_id: &str,
    model_id: &str,
) -> Result<ResolvedPrices> {
    let (upstream_id, model_id) = (upstream_id.to_string(), model_id.to_string());
    store
        .read(move |c| {
            let mut st = c.prepare(
                "SELECT price_input_usd,price_output_usd,price_cache_read_usd,price_cache_write_usd
                   FROM upstream_models WHERE upstream_id=?1 AND model_id=?2",
            )?;
            let mut rows = st.query(params![upstream_id, model_id])?;
            let manual = match rows.next()? {
                Some(r) => Prices {
                    input: r.get(0)?,
                    output: r.get(1)?,
                    cache_read: r.get(2)?,
                    cache_write: r.get(3)?,
                },
                None => Prices::default(),
            };
            drop(rows);
            let snapshot = snapshot_prices(c, &upstream_id, &model_id)?;
            Ok(resolve_prices(&manual, snapshot.as_ref()))
        })
        .await
}

// ───────────────────────── models.dev 快照 ─────────────────────────

/// 把一份 models.dev 形状的 JSON **整体**导进 `model_catalog`（快照语义：先清后写）。
/// 回导入的模型条数。解析不出任何 model 时不动旧表。
pub async fn import_catalog_snapshot(store: &Store, json_text: &str) -> Result<usize> {
    let parsed = serde_json::from_str::<Value>(json_text).context("快照不是合法 JSON")?;
    let providers = parsed
        .as_object()
        .ok_or_else(|| anyhow!("快照顶层应是 provider 对象"))?;

    let mut rows: Vec<(String, String, Option<String>, Prices, String)> = Vec::new();
    for (pid, pobj) in providers {
        let Some(models) = pobj.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (mid, mobj) in models {
            rows.push((
                pid.clone(),
                mid.clone(),
                mobj.get("name").and_then(Value::as_str).map(str::to_string),
                prices_from_cost(mobj.get("cost")),
                mobj.to_string(),
            ));
        }
    }
    let n = rows.len();
    store
        .write(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM model_catalog", [])?;
            {
                let mut ins = tx.prepare(
                    "INSERT INTO model_catalog(provider,model_id,name,cost_input_usd,cost_output_usd,
                                               cost_cache_read_usd,cost_cache_write_usd,raw_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                )?;
                for (provider, model_id, name, p, raw) in &rows {
                    ins.execute(params![
                        provider,
                        model_id,
                        name,
                        p.input,
                        p.output,
                        p.cache_read,
                        p.cache_write,
                        raw
                    ])?;
                }
            }
            tx.commit()?;
            Ok(n)
        })
        .await
}

/// 用随包快照灌一次（首启 / 库被清后）。**不在 `Store::open` 里自动跑**：那是外壳的启动序列。
pub async fn import_bundled_snapshot(store: &Store) -> Result<usize> {
    import_catalog_snapshot(store, BUNDLED_MODELS_DEV_SNAPSHOT).await
}

/// 快照刷新结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogRefresh {
    pub imported: usize,
    pub at: i64,
}

/// 设置页的手动刷新：联网拉一次全量 `api.json`，成功才整体换掉快照并写刷新时间。
/// **失败静默沿用旧快照**（本函数不动 `model_catalog`，由调用方决定是否提示），不自动重试。
pub async fn refresh_catalog(
    store: &Store,
    http: &reqwest::Client,
    url: &str,
) -> Result<CatalogRefresh> {
    let resp = http
        .get(url)
        .timeout(CATALOG_REFRESH_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("拉价格快照失败：{url}"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("价格快照返回 {status}（沿用旧快照）");
    }
    let text = resp.text().await.context("读价格快照响应失败")?;
    let imported = import_catalog_snapshot(store, &text).await?;
    let at = now_unix();
    let value = at.to_string();
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![settings_keys::CATALOG_SNAPSHOT_AT, value],
            )?;
            Ok(())
        })
        .await?;
    Ok(CatalogRefresh { imported, at })
}

/// 上次成功刷新快照的时间（Unix 秒）；从没刷过 = `None`。
pub async fn catalog_snapshot_at(store: &Store) -> Result<Option<i64>> {
    store
        .read(|c| {
            let v: Option<String> = c
                .query_row(
                    "SELECT value FROM settings WHERE key=?1",
                    params![settings_keys::CATALOG_SNAPSHOT_AT],
                    |r| r.get(0),
                )
                .ok();
            Ok(v.and_then(|s| s.parse::<i64>().ok()))
        })
        .await
}

// ───────────────────────── 拉模型 ─────────────────────────

/// 上游 `/models` 报的一条模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteModel {
    pub id: String,
    /// 上游声明的端点路径（commandcode 有，opencode 没有）。
    pub endpoints: Vec<String>,
}

/// 拉取的三种结局（票 35 第 2 条）。
#[derive(Debug, Clone, PartialEq)]
pub enum PullOutcome {
    Fetched(Vec<RemoteModel>),
    /// 404 / 405 / 200 但零条：该上游不提供模型发现（软提示 + 手填入口，不报红）。
    NoDiscovery,
    /// 其他非 2xx / 超时 / 断网：失败（保留旧条目，提示里给「重试」）。
    Failed(String),
}

/// 拉一次上游模型清单。**不自动重试**，20 秒超时；`credential` 由
/// [`pull_credential`] 按「`default` 优先，唯一一份即用，多份交给界面」给出。
pub async fn pull_models(
    http: &reqwest::Client,
    up: &Upstream,
    models_path: &str,
    credential: Option<&str>,
) -> PullOutcome {
    let url = format!("{}{}", up.base_url.trim_end_matches('/'), models_path);
    let mut rb = http.get(&url).timeout(PULL_TIMEOUT);
    if let Some(key) = credential {
        rb = match up.auth_default() {
            AuthScheme::Bearer => rb.header("authorization", format!("Bearer {key}")),
            AuthScheme::XApiKey => rb.header("x-api-key", key),
        };
    }
    // quirk 的唯一执法点：这里只认 quirk，不认供应商名。
    for q in &up.quirks {
        if let Quirk::ClientUserAgent(ua) = q {
            rb = rb.header("user-agent", ua.clone());
        }
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => return PullOutcome::Failed(format!("拉取失败：{e}")),
    };
    let status = resp.status();
    if status.as_u16() == 404 || status.as_u16() == 405 {
        return PullOutcome::NoDiscovery;
    }
    if !status.is_success() {
        return PullOutcome::Failed(format!("拉取失败：上游 {status}"));
    }
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return PullOutcome::Failed(format!("拉取失败：响应不是 JSON（{e}）")),
    };
    let list = body.get("data").and_then(Value::as_array);
    let Some(list) = list else {
        return PullOutcome::NoDiscovery;
    };
    let models: Vec<RemoteModel> = list
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?;
            if id.is_empty() {
                return None;
            }
            let endpoints = m
                .get("supported_endpoints")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            Some(RemoteModel {
                id: id.to_string(),
                endpoints,
            })
        })
        .collect();
    if models.is_empty() {
        return PullOutcome::NoDiscovery;
    }
    PullOutcome::Fetched(models)
}

/// 拉模型用哪份凭据：先 `default`；没有 `default` 时只有一份就用它；多份回 `None`（界面下拉选）。
pub async fn pull_credential(store: &Store, upstream_id: &str) -> Result<Option<String>> {
    let wire = pull_credential_wire(store, upstream_id).await?;
    match wire {
        Some(w) => store.credential_secret(upstream_id, &w).await,
        None => Ok(None),
    }
}

/// 该上游存了哪几份凭据（wire 名，排序）。空 = 一份都没有。
///
/// [`pull_credential`] 回 `None` 有两种截然不同的原因（一份都没有 / 多份且没有 `default`），
/// 界面处置不同（「去填 key」vs「选一份」），所以这两个事实要能分别问出来。
pub async fn credential_wires(store: &Store, upstream_id: &str) -> Result<Vec<String>> {
    let id = upstream_id.to_string();
    store
        .read(move |c| {
            let mut st =
                c.prepare("SELECT wire FROM credentials WHERE upstream_id=?1 ORDER BY wire")?;
            let rows = st.query_map(params![id], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

/// 拉模型该用哪一份凭据（`default` 优先 → 唯一一份 → 多份且无 `default` = 交给界面）。
async fn pull_credential_wire(store: &Store, upstream_id: &str) -> Result<Option<String>> {
    let wires = credential_wires(store, upstream_id).await?;
    if wires.iter().any(|w| w == "default") {
        Ok(Some("default".to_string()))
    } else if wires.len() == 1 {
        Ok(wires.into_iter().next())
    } else {
        Ok(None)
    }
}

/// 一次拉取对目录做了什么（给界面回执用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullApplied {
    /// 本次上游报回来的条目数（拉取失败与「不提供模型发现」都是 0）。
    pub fetched: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// 手工锁定、这次没动的条目数。
    pub kept_manual: usize,
    /// 这次拉完仍是「未声明」的条目数。
    pub undeclared: usize,
    pub warnings: Vec<String>,
}

/// 把拉取结果灌进目录，并写「上次拉取时间 / 结果 / 警告」。
///
/// 来源优先级：`manual`（锁定，重拉不覆盖）> 上游声明 > 随包文档表 > 未声明。
/// 失败与「不提供模型发现」都不动目录（票 35 第 4 / 5 条）。
pub async fn apply_pull(
    store: &Store,
    up: &Upstream,
    outcome: PullOutcome,
    at: i64,
) -> Result<PullApplied> {
    let remotes = match outcome {
        PullOutcome::Failed(msg) => {
            // 失败只记「上次尝试」：成功时间与旧角标留给成功那次去动（票 35 第 5 条）。
            let (ok_at, _) = carried_pull_facts(store, &up.id).await?;
            set_pull_result(
                store,
                &up.id,
                &PullState {
                    at,
                    ok_at,
                    status: PullStatus::Failed,
                    warning: Some(msg),
                },
            )
            .await?;
            return Ok(PullApplied::default());
        }
        PullOutcome::NoDiscovery => {
            // 「不提供模型发现」不算成功：不动目录、不动成功时间，也**不清**上一次的角标。
            let (ok_at, warning) = carried_pull_facts(store, &up.id).await?;
            set_pull_result(
                store,
                &up.id,
                &PullState {
                    at,
                    ok_at,
                    status: PullStatus::NoDiscovery,
                    warning,
                },
            )
            .await?;
            return Ok(PullApplied::default());
        }
        PullOutcome::Fetched(models) => models,
    };

    let mut warnings: Vec<String> = Vec::new();
    let mut wanted: Vec<(String, BTreeSet<Wire>, Source)> = Vec::new();
    for m in &remotes {
        let (from_upstream, mut warns) = reverse_lookup(up, &m.endpoints);
        warnings.append(&mut warns);
        let from_doc = doc_wires(&m.id);
        let (wires, source) = if !from_upstream.is_empty() {
            (from_upstream, Source::Upstream)
        } else if !from_doc.is_empty() {
            (from_doc, Source::Doc)
        } else {
            (BTreeSet::new(), Source::Upstream)
        };
        wanted.push((m.id.clone(), wires, source));
    }
    warnings.sort();
    warnings.dedup();

    let upstream_id = up.id.clone();
    let mut applied = PullApplied {
        fetched: remotes.len(),
        warnings: warnings.clone(),
        ..Default::default()
    };
    let warning_text = if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("\n"))
    };

    let (added, updated, removed, kept_manual, undeclared) = {
        let wanted = wanted.clone();
        store
            .write(move |c| {
                let tx = c.transaction()?;
                let mut existing: BTreeMap<String, Source> = BTreeMap::new();
                {
                    let mut st =
                        tx.prepare("SELECT model_id,source FROM upstream_models WHERE upstream_id=?1")?;
                    let rows = st.query_map(params![upstream_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    for row in rows {
                        let (id, src) = row?;
                        existing.insert(id, Source::parse(&src).unwrap_or(Source::Upstream));
                    }
                }
                let (mut added, mut updated, mut kept_manual, mut undeclared) = (0, 0, 0, 0);
                for (model_id, wires, source) in &wanted {
                    if wires.is_empty() {
                        undeclared += 1;
                    }
                    match existing.get(model_id) {
                        Some(Source::Manual) => {
                            kept_manual += 1;
                        }
                        Some(_) => {
                            let wires_json = wires_json(wires)?;
                            tx.execute(
                                "UPDATE upstream_models SET supported_wires=?3,source=?4
                                   WHERE upstream_id=?1 AND model_id=?2",
                                params![upstream_id, model_id, wires_json, source.as_str()],
                            )?;
                            updated += 1;
                        }
                        None => {
                            let wires_json = wires_json(wires)?;
                            tx.execute(
                                "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source)
                                 VALUES(?1,?2,?3,?4)",
                                params![upstream_id, model_id, wires_json, source.as_str()],
                            )?;
                            added += 1;
                        }
                    }
                }
                // 上游清单里没有了的非锁定条目清掉；manual 留着（不因为一次拉取消失）。
                let live: BTreeSet<&String> = wanted.iter().map(|(id, _, _)| id).collect();
                let mut removed = 0;
                for (model_id, source) in &existing {
                    if *source != Source::Manual && !live.contains(model_id) {
                        tx.execute(
                            "DELETE FROM upstream_models WHERE upstream_id=?1 AND model_id=?2",
                            params![upstream_id, model_id],
                        )?;
                        removed += 1;
                    }
                }
                tx.commit()?;
                Ok((added, updated, removed, kept_manual, undeclared))
            })
            .await?
    };

    set_pull_result(
        store,
        &up.id,
        &PullState {
            at,
            ok_at: Some(at),
            status: PullStatus::Ok,
            warning: warning_text,
        },
    )
    .await?;

    applied.added = added;
    applied.updated = updated;
    applied.removed = removed;
    applied.kept_manual = kept_manual;
    applied.undeclared = undeclared;
    Ok(applied)
}

fn wires_json(wires: &BTreeSet<Wire>) -> Result<String> {
    Ok(serde_json::to_string(
        &wires.iter().map(Wire::as_str).collect::<Vec<_>>(),
    )?)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! quirk 的两向守卫（票 19 §6，票 41 随产品侧 fixtures 搬入）：枚举值 ⇄ fixture 标签。
    //! 票 49 删掉 `ClaudeModelsOnAnthropicOnly`（ADR-0004 第 5 条），它的豁免条目一并删；
    //! spec §6 新给的四个值还没有转换代码执法点，先在豁免表里写明理由。

    use super::*;
    use std::path::{Path, PathBuf};

    fn fixtures_dir() -> PathBuf {
        // fixtures 在产品侧 `tests/fixtures/`（冻结原型那份只读）。
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// 每份 `<scenario>/<wire>.meta.json`。
    fn fixture_metas(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(scenarios) = std::fs::read_dir(dir) else {
            return out;
        };
        for scenario in scenarios.flatten().filter(|e| e.path().is_dir()) {
            let Ok(files) = std::fs::read_dir(scenario.path()) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name.ends_with(".meta.json") {
                    out.push(p);
                }
            }
        }
        out
    }

    /// 每份 meta.json 的 `quirks` 标签 = 它覆盖的 quirk 名。
    fn tagged_quirks(dir: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for p in fixture_metas(dir) {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            for n in v
                .get("quirks")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                out.push((n.to_string(), p.display().to_string()));
            }
        }
        out
    }

    /// 豁免表：只有真有理由的 quirk 才准留在这里，理由写在值里。
    fn exemptions(dir: &Path) -> HashMap<String, String> {
        let Ok(text) = std::fs::read_to_string(dir.join("quirk-exempt.json")) else {
            return HashMap::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return HashMap::new();
        };
        v.get("exempt")
            .and_then(serde_json::Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn every_quirk_declares_what_it_acts_on() {
        for q in Quirk::all() {
            assert!(!q.acts_on().is_empty(), "quirk {} 没声明作用对象", q.name());
        }
    }

    /// 双向守卫（票 19 §6.1）：枚举值 ⇄ fixture 标签，任一侧多出来都红。
    /// 反向验证（票 30 第 12 条）：抽掉一份带标签的 fixture，这条就变红。
    #[test]
    fn every_quirk_is_covered_by_a_fixture_or_explicitly_exempt() {
        let dir = fixtures_dir();
        let tags = tagged_quirks(&dir);
        let exempt = exemptions(&dir);
        assert!(
            !tags.is_empty(),
            "fixtures 里没有任何 quirk 标签，守卫等于没网"
        );
        for q in Quirk::all() {
            let n = q.name();
            assert!(
                tags.iter().any(|(t, _)| t == n) || exempt.contains_key(n),
                "quirk {n} 既没有 fixture 的 quirks 标签，也没写进 fixtures/quirk-exempt.json"
            );
        }
        for (name, why) in &exempt {
            assert!(
                Quirk::all().iter().any(|q| q.name() == name),
                "豁免表里的 {name} 不是现存枚举值"
            );
            assert!(!why.trim().is_empty(), "豁免 {name} 没写理由");
        }
    }

    /// 标签写错字也算守卫失败：否则枚举值能靠一个拼错的标签「假装」被覆盖。
    #[test]
    fn fixture_quirk_tags_are_real_enum_values() {
        for (tag, path) in tagged_quirks(&fixtures_dir()) {
            assert!(
                Quirk::all().iter().any(|q| q.name() == tag),
                "{path} 标了不存在的 quirk {tag}"
            );
        }
    }

    /// 标签指着的 fixture 必须真有字节：抽掉一份 `.sse`，守卫就得红（票 30 第 12 条的反向验证）。
    #[test]
    fn every_tagged_fixture_has_bytes() {
        let dir = fixtures_dir();
        for meta in fixture_metas(&dir) {
            let name = meta
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let sse = meta.with_file_name(name.replace(".meta.json", ".sse"));
            assert!(sse.exists(), "{} 没有对应的字节文件", sse.display());
            let Ok(text) = std::fs::read_to_string(&meta) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                panic!("{} 不是合法 JSON", meta.display());
            };
            for key in ["scenario", "outbound_wire", "model", "request"] {
                assert!(
                    !v.get(key).unwrap_or(&serde_json::Value::Null).is_null(),
                    "{} 缺字段 {key}",
                    meta.display()
                );
            }
        }
    }
}
