//! 票 62 的集成验收面：导出 / 导入（`dist::bundle`）与卸载还原（`dist::restore`）。
//!
//! 全部在**临时根**上跑：客户端配置根（`Claude-3p\configLibrary` / `.claude` / `.codex`）与数据目录
//! 都指临时目录，真实 `%LOCALAPPDATA%`、`~/.claude`、`~/.codex` 一个字节都不碰。
//!
//! 断言的都是「用户能看见的行为」：文件回到官方态、明文密钥不在文件里、导入之后库里能查到东西。

use std::fs;
use std::path::{Path, PathBuf};

use gateway_core::binding::Route;
use gateway_core::dist::bundle::{self, ExportOptions, ImportMode};
use gateway_core::dist::restore::{self, ClientRoots};
use gateway_core::store::Store;
use gateway_core::writer::claude_code::{self, ModelSlots};
use gateway_core::writer::codex::{self, CodexWriter};
use gateway_core::writer::desktop::{self, DesktopConfig};
use gateway_core::writer::{FileOutcome, WritePlan, WriteRoots, Writer};

/// 一把**只应出现在数据库里**的密钥：验收就是拿它在导出文件里反查（票面第 3 条）。
const KNOWN_SECRET: &str = "sk-or-v1-6f2b1c0d9e8f7a6b5c4d3e2f1a0b9c8d-CG62-SHOULD-NOT-LEAK";

const BASE_URL: &str = "http://127.0.0.1:8787";

fn tmp(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cg62-tests-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("读 {} 失败：{e}", path.display()))
}

fn read_text(path: &Path) -> String {
    String::from_utf8(read(path)).expect("UTF-8")
}

/// 一个种了上游 + 密钥 + 设置项 + 模型目录的库。
async fn seeded_store(root: &Path) -> Store {
    let store = Store::open(&root.join("data")).expect("开库");
    store
        .write(|c| {
            c.execute(
                "INSERT INTO upstreams(id,name,base_url,auth_json,path_templates_json,
                                       header_passthrough_json,quirks_json)
                 VALUES('openrouter','OpenRouter','https://openrouter.ai/api','{\"default\":{\"scheme\":\"bearer\"}}','{}','[]','[]')",
                [],
            )?;
            c.execute(
                "INSERT INTO upstream_models(upstream_id,model_id,supported_wires,source,price_input_usd)
                 VALUES('openrouter','deepseek/deepseek-v4.1-flash','[\"anthropic\"]','manual',0.28)",
                [],
            )?;
            c.execute(
                "INSERT INTO virtual_models(client_profile_id,name,is_family,upstream_id,real_model,wire)
                 VALUES('global','claude-sonnet',1,'openrouter','deepseek/deepseek-v4.1-flash','anthropic')",
                [],
            )?;
            c.execute("UPDATE settings SET value='9123' WHERE key='port'", [])?;
            Ok(())
        })
        .await
        .expect("种库");
    store
        .put_credential("openrouter", "anthropic", KNOWN_SECRET)
        .await
        .expect("存密钥");
    store
}

// ── 导出 / 导入 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn default_export_leaks_no_plaintext_secret_and_import_skips_credentials() {
    let root = tmp("export-plain");
    let store = seeded_store(&root).await;

    let text = bundle::export(&store, "0.1.0-test", &ExportOptions::plain())
        .await
        .expect("默认导出");

    // ① 拿已知密钥串反查：整串不在文件里，连大段前缀也不在（只有掩码的前 3 后 4）。
    assert!(
        !text.contains(KNOWN_SECRET),
        "默认导出里出现了明文密钥：\n{text}"
    );
    assert!(
        !text.contains("SHOULD-NOT-LEAK"),
        "默认导出里出现了密钥片段：\n{text}"
    );
    assert!(text.contains("\"secrets\": false"), "要显式标出不含密钥");
    assert!(text.contains("sk-"), "掩码要留下（界面据此显示「有密钥」）");

    // ② 导进一个全新的库，网关侧的数据都在（上游 / 手工价 / 绑定 / 端口）。
    let fresh = Store::open(&root.join("fresh")).expect("开新库");
    let report = bundle::import(&fresh, &text, None, ImportMode::SkipExisting)
        .await
        .expect("导入");
    assert_eq!(report.upstreams_created, 1, "{report:?}");
    assert_eq!(report.upstream_models_imported, 1, "{report:?}");
    assert_eq!(report.virtual_models_imported, 1, "{report:?}");
    assert_eq!(
        report.credentials_skipped, 1,
        "没有密钥的导出 → 凭据整条跳过"
    );
    assert_eq!(report.credentials_imported, 0);

    let (base, price, port) = fresh
        .read(|c| {
            let base: String = c.query_row(
                "SELECT base_url FROM upstreams WHERE id='openrouter'",
                [],
                |r| r.get(0),
            )?;
            let price: f64 = c.query_row(
                "SELECT price_input_usd FROM upstream_models WHERE model_id='deepseek/deepseek-v4.1-flash'",
                [],
                |r| r.get(0),
            )?;
            let port: String =
                c.query_row("SELECT value FROM settings WHERE key='port'", [], |r| r.get(0))?;
            Ok((base, price, port))
        })
        .await
        .expect("读回来");
    assert_eq!(base, "https://openrouter.ai/api");
    assert_eq!(price, 0.28, "手工价跟着走");
    assert_eq!(port, "9123", "设置（端口）跟着走");

    // ③ 密钥没有跟着来（这是「默认不含明文密钥」的代价，界面要提示重填）。
    assert!(
        fresh
            .credential_secret("openrouter", "anthropic")
            .await
            .expect("查密钥")
            .is_none(),
        "默认导出不带密钥，导入后不该有"
    );
}

#[tokio::test]
async fn encrypted_export_round_trips_secrets_so_the_imported_gateway_can_call_out() {
    let root = tmp("export-encrypted");
    let store = seeded_store(&root).await;

    let text = bundle::export(
        &store,
        "0.1.0-test",
        &ExportOptions::encrypted("正确的口令-62"),
    )
    .await
    .expect("加密导出");

    assert!(!text.contains(KNOWN_SECRET), "加密后更不该有明文");
    assert!(text.contains("\"cipher\": \"aes-256-gcm\""), "{text}");
    assert!(text.contains("\"kdf\": \"pbkdf2-hmac-sha256\""), "{text}");
    assert!(text.contains("\"secrets\": true"));

    let fresh = Store::open(&root.join("fresh")).expect("开新库");
    let report = bundle::import(
        &fresh,
        &text,
        Some("正确的口令-62"),
        ImportMode::SkipExisting,
    )
    .await
    .expect("带口令导入");
    assert_eq!(report.credentials_imported, 1, "{report:?}");
    assert_eq!(
        fresh
            .credential_secret("openrouter", "anthropic")
            .await
            .expect("查密钥")
            .as_deref(),
        Some(KNOWN_SECRET),
        "带口令的加密导出能把密钥搬到另一份库（跨机路径，与 DPAPI 无关）"
    );

    // 口令不对：报错说清，不半途写库。
    let other = Store::open(&root.join("other")).expect("开第三个库");
    let err = bundle::import(&other, &text, Some("错口令"), ImportMode::SkipExisting)
        .await
        .expect_err("错口令必须失败");
    assert!(err.to_string().contains("口令不对"), "{err}");
    let count: i64 = other
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM upstreams", [], |r| r.get(0))?))
        .await
        .expect("数上游");
    assert_eq!(count, 0, "失败的导入不许留下半截数据");

    // 压根不给口令：也要说清这个文件是加密的。
    let err = bundle::import(&other, &text, None, ImportMode::SkipExisting)
        .await
        .expect_err("加密文件不给口令必须失败");
    assert!(err.to_string().contains("需要口令"), "{err}");
}

#[tokio::test]
async fn import_reports_version_duplicates_and_overwrites_only_when_asked() {
    let root = tmp("import-rules");
    let store = seeded_store(&root).await;
    let text = bundle::export(&store, "0.1.0-test", &ExportOptions::plain())
        .await
        .expect("导出");

    // ① 版本比本程序新 → 拒绝，并说清要升级。
    let newer = text.replace("\"version\": 1", "\"version\": 99");
    let err = bundle::import(&store, &newer, None, ImportMode::SkipExisting)
        .await
        .expect_err("版本太新必须拒绝");
    assert!(err.to_string().contains("99"), "{err}");
    assert!(err.to_string().contains("升级"), "{err}");

    // ② 不是本程序的文件 → 拒绝。
    let err = bundle::import(
        &store,
        r#"{"format":"other-tool","version":1}"#,
        None,
        ImportMode::SkipExisting,
    )
    .await
    .expect_err("别的工具的文件必须拒绝");
    assert!(err.to_string().contains("导出文件"), "{err}");

    // ③ 往**自己**这份库上导入（同 id 已在）→ 重复上游逐条列出，默认跳过、不覆盖。
    store
        .write(|c| {
            c.execute(
                "UPDATE upstreams SET name='用户改过的名字' WHERE id='openrouter'",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("改名字");
    let report = bundle::import(&store, &text, None, ImportMode::SkipExisting)
        .await
        .expect("重复导入");
    assert_eq!(report.duplicates, vec!["openrouter".to_string()]);
    assert_eq!(report.upstreams_skipped, 1);
    assert_eq!(report.upstreams_created, 0);
    let name: String = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT name FROM upstreams WHERE id='openrouter'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .expect("读名字");
    assert_eq!(name, "用户改过的名字", "默认不许覆盖用户现有的上游");

    // ④ 显式要覆盖 → 名字回到导出文件里的那份。
    let report = bundle::import(&store, &text, None, ImportMode::Replace)
        .await
        .expect("覆盖导入");
    assert_eq!(report.upstreams_replaced, 1);
    let name: String = store
        .read(|c| {
            Ok(c.query_row(
                "SELECT name FROM upstreams WHERE id='openrouter'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .expect("读名字");
    assert_eq!(name, "OpenRouter");
    // 覆盖不是「先删后插」：用户自己的密钥不该被外键连带删掉。
    assert!(
        store
            .credential_secret("openrouter", "anthropic")
            .await
            .expect("查密钥")
            .is_some(),
        "覆盖上游不许把已有凭据连带删掉"
    );
}

// ── 卸载时的还原（引擎级，临时根） ─────────────────────────────────────────────

/// 三个客户端都配上网关，返回（配置根，官方态字节快照）。
async fn seed_three_clients(root: &Path, store: &Store) -> (ClientRoots, Vec<(PathBuf, Vec<u8>)>) {
    let roots = ClientRoots::under(root);
    let data_dir = root.join("data");
    let mut official: Vec<(PathBuf, Vec<u8>)> = Vec::new();

    // ① Claude Desktop：先有官方 1P 的 profile + `_meta.json`，再切 3P。
    let desktop_root = roots.claude_desktop.clone().expect("Desktop 根");
    fs::create_dir_all(&desktop_root).expect("建 Desktop 根");
    let cfg = DesktopConfig::new(BASE_URL, "gateway-key");
    let official_profile = desktop_root.join(desktop::profile_rel_path());
    let official_meta = desktop_root.join(desktop::META_FILE);
    fs::write(&official_profile, "{\n  \"official\": true\n}\n").expect("官方 profile");
    fs::write(
        &official_meta,
        // 必须写成引擎自己那份「两空格缩进」的规范形状：还原是解析→改键→再序列化，
        // 排版保真的判据是缩进 / 行尾 / 末尾换行，不是原样的字节。
        "{\n  \"entries\": [\n    {\n      \"id\": \"official-1\",\n      \"name\": \"Default\"\n    }\n  ],\n  \"appliedId\": \"official-1\"\n}\n",
    )
    .expect("官方 _meta");
    official.push((official_profile.clone(), read(&official_profile)));
    official.push((official_meta.clone(), read(&official_meta)));

    let writer = Writer::new(
        store.clone(),
        WriteRoots::with_data_dir(&desktop_root, &data_dir),
    );
    desktop::switch_to_3p(&writer, &cfg).await.expect("切 3P");
    assert!(
        read_text(&official_meta).contains(desktop::PROFILE_ID),
        "切完 3P 后 _meta.json 要指向网关"
    );

    // ② Claude Code：用户自己已有 `env.OTHER`，网关只加自己那几个键。
    let cc_root = roots.claude_code.clone().expect("Claude Code 根");
    fs::create_dir_all(&cc_root).expect("建 Claude Code 根");
    let cc_file = cc_root.join(claude_code::REL_PATH);
    fs::write(
        &cc_file,
        "{\n  \"env\": {\n    \"OTHER\": \"保持不动\"\n  }\n}\n",
    )
    .expect("用户 settings.json");
    official.push((cc_file.clone(), read(&cc_file)));

    let cc_writer = Writer::new(
        store.clone(),
        WriteRoots::with_data_dir(&cc_root, &data_dir),
    );
    cc_writer
        .write(&WritePlan::new(vec![claude_code::file_spec(
            BASE_URL,
            "gateway-token",
            &ModelSlots::default(),
        )]))
        .await
        .expect("写 Claude Code");
    assert!(
        read_text(&cc_file).contains(claude_code::BASE_URL_KEY),
        "写完要有网关键"
    );

    // ③ Codex：用户自己已有 `[windows]` 与别的 provider，网关加自己的 provider 表 + 目录。
    let codex_root = roots.codex.clone().expect("Codex 根");
    fs::create_dir_all(&codex_root).expect("建 Codex 根");
    let codex_config = codex_root.join(codex::CONFIG_FILE);
    fs::write(
        &codex_config,
        "[windows]\nsandbox = \"elevated\"\n\n[model_providers.other]\nname = \"Other\"\n",
    )
    .expect("用户 config.toml");
    official.push((codex_config.clone(), read(&codex_config)));

    let codex_writer = CodexWriter::new(
        Writer::new(
            store.clone(),
            WriteRoots::with_data_dir(&codex_root, &data_dir),
        ),
        BASE_URL,
    );
    codex_writer
        .write(&routes(), "claude-sonnet")
        .await
        .expect("写 Codex");
    let codex_catalog = codex_root.join(codex::CATALOG_FILE);
    assert!(codex_catalog.is_file(), "写完要有目录文件");
    assert!(
        read_text(&codex_config).contains(codex::PROVIDER_ID),
        "写完 config.toml 要有网关注册"
    );

    (roots, official)
}

fn routes() -> Vec<Route> {
    vec![
        Route {
            name: "claude-sonnet".to_string(),
            upstream_id: "openrouter".to_string(),
            real_model: "deepseek/deepseek-v4.1-flash".to_string(),
            wire: "anthropic".to_string(),
            is_family: true,
        },
        Route {
            name: "claude-opus".to_string(),
            upstream_id: "openrouter".to_string(),
            real_model: "deepseek/deepseek-v4.1-pro".to_string(),
            wire: "anthropic".to_string(),
            is_family: true,
        },
        Route {
            name: "claude-haiku".to_string(),
            upstream_id: "openrouter".to_string(),
            real_model: "deepseek/deepseek-v4.1-flash".to_string(),
            wire: "anthropic".to_string(),
            is_family: true,
        },
        Route {
            name: "claude-fable".to_string(),
            upstream_id: "openrouter".to_string(),
            real_model: "deepseek/deepseek-v4.1-flash".to_string(),
            wire: "anthropic".to_string(),
            is_family: true,
        },
    ]
}

#[tokio::test]
async fn uninstall_restore_puts_all_three_clients_back_to_official_state() {
    let root = tmp("restore-clients");
    let store = seeded_store(&root).await;
    let (roots, official) = seed_three_clients(&root, &store).await;

    let report = restore::restore_clients(&store, &root.join("data"), 8787, &roots).await;
    assert!(
        !report.failed(),
        "三个客户端都该还原成功：{:?}",
        report.errors
    );
    assert_eq!(
        report.clients.len(),
        3,
        "三个客户端各一条回执：{:?}",
        report.clients.iter().map(|c| &c.client).collect::<Vec<_>>()
    );
    let clients: Vec<&str> = report.clients.iter().map(|c| c.client.as_str()).collect();
    for want in ["claude-desktop", "claude-code", "codex"] {
        assert!(clients.contains(&want), "缺少 {want} 的回执：{clients:?}");
    }

    // 三个客户端逐字节回到「网关动手之前」——这就是「回官方态」。
    for (path, before) in &official {
        assert_eq!(
            &read(path),
            before,
            "{} 没有回到官方态：\n{}",
            path.display(),
            read_text(path)
        );
    }

    // 网关自己新建的 Codex 目录文件：还原时整个删掉（不留空壳）。
    assert!(
        !roots
            .codex
            .as_ref()
            .expect("Codex 根")
            .join(codex::CATALOG_FILE)
            .is_file(),
        "网关新建的目录文件应当被删掉"
    );

    // 数据目录（含库与备份）一个字节都不动：卸载默认保留数据。
    assert!(root
        .join("data")
        .join(gateway_core::store::DB_FILE_NAME)
        .is_file());
    assert!(
        store
            .credential_secret("openrouter", "anthropic")
            .await
            .expect("查密钥")
            .is_some(),
        "还原客户端配置不许碰网关自己的数据"
    );
}

#[tokio::test]
async fn restore_leaves_unrelated_files_and_settings_alone() {
    let root = tmp("restore-unrelated");
    let store = seeded_store(&root).await;
    let (roots, _) = seed_three_clients(&root, &store).await;

    // 用户自己的别的文件（不是接管键清单里的目标文件）：还原不该碰。
    let cc_root = roots.claude_code.clone().expect("Claude Code 根");
    let unrelated = cc_root.join("其他文件.json");
    fs::write(&unrelated, "{\"user\": \"自己的东西\"}\n").expect("写无关文件");
    let before = read(&unrelated);

    restore::restore_clients(&store, &root.join("data"), 8787, &roots).await;

    assert_eq!(read(&unrelated), before, "无关文件必须一个字节不动");
}

#[tokio::test]
async fn a_second_restore_is_a_no_op() {
    let root = tmp("restore-idempotent");
    let store = seeded_store(&root).await;
    let (roots, official) = seed_three_clients(&root, &store).await;

    restore::restore_clients(&store, &root.join("data"), 8787, &roots).await;
    let second = restore::restore_clients(&store, &root.join("data"), 8787, &roots).await;

    assert!(!second.failed(), "{:?}", second.errors);
    // 第二次一个字节都不写（全是「不用动」）。
    let unchanged = second
        .clients
        .iter()
        .flat_map(|c| c.files.iter())
        .filter(|(_, o)| matches!(o, FileOutcome::Unchanged))
        .count();
    let total: usize = second.clients.iter().map(|c| c.files.len()).sum();
    assert!(
        unchanged >= total - 3,
        "第二次还原最多只有「文件本来不存在」这类空操作：{unchanged}/{total}"
    );
    for (path, before) in &official {
        assert_eq!(&read(path), before, "{} 被第二次还原改动了", path.display());
    }
}
