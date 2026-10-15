//! 建表与迁移（spec §9）：`PRAGMA user_version` + 顺序迁移函数。
//!
//! 规矩：**只前进、不后退**；一个版本一个函数；每个版本整段跑在一个事务里，失败即回滚
//! （`user_version` 与 DDL 同事务提交，所以不会出现「表改了但版本没动」的半截库）。
//!
//! v1 = 原型期的形状（`prototype/src/store.rs` 的四张表），v2 = spec §9 的形状。
//! 新库也照样从 v1 顺序跑到最新：装一次机就把整条迁移路径走一遍，热路径与升级路径不分叉。

use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};
use std::collections::HashMap;

/// 当前 schema 版本 = 迁移表最后一条。
pub const SCHEMA_VERSION: i32 = 2;

pub(crate) struct Migration {
    pub(crate) version: i32,
    pub(crate) apply: fn(&Transaction<'_>) -> Result<()>,
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        apply: v1_prototype_era,
    },
    Migration {
        version: 2,
        apply: v2_spec9,
    },
];

/// 顺序升到最新版。
pub(crate) fn migrate(c: &mut Connection) -> Result<()> {
    migrate_upto(c, SCHEMA_VERSION)
}

/// 顺序升到 `target`（测试用：造一份旧版库）。
pub(crate) fn migrate_upto(c: &mut Connection, target: i32) -> Result<()> {
    let current: i32 = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for m in MIGRATIONS
        .iter()
        .filter(|m| m.version > current && m.version <= target)
    {
        let tx = c.transaction()?;
        (m.apply)(&tx).with_context(|| format!("迁移到 v{} 失败", m.version))?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    Ok(())
}

/// v1：原型期的四张表（照 `prototype/src/store.rs` 的字面形状，含当时的类型）。
fn v1_prototype_era(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS upstreams(
             id TEXT PRIMARY KEY, base_url TEXT NOT NULL, auth TEXT,
             anthropic_path TEXT, openai_path TEXT);
         CREATE TABLE IF NOT EXISTS quirks(
             upstream_id TEXT NOT NULL, quirk TEXT NOT NULL, arg TEXT NOT NULL DEFAULT '');
         CREATE TABLE IF NOT EXISTS virtual_models(
             name TEXT PRIMARY KEY, upstream_id TEXT, real_model TEXT, wire TEXT, family INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS request_log(
             id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, virtual_model TEXT, upstream_id TEXT,
             real_model TEXT, wire TEXT, status INTEGER, first_byte_ms INTEGER, total_ms INTEGER,
             input_tokens INTEGER, output_tokens INTEGER, cache_read INTEGER, aborted INTEGER NOT NULL DEFAULT 0,
             error TEXT, body TEXT);",
    )?;
    Ok(())
}

/// 原型期 `upstreams` 的一行：id / base_url / auth / anthropic_path / openai_path。
type OldUpstream = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);
/// 原型期 `virtual_models` 的一行：name / upstream_id / real_model / wire / family。
type OldModel = (String, Option<String>, Option<String>, Option<String>, i64);

/// v2：spec §9 的十张表。
///
/// 三处是**数据搬迁**，不是单纯建表：`upstreams` 加列并把 `quirks` 表折进 JSON、
/// `virtual_models` 换成「(客户端档案, 模型名)」复合主键、`request_log` 重建为 `requests`
/// 并把 `body` 搬进 `request_bodies`。旧库里的行一律保住（值搬不动就退成 NULL，不让升级
/// 因为一条脏行整库起不来）。
fn v2_spec9(tx: &Transaction<'_>) -> Result<()> {
    // ── 先把旧数据读出来（改表之前）。
    let old_upstreams: Vec<OldUpstream> = {
        let mut st =
            tx.prepare("SELECT id,base_url,auth,anthropic_path,openai_path FROM upstreams")?;
        let rows = st.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut old_quirks: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    {
        let mut st = tx.prepare("SELECT upstream_id,quirk,arg FROM quirks")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (upstream, quirk, arg) = row?;
            old_quirks
                .entry(upstream)
                .or_default()
                .push(serde_json::json!({ "name": quirk, "arg": arg }));
        }
    }
    // 旧的 wire 取值域是 `anthropic` / `openai`，映射到 §6 的三个值；其余退 NULL（未声明）。
    let old_models: Vec<OldModel> = {
        let mut st = tx.prepare(
            "SELECT name,
                    CASE WHEN upstream_id IN (SELECT id FROM upstreams) THEN upstream_id ELSE NULL END,
                    real_model,
                    CASE wire WHEN 'openai' THEN 'openai-chat' WHEN 'anthropic' THEN 'anthropic'
                              WHEN 'openai-chat' THEN 'openai-chat' WHEN 'openai-responses' THEN 'openai-responses'
                              ELSE NULL END,
                    family
               FROM virtual_models",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // ── upstreams：加 §9 的列（auth/paths 折进 JSON，quirks 独立表并进来）。
    tx.execute_batch(
        "CREATE TABLE upstreams_v2(
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             base_url TEXT NOT NULL,
             auth_json TEXT NOT NULL DEFAULT '{}',
             path_templates_json TEXT NOT NULL DEFAULT '{}',
             header_passthrough_json TEXT NOT NULL DEFAULT '[]',
             quirks_json TEXT NOT NULL DEFAULT '[]',
             last_pull_at INTEGER,
             last_pull_status TEXT,
             last_pull_warning TEXT);",
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO upstreams_v2(id,name,base_url,auth_json,path_templates_json,
                                      header_passthrough_json,quirks_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
        )?;
        for (id, base_url, auth, anthropic_path, openai_path) in &old_upstreams {
            let mut paths = serde_json::Map::new();
            if let Some(p) = anthropic_path {
                if !p.is_empty() {
                    paths.insert("anthropic".into(), p.clone().into());
                }
            }
            if let Some(p) = openai_path {
                if !p.is_empty() {
                    paths.insert("openaiChat".into(), p.clone().into());
                }
            }
            let quirks = old_quirks.remove(id).unwrap_or_default();
            // 原型期没有 name：先用 id 顶（界面上可改）。
            ins.execute(rusqlite::params![
                id,
                id,
                base_url,
                serde_json::json!({ "default": { "scheme": auth.clone().unwrap_or_default() } })
                    .to_string(),
                serde_json::Value::Object(paths).to_string(),
                "[]",
                serde_json::Value::Array(quirks).to_string(),
            ])?;
        }
    }
    tx.execute_batch(
        "DROP TABLE upstreams;
         ALTER TABLE upstreams_v2 RENAME TO upstreams;
         DROP TABLE quirks;",
    )?;

    // ── 其余 §9 表（顺序有讲究：被引用的先建）。
    tx.execute_batch(
        "CREATE TABLE client_profiles(
             id TEXT PRIMARY KEY,
             display_name TEXT NOT NULL,
             match_ua TEXT NOT NULL DEFAULT '',
             enabled INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE upstream_models(
             upstream_id TEXT NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
             model_id TEXT NOT NULL,
             -- JSON 数组；[] = 未声明（正常态，不等于不支持。票 23）
             supported_wires TEXT NOT NULL DEFAULT '[]',
             source TEXT NOT NULL CHECK(source IN ('manual','upstream','doc')),
             -- 手工四项价（每百万 token 美元）；NULL = 回退 models.dev 快照（票 33）
             price_input_usd REAL,
             price_output_usd REAL,
             price_cache_read_usd REAL,
             price_cache_write_usd REAL,
             PRIMARY KEY(upstream_id, model_id));
         CREATE TABLE credentials(
             upstream_id TEXT NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
             -- §6 的三个 wire + 'default'（未填继承 default，票 49）
             wire TEXT NOT NULL CHECK(wire IN ('default','anthropic','openai-chat','openai-responses')),
             -- DPAPI CryptProtectData 密文（CurrentUser 范围）。明文只在内存里（spec §9）
             ciphertext BLOB NOT NULL,
             PRIMARY KEY(upstream_id, wire));
         CREATE TABLE client_config_state(
             -- 每个「客户端 × 文件」一行（ADR-0006 第 2 条）
             client TEXT NOT NULL,
             file_path TEXT NOT NULL,
             -- 上次由网关写入的接管键值（JSON）
             owned_keys_json TEXT NOT NULL DEFAULT '{}',
             last_op TEXT NOT NULL DEFAULT 'write' CHECK(last_op IN ('write','restore')),
             last_write_at INTEGER NOT NULL,
             -- 首写快照路径（常驻不清理，§8.4 第 7 条）
             backup_path TEXT,
             -- 首写时目标文件是否存在（0/1）
             existed_at_first_write INTEGER NOT NULL,
             PRIMARY KEY(client, file_path));
         CREATE TABLE request_bodies(
             -- 默认不写；界面开关打开时才有行（spec §9 日志与隐私）
             request_id INTEGER PRIMARY KEY REFERENCES requests(id) ON DELETE CASCADE,
             request_body TEXT,
             response_body TEXT);
         CREATE TABLE model_catalog(
             -- models.dev 随包快照：顶层 provider id + 模型 id（票 33 / 49）
             provider TEXT NOT NULL,
             model_id TEXT NOT NULL,
             name TEXT,
             cost_input_usd REAL,
             cost_output_usd REAL,
             cost_cache_read_usd REAL,
             cost_cache_write_usd REAL,
             raw_json TEXT,
             PRIMARY KEY(provider, model_id));
         CREATE TABLE settings(
             -- 键：port / log_retention_days / usd_cny_rate / catalog_snapshot_at（§9）
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL);",
    )?;

    // 档案四套 + settings 初值（票 33：汇率不联网，初值随包内置）。
    tx.execute_batch(
        "INSERT OR IGNORE INTO client_profiles(id,display_name,match_ua,enabled) VALUES
             ('global','全局','',1),
             ('claude-desktop','Claude Desktop','',1),
             ('claude-code','Claude Code','',1),
             ('codex','Codex','',1);
         INSERT OR IGNORE INTO settings(key,value) VALUES
             ('port','8787'),
             ('log_retention_days','7'),
             ('usd_cny_rate','7.10'),
             ('catalog_snapshot_at','');",
    )?;

    // ── virtual_models：换成「(客户端档案, 模型名)」复合主键，旧行落 global 档案。
    // 每客户端可覆盖：global 是默认绑定，具体档案（Desktop / Code）各存自己那份（§7 / 票 51）。
    tx.execute_batch(
        "CREATE TABLE virtual_models_v2(
             client_profile_id TEXT NOT NULL DEFAULT 'global' REFERENCES client_profiles(id),
             name TEXT NOT NULL,
             is_family INTEGER NOT NULL DEFAULT 0,
             -- 绑定从目录里选（§7）；被引用的上游不许直接删（票 49 要列引用处）
             upstream_id TEXT REFERENCES upstreams(id) ON DELETE RESTRICT,
             real_model TEXT,
             wire TEXT CHECK(wire IS NULL OR wire IN ('anthropic','openai-chat','openai-responses')),
             purpose TEXT,
             PRIMARY KEY(client_profile_id, name));",
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO virtual_models_v2(client_profile_id,name,is_family,upstream_id,real_model,wire)
             VALUES('global',?1,?2,?3,?4,?5)",
        )?;
        for (name, upstream_id, real_model, wire, family) in &old_models {
            ins.execute(rusqlite::params![
                name,
                family,
                upstream_id,
                real_model,
                wire
            ])?;
        }
    }
    tx.execute_batch(
        "DROP TABLE virtual_models;
         ALTER TABLE virtual_models_v2 RENAME TO virtual_models;",
    )?;

    // ── request_log → requests（§9 的名字 + `cost_usd`），body 搬进 request_bodies。
    // 旧 `ts` 是 'unix:<秒>' 文本（原型不引时间库）：这里改成 Unix 秒整数，日边界留给查询侧算。
    tx.execute_batch(
        "CREATE TABLE requests(
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             -- Unix 秒（UTC）
             ts INTEGER NOT NULL,
             request_id TEXT,
             client TEXT,
             virtual_model TEXT,
             upstream_id TEXT,
             real_model TEXT,
             wire TEXT,
             status INTEGER,
             first_byte_ms INTEGER,
             total_ms INTEGER,
             input_tokens INTEGER,
             output_tokens INTEGER,
             cache_read INTEGER,
             aborted INTEGER NOT NULL DEFAULT 0,
             error TEXT,
             -- 请求时按当时价冻结；NULL = 当时无价（票 33，改价不追古）
             cost_usd REAL);
         INSERT INTO requests(id,ts,request_id,client,virtual_model,upstream_id,real_model,wire,status,
                              first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,aborted,error,cost_usd)
         SELECT id,
                COALESCE(CASE WHEN ts GLOB 'unix:*' THEN CAST(substr(ts,6) AS INTEGER)
                              ELSE CAST(ts AS INTEGER) END, 0),
                NULL, NULL, virtual_model, upstream_id, real_model, wire, status, first_byte_ms, total_ms,
                input_tokens, output_tokens, cache_read, COALESCE(aborted,0), error, NULL
           FROM request_log;
         INSERT INTO request_bodies(request_id,request_body)
         SELECT id, body FROM request_log WHERE body IS NOT NULL;
         DROP TABLE request_log;",
    )?;

    Ok(())
}
