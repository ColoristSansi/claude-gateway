//! 绑定层：家族槽位 / 虚拟模型 → (上游, 真实模型, 出站 wire)，含客户端档案覆盖与自检探针（spec §7）。
//!
//! 票 41 在这里只放了**转发查表要用的最小形状**（一条 [`Route`]）；票 51 换成真表：
//!
//! - 四个家族槽位（[`FAMILY_SLOTS`]）与普通虚拟模型都是 `virtual_models` 里的一行，
//!   **没有特殊命中路径**；绑定只能从上游模型目录里选（[`save`] 先查目录）。
//! - **每客户端可覆盖**：同一条模型名可以按客户端档案（[`Profile`]）各存一份；
//!   解析时按入站 `user-agent` / 客户端标识归属（[`BindingTable::profile_for`]），
//!   档案里没有那份就落回 `global`。
//! - 请求期解析规则 = **去首尾空白 + 大小写敏感**（票 30 第 4 条）；未命中 → 404
//!   `not_found_error`（[`ResolveError`]）。wire 声明判据仍在票 49 的
//!   [`UpstreamModel::validate_wire`](crate::egress::provider::UpstreamModel::validate_wire) 上，
//!   本模块只在配置期先拦一道。
//! - 自检探针 [`selfcheck`]：非流式 `/v1/messages` + `max_tokens:1`，**只看状态码**，
//!   超时 504、连不上 502，结果落 `upstreams.last_selfcheck_*`（票 49 的 v3 列）。

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::egress::provider::{self, EarlyReject, Upstream, UpstreamModel, Wire, WireChoice};
use crate::egress::upstream;
use crate::store::Store;

/// 四个家族槽位，逐字（spec §7）：Claude Desktop / Claude Code 能安全填写的名字。
/// 顺序即 `/v1/models` 里的顺序（票 31：Codex 目录与 `/v1/models` 同一份清单，家族名在前）。
pub const FAMILY_SLOTS: [&str; 4] = [
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
];

/// 这个名字是不是四个家族槽位之一。
pub fn is_family(name: &str) -> bool {
    FAMILY_SLOTS.contains(&name)
}

// ───────────────────────── 客户端档案 ─────────────────────────

/// 四套客户端档案，逐字（spec §335 / 票 10）：`global` 是默认绑定，另外三套各存自己那份覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Profile {
    Global,
    ClaudeDesktop,
    ClaudeCode,
    Codex,
}

impl Profile {
    pub const ALL: [Profile; 4] = [
        Profile::Global,
        Profile::ClaudeDesktop,
        Profile::ClaudeCode,
        Profile::Codex,
    ];

    /// 库里的 id（`client_profiles.id`）。
    pub fn id(self) -> &'static str {
        match self {
            Profile::Global => "global",
            Profile::ClaudeDesktop => "claude-desktop",
            Profile::ClaudeCode => "claude-code",
            Profile::Codex => "codex",
        }
    }

    /// 界面上的名字（spec §10 第 3 节那排标签）。
    pub fn display(self) -> &'static str {
        match self {
            Profile::Global => "全局",
            Profile::ClaudeDesktop => "Claude Desktop",
            Profile::ClaudeCode => "Claude Code",
            Profile::Codex => "Codex",
        }
    }

    pub fn parse(id: &str) -> Option<Profile> {
        match id.trim() {
            "global" => Some(Profile::Global),
            "claude-desktop" => Some(Profile::ClaudeDesktop),
            "claude-code" => Some(Profile::ClaudeCode),
            "codex" => Some(Profile::Codex),
            _ => None,
        }
    }
}

/// `client_profiles` 一行里参与归属的那部分：档案 + 它的 UA 判据。
///
/// `match_ua` 是一串 `;` 分隔的**子串**（大小写不敏感），identity 命中任意一个就归这个档案。
/// 三条实测 UA 的稳定片段见 [`DEFAULT_MATCH_UA`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub profile: Profile,
    pub match_ua: String,
}

/// 三条实测 UA 的判据（v4 迁移把它们写进 `client_profiles.match_ua`）：
///
/// - `Claude/`：Claude Desktop 3P 的 Electron UA 里那一段（票 15 端到端实测）。
/// - `claude-cli/`：Claude Code 的 UA 前缀（本机 2.1.299 实测：`claude-cli/2.1.299 (external, sdk-cli)`）。
/// - `codex_exec/;x-codex-`：Codex CLI 的 UA 前缀（本机 0.162.1 实测）与 **Codex 桌面应用**必带的
///   `x-codex-*` 标识头（票 32 的真录入站体里就有 `x-codex-installation-id`）。
pub const DEFAULT_MATCH_UA: [(&str, &str); 3] = [
    ("claude-desktop", "Claude/"),
    ("claude-code", "claude-cli/"),
    ("codex", "codex_exec/;x-codex-"),
];

/// 库里那份档案判据的默认值（迁移把它种进 `client_profiles`；内存表 / 测试也用它）。
pub fn default_profiles() -> Vec<ProfileRow> {
    DEFAULT_MATCH_UA
        .iter()
        .filter_map(|(id, ua)| {
            Some(ProfileRow {
                profile: Profile::parse(id)?,
                match_ua: (*ua).to_string(),
            })
        })
        .collect()
}

// ───────────────────────── 一条绑定 ─────────────────────────

/// `virtual_models` 一行：客户端填的虚拟模型名 → (上游, 真实模型, 出站 wire)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub profile: Profile,
    pub name: String,
    pub upstream_id: String,
    pub real_model: String,
    pub wire: Wire,
}

impl Binding {
    /// 名字存进去之前先去掉首尾空白（请求期同一条规则）。
    pub fn new(
        profile: Profile,
        name: impl Into<String>,
        upstream_id: impl Into<String>,
        real_model: impl Into<String>,
        wire: Wire,
    ) -> Binding {
        Binding {
            profile,
            name: name.into().trim().to_string(),
            upstream_id: upstream_id.into(),
            real_model: real_model.into(),
            wire,
        }
    }

    /// 是不是四个家族槽位之一（界面按它分组，库里的 `is_family` 也按它写）。
    pub fn is_family(&self) -> bool {
        is_family(&self.name)
    }

    /// 三元组的人话（日志 / 界面用）。
    pub fn triple(&self) -> String {
        format!(
            "{} / {} / {}",
            self.upstream_id,
            self.real_model,
            self.wire.as_str()
        )
    }
}

/// 未命中：404 `not_found_error`（票 30 第 4 条）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveError {
    pub name: String,
}

impl ResolveError {
    pub fn status(&self) -> u16 {
        404
    }

    pub fn error_type(&self) -> &'static str {
        "not_found_error"
    }

    pub fn message(&self) -> String {
        format!("未知虚拟模型 {:?}", self.name)
    }
}

// ───────────────────────── 绑定表 ─────────────────────────

/// 请求期查的那张表：全部档案的绑定行 + 档案判据。
#[derive(Debug, Clone, Default)]
pub struct BindingTable {
    rows: Vec<Binding>,
    profiles: Vec<ProfileRow>,
}

impl BindingTable {
    pub fn new(rows: Vec<Binding>, profiles: Vec<ProfileRow>) -> BindingTable {
        BindingTable { rows, profiles }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[Binding] {
        &self.rows
    }

    /// 入站身份归属：`identity` 里命中哪个档案的判据就归谁，认不出就是 `global`。
    ///
    /// `identity` 由调用方把 `user-agent` 与客户端标识头拼成的一串（见 `ingress::proxy`）；
    /// 匹配是**大小写不敏感的子串**——UA 里的版本号天天变，判据只能是稳定片段。
    pub fn profile_for(&self, identity: &str) -> Profile {
        let identity = identity.to_lowercase();
        for row in &self.profiles {
            for token in row.match_ua.split(';') {
                let token = token.trim();
                if !token.is_empty() && identity.contains(&token.to_lowercase()) {
                    return row.profile;
                }
            }
        }
        Profile::Global
    }

    /// 请求期解析：去首尾空白、大小写敏感；档案没有那份就落回 `global`。
    pub fn resolve(&self, raw_name: &str, identity: &str) -> Result<Binding, ResolveError> {
        let name = raw_name.trim();
        let profile = self.profile_for(identity);
        let hit = self
            .rows
            .iter()
            .find(|r| r.profile == profile && r.name == name)
            .or_else(|| {
                (profile != Profile::Global)
                    .then(|| {
                        self.rows
                            .iter()
                            .find(|r| r.profile == Profile::Global && r.name == name)
                    })
                    .flatten()
            });
        hit.cloned().ok_or_else(|| ResolveError {
            name: name.to_string(),
        })
    }

    /// 网关暴露的模型清单（`GET /v1/models` 与 Codex 模型目录的同一份来源，票 31）：
    /// 四个家族槽位恒在且在最前，其余虚拟模型按名排序；跨档案同名只出一条。
    ///
    /// **不按客户端过滤**（spec §8.2 第 2 条：过滤在配置层完成，发现层用同一判据）；
    /// 只是信封形状按客户端家族给（`ingress::proxy::list_models`）。
    pub fn exposed(&self) -> Vec<ExposedModel> {
        let mut out: Vec<ExposedModel> = FAMILY_SLOTS
            .iter()
            .map(|name| ExposedModel {
                name: (*name).to_string(),
                is_family: true,
            })
            .collect();
        let mut normal: BTreeSet<&str> = BTreeSet::new();
        for row in &self.rows {
            if !row.is_family() {
                normal.insert(row.name.as_str());
            }
        }
        out.extend(normal.into_iter().map(|name| ExposedModel {
            name: name.to_string(),
            is_family: false,
        }));
        out
    }
}

/// `/v1/models` 与 Codex 目录里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposedModel {
    pub name: String,
    pub is_family: bool,
}

// ───────────────────────── 落库 ─────────────────────────

/// 配置期拦截：绑定只能从上游模型目录里选（spec §7），wire 只能选声明过的。
#[derive(Debug)]
pub enum SaveError {
    /// 模型名为空（去空白之后）。
    EmptyName,
    /// 目录里没有这条 `(上游, 真实模型)`：先在目录里手工新增一条（`manual` 锁定）再绑。
    NoCatalogEntry {
        upstream_id: String,
        real_model: String,
    },
    /// 目录声明过这个模型的 wire，选的不在集合里（第二层校验的配置期先手）。
    WireNotDeclared(EarlyReject),
    /// 库读写失败。
    Store(anyhow::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::EmptyName => write!(f, "虚拟模型名不能为空"),
            SaveError::NoCatalogEntry {
                upstream_id,
                real_model,
            } => write!(
                f,
                "上游模型目录里没有 {upstream_id} / {real_model}：绑定只能从目录里选，先在目录里手工新增一条"
            ),
            SaveError::WireNotDeclared(rej) => write!(f, "{}", rej.message),
            SaveError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// 存一条绑定（按「档案 × 模型名」覆盖式）。
///
/// 三层校验的第一层在这里落地：目录里必须有这条模型；声明过 wire 时，选的 wire 必须在集合里
/// （未声明的空集合照票 23 放行，回 [`WireChoice::Undeclared`]，界面标注「未声明，靠自检」）。
pub async fn save(store: &Store, binding: &Binding) -> Result<WireChoice, SaveError> {
    let name = binding.name.trim();
    if name.is_empty() {
        return Err(SaveError::EmptyName);
    }
    let models = provider::list_models(store, &binding.upstream_id)
        .await
        .map_err(SaveError::Store)?;
    let entry: UpstreamModel = models
        .into_iter()
        .find(|m| m.model_id == binding.real_model)
        .ok_or_else(|| SaveError::NoCatalogEntry {
            upstream_id: binding.upstream_id.clone(),
            real_model: binding.real_model.clone(),
        })?;
    let choice = entry
        .validate_wire(binding.wire)
        .map_err(SaveError::WireNotDeclared)?;

    let (profile, name, upstream_id, real_model, wire) = (
        binding.profile.id().to_string(),
        name.to_string(),
        binding.upstream_id.clone(),
        binding.real_model.clone(),
        binding.wire.as_str().to_string(),
    );
    let family = is_family(&name) as i64;
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,real_model,wire)
                 VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(client_profile_id,name) DO UPDATE SET
                     is_family=excluded.is_family, upstream_id=excluded.upstream_id,
                     real_model=excluded.real_model, wire=excluded.wire",
                rusqlite::params![profile, name, family, upstream_id, real_model, wire],
            )?;
            Ok(())
        })
        .await
        .map_err(SaveError::Store)?;
    Ok(choice)
}

/// 读整张绑定表（请求期查表 + 界面展示）。
///
/// 没绑全的行（上游 / 真实模型 / wire 有任一为空）跳过——它们在界面上是「未绑定」，
/// 在请求期是 404，都不该变成一条能路由的绑定。
pub async fn load(store: &Store) -> Result<BindingTable> {
    let (profiles, rows) = store
        .read(|c| {
            let mut profiles = Vec::new();
            {
                let mut st = c.prepare("SELECT id,match_ua FROM client_profiles ORDER BY id")?;
                let mut rs = st.query([])?;
                while let Some(r) = rs.next()? {
                    let id: String = r.get(0)?;
                    let match_ua: String = r.get(1)?;
                    if let Some(profile) = Profile::parse(&id) {
                        profiles.push(ProfileRow { profile, match_ua });
                    }
                }
            }
            let mut rows = Vec::new();
            {
                let mut st = c.prepare(
                    "SELECT client_profile_id,name,upstream_id,real_model,wire FROM virtual_models
                     ORDER BY client_profile_id,name",
                )?;
                let mut rs = st.query([])?;
                while let Some(r) = rs.next()? {
                    let profile: String = r.get(0)?;
                    let name: String = r.get(1)?;
                    let upstream_id: Option<String> = r.get(2)?;
                    let real_model: Option<String> = r.get(3)?;
                    let wire: Option<String> = r.get(4)?;
                    let (Some(profile), Some(upstream_id), Some(real_model), Some(wire)) = (
                        Profile::parse(&profile),
                        upstream_id,
                        real_model,
                        wire.and_then(|w| Wire::parse(&w)),
                    ) else {
                        continue;
                    };
                    rows.push(Binding {
                        profile,
                        name,
                        upstream_id,
                        real_model,
                        wire,
                    });
                }
            }
            Ok((profiles, rows))
        })
        .await?;
    Ok(BindingTable::new(rows, profiles))
}

// ───────────────────────── 自检探针 ─────────────────────────

/// 探针档的两个常量只有一处定义（[`crate::egress::upstream`]）：转发面的 `with_deadline`
/// 与这里的自检探针共用同一个值（`max_tokens <= PROBE_MAX_TOKENS` 才吃 `PROBE_DEADLINE`）。
pub use crate::egress::upstream::{PROBE_DEADLINE, PROBE_MAX_TOKENS};

/// 探针的 user 文本：Desktop 的推理探针也是单条 `"."`（票 16 / 26）。
pub const PROBE_PROMPT: &str = ".";

/// 一次自检的结论：**只看状态码**（票 26 决定 1），body 不解析、不落库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub upstream_id: String,
    pub real_model: String,
    pub wire: Wire,
    /// HTTP 状态码；超时记 504、连不上记 502（与转发面的两个出口同一套口径）。
    pub status: u16,
    /// 2xx 才算通过。
    pub ok: bool,
    pub elapsed_ms: u128,
    /// 人话（界面直接显示）。**不含密钥与 baseUrl**。
    pub detail: String,
}

/// 非流式 `/v1/messages` + `max_tokens:1` 探一次，并把结果落 `upstreams.last_selfcheck_*`。
///
/// 上限由调用方给（转发面用 `Gateway.probe`，测试拨短）；产品语义上的 20 秒见
/// [`PROBE_DEADLINE`]。就绪判据只看这一列（spec §6 / 票 35 第 7 条）。
pub async fn selfcheck(
    client: &reqwest::Client,
    store: &Store,
    up: &Upstream,
    wire: Wire,
    real_model: &str,
    deadline: Duration,
) -> ProbeResult {
    let t0 = std::time::Instant::now();
    let mut result = ProbeResult {
        upstream_id: up.id.clone(),
        real_model: real_model.to_string(),
        wire,
        status: 0,
        ok: false,
        elapsed_ms: 0,
        detail: String::new(),
    };

    match up.url(wire) {
        None => {
            result.status = 502;
            result.detail = format!("上游 {} 没有 {} 面的路径模板", up.id, wire.as_str());
        }
        Some(url) => {
            let key = provider::wire_credential(store, &up.id, wire)
                .await
                .ok()
                .flatten();
            match key {
                None => {
                    result.status = 502;
                    result.detail = format!("上游 {} 没有可用的凭据", up.id);
                }
                Some(key) => {
                    // 探针形状照票 16 的推理探针：非流式、`max_tokens:1`、单条 user。
                    let body = serde_json::json!({
                        "model": real_model,
                        "max_tokens": 1,
                        "messages": [{"role": "user", "content": PROBE_PROMPT}],
                    });
                    let sent = upstream::request(
                        client,
                        up,
                        wire,
                        up.auth_for(wire),
                        &key,
                        &url,
                        &body,
                        PROBE_PROMPT,
                        &[],
                    )
                    .timeout(deadline)
                    .send()
                    .await;
                    match sent {
                        Ok(resp) => {
                            let status = resp.status();
                            result.status = status.as_u16();
                            result.ok = status.is_success();
                            result.detail = format!("上游 {}", status.as_u16());
                        }
                        Err(e) if e.is_timeout() => {
                            result.status = 504;
                            result.detail =
                                format!("上游超时（{} 秒内无响应）", deadline.as_secs());
                        }
                        Err(_) => {
                            result.status = 502;
                            result.detail = "连不上上游（连接失败）".to_string();
                        }
                    }
                }
            }
        }
    }

    result.elapsed_ms = t0.elapsed().as_millis();
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Err(e) = provider::set_selfcheck_result(store, &up.id, result.ok, at).await {
        eprintln!("[warn] 自检结果写不进去（{}）：{e}", up.id);
    }
    result
}
