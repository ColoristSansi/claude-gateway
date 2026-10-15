//! store 的单测：建表 / 迁移（旧版库升上来）/ 读写往返 / 连接并发 / DPAPI 与掩码 / 数据目录。
//!
//! 一律指向临时目录（写入根参数化）：**不碰真实 `%APPDATA%\com.claudegateway.desktop\`**。

use super::*;
use crate::store::schema::{MIGRATIONS, SCHEMA_VERSION};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 每个用例一个确定路径（跑完不清理，便于在**库文件上**做字节反查）。
fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg42-store-tests").join(tag);
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn table_names(c: &Connection) -> Result<Vec<String>> {
    let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn user_version(c: &Connection) -> Result<i32> {
    Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// 目录里所有文件的字节里是否出现过 `needle`（库文件 + `-wal` / `-shm` 都算）。
fn dir_contains_bytes(dir: &Path, needle: &[u8]) -> bool {
    for e in std::fs::read_dir(dir).expect("读数据目录") {
        let p = e.expect("目录项").path();
        if !p.is_file() {
            continue;
        }
        let bytes = std::fs::read(&p).expect("读文件");
        if bytes.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
    }
    false
}

#[test]
fn migrations_are_contiguous_and_cover_schema_version() {
    for (i, m) in MIGRATIONS.iter().enumerate() {
        assert_eq!(m.version, i as i32 + 1, "迁移版本必须从 1 连续递增");
    }
    assert_eq!(MIGRATIONS.len() as i32, SCHEMA_VERSION);
}

#[test]
fn default_data_dir_follows_appdata_and_identifier() {
    // 只算路径、不创建（票 18：Tauri 的 path resolver 同样只算不建）。
    let dir = default_data_dir().expect("APPDATA 在本机应存在");
    assert_eq!(
        dir.file_name().and_then(|s| s.to_str()),
        Some(DATA_DIR_NAME)
    );
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap();
    assert_eq!(dir.parent().unwrap(), appdata.as_path());
}

#[tokio::test]
async fn open_creates_missing_data_dir() {
    let root = tmp_root("fresh");
    assert!(!root.exists());
    let store = Store::open(&root).expect("首次启动自建数据目录并开库");
    assert!(root.is_dir());
    assert!(store.db_path().is_file());
    assert_eq!(store.db_path(), root.join(DB_FILE_NAME).as_path());
}

#[tokio::test]
async fn first_launch_creates_spec9_tables_and_seeds() {
    let root = tmp_root("tables");
    let store = Store::open(&root).expect("开库");
    let tables = store.read(table_names).await.unwrap();
    for t in [
        "upstreams",
        "upstream_models",
        "credentials",
        "virtual_models",
        "client_profiles",
        "client_config_state",
        "requests",
        "request_bodies",
        "model_catalog",
        "settings",
    ] {
        assert!(tables.iter().any(|x| x == t), "缺表 {t}：{tables:?}");
    }
    // 原型期的表不该留在新库里（request_log 已并进 requests）。
    assert!(!tables.iter().any(|x| x == "quirks"), "{tables:?}");
    assert!(!tables.iter().any(|x| x == "request_log"), "{tables:?}");

    assert_eq!(store.read(user_version).await.unwrap(), SCHEMA_VERSION);

    let profiles: i64 = store
        .read(|c| Ok(c.query_row("SELECT count(*) FROM client_profiles", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(
        profiles, 4,
        "四套档案：全局 / Desktop / Claude Code / Codex"
    );
    let port: String = store
        .read(|c| {
            Ok(
                c.query_row("SELECT value FROM settings WHERE key='port'", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(port, "8787", "spec §3：默认端口 8787");
}

#[tokio::test]
async fn read_write_roundtrip_across_spec9_tables() {
    let root = tmp_root("roundtrip");
    let store = Store::open(&root).expect("开库");

    let id = store
        .write(|c| {
            c.execute(
                "INSERT INTO upstreams(id,name,base_url,auth_json,path_templates_json,header_passthrough_json,
                                       quirks_json,last_pull_at,last_pull_status,last_pull_warning)
                 VALUES('qwen','通义','https://up.example/anthropic',
                        '{\"default\":{\"scheme\":\"bearer\"},\"anthropic\":{\"scheme\":\"x-api-key\"}}',
                        '{\"anthropic\":\"/v1/messages\"}','[\"anthropic-version\"]','[\"noModelsEndpoint\"]',
                        1750000000,'ok','忽略端点 /v1/embeddings')",
                [],
            )?;
            c.execute(
                "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source,
                                             price_input_usd,price_output_usd,price_cache_read_usd,price_cache_write_usd)
                 VALUES('qwen','qwen3-max','[\"anthropic\",\"openai-chat\"]','manual',1.5,6.0,NULL,2.25)",
                [],
            )?;
            c.execute(
                "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,real_model,wire,purpose)
                 VALUES('global','claude-sonnet',1,'qwen','qwen3-max','anthropic','家族槽位')",
                [],
            )?;
            c.execute(
                "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,real_model,wire,purpose)
                 VALUES('codex','claude-sonnet',1,'qwen','qwen3-max','openai-responses','Codex 覆盖')",
                [],
            )?;
            c.execute(
                "INSERT INTO client_config_state(client,file_path,owned_keys_json,last_op,last_write_at,
                                                 backup_path,existed_at_first_write)
                 VALUES('codex','C:\\Users\\x\\.codex\\config.toml','{\"model_provider\":\"claudegateway\"}',
                        'write',1750000001,'C:\\backups\\codex\\config.toml.20261015-120000',1)",
                [],
            )?;
            c.execute(
                "INSERT INTO requests(ts,request_id,client,virtual_model,upstream_id,real_model,wire,status,
                                      first_byte_ms,total_ms,input_tokens,output_tokens,cache_read,aborted,error,cost_usd)
                 VALUES(1750000002,'req-1','claude-desktop','claude-sonnet','qwen','qwen3-max','anthropic',200,
                        123,4567,10,20,3,0,NULL,0.0042)",
                [],
            )?;
            c.execute(
                "INSERT INTO request_bodies(request_id,request_body,response_body)
                 VALUES(last_insert_rowid(),'{\"messages\":[]}','{\"content\":[]}')",
                [],
            )?;
            c.execute(
                "INSERT INTO model_catalog(provider,model_id,name,cost_input_usd,cost_output_usd,
                                           cost_cache_read_usd,cost_cache_write_usd,raw_json)
                 VALUES('anthropic','claude-haiku-4-5','Claude Haiku 4.5',1.0,5.0,0.1,1.25,'{\"id\":\"claude-haiku-4-5\"}')",
                [],
            )?;
            c.execute(
                "INSERT INTO settings(key,value) VALUES('log_retention_days','7')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )?;
            Ok(())
        })
        .await;
    id.unwrap();

    // 读回来逐表验形。
    let quirks: String = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT quirks_json FROM upstreams WHERE id='qwen'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    let quirks: Vec<String> = serde_json::from_str(&quirks).unwrap();
    assert_eq!(quirks, vec!["noModelsEndpoint".to_string()]);

    let (wires, src, pin, pwrite): (String, String, Option<f64>, Option<f64>) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT supported_wires,source,price_input_usd,price_cache_write_usd
                   FROM upstream_models WHERE upstream_id='qwen' AND model_id='qwen3-max'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&wires).unwrap(),
        vec!["anthropic", "openai-chat"]
    );
    assert_eq!(src, "manual");
    assert_eq!(pin, Some(1.5));
    assert_eq!(pwrite, Some(2.25));

    // 同一槽位两套档案各绑一条（§7 每客户端可覆盖）。
    let bound: String = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT real_model FROM virtual_models WHERE client_profile_id='codex' AND name='claude-sonnet'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(bound, "qwen3-max");

    let (op, existed, backup): (String, i64, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT last_op,existed_at_first_write,backup_path FROM client_config_state WHERE client='codex'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(op, "write");
    assert_eq!(existed, 1);
    assert!(backup.ends_with(".20261015-120000"));

    let (status, cost, body): (i64, f64, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT r.status,r.cost_usd,b.response_body FROM requests r
                   JOIN request_bodies b ON b.request_id=r.id WHERE r.request_id='req-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(cost, 0.0042);
    assert_eq!(body, "{\"content\":[]}");

    let (cat_price, snap): (f64, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT cost_cache_write_usd,(SELECT value FROM settings WHERE key='log_retention_days')
                   FROM model_catalog WHERE provider='anthropic' AND model_id='claude-haiku-4-5'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(cat_price, 1.25);
    assert_eq!(snap, "7");

    // CHECK 域守住了 wire 取值域（§6 三个）。
    let bad = store
        .write(|c| {
            c.execute(
                "INSERT INTO virtual_models(client_profile_id,name,wire) VALUES('global','x','bogus')",
                [],
            )?;
            Ok(())
        })
        .await;
    assert!(bad.is_err(), "wire 取值域应被 CHECK 拦住");
}

/// 造一份**原型期**（v1）的库，再用现版本升上来。
#[tokio::test]
async fn upgrades_a_prototype_era_db_in_place() {
    let root = tmp_root("migrate");
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join(DB_FILE_NAME);
    {
        let mut c = Connection::open(&db).unwrap();
        super::schema::migrate_upto(&mut c, 1).unwrap();
        c.execute(
            "INSERT INTO upstreams(id,base_url,auth,anthropic_path,openai_path)
             VALUES('opencode','https://oc.example','x-api-key','/v1/messages','/v1/chat/completions')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO quirks(upstream_id,quirk,arg) VALUES('opencode','SessionIdHeader','x-opencode-session')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO virtual_models(name,upstream_id,real_model,wire,family)
             VALUES('claude-haiku','opencode','big-pickle','anthropic',1)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO request_log(ts,virtual_model,upstream_id,real_model,wire,status,first_byte_ms,total_ms,
                                     input_tokens,output_tokens,cache_read,aborted,error,body)
             VALUES('unix:1750000000','claude-haiku','opencode','big-pickle','anthropic',200,11,22,33,44,5,0,NULL,
                    '{\"model\":\"big-pickle\"}')",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&root).expect("旧库升上来");
    assert_eq!(store.read(user_version).await.unwrap(), SCHEMA_VERSION);

    // 上游：base_url 保住，auth / 路径模板折进 JSON，quirks 折进 quirks_json。
    let (name, url, paths, quirks): (String, String, String, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT name,base_url,path_templates_json,quirks_json FROM upstreams WHERE id='opencode'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(name, "opencode");
    assert_eq!(url, "https://oc.example");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&paths).unwrap()["anthropic"],
        "/v1/messages"
    );
    let quirks: Vec<serde_json::Value> = serde_json::from_str(&quirks).unwrap();
    assert_eq!(quirks[0]["name"], "SessionIdHeader");
    assert_eq!(quirks[0]["arg"], "x-opencode-session");

    // v3（票 49）与 v4（票 50）加的两列在旧库上也在，且都还没有记录。
    let (pull_ok_at, selfcheck_at): (Option<i64>, Option<i64>) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT last_pull_ok_at,last_selfcheck_at FROM upstreams WHERE id='opencode'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(pull_ok_at, None, "旧库升上来：从没成功拉过");
    assert_eq!(selfcheck_at, None, "旧库升上来：从没自检过");

    // 原型期的 quirks 表已并走。
    let gone: i64 = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='quirks'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(gone, 0);

    // 虚拟模型：落到 global 档案，family 位保留。
    let (profile, family, real): (String, i64, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT client_profile_id,is_family,real_model FROM virtual_models WHERE name='claude-haiku'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(profile, "global");
    assert_eq!(family, 1);
    assert_eq!(real, "big-pickle");

    // 请求日志改名成 requests，body 搬进 request_bodies，旧列不留。
    let (ts, status, body): (i64, i64, String) = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT r.ts,r.status,b.request_body FROM requests r
                   JOIN request_bodies b ON b.request_id=r.id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(ts, 1750000000);
    assert_eq!(status, 200);
    assert_eq!(body, "{\"model\":\"big-pickle\"}");
    let body_col: i64 = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM pragma_table_info('requests') WHERE name='body'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(body_col, 0, "旧 body 列应已搬走并删掉");

    // 迁移是幂等的：再开一次不会重复写。
    drop(store);
    let again = Store::open(&root).expect("再开");
    assert_eq!(again.read(user_version).await.unwrap(), SCHEMA_VERSION);
    let n: i64 = again
        .read(|c| Ok(c.query_row("SELECT count(*) FROM client_profiles", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(n, 4);
}

/// 判据是「不阻塞 tokio runtime」：写库在专属写线程上，读在 blocking 池上，都不在 runtime 线程上跑。
#[tokio::test]
async fn writes_and_reads_run_off_the_runtime_thread() {
    let root = tmp_root("threads");
    let store = Store::open(&root).unwrap();
    let caller = std::thread::current().id();
    let write_thread = store
        .write(|_c| Ok(std::thread::current().id()))
        .await
        .unwrap();
    assert_ne!(
        write_thread, caller,
        "写库必须在专属写线程上执行，不能在 runtime 线程上同步执行"
    );
    let read_thread = store
        .read(|_c| Ok(std::thread::current().id()))
        .await
        .unwrap();
    assert_ne!(read_thread, caller, "读不能阻塞 runtime 线程");
}

/// 写线程被一个 job 死死占住时，runtime 照样能调度别的异步工作，排队的写照样不丢。
#[tokio::test]
async fn a_stuck_write_thread_does_not_stall_the_runtime() {
    let root = tmp_root("gate");
    let store = Store::open(&root).unwrap();

    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let hog = tokio::spawn({
        let s = store.clone();
        async move {
            s.write(move |_c| {
                let _ = started_tx.send(());
                gate_rx.recv().expect("放行");
                Ok(())
            })
            .await
        }
    });
    started_rx.await.expect("写线程没进 job");
    assert!(!hog.is_finished(), "写线程应被这个 job 占住");

    let queued = tokio::spawn({
        let s = store.clone();
        async move {
            s.write(|c| {
                c.execute("INSERT INTO settings(key,value) VALUES('gate','1')", [])?;
                Ok(())
            })
            .await
        }
    });

    // runtime 线程必须还能跑：定时器照常触发（写线程被占住不影响调度）。
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::time::sleep(std::time::Duration::from_millis(10)),
    )
    .await
    .expect("写线程被占住时 runtime 仍应能调度");

    gate_tx.send(()).unwrap();
    hog.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();

    let v: String = store
        .read(|c| {
            Ok(
                c.query_row("SELECT value FROM settings WHERE key='gate'", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(v, "1");
}

#[tokio::test]
async fn two_concurrent_writers_both_land() {
    let root = tmp_root("concurrent");
    let store = Store::open(&root).unwrap();
    let mut tasks = Vec::new();
    for w in 0..2 {
        let s = store.clone();
        tasks.push(tokio::spawn(async move {
            for i in 0..50 {
                let key = format!("k{w}-{i}");
                s.write(move |c| {
                    c.execute("INSERT INTO settings(key,value) VALUES(?1,'x')", [&key])?;
                    Ok(())
                })
                .await
                .unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let n: i64 = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM settings WHERE key LIKE 'k%'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(n, 100, "两个写各 50 条，串行排队后一条不少");
}

#[test]
fn mask_keeps_head_and_tail_only() {
    assert_eq!(mask_secret("sk-1234567890abcd"), "sk-…abcd");
    assert_eq!(mask_secret("short"), "••••", "太短的整串掩掉，不泄露长度");
    assert!(!mask_secret("sk-1234567890abcd").contains("1234567890"));
}

#[cfg(windows)]
#[tokio::test]
async fn credentials_are_dpapi_ciphertext_and_never_plaintext_on_disk() {
    const SECRET: &str = "sk-cg42-known-plaintext-9f3a1c";
    let root = tmp_root("credentials");
    let store = Store::open(&root).unwrap();
    store
        .write(|c| {
            c.execute(
                "INSERT INTO upstreams(id,name,base_url) VALUES('u1','上游一','https://up.example')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    store.put_credential("u1", "default", SECRET).await.unwrap();

    // 读回明文（出站取凭据走这条），掩码接口给界面侧用。
    assert_eq!(
        store
            .credential_secret("u1", "default")
            .await
            .unwrap()
            .as_deref(),
        Some(SECRET)
    );
    assert_eq!(
        store
            .credential_mask("u1", "default")
            .await
            .unwrap()
            .as_deref(),
        Some("sk-…3a1c")
    );
    assert_eq!(
        store.credential_mask("u1", "anthropic").await.unwrap(),
        None
    );

    // 库里那一列是密文，不是明文。
    let cipher: Vec<u8> = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT ciphertext FROM credentials WHERE upstream_id='u1' AND wire='default'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_ne!(cipher, SECRET.as_bytes());
    assert!(cipher.len() > SECRET.len(), "DPAPI 密文带头部，比明文长");
    assert!(!cipher.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()));

    // 整个数据目录（含 -wal / -shm）字节里搜不到明文。
    // 先放一条**故意**存明文的串做正对照：证明这套字节反查不是空转。
    const PROBE: &str = "cg42-scan-probe-plain";
    store
        .write(|c| {
            c.execute(
                "INSERT INTO settings(key,value) VALUES('scan_probe',?1)",
                [PROBE],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        dir_contains_bytes(&root, PROBE.as_bytes()),
        "正对照：目录里能找到故意存下的明文串，反查才有意义"
    );
    assert!(
        !dir_contains_bytes(&root, SECRET.as_bytes()),
        "库文件里不该出现明文密钥"
    );

    // 覆盖写 + 删：换一份 wire 动，`default` 那份留着（验收时要在库文件上反查它）。
    store
        .put_credential("u1", "anthropic", SECRET)
        .await
        .unwrap();
    store
        .put_credential("u1", "anthropic", "sk-other-0000")
        .await
        .unwrap();
    let n: i64 = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM credentials WHERE wire='anthropic'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(n, 1, "同一「上游 × wire」只有一行");
    store.delete_credential("u1", "anthropic").await.unwrap();
    assert_eq!(
        store.credential_secret("u1", "anthropic").await.unwrap(),
        None
    );
    assert_eq!(
        store
            .credential_secret("u1", "default")
            .await
            .unwrap()
            .as_deref(),
        Some(SECRET),
        "default 那份仍在（库文件反查用）"
    );
}

#[cfg(windows)]
#[test]
fn dpapi_roundtrip_is_current_user_scoped() {
    let plain = b"sk-dpapi-roundtrip-\x00-\xff";
    let cipher = super::dpapi::protect(plain).unwrap();
    assert_ne!(cipher.as_slice(), plain.as_slice());
    assert_eq!(super::dpapi::unprotect(&cipher).unwrap(), plain);
    // 改一位就必须解不开（DPAPI 带完整性校验），而不是悄悄给错值。
    let mut broken = cipher.clone();
    let last = broken.len() - 1;
    broken[last] ^= 0x01;
    assert!(super::dpapi::unprotect(&broken).is_err());
}
