//! obs 的单测：成本冻结 / 价格口径 / 保留期清理 / 日志查询与筛选 / 用量汇总 / body 窗口。
//!
//! 一律指向临时目录（写入根参数化）：**不碰真实 `%APPDATA%\com.claudegateway.desktop\`**。
//! 期望值取「手算的已知字面量」——不用被测函数反算（票 33 的三个口径：手工 > 快照 > 未知、
//! 缺价不计入总额、上游没回 usage = token 未知）。

use super::*;
use crate::egress::provider::{self, Prices, ResolvedPrice, ResolvedPrices};
use crate::store::Store;
use std::path::PathBuf;

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg53-obs-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// 一句筛选条件 → 一份 [`LogQuery`]（测试里读起来像那句话本身）。
fn query(f: impl FnOnce(&mut LogQuery)) -> LogQuery {
    let mut q = LogQuery::default();
    f(&mut q);
    q
}

/// 三项价都已知的解：输入 $3/Mtok、输出 $15/Mtok、缓存读 $0.30/Mtok。
fn prices_known() -> ResolvedPrices {
    let p = |v: f64| ResolvedPrice {
        usd_per_mtok: Some(v),
        source: provider::PriceSource::Manual,
    };
    ResolvedPrices {
        input: p(3.0),
        output: p(15.0),
        cache_read: p(0.30),
        cache_write: ResolvedPrice {
            usd_per_mtok: None,
            source: provider::PriceSource::Unknown,
        },
    }
}

fn prices_unknown() -> ResolvedPrices {
    let u = || ResolvedPrice {
        usd_per_mtok: None,
        source: provider::PriceSource::Unknown,
    };
    ResolvedPrices {
        input: u(),
        output: u(),
        cache_read: u(),
        cache_write: u(),
    }
}

fn row(upstream: &str, model: &str, ts: i64) -> RequestLog {
    RequestLog {
        ts,
        virtual_model: "claude-haiku".into(),
        client: Some("claude-code".into()),
        upstream_id: upstream.into(),
        real_model: model.into(),
        wire: "anthropic".into(),
        status: 200,
        input_tokens: Some(1000),
        output_tokens: Some(500),
        cache_read: None,
        total_ms: 120,
        first_byte_ms: 40,
        ..Default::default()
    }
}

/// 一笔请求的总价：输入 1000 tok × $3/Mtok = $0.003，输出 500 × $15/Mtok = $0.0075，
/// 缓存读 2000 × $0.30/Mtok = $0.0006，合计 $0.0111（手算的独立来源，不是本函数算的）。
#[test]
fn cost_is_sum_of_the_three_parts_at_known_prices() {
    let c = resolve_cost(&prices_known(), Some(1000), Some(500), Some(2000), None).unwrap();
    assert!((c - 0.0111).abs() < 1e-12, "实际 {c}");

    // 没有缓存读 → 只有前两项
    let c = resolve_cost(&prices_known(), Some(1000), Some(500), None, None).unwrap();
    assert!((c - 0.0105).abs() < 1e-12, "实际 {c}");
}

/// 缓存读缺价 → **退输入价**（票 33 第 4 节）；缓存写不在这条路径上（没这一项 token）。
#[test]
fn cache_read_falls_back_to_input_price_when_it_has_none() {
    let mut p = prices_known();
    p.cache_read.usd_per_mtok = None;
    let c = resolve_cost(&p, Some(0), Some(0), Some(2000), None).unwrap();
    // 2000 × $3/Mtok = $0.006
    assert!((c - 0.006).abs() < 1e-12, "实际 {c}");
}

/// 票 58：缓存写按**缓存写价**单独算，四项相加。
///
/// 手算（独立来源，帧文件 `tests/fixtures/cache-write/anthropic.sse` 那份真录请求的形状）：
/// 输入 1000 × $3/Mtok = $0.003；输出 500 × $15/Mtok = $0.0075；
/// 缓存读 2000 × $0.30/Mtok = $0.0006；缓存写 4000 × $3.75/Mtok = $0.015 → 合计 **$0.0261**。
#[test]
fn cost_adds_cache_write_at_its_own_price() {
    let mut p = prices_known();
    p.cache_write = ResolvedPrice {
        usd_per_mtok: Some(3.75),
        source: provider::PriceSource::Manual,
    };
    let c = resolve_cost(&p, Some(1000), Some(500), Some(2000), Some(4000)).unwrap();
    assert!((c - 0.0261).abs() < 1e-12, "实际 {c}");
}

/// 票 58：缓存写**没有缓存写价**就是缺价 → 整条不算（票 33 第 4 节只给缓存读留了「退输入价」，
/// 缓存写没有退路，不许拿输入价顶替）。
#[test]
fn cache_write_without_its_price_leaves_the_row_unpriced() {
    let p = prices_known(); // cache_write 价是 Unknown
    assert_eq!(
        resolve_cost(&p, Some(1000), Some(500), None, Some(4000)),
        None,
        "缺缓存写价不能拿输入价顶替，也不能半算"
    );
    // 上游报了 0 个缓存写 token（不是 None）→ 这一项不拖累整条。
    let c = resolve_cost(&p, Some(1000), Some(500), None, Some(0)).unwrap();
    assert!((c - 0.0105).abs() < 1e-12, "实际 {c}");
}

/// 口径两条（票 33 第 1、3 节）：缺价 → 整个 `cost_usd` 留空、不计入总额；
/// 上游没回 usage（token 全空）→ 同样留空，不拿本地估算反算。
#[test]
fn unknown_price_or_unknown_tokens_yield_no_cost() {
    assert_eq!(
        resolve_cost(&prices_unknown(), Some(1000), Some(500), None, None),
        None,
        "一分钱都算不出来时不该编一个 0"
    );
    assert_eq!(
        resolve_cost(&prices_known(), None, None, None, None),
        None,
        "token 未知就不算成本"
    );
    // 只缺输出价：这一笔也整条不计（不半算）
    let mut p = prices_known();
    p.output.usd_per_mtok = None;
    assert_eq!(resolve_cost(&p, Some(1000), Some(500), None, None), None);
}

/// 到点自关：窗口拨短，过了就是关。
#[test]
fn body_window_closes_by_itself() {
    let w = BodyLogWindow::new();
    assert!(!w.on(), "默认关");
    assert_eq!(w.seconds_remaining(), None);

    w.set(60);
    assert!(w.on());
    assert!(w
        .seconds_remaining()
        .map(|s| s <= 60 && s > 58)
        .unwrap_or(false));

    w.set(0);
    assert!(!w.on(), "0 = 立即关");
}

/// 日边界是**本机本地时区自然日**（票 33 第 5 节 / spec §9）：今天的起点 = 本地 00:00。
#[test]
fn day_boundaries_use_local_civil_days() {
    // 独立来源：1970-01-01 是第 0 天，2000-02-29 是第 11016 天（951782400 / 86400）。
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 2, 29), 11_016);

    // 本地 00:00 的 UTC 秒：对 1e9 这个瞬间（2001-09-09T01:46:40Z）取本地日界，
    // 结果必须落在本地日界上：把日界加回偏移再取商，应为整天。
    let off = local_offset_secs();
    let start = local_day_start(1_000_000_000, off);
    assert_eq!(
        (start + off).rem_euclid(86_400),
        0,
        "日界必须落在本地 00:00"
    );
    assert!(start <= 1_000_000_000 && 1_000_000_000 - start < 86_400);
}

/// 冻结：落库那一刻按当时价算好；之后改手工价或刷新快照，**历史行不动**（票 33 第 3 节）。
#[tokio::test]
async fn cost_is_frozen_at_write_time_and_not_recomputed() {
    let root = tmp_root("freeze");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    provider::save_manual_model(&store, "u1", "real-model", &[])
        .await
        .unwrap();
    provider::set_manual_prices(
        &store,
        "u1",
        "real-model",
        &Prices {
            input: Some(3.0),
            output: Some(15.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut r = row("u1", "real-model", 1_000_000_000);
    r.cache_read = None;
    log(&store, &r).await.unwrap();
    let before = list(&store, &LogQuery::default()).await.unwrap();
    let cost_before = before[0]["cost_usd"].as_f64().unwrap();
    assert!((cost_before - 0.0105).abs() < 1e-12, "实际 {cost_before}");

    // 改手工价（涨 10 倍）+ 换快照，历史行必须一动不动。
    provider::set_manual_prices(
        &store,
        "u1",
        "real-model",
        &Prices {
            input: Some(30.0),
            output: Some(150.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    provider::import_catalog_snapshot(
        &store,
        r#"{"u1":{"models":{"real-model":{"name":"x","cost":{"input":99,"output":99}}}}}"#,
    )
    .await
    .unwrap();

    let after = list(&store, &LogQuery::default()).await.unwrap();
    assert_eq!(
        after[0]["cost_usd"].as_f64().unwrap(),
        cost_before,
        "改价 / 刷新快照不许追古"
    );

    // 同一条上游的同一条模型，下一次请求按新价（说明价是「当时」读的，不是启动时缓存的）。
    log(&store, &row("u1", "real-model", 1_000_000_100))
        .await
        .unwrap();
    let rows = list(&store, &LogQuery::default()).await.unwrap();
    let fresh = rows[0]["cost_usd"].as_f64().unwrap();
    assert!(
        (fresh - 0.105).abs() < 1e-12,
        "新价下应为 $0.105，实际 {fresh}"
    );
}

/// 票 58：带缓存写的一行落库并计入成本，且**改价不追古**（历史行的数值一动不动）。
///
/// 手算（独立来源）：输入 1000 × $3/Mtok = $0.003；输出 500 × $15/Mtok = $0.0075；
/// 缓存写 4000 × $3.75/Mtok = $0.015 → **$0.0255**。四项价全抬 10 倍后，新的一发是 **$0.255**。
#[tokio::test]
async fn cache_write_lands_in_the_db_with_cost_and_is_never_recomputed() {
    let root = tmp_root("freeze-cache-write");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    provider::save_manual_model(&store, "u1", "real-model", &[])
        .await
        .unwrap();
    provider::set_manual_prices(
        &store,
        "u1",
        "real-model",
        &Prices {
            input: Some(3.0),
            output: Some(15.0),
            cache_read: Some(0.30),
            cache_write: Some(3.75),
        },
    )
    .await
    .unwrap();

    let mut r = row("u1", "real-model", 1_000_000_000);
    r.cache_write = Some(4000);
    log(&store, &r).await.unwrap();

    let before = list(&store, &LogQuery::default()).await.unwrap();
    assert_eq!(before[0]["cache_write"], json!(4000), "缓存写必须落库");
    assert_eq!(before[0]["cache_read"], Value::Null, "没报的项留 NULL");
    let cost_before = before[0]["cost_usd"].as_f64().unwrap();
    assert!((cost_before - 0.0255).abs() < 1e-12, "实际 {cost_before}");

    // 四项价全抬 10 倍 + 换一份快照：历史行必须一动不动（票 33 第 3 节）。
    provider::set_manual_prices(
        &store,
        "u1",
        "real-model",
        &Prices {
            input: Some(30.0),
            output: Some(150.0),
            cache_read: Some(3.0),
            cache_write: Some(37.5),
        },
    )
    .await
    .unwrap();
    provider::import_catalog_snapshot(
        &store,
        r#"{"u1":{"models":{"real-model":{"name":"x","cost":{"input":99,"output":99,"cache_write":99}}}}}"#,
    )
    .await
    .unwrap();

    let after = list(&store, &LogQuery::default()).await.unwrap();
    assert_eq!(
        after[0]["cost_usd"].as_f64().unwrap(),
        cost_before,
        "改价 / 刷新快照不许追古"
    );

    // 同一条上游的同一条模型，下一发按新价（说明价是「当时」读的）。
    let mut r2 = row("u1", "real-model", 1_000_000_100);
    r2.cache_write = Some(4000);
    log(&store, &r2).await.unwrap();
    let rows = list(&store, &LogQuery::default()).await.unwrap();
    let fresh = rows[0]["cost_usd"].as_f64().unwrap();
    assert!(
        (fresh - 0.255).abs() < 1e-12,
        "新价下应为 $0.255，实际 {fresh}"
    );
    assert_eq!(rows[0]["cache_write"], json!(4000), "新行也带着缓存写");
}

/// 快照兜底：手工没填的项回退 models.dev 快照（票 33 第 1 节）。
#[tokio::test]
async fn snapshot_fills_prices_that_manual_leaves_empty() {
    let root = tmp_root("snapshot");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    provider::save_manual_model(&store, "u1", "real-model", &[])
        .await
        .unwrap();
    provider::import_catalog_snapshot(
        &store,
        r#"{"u1":{"models":{"real-model":{"name":"x","cost":{"input":3,"output":15}}}}}"#,
    )
    .await
    .unwrap();

    let mut r = row("u1", "real-model", 1_000_000_000);
    r.cache_read = None;
    log(&store, &r).await.unwrap();
    let rows = list(&store, &LogQuery::default()).await.unwrap();
    let c = rows[0]["cost_usd"].as_f64().unwrap();
    assert!((c - 0.0105).abs() < 1e-12, "实际 {c}");
}

/// 每一行都有自己的请求 ID（日志页按它搜、详情页按它取），且不重复。
#[tokio::test]
async fn every_row_gets_a_unique_request_id() {
    let root = tmp_root("request-id");
    let store = Store::open(&root).unwrap();
    for i in 0..5 {
        log(&store, &row("u1", "m", 1_000_000_000 + i))
            .await
            .unwrap();
    }
    let rows = list(&store, &LogQuery::default()).await.unwrap();
    let ids: Vec<String> = rows
        .iter()
        .map(|r| r["request_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 5);
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 5, "请求 ID 不该重复：{ids:?}");
    assert!(ids.iter().all(|i| i.starts_with("req-")), "{ids:?}");
}

/// 清理表必须能被停掉（评审「中」第 5 条）：从前它是无取消句柄的 `tokio::spawn` 死循环，
/// 每次 `start_gateway` 再起一个、`stop_gateway` 不停它 → N 个常驻循环清同一个库。
#[tokio::test]
async fn the_purge_sweeper_stops_when_told_to() {
    let root = tmp_root("purge-sweeper");
    let store = Store::open(&root).unwrap();

    let sweeper = spawn_purge_sweeper(store);
    let probe = sweeper.abort_handle();
    assert!(!probe.is_finished(), "刚起的清理循环不该已经结束");

    sweeper.stop().await;

    assert!(
        probe.is_finished(),
        "收到关停之后清理循环必须结束（否则每次重启多一个常驻循环）"
    );
}

/// 保留期：8 天前的行清掉，7 天内的留着；body 跟着行一起走（外键级联）。
#[tokio::test]
async fn purge_drops_rows_past_retention_and_keeps_recent_ones() {
    let root = tmp_root("purge");
    let store = Store::open(&root).unwrap();
    let now = unix_now();
    let old = now - 8 * 86_400;
    let recent = now - 6 * 86_400;

    let mut with_body = row("u1", "m", old);
    with_body.body = Some("{\"secret\":\"body\"}".into());
    log(&store, &with_body).await.unwrap();
    log(&store, &row("u1", "m", recent)).await.unwrap();
    assert_eq!(list(&store, &LogQuery::default()).await.unwrap().len(), 2);

    let deleted = purge(&store).await.unwrap();
    assert_eq!(deleted, 1, "只该清掉那 1 条 8 天前的行");

    let rows = list(&store, &LogQuery::default()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ts"].as_i64().unwrap(), recent, "7 天内的还在");

    // body 行也不该留下（`request_bodies` 随 `requests` 级联删）。
    let left: i64 = store
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM request_bodies", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(left, 0, "清掉的行不该留着 body");
}

/// 保留期读 `settings.log_retention_days`（设置页那一项，spec §9），改了就按新的清。
#[tokio::test]
async fn retention_days_come_from_settings() {
    let root = tmp_root("retention-setting");
    let store = Store::open(&root).unwrap();
    assert_eq!(
        retention_days(&store).await.unwrap(),
        7,
        "库里的初值是 7 天"
    );

    store
        .write(|c| {
            c.execute(
                "UPDATE settings SET value='30' WHERE key=?1",
                rusqlite::params![settings_keys::LOG_RETENTION_DAYS],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(retention_days(&store).await.unwrap(), 30);
}

/// 筛选与搜索：四个维度各自可用、模型名与请求 ID 可搜、**不搜 body**、时间倒序、可分页。
#[tokio::test]
async fn query_filters_search_and_order_by_time_desc() {
    let root = tmp_root("query");
    let store = Store::open(&root).unwrap();

    let mut a = row("u1", "real-a", 1_000_000_000);
    a.virtual_model = "claude-haiku".into();
    a.client = Some("claude-code".into());
    a.status = 200;
    let mut b = row("u2", "real-b", 1_000_000_100);
    b.virtual_model = "claude-opus".into();
    b.client = Some("codex".into());
    b.status = 502;
    b.error = Some("上游 502".into());
    let mut c = row("u1", "real-a", 1_000_000_200);
    c.virtual_model = "claude-sonnet".into();
    c.client = Some("claude-code".into());
    c.status = 429;
    c.body = Some("{\"marker\":\"body-only-marker\"}".into());
    for r in [&a, &b, &c] {
        log(&store, r).await.unwrap();
    }

    let ids: Vec<i64> = list(&store, &LogQuery::default())
        .await
        .unwrap()
        .iter()
        .map(|r| r["ts"].as_i64().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![1_000_000_200, 1_000_000_100, 1_000_000_000],
        "时间倒序"
    );

    let n = |v: Vec<Value>| v.len();

    assert_eq!(
        n(list(&store, &query(|q| q.client = Some("codex".into())))
            .await
            .unwrap()),
        1
    );
    assert_eq!(
        n(list(&store, &query(|q| q.upstream = Some("u1".into())))
            .await
            .unwrap()),
        2
    );
    assert_eq!(
        n(list(
            &store,
            &query(|q| q.virtual_model = Some("claude-opus".into()))
        )
        .await
        .unwrap()),
        1
    );
    assert_eq!(
        n(list(&store, &query(|q| q.status = Some("ok".into())))
            .await
            .unwrap()),
        1
    );
    assert_eq!(
        n(list(&store, &query(|q| q.status = Some("4xx".into())))
            .await
            .unwrap()),
        1
    );
    assert_eq!(
        n(list(&store, &query(|q| q.status = Some("5xx".into())))
            .await
            .unwrap()),
        1
    );
    assert_eq!(
        n(list(&store, &query(|q| q.status = Some("error".into())))
            .await
            .unwrap()),
        2
    );
    assert_eq!(
        n(list(&store, &query(|q| q.q = Some("real-a".into())))
            .await
            .unwrap()),
        2
    );
    assert_eq!(
        n(list(&store, &query(|q| q.q = Some("claude-sonnet".into())))
            .await
            .unwrap()),
        1
    );
    assert_eq!(
        n(
            list(&store, &query(|q| q.q = Some("body-only-marker".into())))
                .await
                .unwrap()
        ),
        0,
        "搜索不搜 body"
    );

    // 请求 ID 可搜
    let want = list(&store, &LogQuery::default()).await.unwrap()[0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        n(list(&store, &query(|q| q.q = Some(want))).await.unwrap()),
        1
    );

    // 分页：取 2 条，用最后一行的 id 当游标再取剩下 1 条
    let page1 = list(&store, &query(|q| q.limit = Some(2))).await.unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = page1[1]["id"].as_i64().unwrap();
    let page2 = list(
        &store,
        &query(|q| {
            q.limit = Some(2);
            q.cursor = Some(cursor);
        }),
    )
    .await
    .unwrap();
    assert_eq!(page2.len(), 1, "游标该把翻过的行挡在外面");
    assert_eq!(page2[0]["ts"].as_i64().unwrap(), 1_000_000_000);
}

/// 筛选下拉的候选值来自**库里真出现过的值**，不是写死的清单。
#[tokio::test]
async fn filter_candidates_come_from_stored_rows() {
    let root = tmp_root("filters");
    let store = Store::open(&root).unwrap();
    log(&store, &row("u1", "m", 1_000_000_000)).await.unwrap();
    let mut b = row("u2", "m", 1_000_000_100);
    b.client = Some("codex".into());
    b.virtual_model = "claude-opus".into();
    log(&store, &b).await.unwrap();

    let f = filters(&store).await.unwrap();
    assert_eq!(f["upstreams"], json!(["u1", "u2"]));
    assert_eq!(f["clients"], json!(["claude-code", "codex"]));
    assert_eq!(f["virtual_models"], json!(["claude-haiku", "claude-opus"]));
    assert_eq!(f["statuses"], json!(["ok", "4xx", "5xx", "error"]));
}

/// 用量汇总（票 53 只做概览要的**今日 + 近 7 天** × 虚拟模型 / 上游两维）：
/// 总额只算算得出价的那些，无价与 token 未知**各自另计**（票 33 第 1 节）。
#[tokio::test]
async fn usage_summary_counts_unpriced_and_unknown_tokens_separately() {
    let root = tmp_root("usage");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    provider::save_manual_model(&store, "u1", "priced", &[])
        .await
        .unwrap();
    provider::set_manual_prices(
        &store,
        "u1",
        "priced",
        &Prices {
            input: Some(3.0),
            output: Some(15.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let now = unix_now();
    // ① 今天、有价：$0.0105
    let mut a = row("u1", "priced", now - 60);
    a.cache_read = None;
    log(&store, &a).await.unwrap();
    // ② 今天、无价（目录里没有 → 未知价），token 有
    let mut b = row("u1", "unpriced", now - 120);
    b.cache_read = None;
    b.virtual_model = "claude-opus".into();
    log(&store, &b).await.unwrap();
    // ③ 今天、有价但上游没回 usage → token 未知，成本不计
    let mut c = row("u1", "priced", now - 180);
    c.input_tokens = None;
    c.output_tokens = None;
    c.cache_read = None;
    c.virtual_model = "claude-opus".into();
    log(&store, &c).await.unwrap();
    // ④ 3 天前、有价：$0.0105（只进近 7 天，不进今日）
    let mut d = row("u1", "priced", now - 3 * 86_400);
    d.cache_read = None;
    log(&store, &d).await.unwrap();
    // ⑤ 9 天前：两个范围都不该有它
    let mut e = row("u1", "priced", now - 9 * 86_400);
    e.cache_read = None;
    log(&store, &e).await.unwrap();

    let today = usage_summary(&store, "today", "virtual_model")
        .await
        .unwrap();
    assert_eq!(today["totals"]["requests"], 3, "{today}");
    assert_eq!(today["totals"]["no_price"], 1);
    assert_eq!(today["totals"]["token_unknown"], 1);
    let c7 = today["totals"]["cost_usd"].as_f64().unwrap();
    assert!(
        (c7 - 0.0105).abs() < 1e-12,
        "今日只该算那一条有价的：{today}"
    );

    let rows = today["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "两个虚拟模型各一行：{today}");
    let haiku = rows.iter().find(|r| r["key"] == "claude-haiku").unwrap();
    assert_eq!(haiku["requests"], 1);
    assert!((haiku["cost_usd"].as_f64().unwrap() - 0.0105).abs() < 1e-12);
    let opus = rows.iter().find(|r| r["key"] == "claude-opus").unwrap();
    assert_eq!(opus["requests"], 2);
    assert_eq!(opus["cost_usd"], Value::Null, "整行无价 → 不显示 0");

    let week = usage_summary(&store, "7d", "upstream").await.unwrap();
    assert_eq!(week["totals"]["requests"], 4, "{week}");
    let w = week["totals"]["cost_usd"].as_f64().unwrap();
    assert!((w - 0.021).abs() < 1e-12, "近 7 天两条有价的：{week}");
    assert_eq!(week["rows"].as_array().unwrap().len(), 1, "只有一条上游");
    assert_eq!(week["rows"][0]["key"], "u1");

    // 范围 / 维度取值域之外的，报错而不是悄悄给一份错的。
    // （票 59 起 `30d` / `all` / `custom` 也认——五个范围共用 `usage` 一份实现。）
    let month = usage_summary(&store, "30d", "virtual_model").await.unwrap();
    assert_eq!(
        month["totals"]["requests"], 5,
        "9 天前那条也在近 30 天里：{month}"
    );
    let all = usage_summary(&store, "all", "client").await.unwrap();
    assert_eq!(all["totals"]["requests"], 5, "{all}");
    assert!(usage_summary(&store, "last-year", "virtual_model")
        .await
        .is_err());
    assert!(usage_summary(&store, "today", "pizza").await.is_err());
}

fn upstream(id: &str) -> provider::Upstream {
    provider::Upstream::new(id, id, "http://127.0.0.1:1")
}

/// 票 58：「N 次无价格 / N 次 token 未知」两个计数把 `cache_write` 算进去，汇总也带这一列。
///
/// - 只报了缓存写的行**不是**「token 未知」（否则会被说成「上游没回 usage」）；
/// - 有缓存写 token 但**没有缓存写价**的行算「无价格」（整条不计入总额）。
#[tokio::test]
async fn usage_summary_counts_cache_write_rows_and_totals_it() {
    let root = tmp_root("usage-cache-write");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    // 四项价都填的目录条目 + 只填前三项的目录条目。
    for (model, cache_write) in [("four", Some(3.75)), ("three", None)] {
        provider::save_manual_model(&store, "u1", model, &[])
            .await
            .unwrap();
        provider::set_manual_prices(
            &store,
            "u1",
            model,
            &Prices {
                input: Some(3.0),
                output: Some(15.0),
                cache_read: Some(0.30),
                cache_write,
            },
        )
        .await
        .unwrap();
    }

    let now = unix_now();
    // ① 只报缓存写（别的都没报）→ 不是 token 未知；4000 × $3.75/Mtok = $0.015
    let mut a = row("u1", "four", now - 60);
    a.input_tokens = None;
    a.output_tokens = None;
    a.cache_read = None;
    a.cache_write = Some(4000);
    log(&store, &a).await.unwrap();
    // ② 有缓存写 token，但目录没填第四项价 → 整条无价格
    let mut b = row("u1", "three", now - 120);
    b.input_tokens = None;
    b.output_tokens = None;
    b.cache_read = None;
    b.cache_write = Some(10);
    log(&store, &b).await.unwrap();

    let today = usage_summary(&store, "today", "virtual_model")
        .await
        .unwrap();
    assert_eq!(today["totals"]["requests"], 2, "{today}");
    assert_eq!(
        today["totals"]["token_unknown"], 0,
        "只报了缓存写不该算 token 未知：{today}"
    );
    assert_eq!(
        today["totals"]["no_price"], 1,
        "缺缓存写价那一行算无价格：{today}"
    );
    assert_eq!(today["totals"]["cache_write"], 4010, "{today}");
    let c = today["totals"]["cost_usd"].as_f64().unwrap();
    assert!((c - 0.015).abs() < 1e-12, "实际 {c}：{today}");

    // 两个虚拟模型名相同（`row()` 的默认）→ 一组；分组行也带这一列。
    let rows = today["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{today}");
    assert_eq!(rows[0]["key"], "claude-haiku", "{today}");
    assert_eq!(rows[0]["cache_write"], 4010, "分组行也要带缓存写：{today}");
}

// ───────────────────────── 票 59：五段范围 / 三维度 / 按天明细 / 详情四段 ─────────────────────────

/// 时间窗的日边界按**本机本地时区自然日**（票 33 第 5 节）。期望值取历法本身
/// （用 .NET `DateTimeOffset` 独立算过）：东八区 `now = 2026-10-15T00:00:00Z`（本地 08:00），
/// 本地那一天的 0 点 = UTC `2026-10-14T16:00:00Z`。
#[test]
fn range_windows_use_local_natural_days() {
    let offset = 8 * 3_600; // 东八区
    let now = 1_792_022_400; // 2026-10-15T00:00:00Z
    let local_midnight = 1_791_993_600; // 2026-10-14T16:00:00Z

    let (from, to) = range_window("today", now, offset, None, None).unwrap();
    assert_eq!(to, None, "今日 = 到现在为止");
    assert_eq!(from, local_midnight);

    let (from7, _) = range_window("7d", now, offset, None, None).unwrap();
    assert_eq!(from7, local_midnight - 6 * 86_400, "近 7 天含今天那一天");
    let (from30, _) = range_window("30d", now, offset, None, None).unwrap();
    assert_eq!(from30, local_midnight - 29 * 86_400);
    let (from_all, _) = range_window("all", now, offset, None, None).unwrap();
    assert_eq!(from_all, 0, "全部 = 从纪元起");
    assert!(range_window("last-year", now, offset, None, None).is_err());

    // 换个时区（UTC）时同一刻的「今天」从 UTC 0 点算起：日边界真的跟着本机时区走。
    let (utc_from, _) = range_window("today", now, 0, None, None).unwrap();
    assert_eq!(utc_from, 1_792_022_400, "UTC 下今天就是 UTC 那天");
}

/// 自定义区间：[from, to] 两个**本地日期**，`to` 含当天（右端是它下一天的起点）。
#[test]
fn custom_range_covers_whole_local_days_including_the_end_date() {
    let offset = 8 * 3_600;
    let (from, to) = range_window(
        "custom",
        1_792_022_400,
        offset,
        Some("2026-10-01"),
        Some("2026-10-15"),
    )
    .unwrap();
    // 本地 10-01 0 点 → UTC 09-30 16:00 = 1_790_784_000（.NET 独立算过）。
    assert_eq!(from, 1_790_784_000);
    // 右端 = 本地 10-16 0 点（左闭右开，10-15 全天在内）= 1_792_080_000。
    assert_eq!(to, Some(1_792_080_000));
    assert!(to.unwrap() - from == 15 * 86_400);

    // 只给 from = 从那天到此刻；日期写坏 / 起止倒置都要报错，不静默全量。
    let (_, open) =
        range_window("custom", 1_792_022_400, offset, Some("2026-10-01"), None).unwrap();
    assert_eq!(open, None);
    assert!(range_window("custom", 1_792_022_400, offset, None, None).is_err());
    assert!(range_window("custom", 1_792_022_400, offset, Some("2026/10/01"), None).is_err());
    assert!(
        range_window(
            "custom",
            1_792_022_400,
            offset,
            Some("2026-10-15"),
            Some("2026-10-01")
        )
        .is_err(),
        "结束早于开始要报错，不能悄悄给空表"
    );
}

/// 用量页：三维度各自一组、按天明细按**本地自然日**分桶、五个范围都认。
#[tokio::test]
async fn usage_report_groups_by_every_dimension_and_lists_local_days() {
    let root = tmp_root("usage-report");
    let store = Store::open(&root).unwrap();
    provider::save_upstream(&store, &upstream("u1"))
        .await
        .unwrap();
    provider::save_manual_model(&store, "u1", "priced", &[])
        .await
        .unwrap();
    provider::set_manual_prices(
        &store,
        "u1",
        "priced",
        &Prices {
            input: Some(3.0),
            output: Some(15.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let now = unix_now();
    let day = 86_400;
    // 今天两条（不同客户端 / 虚拟模型），昨天一条，40 天前一条（只进「全部」）。
    for (ts, vm, client) in [
        (now - 60, "claude-haiku", "claude-code"),
        (now - 120, "claude-opus", "codex"),
        (now - day, "claude-haiku", "codex"),
        (now - 40 * day, "claude-haiku", "claude-code"),
    ] {
        let mut r = row("u1", "priced", ts);
        r.virtual_model = vm.into();
        r.client = Some(client.into());
        r.cache_read = None;
        log(&store, &r).await.unwrap();
    }

    // 三维度都验：同一份数据换个维度就是另一种分组键。
    let by_model = usage(&store, "all", "virtual_model", None, None)
        .await
        .unwrap();
    assert_eq!(by_model["totals"]["requests"], 4, "{by_model}");
    assert_eq!(by_model["rows"].as_array().unwrap().len(), 2);
    let keys: Vec<&str> = by_model["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, vec!["claude-haiku", "claude-opus"], "按花费降序");

    let by_up = usage(&store, "all", "upstream", None, None).await.unwrap();
    assert_eq!(by_up["rows"].as_array().unwrap().len(), 1, "{by_up}");
    assert_eq!(by_up["rows"][0]["key"], "u1");

    let by_client = usage(&store, "all", "client", None, None).await.unwrap();
    let clients: Vec<&str> = by_client["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert_eq!(clients, vec!["claude-code", "codex"], "{by_client}");

    // 近 30 天不含那条 40 天前的；今日只算今天的两条。
    assert_eq!(
        usage(&store, "30d", "virtual_model", None, None)
            .await
            .unwrap()["totals"]["requests"],
        3
    );
    assert_eq!(
        usage(&store, "today", "virtual_model", None, None)
            .await
            .unwrap()["totals"]["requests"],
        2
    );

    // 按天明细：三条各一天（今天 / 昨天 / 40 天前），日期串按**本地**自然日给。
    let daily = by_model["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 3, "{by_model}");
    let offset = local_offset_secs();
    let want_today = {
        let d = civil_from_days((now + offset).div_euclid(day));
        format!("{:04}-{:02}-{:02}", d.0, d.1, d.2)
    };
    assert_eq!(daily[2]["day"], want_today, "最后一行是今天：{by_model}");
    assert_eq!(daily[2]["requests"], 2);
    assert_eq!(daily[0]["requests"], 1, "最早那天是 40 天前：{by_model}");
    // 每天的花费 = 那一天的行之和（两条有价的 → $0.021）。
    assert!((daily[2]["cost_usd"].as_f64().unwrap() - 0.021).abs() < 1e-12);

    // 自定义区间：只要昨天那一天 → 1 条。
    let yesterday = {
        let d = civil_from_days((now + offset).div_euclid(day) - 1);
        format!("{:04}-{:02}-{:02}", d.0, d.1, d.2)
    };
    let custom = usage(
        &store,
        "custom",
        "virtual_model",
        Some(&yesterday),
        Some(&yesterday),
    )
    .await
    .unwrap();
    assert_eq!(custom["totals"]["requests"], 1, "{custom}");
    assert_eq!(custom["daily"].as_array().unwrap().len(), 1);
    assert_eq!(custom["daily"][0]["day"], yesterday);
}

/// 详情三段的数据源：概要 / 请求与响应体 / 帧时间轴；`id` 认行 id 也认 `request_id`。
#[tokio::test]
async fn log_detail_serves_summary_bodies_and_frames_by_both_keys() {
    let root = tmp_root("log-detail");
    let store = Store::open(&root).unwrap();
    let mut r = row("u1", "priced", unix_now() - 30);
    r.request_id = Some("req-known".into());
    r.error = Some("上游 502：bad gateway".into());
    r.status = 502;
    r.aborted = true;
    r.body = Some(r#"{"model":"出站请求体"}"#.into());
    r.response_body = Some("event: message_start\ndata: {\"type\":\"message_start\"}\n\n".into());
    r.frames = vec![
        FrameLog {
            seq: 0,
            at_ms: 3,
            event: Some("message_start".into()),
            summary: "{\"type\":\"message_start\"}".into(),
            raw: "event: message_start\ndata: {\"type\":\"message_start\"}\n\n".into(),
        },
        FrameLog {
            seq: 1,
            at_ms: 41,
            event: Some("message_stop".into()),
            summary: "{\"type\":\"message_stop\"}".into(),
            raw: "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".into(),
        },
    ];
    log(&store, &r).await.unwrap();

    // 按 request_id 取。
    let by_rid = log_detail(&store, "req-known").await.unwrap().unwrap();
    assert_eq!(by_rid["summary"]["request_id"], "req-known");
    assert_eq!(by_rid["summary"]["status"], 502);
    assert_eq!(by_rid["bodies"]["request"], r#"{"model":"出站请求体"}"#);
    assert!(by_rid["bodies"]["response"]
        .as_str()
        .unwrap()
        .contains("message_start"));
    let frames = by_rid["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2, "两帧按 seq 排序：{by_rid}");
    assert_eq!(frames[0]["event"], "message_start");
    assert_eq!(frames[0]["seq"], 0);
    assert_eq!(frames[1]["at_ms"], 41);
    assert!(frames[1]["raw"].as_str().unwrap().contains("message_stop"));

    // 按行 id 取（界面从列表跳过来用的是这把钥匙）。
    let id = by_rid["summary"]["id"].as_i64().unwrap().to_string();
    let by_id = log_detail(&store, &id).await.unwrap().unwrap();
    assert_eq!(by_id["summary"]["request_id"], "req-known");

    // 查不到是 None（管理面翻 404），不是一份空详情。
    assert!(log_detail(&store, "req-nope").await.unwrap().is_none());

    // 清空日志：行、body、帧一起走（外键级联），回删掉的行数。
    assert_eq!(clear_logs(&store).await.unwrap(), 1);
    assert!(log_detail(&store, "req-known").await.unwrap().is_none());
    let bodies: i64 = store
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM request_bodies", [], |r| r.get(0))?))
        .await
        .unwrap();
    let frames_left: i64 = store
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM request_frames", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!((bodies, frames_left), (0, 0), "body 与帧要跟着请求行走");
}
