//! Claude Desktop 写入器的用例（票 46）。
//!
//! 四条主线：写入内容**逐字**、幂等（第二次不写、mtime 不变）、还原**逐字节**（别的 profile 一个
//! 字节不动）、网关新建且只剩网关内容的文件整份删；外加 §8.2 模型 ID 判据与「根来自环境变量」。
//!
//! 一律临时目录（[`WriteRoots`] 参数化）：真实 `%LOCALAPPDATA%\Claude-3p` 一个字节都不碰。

use super::desktop::{
    config_library_root, config_library_root_under, filter_anthropic_models, is_anthropic_model_id,
    meta_restore, plan, profile_rel_path, rollback, switch_to_1p, switch_to_3p, DesktopConfig,
    META_FILE, PROFILE_ID, PROFILE_NAME,
};
use super::plan::{FileOutcome, RollbackOutcome, WriteReport, WriteRoots};
use super::Writer;
use crate::store::Store;
use serde_json::Value as Json;
use std::fs;
use std::path::{Path, PathBuf};

/// 本机 pristine 1P 现场里的默认 profile id（取自 `configLibrary.backup-20261014-191819`）。
const DEFAULT_ID: &str = "ac278ce7-9506-4f19-b147-25637015682c";
const BASE_URL: &str = "http://127.0.0.1:8787";
const API_KEY: &str = "cg-local-key";

struct Env {
    writer: Writer,
    config: PathBuf,
}

fn env(tag: &str) -> Env {
    let root = std::env::temp_dir().join("cg46-desktop-tests").join(tag);
    let _ = fs::remove_dir_all(&root);
    let store = Store::open(&root.join("data")).expect("开库");
    let config = root.join("config");
    fs::create_dir_all(&config).expect("建配置根");
    let writer = Writer::new(
        store,
        WriteRoots {
            config_root: config.clone(),
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        },
    );
    Env { writer, config }
}

fn cfg() -> DesktopConfig {
    DesktopConfig::new(BASE_URL, API_KEY)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {} 失败：{e}", path.display()))
}

fn bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("读 {} 失败：{e}", path.display()))
}

fn mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).expect("建目录");
    }
    fs::write(path, text).expect("写文件");
}

/// 行尾换成 CRLF（本机客户端配置实测都是 CRLF；逐字节用例要按真实排版比）。
fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn paths(e: &Env) -> (PathBuf, PathBuf, PathBuf) {
    (
        e.config.join(META_FILE),
        e.config.join(format!("{DEFAULT_ID}.json")),
        e.config.join(format!("{PROFILE_ID}.json")),
    )
}

/// 首写前的干净 1P 现场：`_meta.json` 指着 Default、另一个 profile 有用户内容。
/// `gateway_profile` = 网关那份 profile 是否已经存在（存在 → 还原要把它变回原内容而不是删掉）。
fn pristine_1p(e: &Env, gateway_profile: bool) -> (PathBuf, PathBuf, PathBuf) {
    let (meta, other, mine) = paths(e);
    write(
        &meta,
        &crlf(&format!(
            "{{\n  \"appliedId\": \"{DEFAULT_ID}\",\n  \"entries\": [\n    {{\n      \"id\": \"{DEFAULT_ID}\",\n      \"name\": \"Default\"\n    }}\n  ]\n}}\n"
        )),
    );
    write(&other, &crlf("{\n  \"user\": \"keep\"\n}\n"));
    if gateway_profile {
        write(&mine, "{}\r\n");
    }
    (meta, other, mine)
}

/// 期望的 3P profile 逐字内容（新建文件：引擎默认排版 = 2 空格缩进、LF、末尾换行）。
fn expected_profile_3p() -> String {
    format!(
        "{{\n  \"inferenceProvider\": \"gateway\",\n  \"inferenceGatewayBaseUrl\": \"{BASE_URL}\",\n  \
         \"inferenceGatewayApiKey\": \"{API_KEY}\",\n  \"inferenceGatewayAuthScheme\": \"bearer\",\n  \
         \"inferenceCredentialKind\": \"static\",\n  \"inferenceModels\": [\n    {{\n      \"name\": \
         \"claude-fable\"\n    }},\n    {{\n      \"name\": \"claude-opus\"\n    }},\n    {{\n      \
         \"name\": \"claude-sonnet\"\n    }},\n    {{\n      \"name\": \"claude-haiku\"\n    }}\n  \
         ]\n}}\n"
    )
}

fn json(text: &str) -> Json {
    serde_json::from_str(text).expect("JSON 可解析")
}

fn rel(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().to_string()
}

fn outcome_of<'a>(report: &'a WriteReport, file: &str) -> &'a FileOutcome {
    report
        .files
        .iter()
        .find(|(p, _)| p.to_string_lossy().replace('\\', "/") == file)
        .map(|(_, o)| o)
        .unwrap_or_else(|| panic!("报告里没有 {file}：{:?}", report.files))
}

// ── 1. 装 3P：内容逐字 ────────────────────────────────────────────────────────

#[tokio::test]
async fn switch_to_3p_writes_the_expected_profile_and_meta() {
    let e = env("write-3p");
    let (meta, other, mine) = pristine_1p(&e, false);
    let other_before = bytes(&other);
    let other_mtime = mtime(&other);

    let report = switch_to_3p(&e.writer, &cfg()).await.unwrap();

    // 依赖序：profile 先、指针后（spec §8.4 第 6 条）。
    assert_eq!(
        report
            .files
            .iter()
            .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>(),
        vec![rel(&mine), META_FILE.to_string()]
    );
    // 原先没有这个文件 → 首写只记 marker（不造空备份）。
    assert!(
        matches!(
            outcome_of(&report, &rel(&mine)),
            FileOutcome::Written {
                first_write: true,
                marker_only: true,
                ..
            }
        ),
        "{:?}",
        outcome_of(&report, &rel(&mine))
    );

    // profile JSON：逐字。
    assert_eq!(read(&mine), expected_profile_3p());

    // _meta.json：网关条目追加在末尾、appliedId 指向网关，Default 那条原样。
    let meta_json = json(&read(&meta));
    assert_eq!(meta_json["appliedId"], Json::String(PROFILE_ID.into()));
    let entries = meta_json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["id"], Json::String(DEFAULT_ID.into()));
    assert_eq!(entries[0]["name"], Json::String("Default".into()));
    assert_eq!(entries[1]["id"], Json::String(PROFILE_ID.into()));
    assert_eq!(
        entries[1]["name"],
        Json::String(PROFILE_NAME.into()),
        "网关条目用我们的显示名"
    );
    assert_eq!(read(&meta), crlf(&format!(
        "{{\n  \"appliedId\": \"{PROFILE_ID}\",\n  \"entries\": [\n    {{\n      \"id\": \"{DEFAULT_ID}\",\n      \"name\": \"Default\"\n    }},\n    {{\n      \"id\": \"{PROFILE_ID}\",\n      \"name\": \"{PROFILE_NAME}\"\n    }}\n  ]\n}}\n"
    )));

    // 别的 profile 一个字节不动（连 mtime 都不变）。
    assert_eq!(bytes(&other), other_before);
    assert_eq!(mtime(&other), other_mtime);
}

#[tokio::test]
async fn base_url_is_written_without_the_v1_suffix() {
    let e = env("no-v1");
    pristine_1p(&e, false);
    let cfg = DesktopConfig::new("http://127.0.0.1:8787/v1/", API_KEY);

    switch_to_3p(&e.writer, &cfg).await.unwrap();

    let written = json(&read(&e.config.join(format!("{PROFILE_ID}.json"))));
    assert_eq!(
        written["inferenceGatewayBaseUrl"],
        Json::String("http://127.0.0.1:8787".into())
    );
}

// ── 2. 幂等：连切两次 3P，第二次不写 ──────────────────────────────────────────

#[tokio::test]
async fn switching_to_3p_twice_does_not_touch_the_files() {
    let e = env("idempotent-3p");
    let (meta, _, mine) = pristine_1p(&e, true);
    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    let mine_text = bytes(&mine);
    let meta_text = bytes(&meta);
    let mine_mtime = mtime(&mine);
    let meta_mtime = mtime(&meta);

    let report = switch_to_3p(&e.writer, &cfg()).await.unwrap();

    assert!(matches!(
        outcome_of(&report, &rel(&mine)),
        FileOutcome::Unchanged
    ));
    assert!(matches!(
        outcome_of(&report, META_FILE),
        FileOutcome::Unchanged
    ));
    assert_eq!(bytes(&mine), mine_text);
    assert_eq!(bytes(&meta), meta_text);
    assert_eq!(mtime(&mine), mine_mtime);
    assert_eq!(mtime(&meta), meta_mtime);
}

// ── 3. 还原：逐字节回到首写前；别的 profile 不动 ──────────────────────────────

#[tokio::test]
async fn restore_returns_every_file_to_its_pre_write_bytes() {
    let e = env("restore-byte-exact");
    let (meta, other, mine) = pristine_1p(&e, true);
    let before: Vec<Vec<u8>> = [&meta, &other, &mine].iter().map(|p| bytes(p)).collect();
    let other_mtime = mtime(&other);

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    assert_ne!(bytes(&mine), before[2], "3P 之后 profile 必须变过");
    let report = switch_to_1p(&e.writer, &cfg()).await.unwrap();

    assert!(matches!(
        outcome_of(&report, &rel(&mine)),
        FileOutcome::Written { .. }
    ));
    assert_eq!(bytes(&meta), before[0], "_meta.json 回到首写前");
    assert_eq!(bytes(&other), before[1], "别的 profile 一个字节不动");
    assert_eq!(bytes(&mine), before[2], "profile 回到首写前");
    assert_eq!(mtime(&other), other_mtime);
}

#[tokio::test]
async fn restore_is_idempotent_and_keeps_user_keys() {
    let e = env("restore-idempotent");
    let (_, _, mine) = pristine_1p(&e, true);
    // 网关 profile 带一个非接管键：还原只摘网关键，文件留着。
    write(&mine, &crlf("{\n  \"custom\": 1\n}\n"));

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    switch_to_1p(&e.writer, &cfg()).await.unwrap();
    let after_first = bytes(&mine);
    assert_eq!(json(&read(&mine))["custom"], Json::from(1));
    assert_eq!(json(&read(&mine)).get("inferenceProvider"), None);

    let report = switch_to_1p(&e.writer, &cfg()).await.unwrap();

    assert!(matches!(
        outcome_of(&report, &rel(&mine)),
        FileOutcome::Unchanged
    ));
    assert_eq!(bytes(&mine), after_first);
}

/// 第 6 个接管键 `inferenceCredentialKind`：还原必须写回**用户原来的值**。
///
/// 评审「高」第 4 条 / 票 46:69 自认：从前这条走摘键 → 用户原有的 `interactive`（SSO）静默丢失，
/// 只有整份回滚能救。现在按首写快照里的旧值还回去。
#[tokio::test]
async fn restore_puts_back_the_users_own_credential_kind() {
    let e = env("credential-kind-restore");
    let (_, _, mine) = pristine_1p(&e, false);
    write(
        &mine,
        &crlf("{\n  \"inferenceCredentialKind\": \"interactive\",\n  \"custom\": 1\n}\n"),
    );

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    assert_eq!(
        json(&read(&mine))["inferenceCredentialKind"],
        Json::String("static".into()),
        "3P 期间以网关为准"
    );

    switch_to_1p(&e.writer, &cfg()).await.unwrap();

    let back = json(&read(&mine));
    assert_eq!(
        back["inferenceCredentialKind"],
        Json::String("interactive".into()),
        "还原要写回用户原来的凭据模式：{back}"
    );
    assert_eq!(back["custom"], Json::from(1), "别的键一个字节不动：{back}");
    assert_eq!(
        back.get("inferenceProvider"),
        None,
        "其余接管键照旧摘掉：{back}"
    );
}

/// 首写时**没有**这个键 → 还原后仍然没有（不许凭空造一个 `null` / 空串）。
#[tokio::test]
async fn restore_leaves_the_credential_kind_absent_when_the_user_never_had_one() {
    let e = env("credential-kind-absent");
    let (_, _, mine) = pristine_1p(&e, false);
    write(&mine, &crlf("{\n  \"custom\": 1\n}\n"));

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    assert_eq!(
        json(&read(&mine))["inferenceCredentialKind"],
        Json::String("static".into())
    );

    switch_to_1p(&e.writer, &cfg()).await.unwrap();

    let back = json(&read(&mine));
    assert_eq!(
        back.get("inferenceCredentialKind"),
        None,
        "首写时没有这个键，还原后也不该有：{back}"
    );
    assert_eq!(back["custom"], Json::from(1), "{back}");
}

// ── 4. 网关新建且只剩网关内容的文件：整份删 ──────────────────────────────────
#[tokio::test]
async fn restore_deletes_the_files_the_gateway_created() {
    let e = env("restore-delete");
    let (meta, other, mine) = paths(&e); // 干净机器：configLibrary 里什么都没有
    assert!(!meta.exists() && !mine.exists());

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    assert!(meta.exists() && mine.exists());
    let report = switch_to_1p(&e.writer, &cfg()).await.unwrap();

    assert!(matches!(
        outcome_of(&report, &rel(&mine)),
        FileOutcome::Removed { .. }
    ));
    assert!(matches!(
        outcome_of(&report, META_FILE),
        FileOutcome::Removed { .. }
    ));
    assert!(!meta.exists() && !mine.exists());
    assert!(!other.exists(), "别的 profile 不该被造出来");
}

// ── 5. appliedId 回退 / 摘掉 ────────────────────────────────────────────────

#[tokio::test]
async fn applied_id_falls_back_to_the_surviving_entry() {
    let e = env("applied-fallback");
    let (meta, _, _) = pristine_1p(&e, false);
    switch_to_3p(&e.writer, &cfg()).await.unwrap();

    switch_to_1p(&e.writer, &cfg()).await.unwrap();

    let meta_json = json(&read(&meta));
    assert_eq!(meta_json["appliedId"], Json::String(DEFAULT_ID.into()));
    assert_eq!(meta_json["entries"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn applied_id_is_dropped_when_no_entry_survives() {
    let e = env("applied-drop");
    // 只有网关自己那一条 entry 的 _meta.json。
    let (meta, _, _) = paths(&e);
    write(
        &meta,
        &crlf(&format!(
            "{{\n  \"appliedId\": \"{PROFILE_ID}\",\n  \"entries\": [\n    {{\n      \"id\": \"{PROFILE_ID}\",\n      \"name\": \"{PROFILE_NAME}\"\n    }}\n  ]\n}}\n"
        )),
    );

    switch_to_1p(&e.writer, &cfg()).await.unwrap();

    let meta_json = json(&read(&meta));
    assert_eq!(meta_json.get("appliedId"), None);
    assert_eq!(meta_json.get("entries"), None);
}

// ── 6. meta_restore 读磁盘（解析失败即停） ───────────────────────────────────

#[test]
fn meta_restore_reads_the_current_disk_state() {
    let e = env("meta-restore-read");
    pristine_1p(&e, false);
    let r = meta_restore(&e.config).unwrap();
    assert_eq!(r.applied_fallback, Some(Json::String(DEFAULT_ID.into())));
    assert!(!r.drop_entries_when_empty);

    // 没有 _meta.json：没有回退目标，摘完就整份删。
    fs::remove_file(e.config.join(META_FILE)).unwrap();
    let r = meta_restore(&e.config).unwrap();
    assert_eq!(r.applied_fallback, None);
    assert!(r.drop_entries_when_empty);

    // 解析不过 → 报错，绝不猜。
    write(&e.config.join(META_FILE), "{ 这不是 JSON");
    assert!(meta_restore(&e.config).is_err());
}

// ── 7. 整份回滚（第二选项） ─────────────────────────────────────────────────

#[tokio::test]
async fn rollback_restores_the_first_snapshot_byte_for_byte() {
    let e = env("rollback");
    let (meta, other, mine) = pristine_1p(&e, true);
    let before = [bytes(&meta), bytes(&other), bytes(&mine)];

    switch_to_3p(&e.writer, &cfg()).await.unwrap();
    let outcomes = rollback(&e.writer, &cfg()).await.unwrap();

    assert!(
        outcomes
            .iter()
            .all(|(_, o)| matches!(o, RollbackOutcome::Rolled { .. })),
        "{outcomes:?}"
    );
    assert_eq!(bytes(&meta), before[0], "_meta.json 按快照整份回去");
    assert_eq!(bytes(&other), before[1]);
    assert_eq!(bytes(&mine), before[2], "profile 按快照整份回去");
}

// ── 8. §8.2 模型 ID 判据（票 16 逐字） ───────────────────────────────────────

#[test]
fn model_id_predicate_follows_spec_8_2() {
    let legal = [
        // 四个家族名（子串 claude）
        "claude-fable",
        "claude-opus",
        "claude-sonnet",
        "claude-haiku",
        // 裸档位名（可带 -<数字>）
        "sonnet",
        "opus",
        "haiku",
        "fable",
        "mythos",
        "opus-4.5",
        "haiku-4",
        // 子串 claude / 档位名 / anthropic
        "my-claude-thing",
        "claude-3-5-sonnet",
        "mock-anthropic-text",
        "anthropic/claude-3",
        // 子串命中就够，形状不限（`sonnet4` 不在裸档位名正则里，但含 `sonnet`）
        "sonnet4",
    ];
    let illegal = [
        // 黑名单：命中即非法，含 claude 也救不回
        "deepseek-v4.1-flash",
        "claude-deepseek-v4-pro",
        "gpt-5",
        "openai/gpt-4o",
        "gemini-2.0-flash",
        "qwen3",
        "llama-3-70b",
        "glm-4",
        "kimi-k2",
        "phi-3",
        "phi4",
        "codex-mini",
        "ds-v3",
        "dpsk/x",
        "m2.1",
        "k2.5",
        "ling",
        "ling-1",
        "unic",
        "xiaomi-mimo",
        "step-3.5",
        "nova-pro",
        "amazon.nova-micro",
        "grok-4",
        "command-r-plus",
        // 不在黑名单，但也没有任何合法子串
        "unicorn",
    ];
    for name in legal {
        assert!(is_anthropic_model_id(name), "{name} 应该合法");
    }
    for name in illegal {
        assert!(!is_anthropic_model_id(name), "{name} 应该非法");
    }

    // 逐条剔除（无 fail-all），大小写不敏感。
    let names: Vec<String> = vec![
        "claude-fable".into(),
        "deepseek-v4.1-flash".into(),
        "CLAUDE-Opus".into(),
    ];
    let (kept, dropped) = filter_anthropic_models(&names);
    assert_eq!(
        kept,
        vec!["claude-fable".to_string(), "CLAUDE-Opus".to_string()]
    );
    assert_eq!(dropped, vec!["deepseek-v4.1-flash".to_string()]);
}

// ── 9. 根来自环境变量 ──────────────────────────────────────────────────────

#[test]
fn config_library_root_comes_from_local_app_data() {
    let local = std::env::var_os("LOCALAPPDATA").expect("Windows 上 LOCALAPPDATA 一定有");
    let root = config_library_root().expect("LOCALAPPDATA 在，根就该算得出来");
    assert_eq!(root, config_library_root_under(Path::new(&local)));
    assert!(
        root.ends_with(Path::new("Claude-3p").join("configLibrary")),
        "{}",
        root.display()
    );
}

// ── 10. 计划里的接管键清单（写与还原共用一份） ──────────────────────────────

#[test]
fn plan_carries_the_six_gateway_keys_and_the_pointer() {
    let e = env("plan-shape");
    pristine_1p(&e, false);
    let plan = plan(&e.config, &cfg()).unwrap();
    assert_eq!(plan.files.len(), 2);

    let profile = &plan.files[0];
    assert_eq!(profile.client, "claude-desktop");
    assert_eq!(profile.rel_path, profile_rel_path());
    assert_eq!(
        profile
            .write_ops
            .iter()
            .map(|o| o.path().join("."))
            .collect::<Vec<_>>(),
        vec![
            "inferenceProvider",
            "inferenceGatewayBaseUrl",
            "inferenceGatewayApiKey",
            "inferenceGatewayAuthScheme",
            "inferenceCredentialKind",
            "inferenceModels",
        ]
    );
    // 一份清单两面：还原动作与写入动作一一对应。
    assert_eq!(profile.restore_ops.len(), profile.write_ops.len());

    let meta = &plan.files[1];
    assert_eq!(meta.rel_path, PathBuf::from(META_FILE));
    assert_eq!(meta.write_ops.len(), 2);
    assert_eq!(meta.restore_ops.len(), 2);
}

// ── 11. 真实路径：手动跑（`check.ps1 -Live` 会跑 #[ignore]，所以没 env 就直接返回） ──

/// 真实 `%LOCALAPPDATA%\Claude-3p\configLibrary\` 上走一次「装 3P → 切回 1P」。
///
/// **只在显式给了两个环境变量时动作**，否则直接返回（当跳过）：`CG_DESKTOP_REAL_RUN=1`、
/// `CG_DESKTOP_REAL_ROOT=<configLibrary 绝对路径>`；备份 / 日志 / 库写到 `CG_DESKTOP_REAL_DATA`
/// （临时目录，两次调用要指**同一个**目录，记录才连续）。
///
/// `CG_DESKTOP_REAL_STEP` 切片：`both`（默认）/ `3p` 只装 3P / `1p` 只切回——分两次跑才能在中间对
/// 磁盘做校验和（收口验收里「真实路径那半」就是这么跑的）。
#[tokio::test]
#[ignore]
async fn real_config_library_switch_and_restore() {
    if std::env::var("CG_DESKTOP_REAL_RUN").as_deref() != Ok("1") {
        eprintln!("跳过：没给 CG_DESKTOP_REAL_RUN=1");
        return;
    }
    let root =
        PathBuf::from(std::env::var_os("CG_DESKTOP_REAL_ROOT").expect("要给 CG_DESKTOP_REAL_ROOT"));
    let data =
        PathBuf::from(std::env::var_os("CG_DESKTOP_REAL_DATA").expect("要给 CG_DESKTOP_REAL_DATA"));
    let step = std::env::var("CG_DESKTOP_REAL_STEP").unwrap_or_else(|_| "both".to_string());
    let store = Store::open(&data.join("db")).expect("开库");
    let writer = Writer::new(store, WriteRoots::with_data_dir(&root, &data));
    let cfg = DesktopConfig::new(BASE_URL, API_KEY);

    let dump = |label: &str| {
        println!("== {label} ==");
        for name in [format!("{PROFILE_ID}.json"), META_FILE.to_string()] {
            match fs::read_to_string(root.join(&name)) {
                Ok(text) => println!("--- {name} ---\n{text}"),
                Err(e) => println!("--- {name} --- <{e}>"),
            }
        }
    };

    if step != "1p" {
        let report = switch_to_3p(&writer, &cfg).await.unwrap();
        println!("== 装 3P ==\n{report:#?}");
        dump("装 3P 之后");
    }
    if step != "3p" {
        let report = switch_to_1p(&writer, &cfg).await.unwrap();
        println!("== 切回 1P（摘键）==\n{report:#?}");
        dump("切回 1P 之后");
    }
}
