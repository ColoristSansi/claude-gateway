//! 可观测：日志 / 用量 / 成本（spec §9）。旁路，不阻塞转发路径。
//!
//! 票 41 只落地转发路径要用的那两件事：请求日志一行（`requests`）+ body（`request_bodies`，
//! **默认不写**，票 11 的日志定案）。写走票 42 的 [`Store::write`]（单写线程 + 队列），
//! 所以这里两个函数都是异步的；转发侧不 `await` 到底，只在 `Drop` 里 `spawn` 一次。
//!
//! 界面侧的历史页 / 成本汇总归票 53，它会在这两个函数上加查询，不另开一条写路径。

use anyhow::Result;
use rusqlite::params;
use serde_json::{json, Value};

use crate::store::Store;

/// 一次请求的日志行（列名与 §9 的 `requests` 表对齐）。
#[derive(Debug, Clone, Default)]
pub struct RequestLog {
    /// Unix 秒（UTC）；0 = 落库时现算。
    pub ts: i64,
    pub virtual_model: String,
    /// 客户端档案 id（`global` / `claude-desktop` / `claude-code` / `codex`，票 51）：
    /// 按客户端维度聚合用量就看这一列（票 53）。
    pub client: Option<String>,
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
    /// 出站请求体：只有 body 日志开关开着时才是 `Some`。
    pub body: Option<String>,
}

/// 写一行请求日志（body 有就顺带写进 `request_bodies`）。
pub async fn log(store: &Store, row: &RequestLog) -> Result<()> {
    let row = row.clone();
    let ts = if row.ts > 0 { row.ts } else { unix_now() };
    store
        .write(move |c| {
            // 一行日志 + 它的 body 必须一起可见（票 41：测试读的是「有行就有 body」）。
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO requests(ts,virtual_model,client,upstream_id,real_model,wire,status,
                                      first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,
                                      aborted,error)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    ts,
                    row.virtual_model,
                    row.client,
                    row.upstream_id,
                    row.real_model,
                    row.wire,
                    row.status,
                    row.first_byte_ms,
                    row.total_ms,
                    row.input_tokens.map(|v| v as i64),
                    row.output_tokens.map(|v| v as i64),
                    row.cache_read.map(|v| v as i64),
                    row.aborted as i64,
                    row.error,
                ],
            )?;
            if let Some(body) = &row.body {
                tx.execute(
                    "INSERT INTO request_bodies(request_id,request_body) VALUES(last_insert_rowid(),?1)",
                    params![body],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
}

/// 最近 `limit` 行（新到旧），字段名与原型 `GET /api/logs` 一致（body 来自 `request_bodies`）。
pub async fn recent(store: &Store, limit: usize) -> Result<Vec<Value>> {
    let limit = limit as i64;
    store
        .read(move |c| {
            let mut st = c.prepare(
                "SELECT r.ts,r.virtual_model,r.client,r.upstream_id,r.real_model,r.wire,r.status,
                        r.first_byte_ms,r.total_ms,r.input_tokens,r.output_tokens,r.cache_read,
                        r.aborted,r.error,b.request_body
                   FROM requests r
                   LEFT JOIN request_bodies b ON b.request_id = r.id
                  ORDER BY r.id DESC LIMIT ?1",
            )?;
            let it = st.query_map(params![limit], |r| {
                Ok(json!({
                    "ts": r.get::<_, i64>(0)?,
                    "virtual_model": r.get::<_, Option<String>>(1)?,
                    "client": r.get::<_, Option<String>>(2)?,
                    "upstream": r.get::<_, Option<String>>(3)?,
                    "real_model": r.get::<_, Option<String>>(4)?,
                    "wire": r.get::<_, Option<String>>(5)?,
                    "status": r.get::<_, Option<i64>>(6)?,
                    "first_byte_ms": r.get::<_, Option<i64>>(7)?,
                    "total_ms": r.get::<_, Option<i64>>(8)?,
                    "input_tokens": r.get::<_, Option<i64>>(9)?,
                    "output_tokens": r.get::<_, Option<i64>>(10)?,
                    "cache_read": r.get::<_, Option<i64>>(11)?,
                    "aborted": r.get::<_, i64>(12)? != 0,
                    "error": r.get::<_, Option<String>>(13)?,
                    "body": r.get::<_, Option<String>>(14)?,
                }))
            })?;
            Ok(it.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
