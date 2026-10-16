//! 可观测：日志 / 用量 / 成本 / 保留期（spec §9，票 41 起、票 53 补全）。
//!
//! 旁路，不阻塞转发路径：请求日志一行（`requests`）+ body（`request_bodies`，**默认不写**），
//! 写走票 42 的 [`Store::write`]（单写线程 + 队列），所以写入口是异步的；转发侧不 `await`
//! 到底，只在 `Drop` 里 `spawn` 一次。
//!
//! 票 53 在这条已有写路径上补齐三件事：
//!
//! - **成本冻结**：`cost_usd` 在**写这一行的事务里**按当时价算好（[`resolve_cost`]），
//!   之后改手工价或刷新 models.dev 快照都不追古（票 33 第 3 节）。价格只读库、不发网络。
//! - **保留期**：[`purge`] 按 `settings.log_retention_days`（默认 7 天）清老行，
//!   [`spawn_purge_sweeper`] 是启动时跑一次、之后每小时一次的那只表。
//! - **查询面**：日志页要的筛选 / 搜索 / 分页（[`LogQuery`] / [`list`] / [`filters`]）
//!   与概览要的今日 + 近 7 天汇总（[`usage_summary`]）；完整用量页的四段详情与五个时间范围归票 59。
//!
//! 时区口径：日边界按**本机本地时区自然日**（spec §9），所以这里自己算本地偏移，
//! 不引时间库。
//!
//! 票 59 在查询面补齐两件事：
//!
//! - **用量页的五段范围 × 三维度 + 按天明细**（[`usage`] / [`range_window`]）：概览那份
//!   （今日 / 近 7 天）走的是同一个实现，两边不会各算一套。
//! - **详情三段的数据源**（[`log_detail`]）：概要 / 请求与响应体 / 帧时间轴。上游那侧
//!   （`egress::upstream`）在 body 开关开着时顺手记下**原始 SSE 文本**与逐帧元数据，
//!   落点是 `request_bodies.response_body` + 新表 `request_frames`（迁移 v8，见 `store::schema`）。

use std::sync::{atomic::AtomicU64, atomic::Ordering, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use rusqlite::{params, types::Value as SqlValue, Connection};
use serde_json::{json, Value};

use crate::egress::provider::{self, ResolvedPrices};
use crate::store::{settings_keys, Store};

#[cfg(test)]
mod tests;

/// body 日志窗口：开启后 1 小时自动关（票 11 的日志定案 / 票 30 第 7 条）。
pub const BODY_LOG_WINDOW: Duration = Duration::from_secs(3600);

/// 保留期初值（天）。库里 `settings.log_retention_days` 是唯一真相，这个常量只是兜底。
pub const DEFAULT_RETENTION_DAYS: i64 = 7;

/// 清理节奏：每小时扫一次（票 53「保留 7 天自动清理」）。
pub const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

/// 日志页默认页大小（spec §10 第 5 条：时间倒序 50 条）。
pub const DEFAULT_PAGE: usize = 50;

/// body 日志开关的窗口：`None` = 关。
///
/// 转发面（[`crate::ingress::proxy::Gateway`]）与管理面（`/api/logs/body-recording`）**共用同一个**
/// 窗口实例（票 53 第 19 行：日志页右上角与设置页是同一个开关）。
pub struct BodyLogWindow {
    until: Mutex<Option<Instant>>,
}

impl BodyLogWindow {
    pub fn new() -> Self {
        Self {
            until: Mutex::new(None),
        }
    }

    /// 现在开着吗（到点即关，不需要谁来翻开关）。
    pub fn on(&self) -> bool {
        self.until
            .lock()
            .map(|until| until.map(|d| Instant::now() < d).unwrap_or(false))
            .unwrap_or(false)
    }

    /// 拨窗口：`0` = 立即关，其余 = 从现在起这么多秒（到点自关）。
    pub fn set(&self, seconds: u64) {
        if let Ok(mut until) = self.until.lock() {
            *until = if seconds == 0 {
                None
            } else {
                Some(Instant::now() + Duration::from_secs(seconds))
            };
        }
    }

    /// 还剩几秒自动关（关了就是 `None`）——界面上要报「1 小时后自动关」的倒计时。
    pub fn seconds_remaining(&self) -> Option<u64> {
        let until = self.until.lock().ok().and_then(|u| *u)?;
        let now = Instant::now();
        (until > now).then(|| (until - now).as_secs())
    }
}

impl Default for BodyLogWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次请求的日志行（列名与 §9 的 `requests` 表对齐）。
#[derive(Debug, Clone, Default)]
pub struct RequestLog {
    /// Unix 秒（UTC）；0 = 落库时现算。
    pub ts: i64,
    /// 请求 ID（日志页按它搜、详情页按它取）；`None` = 落库时现生成。
    pub request_id: Option<String>,
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
    /// 缓存写（票 58）：上游报才记，没报就是 `None`（**不编 0**）。
    pub cache_write: Option<u64>,
    pub aborted: bool,
    pub error: Option<String>,
    /// 出站请求体：只有 body 日志开关开着时才是 `Some`。
    pub body: Option<String>,
    /// 上游回来的**原始 SSE 文本**（票 59 详情第三段「可切原始 SSE 文本」）：
    /// 同样只有 body 日志开关开着时才是 `Some`（响应内容也是内容）。
    pub response_body: Option<String>,
    /// 帧时间轴（票 59 详情第三段）：与 `response_body` 同一个闸门，关了就是空表。
    pub frames: Vec<FrameLog>,
}

/// 上游流里的一帧（票 59）：时间戳 + 事件类型 + 摘要 + 可展开的原文。
///
/// 落点是自己的一张表（`request_frames`，迁移 v8）：spec §9 的表草案里**没有**帧的落点
/// （`.scratch/model-gateway/frontend.md` §⑤.5 标的就是这个缺口），一帧一行比把整条时间轴
/// 塞进 JSON 列更好查（按 seq 排序、随 `requests` 级联删）。原文那半另存 `request_bodies.response_body`
/// ——那里是**字节原样**的整条 SSE，帧上的 `raw` 是从解析结果重建的（见 `sse::frame`）。
#[derive(Debug, Clone, Default)]
pub struct FrameLog {
    /// 帧序号（0 起，按到达顺序）。
    pub seq: i64,
    /// 相对本次请求开始（`t0`）的毫秒数。
    pub at_ms: i64,
    /// 事件名（`event:` 那一行；没有就是 `None`）。
    pub event: Option<String>,
    /// 摘要：数据部分的前若干字符（界面表格里那一列）。
    pub summary: String,
    /// 该帧的原文（从解析结果重建：`event:` + `data:` + 空行）。
    pub raw: String,
}

/// 写一行请求日志：现生成请求 ID、按**当时价**冻结 `cost_usd`，body 有就顺带写进 `request_bodies`。
pub async fn log(store: &Store, row: &RequestLog) -> Result<()> {
    let row = row.clone();
    let ts = if row.ts > 0 { row.ts } else { unix_now() };
    let request_id = row.request_id.clone().unwrap_or_else(next_request_id);
    store
        .write(move |c| {
            // 「当时价」在**同一事务的同一条连接**上读：落库与计价之间没有窗口，
            // 也不会因为读队列排队而把价读成「写的时候」的价。
            let cost = cost_for(c, &row);
            // 一行日志 + 它的 body 必须一起可见（票 41：测试读的是「有行就有 body」）。
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO requests(ts,request_id,virtual_model,client,upstream_id,real_model,wire,status,
                                      first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,cache_write,
                                      aborted,error,cost_usd)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    ts,
                    request_id,
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
                    row.cache_write.map(|v| v as i64),
                    row.aborted as i64,
                    row.error,
                    cost,
                ],
            )?;
            // 请求行的 id 只在这里读一次：`last_insert_rowid()` 会被后面每一条 INSERT 改写
            // （帧表不是 rowid 别名），**不能**在循环里反复调它。
            let row_id = tx.last_insert_rowid();
            if row.body.is_some() || row.response_body.is_some() {
                tx.execute(
                    "INSERT INTO request_bodies(request_id,request_body,response_body)
                     VALUES(?1,?2,?3)",
                    params![row_id, row.body, row.response_body],
                )
                .map_err(|e| anyhow::anyhow!("写 request_bodies 失败：{e}"))?;
            }
            // 帧时间轴（票 59）：与 body 同一个闸门，一帧一行，随 requests 级联删。
            for f in &row.frames {
                tx.execute(
                    "INSERT INTO request_frames(request_id,seq,at_ms,event,summary,raw)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![row_id, f.seq, f.at_ms, f.event, f.summary, f.raw],
                )
                .map_err(|e| anyhow::anyhow!("写 request_frames(seq={}) 失败：{e}", f.seq))?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
}

/// 这一行按「当时价」该记多少美元；算不出来就是 `None`（**不记 0**，票 33 第 1 节）。
///
/// 价格读失败（库抖动）也退成 `None`：宁可这一行标「无价格」，也不让日志写不进去。
fn cost_for(c: &Connection, row: &RequestLog) -> Option<f64> {
    let prices = provider::resolved_prices_from(c, &row.upstream_id, &row.real_model).ok()?;
    resolve_cost(
        &prices,
        row.input_tokens,
        row.output_tokens,
        row.cache_read,
        row.cache_write,
    )
}

/// 成本口径（票 33 第 1、3、4 节），纯函数好断言：
///
/// - `token 未知`（四项全空）→ `None`；**不用本地估算反算**。
/// - 缺价 → 整条 `None`（**不计入总额**，不半算、不记 0）。
/// - `cache_read` 缺缓存读价则**退输入价**；`cache_write` 按**缓存写价**算，没有缓存写价
///   就是缺价（票 33 第 4 节只给缓存读留了退路，缓存写没有这一条 → 不拿输入价顶替）。
pub fn resolve_cost(
    prices: &ResolvedPrices,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
) -> Option<f64> {
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    // 一项 = 有 token 就必须有价；没 token 的项（含 0）不拖累整条。
    let part = |tokens: Option<u64>, usd_per_mtok: Option<f64>| -> Option<f64> {
        match tokens {
            None | Some(0) => Some(0.0),
            Some(t) => usd_per_mtok.map(|p| t as f64 * p / 1_000_000.0),
        }
    };
    let cache_price = prices.cache_read.value().or_else(|| prices.input.value());
    let total = part(input, prices.input.value())?
        + part(output, prices.output.value())?
        + part(cache_read, cache_price)?
        + part(cache_write, prices.cache_write.value())?;
    Some(total)
}

/// 日志页的查询条件；全空 = 最近 [`DEFAULT_PAGE`] 行。
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub limit: Option<usize>,
    /// 只取 `id < cursor` 的行（翻页用上一页最后一行的 `id`）。
    pub cursor: Option<i64>,
    pub client: Option<String>,
    pub upstream: Option<String>,
    pub virtual_model: Option<String>,
    /// `ok` / `4xx` / `5xx` / `error`（前三个是状态码档，`error` 还含提前中断）。
    pub status: Option<String>,
    /// 匹配虚拟模型名 / 真实模型名 / 请求 ID（**不搜 body**，spec §10 第 5 条）。
    pub q: Option<String>,
}

/// 一节 SQL 条件 + 它的绑定值。
fn where_of(query: &LogQuery) -> (String, Vec<SqlValue>) {
    let mut sql = String::new();
    let mut args: Vec<SqlValue> = Vec::new();
    if let Some(v) = &query.client {
        sql.push_str(" AND r.client = ?");
        args.push(SqlValue::Text(v.clone()));
    }
    if let Some(v) = &query.upstream {
        sql.push_str(" AND r.upstream_id = ?");
        args.push(SqlValue::Text(v.clone()));
    }
    if let Some(v) = &query.virtual_model {
        sql.push_str(" AND r.virtual_model = ?");
        args.push(SqlValue::Text(v.clone()));
    }
    match query.status.as_deref() {
        Some("ok") => sql.push_str(" AND r.status < 400"),
        Some("4xx") => sql.push_str(" AND r.status >= 400 AND r.status < 500"),
        Some("5xx") => sql.push_str(" AND r.status >= 500"),
        Some("error") => sql.push_str(" AND (r.status >= 400 OR r.aborted = 1)"),
        // 认不出的档当没筛（界面传错不该把整页清空）
        _ => {}
    }
    if let Some(v) = &query.q {
        // `%` / `_` 是 LIKE 的通配符：搜索词里的它们要当字面量（否则输个 `%` 就成「全选」）。
        let esc = v
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pat = format!("%{esc}%");
        sql.push_str(
            " AND (r.virtual_model LIKE ? ESCAPE '\\' OR r.real_model LIKE ? ESCAPE '\\' \
               OR r.request_id LIKE ? ESCAPE '\\')",
        );
        for _ in 0..3 {
            args.push(SqlValue::Text(pat.clone()));
        }
    }
    if let Some(cursor) = query.cursor {
        sql.push_str(" AND r.id < ?");
        args.push(SqlValue::Integer(cursor));
    }
    (sql, args)
}

/// 日志行 → 给界面的 JSON（字段名与原型 `GET /api/logs` 对齐，票 53 补 `id` / `request_id` / `cost_usd`）。
fn row_json(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "ts": r.get::<_, i64>(1)?,
        "request_id": r.get::<_, Option<String>>(2)?,
        "virtual_model": r.get::<_, Option<String>>(3)?,
        "client": r.get::<_, Option<String>>(4)?,
        "upstream": r.get::<_, Option<String>>(5)?,
        "real_model": r.get::<_, Option<String>>(6)?,
        "wire": r.get::<_, Option<String>>(7)?,
        "status": r.get::<_, Option<i64>>(8)?,
        "first_byte_ms": r.get::<_, Option<i64>>(9)?,
        "total_ms": r.get::<_, Option<i64>>(10)?,
        "input_tokens": r.get::<_, Option<i64>>(11)?,
        "output_tokens": r.get::<_, Option<i64>>(12)?,
        "cache_read": r.get::<_, Option<i64>>(13)?,
        "cache_write": r.get::<_, Option<i64>>(14)?,
        "aborted": r.get::<_, i64>(15)? != 0,
        "error": r.get::<_, Option<String>>(16)?,
        "cost_usd": r.get::<_, Option<f64>>(17)?,
        "body": r.get::<_, Option<String>>(18)?,
    }))
}

const ROW_COLUMNS: &str = "r.id,r.ts,r.request_id,r.virtual_model,r.client,r.upstream_id,r.real_model,r.wire,
       r.status,r.first_byte_ms,r.total_ms,r.input_tokens,r.output_tokens,r.cache_read,r.cache_write,
       r.aborted,r.error,r.cost_usd,b.request_body";

/// 日志查询（时间倒序）。`limit` 缺省 = [`DEFAULT_PAGE`]，上限 500（一页别把界面压死）。
pub async fn list(store: &Store, query: &LogQuery) -> Result<Vec<Value>> {
    let (cond, args) = where_of(query);
    let limit = query.limit.unwrap_or(DEFAULT_PAGE).min(500) as i64;
    let sql = format!(
        "SELECT {ROW_COLUMNS} FROM requests r
           LEFT JOIN request_bodies b ON b.request_id = r.id
          WHERE 1=1{cond}
          ORDER BY r.id DESC LIMIT ?"
    );
    store
        .read(move |c| {
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(
                rusqlite::params_from_iter(args.into_iter().chain([SqlValue::Integer(limit)])),
                row_json,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
}

/// 最近 `limit` 行（新到旧）。转发侧与回放测试读它，字段与 [`list`] 同一套。
pub async fn recent(store: &Store, limit: usize) -> Result<Vec<Value>> {
    list(
        store,
        &LogQuery {
            limit: Some(limit),
            ..Default::default()
        },
    )
    .await
}

/// 筛选下拉的候选值：**库里真出现过的值**（客户端 / 上游 / 虚拟模型）+ 固定的状态码四档。
pub async fn filters(store: &Store) -> Result<Value> {
    store
        .read(|c| {
            let col = |sql: &str| -> Result<Vec<String>> {
                let mut st = c.prepare(sql)?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            };
            Ok(json!({
                "clients": col("SELECT DISTINCT client FROM requests WHERE client IS NOT NULL ORDER BY client")?,
                "upstreams": col("SELECT DISTINCT upstream_id FROM requests WHERE upstream_id IS NOT NULL ORDER BY upstream_id")?,
                "virtual_models": col("SELECT DISTINCT virtual_model FROM requests WHERE virtual_model IS NOT NULL ORDER BY virtual_model")?,
                "statuses": STATUS_FILTERS,
            }))
        })
        .await
}

/// 状态码四档的取值域（与界面上的下拉逐字一致）。
pub const STATUS_FILTERS: [&str; 4] = ["ok", "4xx", "5xx", "error"];

/// 保留期（天）：读 `settings.log_retention_days`，读不到（老库 / 空值）回 [`DEFAULT_RETENTION_DAYS`]。
pub async fn retention_days(store: &Store) -> Result<i64> {
    let raw: Option<String> = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![settings_keys::LOG_RETENTION_DAYS],
                |r| r.get::<_, String>(0),
            )
            .ok())
        })
        .await?;
    Ok(raw
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS))
}

/// 清理超过保留期的请求行 + 它们的 body（`request_bodies` 靠外键级联），回删掉的条数。
pub async fn purge(store: &Store) -> Result<usize> {
    let days = retention_days(store).await?;
    let cutoff = unix_now() - days * 86_400;
    store
        .write(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute("DELETE FROM requests WHERE ts < ?1", params![cutoff])?;
            tx.commit()?;
            Ok(n)
        })
        .await
}

/// 清理表的句柄：挂在 [`crate::service::Gateway`] 上，网关停的时候一并 [`PurgeSweeper::stop`]。
///
/// 从前这里是裸 `tokio::spawn` 的死循环：每次 `start_gateway`（换端口 / 手动重启）再起一个、
/// `stop_gateway` 不停它 → N 个常驻循环清同一个库（评审「中」第 5 条）。
pub struct PurgeSweeper {
    handle: tokio::task::JoinHandle<()>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl PurgeSweeper {
    /// 一个与循环同生共死的观察 / 取消点（`stop` 之后问它 `is_finished()` 必然是 `true`）。
    pub fn abort_handle(&self) -> tokio::task::AbortHandle {
        self.handle.abort_handle()
    }

    /// 停掉这个清理循环并等它收尾（幂等：句柄只能被消费一次）。
    pub async fn stop(self) {
        let PurgeSweeper { handle, shutdown } = self;
        if let Some(tx) = shutdown {
            let _ = tx.send(());
        }
        let _ = handle.await;
    }
}

/// 起清理表：**立刻跑一次**（把上次退出后攒下的老行清掉），之后每小时一次；
/// 停不停由 [`PurgeSweeper`] 说了算（网关 `stop()` 时一并停）。
pub fn spawn_purge_sweeper(store: Store) -> PurgeSweeper {
    let (tx, mut shutdown) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        loop {
            match purge(&store).await {
                Ok(n) if n > 0 => println!("[obs] 清掉 {n} 条超过保留期的请求日志"),
                Ok(_) => {}
                Err(e) => eprintln!("[warn] 日志清理失败：{e}"),
            }
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(PURGE_INTERVAL) => {}
            }
        }
    });
    PurgeSweeper {
        handle,
        shutdown: Some(tx),
    }
}

// ───────────────────────── 用量汇总（概览与用量页共用一份） ─────────────────────────

/// 用量维度（票 33 第 6 节：归集口径是虚拟模型，上游与客户端只作另一维度）。
fn dim_column(dim: &str) -> Result<&'static str> {
    Ok(match dim {
        "virtual_model" => "r.virtual_model",
        "upstream" => "r.upstream_id",
        "client" => "r.client",
        other => bail!("未知的用量维度：{other}（virtual_model / upstream / client）"),
    })
}

/// 时间范围的窗口 `[from, to)`（Unix 秒；`to = None` = 一直到现在）。
///
/// 日边界按**本机本地时区自然日**（spec §9 / 票 33 第 5 节）：把本地墙上时间折算成 UTC 秒
/// 再取整到天。`custom` 的 `from` / `to` 是界面给的本地日期 `YYYY-MM-DD`，**`to` 含当天**
/// ——右端取下一天的起点（左闭右开，边界那一秒不会归错天）。
///
/// `offset_secs` 由调用方给（产品里是 [`local_offset_secs`]）：测试要能钉住一个固定时区。
pub fn range_window(
    range: &str,
    now: i64,
    offset_secs: i64,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<(i64, Option<i64>)> {
    let today = local_day_start(now, offset_secs);
    Ok(match range {
        "today" => (today, None),
        "7d" => (today - 6 * 86_400, None),
        "30d" => (today - 29 * 86_400, None),
        // 「全部」= 从纪元起（`requests.ts` 就是 Unix 秒，没有比 1970 更早的行）。
        "all" => (0, None),
        "custom" => {
            let from = from_date
                .filter(|d| !d.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("自定义区间要给开始日期 from（YYYY-MM-DD）"))?;
            let from = local_date_start(from, offset_secs)?;
            let to = match to_date.filter(|d| !d.trim().is_empty()) {
                Some(d) => Some(local_date_start(d, offset_secs)? + 86_400),
                None => None,
            };
            if let Some(to) = to {
                if to <= from {
                    bail!("自定义区间的结束日期早于开始日期");
                }
            }
            (from, to)
        }
        other => bail!("未知的时间范围：{other}（today / 7d / 30d / all / custom）"),
    })
}

/// `YYYY-MM-DD`（**本地**日期）→ 那天本地 0 点的 Unix 秒。
fn local_date_start(date: &str, offset_secs: i64) -> Result<i64> {
    let bad = || anyhow::anyhow!("日期要写成 YYYY-MM-DD：{date}");
    let mut it = date.trim().split('-');
    let y: i64 = it.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let m: i64 = it.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let d: i64 = it.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    Ok(days_from_civil(y, m, d) * 86_400 - offset_secs)
}

/// 上游没回 usage 的判据：**四项** token 全空 = 「token 未知」（票 33 第 3 节；票 58 把
/// `cache_write` 也算进来——上游只报了缓存写、别的都没报，同样不该说成「token 未知」的反面）。
const TOKEN_UNKNOWN_SQL: &str =
    "input_tokens IS NULL AND output_tokens IS NULL AND cache_read IS NULL AND cache_write IS NULL";

/// 分组 / 总计共用的八项聚合（顺序：花费、请求数、四项 token、无价格、token 未知）。
///
/// 「无价格」是**价格缺失**那一类；token 未知另算一格，两处不重复计（票 33 第 1 节的
/// 两个计数是两句不同的话），两者都**不计入总额**。
fn usage_aggregates() -> String {
    format!(
        "SUM(cost_usd), COUNT(*),
         COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read),0),
         COALESCE(SUM(cache_write),0),
         SUM(CASE WHEN cost_usd IS NULL AND NOT ({TOKEN_UNKNOWN_SQL}) THEN 1 ELSE 0 END),
         SUM(CASE WHEN {TOKEN_UNKNOWN_SQL} THEN 1 ELSE 0 END)"
    )
}

/// 八项聚合 → JSON（分组行与总计同一套字段）。
fn totals_json(r: &rusqlite::Row<'_>, base: usize) -> rusqlite::Result<Value> {
    Ok(json!({
        "cost_usd": r.get::<_, Option<f64>>(base)?,
        "requests": r.get::<_, i64>(base + 1)?,
        "input_tokens": r.get::<_, i64>(base + 2)?,
        "output_tokens": r.get::<_, i64>(base + 3)?,
        "cache_read": r.get::<_, i64>(base + 4)?,
        "cache_write": r.get::<_, i64>(base + 5)?,
        "no_price": r.get::<_, i64>(base + 6)?,
        "token_unknown": r.get::<_, i64>(base + 7)?,
    }))
}

/// 用量报表（票 59 的完整粒度）：五个时间范围 × 三个维度 + **按天明细** + 总计。
///
/// 概览要的那份（今日 / 近 7 天 × 两维）走的是**同一个实现**（[`usage_summary`] 是它的薄壳），
/// 免得概览与用量页各算一套（`.scratch/model-gateway/frontend.md` §③ 第 10 / 39 条的附注）。
pub async fn usage(
    store: &Store,
    range: &str,
    dim: &str,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<Value> {
    let col = dim_column(dim)?;
    let offset = local_offset_secs();
    let (from, to) = range_window(range, unix_now(), offset, from_date, to_date)?;
    // 右端缺省用 `i64::MAX`：一条 SQL 就够，不必按有没有右端拼两种语句。
    let upper = to.unwrap_or(i64::MAX);
    let agg = usage_aggregates();
    let rows_sql = format!(
        "SELECT {col} AS k, {agg} FROM requests r WHERE r.ts >= ?1 AND r.ts < ?2
          GROUP BY k ORDER BY (SUM(cost_usd) IS NULL), SUM(cost_usd) DESC, k"
    );
    let totals_sql = format!("SELECT {agg} FROM requests r WHERE r.ts >= ?1 AND r.ts < ?2");
    // 按天：`ts + 本地偏移` 落到「本地自然日序号」，再折回那天的 0 点（左闭右开，跨时区不串天）。
    let daily_sql = format!(
        "SELECT ((r.ts + ?3) / 86400) AS d, {agg} FROM requests r WHERE r.ts >= ?1 AND r.ts < ?2
          GROUP BY d ORDER BY d"
    );

    let (rows, totals, daily) = store
        .read(move |c| {
            let mut st = c.prepare(&totals_sql)?;
            let totals = st.query_row(params![from, upper], |r| totals_json(r, 0))?;
            let mut st = c.prepare(&rows_sql)?;
            let rows = st
                .query_map(params![from, upper], |r| {
                    let mut v = totals_json(r, 1)?;
                    v["key"] = json!(r.get::<_, Option<String>>(0)?);
                    Ok(v)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut st = c.prepare(&daily_sql)?;
            let daily = st
                .query_map(params![from, upper, offset], |r| {
                    let day: i64 = r.get(0)?;
                    let mut v = totals_json(r, 1)?;
                    let (y, m, d) = civil_from_days(day);
                    v["day"] = json!(format!("{y:04}-{m:02}-{d:02}"));
                    Ok(v)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, totals, daily))
        })
        .await?;

    Ok(json!({
        "range": range,
        "dim": dim,
        "from": from,
        "to": to,
        "totals": totals,
        "rows": rows,
        "daily": daily,
    }))
}

/// 概览要的那半（票 53 的接口不动）：今日 / 近 7 天 × 虚拟模型 / 上游。
/// 票 59 起 `range` 也认 `30d` / `all` / `custom`——两边共用 [`usage`] 一份实现。
pub async fn usage_summary(store: &Store, range: &str, dim: &str) -> Result<Value> {
    usage(store, range, dim, None, None).await
}

/// 库内一律美元为锚，界面按**当前汇率**换算（票 33 第 2 节）。回 `None` = 库里没这一项。
pub async fn usd_cny_rate(store: &Store) -> Result<Option<f64>> {
    Ok(store
        .read(|c| {
            Ok(c.query_row(
                "SELECT value FROM settings WHERE key=?1",
                params![settings_keys::USD_CNY_RATE],
                |r| r.get::<_, String>(0),
            )
            .ok())
        })
        .await?
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|r| *r > 0.0))
}

// ───────────────────────── 时间 ─────────────────────────

/// 请求 ID：`req-<微秒><进程内序号>`。唯一即可（详情页按它取、日志页按它搜），不做 UUID 依赖。
fn next_request_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    format!("req-{micros:012x}{:04x}", n & 0xffff)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 本地自然日的起点（Unix 秒）：把「本地墙上时间」折算成 UTC 秒再取整到天，最后折回 UTC。
fn local_day_start(now: i64, offset_secs: i64) -> i64 {
    (now + offset_secs).div_euclid(86_400) * 86_400 - offset_secs
}

/// 本机时区相对 UTC 的偏移秒数（东八区 = +28800）。
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    sys_time_secs(&local) - sys_time_secs(&utc)
}

/// 非 Windows（只为编译通过）：本地即 UTC。
#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

/// `SYSTEMTIME` → Unix 秒（秒精度足够：日边界不会因为毫秒抖动而挪一天）。
#[cfg(windows)]
fn sys_time_secs(st: &windows_sys::Win32::Foundation::SYSTEMTIME) -> i64 {
    days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * 86_400
        + st.wHour as i64 * 3_600
        + st.wMinute as i64 * 60
        + st.wSecond as i64
}

/// 公历年月日 → 1970-01-01 起的天数（Howard Hinnant 的 `days_from_civil`，与 `api::rfc3339_utc`
/// 里的 `civil_from_days` 是一对）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 起的天数 → 公历年月日（`days_from_civil` 的逆；按天明细的日期串用它）。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

// ───────────────────────── 详情四段与清空（票 59） ─────────────────────────

/// 详情里的那一行（元数据；字段与日志列表同一套形状）。
const DETAIL_COLUMNS: &str = "r.id,r.ts,r.request_id,r.virtual_model,r.client,r.upstream_id,r.real_model,
     r.wire,r.status,r.first_byte_ms,r.total_ms,r.input_tokens,r.output_tokens,r.cache_read,r.cache_write,
     r.aborted,r.error,r.cost_usd";

/// 一条请求的详情三段（**概要 / 请求与响应体 / 帧时间轴**；第四段「错误信息」由管理面
/// 在 `error` 上补模板，模板口径与概览「最近错误」共用一份）。
///
/// `id` 认两种写法：纯数字 = 行 id（界面跳转用的那把钥匙），其余 = `request_id`。
/// 找不到回 `Ok(None)`（管理面翻成 404，不是空详情）。
pub async fn log_detail(store: &Store, id: &str) -> Result<Option<Value>> {
    let key = id.trim().to_string();
    let by_row_id = key.parse::<i64>().ok();
    let detail = store
        .read(move |c| {
            use rusqlite::OptionalExtension;
            let sql = format!(
                "SELECT {DETAIL_COLUMNS} FROM requests r WHERE {} ORDER BY r.id DESC LIMIT 1",
                if by_row_id.is_some() {
                    "r.id = ?1"
                } else {
                    "r.request_id = ?1"
                }
            );
            let lookup: SqlValue = match by_row_id {
                Some(n) => SqlValue::Integer(n),
                None => SqlValue::Text(key.clone()),
            };
            let row = c
                .query_row(&sql, params![lookup], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "ts": r.get::<_, i64>(1)?,
                        "request_id": r.get::<_, Option<String>>(2)?,
                        "virtual_model": r.get::<_, Option<String>>(3)?,
                        "client": r.get::<_, Option<String>>(4)?,
                        "upstream": r.get::<_, Option<String>>(5)?,
                        "real_model": r.get::<_, Option<String>>(6)?,
                        "wire": r.get::<_, Option<String>>(7)?,
                        "status": r.get::<_, Option<i64>>(8)?,
                        "first_byte_ms": r.get::<_, Option<i64>>(9)?,
                        "total_ms": r.get::<_, Option<i64>>(10)?,
                        "input_tokens": r.get::<_, Option<i64>>(11)?,
                        "output_tokens": r.get::<_, Option<i64>>(12)?,
                        "cache_read": r.get::<_, Option<i64>>(13)?,
                        "cache_write": r.get::<_, Option<i64>>(14)?,
                        "aborted": r.get::<_, i64>(15)? != 0,
                        "error": r.get::<_, Option<String>>(16)?,
                        "cost_usd": r.get::<_, Option<f64>>(17)?,
                    }))
                })
                .optional()?;
            let Some(row) = row else {
                return Ok(None);
            };
            let row_id = row["id"].as_i64().unwrap_or(0);
            let bodies = c
                .query_row(
                    "SELECT request_body,response_body FROM request_bodies WHERE request_id=?1",
                    params![row_id],
                    |r| {
                        Ok(json!({
                            "request": r.get::<_, Option<String>>(0)?,
                            "response": r.get::<_, Option<String>>(1)?,
                        }))
                    },
                )
                .optional()?
                .unwrap_or_else(|| json!({"request": null, "response": null}));
            let mut st = c.prepare(
                "SELECT seq,at_ms,event,summary,raw FROM request_frames
                  WHERE request_id=?1 ORDER BY seq",
            )?;
            let frames = st
                .query_map(params![row_id], |r| {
                    Ok(json!({
                        "seq": r.get::<_, i64>(0)?,
                        "at_ms": r.get::<_, i64>(1)?,
                        "event": r.get::<_, Option<String>>(2)?,
                        "summary": r.get::<_, String>(3)?,
                        "raw": r.get::<_, Option<String>>(4)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Some(json!({
                "summary": row,
                "bodies": bodies,
                "frames": frames,
            })))
        })
        .await?;
    Ok(detail)
}

/// 清空日志（票 59）：删 `requests`（`request_bodies` / `request_frames` 靠外键级联），
/// 回删掉的请求行数。**二次确认是界面的事**，这里只认「调用方已经确认过」。
pub async fn clear_logs(store: &Store) -> Result<usize> {
    store
        .write(|c| {
            let tx = c.transaction()?;
            let n = tx.execute("DELETE FROM requests", [])?;
            tx.commit()?;
            Ok(n)
        })
        .await
}
