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
    pub aborted: bool,
    pub error: Option<String>,
    /// 出站请求体：只有 body 日志开关开着时才是 `Some`。
    pub body: Option<String>,
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
                                      first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,
                                      aborted,error,cost_usd)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
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
                    row.aborted as i64,
                    row.error,
                    cost,
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

/// 这一行按「当时价」该记多少美元；算不出来就是 `None`（**不记 0**，票 33 第 1 节）。
///
/// 价格读失败（库抖动）也退成 `None`：宁可这一行标「无价格」，也不让日志写不进去。
fn cost_for(c: &Connection, row: &RequestLog) -> Option<f64> {
    let prices = provider::resolved_prices_from(c, &row.upstream_id, &row.real_model).ok()?;
    resolve_cost(&prices, row.input_tokens, row.output_tokens, row.cache_read)
}

/// 成本口径（票 33 第 1、3、4 节），纯函数好断言：
///
/// - `token 未知`（三项全空）→ `None`；**不用本地估算反算**。
/// - 缺价 → 整条 `None`（**不计入总额**，不半算、不记 0）。
/// - `cache_read` 缺缓存读价则**退输入价**；`cache_write` 本期不计（票 58 补列后再算）。
pub fn resolve_cost(
    prices: &ResolvedPrices,
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
) -> Option<f64> {
    if input.is_none() && output.is_none() && cache_read.is_none() {
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
        + part(cache_read, cache_price)?;
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
        "aborted": r.get::<_, i64>(14)? != 0,
        "error": r.get::<_, Option<String>>(15)?,
        "cost_usd": r.get::<_, Option<f64>>(16)?,
        "body": r.get::<_, Option<String>>(17)?,
    }))
}

const ROW_COLUMNS: &str = "r.id,r.ts,r.request_id,r.virtual_model,r.client,r.upstream_id,r.real_model,r.wire,
       r.status,r.first_byte_ms,r.total_ms,r.input_tokens,r.output_tokens,r.cache_read,r.aborted,r.error,
       r.cost_usd,b.request_body";

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

/// 起清理表：**立刻跑一次**（把上次退出后攒下的老行清掉），之后每小时一次。
///
/// 它只做一件事、出错只报一行，所以不占网关的关停路径：网关停了它跟着 runtime 走。
pub fn spawn_purge_sweeper(store: Store) {
    tokio::spawn(async move {
        loop {
            match purge(&store).await {
                Ok(n) if n > 0 => println!("[obs] 清掉 {n} 条超过保留期的请求日志"),
                Ok(_) => {}
                Err(e) => eprintln!("[warn] 日志清理失败：{e}"),
            }
            tokio::time::sleep(PURGE_INTERVAL).await;
        }
    });
}

// ───────────────────────── 用量汇总（概览要的那半） ─────────────────────────

/// 用量维度（票 33 第 6 节：归集口径是虚拟模型，上游与客户端只作另一维度）。
fn dim_column(dim: &str) -> Result<&'static str> {
    Ok(match dim {
        "virtual_model" => "r.virtual_model",
        "upstream" => "r.upstream_id",
        "client" => "r.client",
        other => bail!("未知的用量维度：{other}"),
    })
}

/// 时间范围起点（Unix 秒，本地自然日边界）。**票 53 只做概览要的今日 / 近 7 天**，
/// 近 30 天 / 全部 / 自定义区间与按天明细归票 59（`/api/usage`）。
fn range_start(range: &str, now: i64) -> Result<i64> {
    let today = local_day_start(now, local_offset_secs());
    Ok(match range {
        "today" => today,
        "7d" => today - 6 * 86_400,
        other => bail!("本端点只做 today / 7d（其余范围归票 59）：{other}"),
    })
}

/// 上游没回 usage 的判据：三项 token 全空 = 「token 未知」（票 33 第 3 节）。
const TOKEN_UNKNOWN_SQL: &str =
    "input_tokens IS NULL AND output_tokens IS NULL AND cache_read IS NULL";

/// 概览要的用量汇总：范围内按维度分组的花费 / 请求数 / token + 总计，
/// 另计「N 次无价格 / N 次 token 未知」（两者都**不计入总额**，票 33 第 1 节）。
pub async fn usage_summary(store: &Store, range: &str, dim: &str) -> Result<Value> {
    let col = dim_column(dim)?;
    let from = range_start(range, unix_now())?;
    // 「无价格」是**价格缺失**那一类；token 未知另算一格，两处不重复计（票 33 第 1 节的
    // 两个计数是两句不同的话）。
    let aggregates = format!(
        "SUM(cost_usd), COUNT(*),
         COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read),0),
         SUM(CASE WHEN cost_usd IS NULL AND NOT ({TOKEN_UNKNOWN_SQL}) THEN 1 ELSE 0 END),
         SUM(CASE WHEN {TOKEN_UNKNOWN_SQL} THEN 1 ELSE 0 END)"
    );
    let rows_sql = format!(
        "SELECT {col} AS k, {aggregates}
           FROM requests r WHERE r.ts >= ?1
          GROUP BY k
          ORDER BY (SUM(cost_usd) IS NULL), SUM(cost_usd) DESC, k"
    );
    let totals_sql = format!("SELECT {aggregates} FROM requests r WHERE r.ts >= ?1");
    let (rows_sql, totals_sql) = (rows_sql, totals_sql);

    let (rows, totals) = store
        .read(move |c| {
            let read_totals = |sql: &str| -> Result<Value> {
                let mut st = c.prepare(sql)?;
                Ok(st.query_row(params![from], |r| {
                    Ok(json!({
                        "cost_usd": r.get::<_, Option<f64>>(0)?,
                        "requests": r.get::<_, i64>(1)?,
                        "input_tokens": r.get::<_, i64>(2)?,
                        "output_tokens": r.get::<_, i64>(3)?,
                        "cache_read": r.get::<_, i64>(4)?,
                        "no_price": r.get::<_, i64>(5)?,
                        "token_unknown": r.get::<_, i64>(6)?,
                    }))
                })?)
            };
            let mut st = c.prepare(&rows_sql)?;
            let rows = st
                .query_map(params![from], |r| {
                    Ok(json!({
                        "key": r.get::<_, Option<String>>(0)?,
                        "cost_usd": r.get::<_, Option<f64>>(1)?,
                        "requests": r.get::<_, i64>(2)?,
                        "input_tokens": r.get::<_, i64>(3)?,
                        "output_tokens": r.get::<_, i64>(4)?,
                        "cache_read": r.get::<_, i64>(5)?,
                        "no_price": r.get::<_, i64>(6)?,
                        "token_unknown": r.get::<_, i64>(7)?,
                    }))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows, read_totals(&totals_sql)?))
        })
        .await?;

    Ok(json!({
        "range": range,
        "dim": dim,
        "from": from,
        "rows": rows,
        "totals": totals,
    }))
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
