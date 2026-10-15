//! SQLite：上游 / quirks / 虚拟模型（含四个家族槽位）/ 请求日志。
//! 原型里路由表启动时从库里读，所以「DB 在路径上」不是摆设。

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::provider::{Quirk, Upstream};

#[derive(Debug, Clone)]
pub struct Route {
    pub name: String,
    pub upstream_id: String,
    pub real_model: String,
    pub wire: String,
    pub family: bool,
}

pub fn open(db: &str) -> Result<Connection> {
    Ok(Connection::open(db)?)
}

pub fn init(db: &str, ups: &[Upstream], routes: &[Route]) -> Result<()> {
    let c = open(db)?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS upstreams(
             id TEXT PRIMARY KEY, base_url TEXT, auth TEXT, anthropic_path TEXT, openai_path TEXT);
         CREATE TABLE IF NOT EXISTS quirks(upstream_id TEXT, quirk TEXT, arg TEXT);
         CREATE TABLE IF NOT EXISTS virtual_models(
             name TEXT PRIMARY KEY, upstream_id TEXT, real_model TEXT, wire TEXT, family INTEGER);
         CREATE TABLE IF NOT EXISTS request_log(
             id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, virtual_model TEXT, upstream_id TEXT,
             real_model TEXT, wire TEXT, status INTEGER, first_byte_ms INTEGER, total_ms INTEGER,
             input_tokens INTEGER, output_tokens INTEGER, cache_read INTEGER, aborted INTEGER, error TEXT,
              body TEXT);",
    )?;
    // 老库补列（票 30 第 7 条）：CREATE TABLE IF NOT EXISTS 不会给已存在的表加列，缺列会让 INSERT 静默失败。
    let _ = c.execute("ALTER TABLE request_log ADD COLUMN body TEXT", []);
    for u in ups {
        c.execute(
            "INSERT OR REPLACE INTO upstreams(id,base_url,auth,anthropic_path,openai_path) VALUES(?1,?2,?3,?4,?5)",
            params![u.id, u.base_url, u.auth.as_str(), u.path_anthropic, u.path_openai],
        )?;
        c.execute("DELETE FROM quirks WHERE upstream_id=?1", params![u.id])?;
        for q in &u.quirks {
            let (name, arg) = match q {
                Quirk::ClientUserAgent(a) => ("ClientUserAgent", *a),
                Quirk::SessionIdHeader(a) => ("SessionIdHeader", *a),
                Quirk::ClaudeModelsOnAnthropicOnly => ("ClaudeModelsOnAnthropicOnly", ""),
            };
            c.execute(
                "INSERT INTO quirks(upstream_id,quirk,arg) VALUES(?1,?2,?3)",
                params![u.id, name, arg],
            )?;
        }
    }
    for r in routes {
        c.execute(
            "INSERT OR REPLACE INTO virtual_models(name,upstream_id,real_model,wire,family) VALUES(?1,?2,?3,?4,?5)",
            params![r.name, r.upstream_id, r.real_model, r.wire, r.family as i64],
        )?;
    }
    Ok(())
}

pub fn routes(db: &str) -> Result<Vec<Route>> {
    let c = open(db)?;
    let mut st = c.prepare("SELECT name,upstream_id,real_model,wire,family FROM virtual_models ORDER BY family DESC, name")?;
    let rows = st.query_map([], |r| {
        Ok(Route {
            name: r.get(0)?,
            upstream_id: r.get(1)?,
            real_model: r.get(2)?,
            wire: r.get(3)?,
            family: r.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, Default)]
pub struct LogRow {
    pub virtual_model: String,
    pub upstream_id: String,
    pub real_model: String,
    pub wire: String,
    pub status: i64,
    pub first_byte_ms: i64,
    pub total_ms: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read: Option<u64>,
    pub aborted: bool,
    pub error: Option<String>,
    /// 出站请求体（发给上游的那一份）。**只有** body 日志开关打开且未到点时才有值（票 30 第 7 条）。
    pub body: Option<String>,
}

/// 每行单开一个连接：原型不做连接池。
/// shortcut: 同步写库会短暂阻塞 runtime，日志量大了要换成专用写线程。
pub fn log(db: &str, r: &LogRow) {
    let Ok(c) = open(db) else { return };
    let _ = c.execute(
        "INSERT INTO request_log(ts,virtual_model,upstream_id,real_model,wire,status,first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,aborted,error,body)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        params![
            now(),
            r.virtual_model,
            r.upstream_id,
            r.real_model,
            r.wire,
            r.status,
            r.first_byte_ms,
            r.total_ms,
            r.input_tokens.map(|v| v as i64),
            r.output_tokens.map(|v| v as i64),
            r.cache_read.map(|v| v as i64),
            r.aborted as i64,
            r.error,
            r.body
        ],
    );
}

fn now() -> String {
    // 原型不引时间库：Unix 秒足够定位一次请求。
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unix:0".to_string(),
    }
}
