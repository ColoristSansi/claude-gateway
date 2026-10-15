//! 导出 / 导入（票 62；spec §9「导出 JSON 时默认不含明文密钥（可选带密码的加密导出）」，
//! spec §13「DPAPI 换机不可解密 → 导出时提供带密码的加密导出」）。
//!
//! # 文件形状（脚本可读、版本可判）
//!
//! 明文导出（默认，**不含明文密钥**）：
//!
//! ```json
//! { "format": "claude-gateway-export", "version": 1, "appVersion": "0.1.0",
//!   "exportedAt": "2026-10-15T12:00:00Z", "secrets": false,
//!   "data": { "upstreams": [ { "id": "openrouter", "baseUrl": "https://…",
//!             "credentials": [ { "wire": "anthropic", "masked": "sk-…abcd" } ] } ],
//!             "upstreamModels": [], "virtualModels": [], "clientProfiles": [], "settings": {} } }
//! ```
//!
//! 带口令的加密导出（`--password`）：`data` 换成 `encrypted`，密钥用口令重新加密，
//! **与 DPAPI 无关**，所以能跨机导入。
//!
//! ```json
//! { "format": "claude-gateway-export", "version": 1, "secrets": true,
//!   "encrypted": { "cipher": "aes-256-gcm", "kdf": "pbkdf2-hmac-sha256", "iterations": 210000,
//!                  "salt": "<hex>", "nonce": "<hex>", "payload": "<hex>" } }
//! ```
//!
//! # 范围
//!
//! 导出的是**配置**：上游表（含手工价与「上游 × 真实模型」目录）、虚拟模型与家族槽位、客户端档案、
//! `settings`、凭据。**不含**请求日志 / 用量（`requests` / `request_bodies`）与拉取、自检的运行态时间
//! （那是本机的运行痕迹，不是配置）。
//!
//! # 判据
//!
//! - 默认导出**先摘掉密钥再序列化**：明文密钥一个字节都不进文件（验收就是拿已知密钥串反查文件）。
//! - `secrets: true` 必须配口令（否则等于把明文密钥写成明文文件，宁可报错）。
//! - 导入认 `format` 与 `version`：不是本程序的文件 / 版本比本程序新 → 报错并说清；
//!   重复上游（同 id 已在库）逐条列进回执，默认**跳过**，显式 `--overwrite` 才覆盖。

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::pbkdf2::{self, PBKDF2_HMAC_SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::ingress::api::rfc3339_utc;
use crate::store::{mask_secret, Store};

/// 文件里的身份标签；不是这个值就不是本程序的文件。
pub const FORMAT_TAG: &str = "claude-gateway-export";
/// 当前的导出格式版本。加字段不算新版本；语义变了才 +1，导入侧只拒「比本程序新」的。
pub const FORMAT_VERSION: u32 = 1;

/// PBKDF2-HMAC-SHA256 迭代次数（2026 年本机 <0.2s，够挡住离线爆破）。
const KDF_ITERATIONS: u32 = 210_000;
const SALT_LEN: usize = 16;
const CIPHER: &str = "aes-256-gcm";
const KDF: &str = "pbkdf2-hmac-sha256";

/// 导出选项。默认 = 不含明文密钥、不加密（`Default::default()`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// 是否把密钥**明文**放进导出文件。必须配 `password`，否则 [`export`] 报错。
    pub include_secrets: bool,
    /// 口令：给了就加密（也能跨机搬密钥，见 spec §13）。
    pub password: Option<String>,
}

impl ExportOptions {
    /// 带口令的加密导出（keys 一起搬）。
    pub fn encrypted(password: impl Into<String>) -> ExportOptions {
        ExportOptions {
            include_secrets: true,
            password: Some(password.into()),
        }
    }

    /// 不带口令、也不含明文密钥的默认导出。
    pub fn plain() -> ExportOptions {
        ExportOptions::default()
    }
}

/// 导入时撞上重复上游怎么办。默认跳过（不覆盖用户现有的东西）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportMode {
    /// 库里有同 id 的上游 → 保留库里的，只记进回执。
    #[default]
    SkipExisting,
    /// 库里有同 id 的上游 → 用导出文件里的覆盖。
    Replace,
}

/// 导出文件（明文态或加密态二选一）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    pub app_version: String,
    pub exported_at: String,
    /// 文件里有没有明文密钥（加密导出恒为 `true`，因为载荷里有）。
    pub secrets: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Data>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<Encrypted>,
}

/// 加密信封：口令派生 + AES-256-GCM。**不依赖 DPAPI**，所以跨机可用。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Encrypted {
    pub cipher: String,
    pub kdf: String,
    pub iterations: u32,
    /// 十六进制（不引 base64 依赖）。
    pub salt: String,
    pub nonce: String,
    pub payload: String,
}

/// 导出的配置本体（`settings` 用 BTreeMap：键有序，文件 diff 友好）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    #[serde(default)]
    pub upstreams: Vec<Upstream>,
    #[serde(default)]
    pub upstream_models: Vec<UpstreamModel>,
    #[serde(default)]
    pub virtual_models: Vec<VirtualModel>,
    #[serde(default)]
    pub client_profiles: Vec<ClientProfile>,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
}

/// 一个上游 + 它在库里的凭据（默认只有掩码）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default = "empty_object")]
    pub auth_json: String,
    #[serde(default = "empty_object")]
    pub path_templates_json: String,
    #[serde(default = "empty_array")]
    pub header_passthrough_json: String,
    #[serde(default = "empty_array")]
    pub quirks_json: String,
    #[serde(default)]
    pub credentials: Vec<Credential>,
}

fn empty_object() -> String {
    "{}".to_string()
}
fn empty_array() -> String {
    "[]".to_string()
}

/// 「上游 × wire」的一条凭据。`secret` 只在 `secrets: true` 的导出里出现。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub wire: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// 掩码（界面同款，`mask_secret`）。DPAPI 解不开（换机）时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked: Option<String>,
}

/// 「上游 × 真实模型」目录条目（含手工四项价，票 33）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModel {
    pub upstream_id: String,
    pub model_id: String,
    #[serde(default = "empty_array")]
    pub supported_wires: String,
    pub source: String,
    #[serde(default)]
    pub price_input_usd: Option<f64>,
    #[serde(default)]
    pub price_output_usd: Option<f64>,
    #[serde(default)]
    pub price_cache_read_usd: Option<f64>,
    #[serde(default)]
    pub price_cache_write_usd: Option<f64>,
}

/// 虚拟模型 / 家族槽位（按客户端档案分开存，票 51）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualModel {
    pub client_profile_id: String,
    pub name: String,
    #[serde(default)]
    pub is_family: bool,
    #[serde(default)]
    pub upstream_id: Option<String>,
    #[serde(default)]
    pub real_model: Option<String>,
    #[serde(default)]
    pub wire: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
}

/// 客户端档案（四个内置档案，票 42）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientProfile {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub match_ua: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// 导入回执（给界面与脚本看的一句话账）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub format: String,
    pub version: u32,
    pub app_version: String,
    pub exported_at: String,
    /// 库里有同 id 的重复上游（全列出来，不管跳没跳）。
    pub duplicates: Vec<String>,
    pub upstreams_created: usize,
    pub upstreams_replaced: usize,
    pub upstreams_skipped: usize,
    pub upstream_models_imported: usize,
    pub virtual_models_imported: usize,
    /// 绑定的上游在库里也不在本文件里 → 没敢写（外键会拒），单独计数。
    pub virtual_models_dropped: usize,
    pub settings_imported: usize,
    pub credentials_imported: usize,
    /// 文件里没有密钥（默认导出）→ 这些凭据要用户重填。
    pub credentials_skipped: usize,
}

/// 导出成 JSON 文本。`--password` 给了就加密（并允许带密钥）；没给口令却要密钥 → 报错。
pub async fn export(store: &Store, app_version: &str, opts: &ExportOptions) -> Result<String> {
    if opts.include_secrets && opts.password.is_none() {
        bail!(
            "要么不带明文密钥（默认），要么配一个口令做加密导出：明文密钥的未加密文件不该落到盘上"
        );
    }
    let include_secrets = opts.include_secrets;
    let data = store.read(move |c| read_data(c, include_secrets)).await?;

    let mut bundle = Bundle {
        format: FORMAT_TAG.to_string(),
        version: FORMAT_VERSION,
        app_version: app_version.to_string(),
        exported_at: rfc3339_utc(SystemTime::now()),
        secrets: opts.include_secrets,
        data: None,
        encrypted: None,
    };

    match &opts.password {
        None => bundle.data = Some(data),
        Some(password) => {
            let plain = serde_json::to_vec(&data).context("序列化导出载荷失败")?;
            bundle.encrypted = Some(encrypt(password, &plain)?);
        }
    }

    let mut text = serde_json::to_string_pretty(&bundle).context("序列化导出文件失败")?;
    text.push('\n');
    Ok(text)
}

/// 从 JSON 文本导入。`password` 只在加密文件上需要。
pub async fn import(
    store: &Store,
    text: &str,
    password: Option<&str>,
    mode: ImportMode,
) -> Result<ImportReport> {
    let bundle: Bundle = serde_json::from_str(text).context("导出文件不是合法 JSON")?;
    if bundle.format != FORMAT_TAG {
        bail!(
            "这不像是 Claude Gateway 的导出文件（format = {:?}，应当是 {FORMAT_TAG:?}）",
            bundle.format
        );
    }
    if bundle.version > FORMAT_VERSION {
        bail!(
            "导出文件的版本 {} 比本程序支持的（{FORMAT_VERSION}）新：请先升级 Claude Gateway 再导入",
            bundle.version
        );
    }
    if bundle.version == 0 {
        bail!("导出文件没有版本号（version = 0），无法判断格式");
    }

    let data = match (&bundle.encrypted, &bundle.data) {
        (Some(enc), _) => {
            let password = password
                .ok_or_else(|| anyhow!("这个导出文件是加密的（{CIPHER} / {KDF}），导入需要口令"))?;
            let plain = decrypt(enc, password)?;
            serde_json::from_slice::<Data>(&plain)
                .context("导出文件解密成功但载荷不是本程序的配置")?
        }
        (None, Some(data)) => data.clone(),
        (None, None) => bail!("导出文件既没有 data 也没有 encrypted，是空文件"),
    };

    let mut report = write_data(store, data, mode).await?;
    report.format = bundle.format.clone();
    report.version = bundle.version;
    report.app_version = bundle.app_version.clone();
    report.exported_at = bundle.exported_at.clone();
    Ok(report)
}

// ── 读库 ──────────────────────────────────────────────────────────────────────

/// 一次读齐配置（一个读连接上跑完，不在请求路径上）。
fn read_data(c: &Connection, include_secrets: bool) -> Result<Data> {
    let mut upstreams = Vec::new();
    {
        let mut st = c.prepare(
            "SELECT id,name,base_url,auth_json,path_templates_json,header_passthrough_json,quirks_json
               FROM upstreams ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Upstream {
                id: r.get(0)?,
                name: r.get(1)?,
                base_url: r.get(2)?,
                auth_json: r.get(3)?,
                path_templates_json: r.get(4)?,
                header_passthrough_json: r.get(5)?,
                quirks_json: r.get(6)?,
                credentials: Vec::new(),
            })
        })?;
        for row in rows {
            upstreams.push(row?);
        }
    }
    {
        let mut st = c.prepare(
            "SELECT upstream_id,wire,ciphertext FROM credentials ORDER BY upstream_id,wire",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (id, wire, cipher) = row?;
            // 明文只在内存里过一下：不进文件（默认）或进加密载荷。
            let (secret, masked) = match crate::store::dpapi::unprotect(&cipher) {
                Ok(plain) => {
                    let text = String::from_utf8(plain).context("凭据明文不是 UTF-8")?;
                    let masked = Some(mask_secret(&text));
                    (include_secrets.then_some(text), masked)
                }
                // 换机 / 换用户：DPAPI 解不开。掩码也给不出来，但导出照样成功。
                Err(_) => (None, None),
            };
            if let Some(u) = upstreams.iter_mut().find(|u| u.id == id) {
                u.credentials.push(Credential {
                    wire,
                    secret,
                    masked,
                });
            }
        }
    }

    let upstream_models = {
        let mut out = Vec::new();
        let mut st = c.prepare(
            "SELECT upstream_id,model_id,supported_wires,source,
                    price_input_usd,price_output_usd,price_cache_read_usd,price_cache_write_usd
               FROM upstream_models ORDER BY upstream_id,model_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(UpstreamModel {
                upstream_id: r.get(0)?,
                model_id: r.get(1)?,
                supported_wires: r.get(2)?,
                source: r.get(3)?,
                price_input_usd: r.get(4)?,
                price_output_usd: r.get(5)?,
                price_cache_read_usd: r.get(6)?,
                price_cache_write_usd: r.get(7)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
        out
    };

    let virtual_models = {
        let mut out = Vec::new();
        let mut st = c.prepare(
            "SELECT client_profile_id,name,is_family,upstream_id,real_model,wire,purpose
               FROM virtual_models ORDER BY client_profile_id,name",
        )?;
        let rows = st.query_map([], |r| {
            Ok(VirtualModel {
                client_profile_id: r.get(0)?,
                name: r.get(1)?,
                is_family: r.get::<_, i64>(2)? != 0,
                upstream_id: r.get(3)?,
                real_model: r.get(4)?,
                wire: r.get(5)?,
                purpose: r.get(6)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
        out
    };

    let client_profiles = {
        let mut out = Vec::new();
        let mut st =
            c.prepare("SELECT id,display_name,match_ua,enabled FROM client_profiles ORDER BY id")?;
        let rows = st.query_map([], |r| {
            Ok(ClientProfile {
                id: r.get(0)?,
                display_name: r.get(1)?,
                match_ua: r.get(2)?,
                enabled: r.get::<_, i64>(3)? != 0,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
        out
    };

    let settings = {
        let mut out = BTreeMap::new();
        let mut st = c.prepare("SELECT key,value FROM settings ORDER BY key")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            out.insert(k, v);
        }
        out
    };

    Ok(Data {
        upstreams,
        upstream_models,
        virtual_models,
        client_profiles,
        settings,
    })
}

// ── 写库 ──────────────────────────────────────────────────────────────────────

/// 全部写在一个事务里（`Store::write` 一个 job）：中途失败不留半截数据。
///
/// 一律 `ON CONFLICT DO UPDATE`，**不用 `INSERT OR REPLACE`**：REPLACE 是先删后插，
/// 会顺着外键把用户已有的 `credentials` / `upstream_models` 连带删掉。
async fn write_data(store: &Store, data: Data, mode: ImportMode) -> Result<ImportReport> {
    // 凭据在写库之后单独走 `put_credential`（那是唯一的 DPAPI 加密入口），所以先把要写的
    // 三件事（上游 id / wire / 明文）拷出来，`data` 本体随后整个搬进写库闭包。
    let secrets: Vec<(String, String, String)> = data
        .upstreams
        .iter()
        .flat_map(|u| {
            u.credentials
                .iter()
                .filter_map(|c| c.secret.clone().map(|s| (u.id.clone(), c.wire.clone(), s)))
        })
        .collect();
    let skipped_credentials = data
        .upstreams
        .iter()
        .flat_map(|u| u.credentials.iter())
        .filter(|c| c.secret.is_none())
        .count();

    let report = store
        .write(move |c| {
            let mut report = ImportReport::default();
            let tx = c.transaction()?;

            let existing: Vec<String> = {
                let mut st = tx.prepare("SELECT id FROM upstreams")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            // ① 上游
            let mut accepted: Vec<String> = Vec::new();
            for u in &data.upstreams {
                if existing.iter().any(|id| id == &u.id) {
                    report.duplicates.push(u.id.clone());
                    if mode == ImportMode::SkipExisting {
                        report.upstreams_skipped += 1;
                        accepted.push(u.id.clone()); // 库里本来就有，下面的模型照样能落
                        continue;
                    }
                    report.upstreams_replaced += 1;
                } else {
                    report.upstreams_created += 1;
                }
                tx.execute(
                    "INSERT INTO upstreams(id,name,base_url,auth_json,path_templates_json,
                                           header_passthrough_json,quirks_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(id) DO UPDATE SET
                        name=excluded.name, base_url=excluded.base_url, auth_json=excluded.auth_json,
                        path_templates_json=excluded.path_templates_json,
                        header_passthrough_json=excluded.header_passthrough_json,
                        quirks_json=excluded.quirks_json",
                    params![
                        u.id,
                        u.name,
                        u.base_url,
                        u.auth_json,
                        u.path_templates_json,
                        u.header_passthrough_json,
                        u.quirks_json
                    ],
                )?;
                accepted.push(u.id.clone());
            }

            // ② 上游模型目录（含手工价）
            for m in &data.upstream_models {
                if !accepted.iter().any(|id| id == &m.upstream_id) {
                    continue;
                }
                tx.execute(
                    "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source,
                                                 price_input_usd,price_output_usd,
                                                 price_cache_read_usd,price_cache_write_usd)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(upstream_id,model_id) DO UPDATE SET
                        supported_wires=excluded.supported_wires, source=excluded.source,
                        price_input_usd=excluded.price_input_usd,
                        price_output_usd=excluded.price_output_usd,
                        price_cache_read_usd=excluded.price_cache_read_usd,
                        price_cache_write_usd=excluded.price_cache_write_usd",
                    params![
                        m.upstream_id,
                        m.model_id,
                        m.supported_wires,
                        m.source,
                        m.price_input_usd,
                        m.price_output_usd,
                        m.price_cache_read_usd,
                        m.price_cache_write_usd
                    ],
                )?;
                report.upstream_models_imported += 1;
            }

            // ③ 客户端档案（先于虚拟模型：后者外键引用它）
            for p in &data.client_profiles {
                tx.execute(
                    "INSERT INTO client_profiles(id,display_name,match_ua,enabled)
                     VALUES(?1,?2,?3,?4)
                     ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,
                        match_ua=excluded.match_ua, enabled=excluded.enabled",
                    params![p.id, p.display_name, p.match_ua, i64::from(p.enabled)],
                )?;
            }

            // ④ 虚拟模型 / 家族槽位
            let known: Vec<String> = {
                let mut st = tx.prepare("SELECT id FROM upstreams")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let profiles: Vec<String> = {
                let mut st = tx.prepare("SELECT id FROM client_profiles")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for v in &data.virtual_models {
                let bound_ok = match &v.upstream_id {
                    None => true,
                    Some(id) => known.iter().any(|k| k == id),
                };
                if !bound_ok || !profiles.iter().any(|p| p == &v.client_profile_id) {
                    report.virtual_models_dropped += 1;
                    continue;
                }
                tx.execute(
                    "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,
                                                real_model,wire,purpose)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(client_profile_id,name) DO UPDATE SET
                        is_family=excluded.is_family, upstream_id=excluded.upstream_id,
                        real_model=excluded.real_model, wire=excluded.wire,
                        purpose=excluded.purpose",
                    params![
                        v.client_profile_id,
                        v.name,
                        i64::from(v.is_family),
                        v.upstream_id,
                        v.real_model,
                        v.wire,
                        v.purpose
                    ],
                )?;
                report.virtual_models_imported += 1;
            }

            // ⑤ 设置（端口 / 汇率 / 日志保留都在这里；改端口要重启才生效）
            for (k, v) in &data.settings {
                tx.execute(
                    "INSERT INTO settings(key,value) VALUES(?1,?2)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![k, v],
                )?;
                report.settings_imported += 1;
            }

            tx.commit()?;
            Ok(report)
        })
        .await?;

    // ⑥ 凭据：只认文件里带了明文的那些（默认导出没有 → 跳过并计数）
    let mut report = report;
    for (upstream_id, wire, secret) in &secrets {
        store.put_credential(upstream_id, wire, secret).await?;
        report.credentials_imported += 1;
    }
    report.credentials_skipped = skipped_credentials;
    Ok(report)
}

// ── 口令加密（AES-256-GCM + PBKDF2-HMAC-SHA256） ───────────────────────────────

fn encrypt(password: &str, plain: &[u8]) -> Result<Encrypted> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    rng.fill(&mut salt).map_err(|_| anyhow!("取随机盐失败"))?;
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut nonce)
        .map_err(|_| anyhow!("取随机 nonce 失败"))?;

    let key = aes_key(password, &salt, KDF_ITERATIONS)?;
    let mut buf = plain.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| anyhow!("加密导出载荷失败"))?;

    Ok(Encrypted {
        cipher: CIPHER.to_string(),
        kdf: KDF.to_string(),
        iterations: KDF_ITERATIONS,
        salt: hex(&salt),
        nonce: hex(&nonce),
        payload: hex(&buf),
    })
}

fn decrypt(enc: &Encrypted, password: &str) -> Result<Vec<u8>> {
    if enc.cipher != CIPHER || enc.kdf != KDF {
        bail!(
            "不认识的加密方式：cipher={:?} kdf={:?}（本程序支持 {CIPHER} / {KDF}）",
            enc.cipher,
            enc.kdf
        );
    }
    // 迭代次数来自文件：挡住 0（ring 会 panic）与荒谬的大数（拿它当 DoS）。
    if enc.iterations == 0 || enc.iterations > 10_000_000 {
        bail!("导出文件里的 PBKDF2 迭代次数不合理：{}", enc.iterations);
    }
    let salt = unhex(&enc.salt).context("salt 不是十六进制")?;
    let nonce = unhex(&enc.nonce).context("nonce 不是十六进制")?;
    let nonce: [u8; NONCE_LEN] = nonce
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("nonce 长度不是 {NONCE_LEN} 字节"))?;
    let mut buf = unhex(&enc.payload).context("payload 不是十六进制")?;

    let key = aes_key(password, &salt, enc.iterations)?;
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
        .map_err(|_| anyhow!("口令不对，或导出文件被改过（解密失败）"))?;
    Ok(plain.to_vec())
}

fn aes_key(password: &str, salt: &[u8], iterations: u32) -> Result<LessSafeKey> {
    let iterations =
        NonZeroU32::new(iterations).ok_or_else(|| anyhow!("PBKDF2 迭代次数不能为 0"))?;
    let mut key_bytes = [0u8; 32];
    pbkdf2::derive(
        PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        password.as_bytes(),
        &mut key_bytes,
    );
    let unbound =
        UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| anyhow!("AES-256-GCM 密钥不合法"))?;
    Ok(LessSafeKey::new(unbound))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        bail!("十六进制串长度是奇数：{}", s.len());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("{e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_odd_input() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(unhex("000fff").expect("解十六进制"), vec![0x00, 0x0f, 0xff]);
        assert!(unhex("abc").is_err(), "奇数长度必须报错");
        assert!(unhex("zz").is_err(), "非十六进制必须报错");
    }

    #[test]
    fn password_encryption_round_trips_and_rejects_wrong_password_or_tampering() {
        let secret_text = "sk-or-v1-DEADBEEF-这是一个只应出现在载荷里的密钥";
        let enc = encrypt("正确口令", secret_text.as_bytes()).expect("加密");
        assert_ne!(enc.payload, hex(secret_text.as_bytes()), "载荷必须不是明文");
        assert_eq!(enc.iterations, KDF_ITERATIONS);

        let back = decrypt(&enc, "正确口令").expect("同口令解得开");
        assert_eq!(String::from_utf8(back).expect("UTF-8"), secret_text);

        // 期望值来自「口令错了」这个独立事实，不是用 encrypt 反算的。
        let wrong = decrypt(&enc, "错口令").expect_err("错口令必须解不开");
        assert!(wrong.to_string().contains("口令不对"), "{wrong}");

        let mut tampered = enc.clone();
        let mut bytes = unhex(&tampered.payload).expect("解载荷");
        bytes[0] ^= 0xff;
        tampered.payload = hex(&bytes);
        assert!(
            decrypt(&tampered, "正确口令").is_err(),
            "改一个字节就必须解不开"
        );

        // 同一个口令两次加密的盐不同 → 密文不同（盐是随机的）。
        let again = encrypt("正确口令", secret_text.as_bytes()).expect("再加密");
        assert_ne!(again.salt, enc.salt);
        assert_ne!(again.payload, enc.payload);
    }
}
