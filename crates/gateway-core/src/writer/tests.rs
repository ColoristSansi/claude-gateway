//! writer 的单测：格式保真 / 幂等 / 冲突 / 备份 / 还原 / 原子替换 / 多文件顺序 / 记账。
//!
//! 一律指向临时目录（写入根参数化）：**不碰真实 `%LOCALAPPDATA%\Claude-3p`、`~/.claude`、
//! `~/.codex`**。

use super::doc::{json_eq, Doc, Fidelity};
use super::plan::{
    ConflictMode, FileOutcome, FileSpec, Format, OwnedOp, RollbackOutcome, WritePlan, WriteRoots,
};
use super::state;
use super::Writer;
use crate::store::Store;
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg45-writer-tests").join(tag);
    let _ = fs::remove_dir_all(&root);
    root
}

struct Env {
    writer: Writer,
    root: PathBuf,
    config: PathBuf,
}

fn env(tag: &str) -> Env {
    let root = tmp_root(tag);
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
    Env {
        writer,
        root,
        config,
    }
}

fn p(segments: &[&str]) -> Vec<String> {
    segments.iter().map(|s| s.to_string()).collect()
}

fn set(segments: &[&str], value: Json) -> OwnedOp {
    OwnedOp::Set {
        path: p(segments),
        value,
    }
}

fn remove(segments: &[&str]) -> OwnedOp {
    OwnedOp::Remove { path: p(segments) }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("读文件")
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("读文件字节")
}

fn mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

fn write_file(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).expect("建目录");
    }
    fs::write(path, text).expect("写文件");
}

/// 备份目录里的文件（按名排序）。
fn backups_of(root: &Path, client: &str) -> Vec<PathBuf> {
    let dir = root.join("backups").join(client);
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

fn cg_tmp_leftovers(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().contains("cg-tmp"))
        .collect()
}

/// 一句「客户端接管一个 JSON 文件的标注」：`config.json` 的 `inferenceProvider`。
fn provider_spec() -> FileSpec {
    FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Json,
        vec![set(&["inferenceProvider"], json!("gateway"))],
    )
    .with_restore(vec![remove(&["inferenceProvider"])])
}

// ── 幂等 / 保序 / 只动接管键 ───────────────────────────────────────────────────

#[tokio::test]
async fn json_write_is_idempotent_and_keeps_user_keys_and_layout() {
    let e = env("json-idempotent");
    let file = e.config.join("settings.json");
    let original = "{\n  \"user\": 1,\n  \"env\": {\n    \"KEEP\": \"x\"\n  }\n}\n";
    write_file(&file, original);

    let spec = FileSpec::new(
        "claude-code",
        "settings.json",
        Format::Json,
        vec![
            set(
                &["env", "ANTHROPIC_BASE_URL"],
                json!("http://127.0.0.1:8787"),
            ),
            set(&["env", "ANTHROPIC_AUTH_TOKEN"], json!("cg-local")),
        ],
    )
    .with_restore(vec![
        remove(&["env", "ANTHROPIC_BASE_URL"]),
        remove(&["env", "ANTHROPIC_AUTH_TOKEN"]),
    ]);

    let first = e
        .writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    match &first {
        FileOutcome::Written {
            first_write: true,
            marker_only: false,
            notice,
            ..
        } => assert!(notice.as_ref().unwrap().contains("首次接管"), "{notice:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(first_backup_count(&e.root, "claude-code"), 1);

    let text = read(&file);
    let v: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["env"]["ANTHROPIC_BASE_URL"],
        json!("http://127.0.0.1:8787")
    );
    assert_eq!(v["env"]["KEEP"], json!("x"), "非接管键必须原样保留");
    assert_eq!(v["user"], json!(1));
    assert!(
        text.find("\"user\"").unwrap() < text.find("\"env\"").unwrap(),
        "JSON 必须保键序：{text}"
    );
    assert!(text.contains("\n  \"user\": 1"), "沿用原缩进：{text}");

    // 幂等：语义相同 → 完全不写，连 mtime 都不变。
    let before = mtime(&file);
    let again = e
        .writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    assert_eq!(again, FileOutcome::Unchanged);
    assert_eq!(mtime(&file), before, "无变化时连 mtime 都不许变");
    assert_eq!(read(&file), text);

    // 要写则连续两次字节相同：把文件还原成网关写之前的样子，再写一次（认领模式绕过冲突）。
    write_file(&file, original);
    let third = e
        .writer
        .write_one(&spec, ConflictMode::Claim)
        .await
        .unwrap();
    assert!(matches!(third, FileOutcome::Written { .. }), "{third:?}");
    assert_eq!(read(&file), text, "同一输入必须产出同一份字节");
}

fn first_backup_count(root: &Path, client: &str) -> usize {
    backups_of(root, client).len()
}

#[tokio::test]
async fn user_edit_to_a_non_owned_key_is_not_a_conflict_and_survives() {
    let e = env("non-owned-edit");
    let file = e.config.join("config.json");
    write_file(
        &file,
        "{\n  \"user\": 1,\n  \"inferenceProvider\": \"anthropic\"\n}\n",
    );
    let spec = provider_spec();
    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();

    // 用户改了**非**接管键 → 不算冲突，也不被抹掉。
    let mut v: Json = serde_json::from_str(&read(&file)).unwrap();
    v["user"] = json!(2);
    write_file(&file, &serde_json::to_string_pretty(&v).unwrap());

    let out = e
        .writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    assert_eq!(out, FileOutcome::Unchanged, "非接管键变了不算冲突");
    assert!(read(&file).contains("\"user\": 2"));

    // 目标值变了才真写，非接管键照样在。
    let spec2 = FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Json,
        vec![
            set(&["inferenceProvider"], json!("gateway")),
            set(&["inferenceBaseUrl"], json!("http://127.0.0.1:8787")),
        ],
    );
    let out = e
        .writer
        .write_one(&spec2, ConflictMode::Abort)
        .await
        .unwrap();
    assert!(matches!(out, FileOutcome::Written { .. }), "{out:?}");
    let text = read(&file);
    assert!(text.contains("\"user\": 2"), "{text}");
}

#[tokio::test]
async fn parse_failure_stops_without_writing_or_backing_up() {
    let e = env("parse-failure");
    let file = e.config.join("config.json");
    let broken = "{\n  \"a\": ,\n}\n";
    write_file(&file, broken);

    let err = e
        .writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .expect_err("解析失败必须报错");
    let msg = format!("{err:#}");
    assert!(msg.contains("解析失败即停"), "{msg}");
    assert!(msg.contains("第 2 行第 8 列"), "报错要给行列位置：{msg}");
    assert_eq!(read(&file), broken, "绝不退回空文档重写");
    assert!(
        backups_of(&e.root, "claude-desktop").is_empty(),
        "解析失败即停：连备份都不该留"
    );
}

#[tokio::test]
async fn jsonc_comments_and_trailing_commas_are_accepted_and_bom_crlf_kept() {
    let e = env("jsonc-fidelity");
    let file = e.config.join("config.json");
    let mut original = Vec::new();
    original.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    original.extend_from_slice("{\r\n  // 用户注释\r\n  \"user\": 1,\r\n}\r\n".as_bytes());
    fs::write(&file, &original).unwrap();

    let jsonc_spec = FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Jsonc,
        vec![set(&["inferenceProvider"], json!("gateway"))],
    )
    .with_restore(vec![remove(&["inferenceProvider"])]);
    let out = e
        .writer
        .write_one(&jsonc_spec, ConflictMode::Abort)
        .await
        .unwrap();
    assert!(matches!(out, FileOutcome::Written { .. }), "{out:?}");

    let bytes = read_bytes(&file);
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "BOM 要沿用");
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            assert_eq!(bytes[i - 1], b'\r', "CRLF 文件写回必须还是 CRLF");
        }
    }
    assert!(bytes.ends_with(b"\r\n"), "末尾换行沿用");
    let v: Json = serde_json::from_slice(&bytes[3..]).expect("结果是合法 JSON");
    assert_eq!(v["inferenceProvider"], json!("gateway"));
    assert_eq!(v["user"], json!(1), "非接管键保住");
}

// ── 冲突 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn conflict_gives_up_and_leaves_the_file_untouched() {
    let e = env("conflict");
    let file = e.config.join("config.json");
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");
    let spec = provider_spec();
    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    let backups_before = backups_of(&e.root, "claude-desktop").len();

    // 用户手改**接管键** → 冲突：放弃本次 + 提示，不覆盖。
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");
    let text = read(&file);
    let before = mtime(&file);
    let out = e
        .writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    match out {
        FileOutcome::Conflict {
            path,
            recorded,
            current,
        } => {
            assert_eq!(path, p(&["inferenceProvider"]));
            assert_eq!(recorded, json!("gateway"));
            assert_eq!(current, json!("anthropic"));
        }
        other => panic!("应当冲突放弃，实得 {other:?}"),
    }
    assert_eq!(read(&file), text, "冲突不许覆盖");
    assert_eq!(mtime(&file), before);
    assert_eq!(
        backups_of(&e.root, "claude-desktop").len(),
        backups_before,
        "放弃 = 不写也不备份"
    );
}

#[tokio::test]
async fn claim_mode_overwrites_the_owned_key() {
    let e = env("conflict-claim");
    let file = e.config.join("config.json");
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");
    let spec = provider_spec();
    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");

    let out = e
        .writer
        .write_one(&spec, ConflictMode::Claim)
        .await
        .unwrap();
    match out {
        FileOutcome::Written { notice, .. } => {
            assert!(notice.unwrap().contains("认领并覆盖"));
        }
        other => panic!("认领模式应当写，实得 {other:?}"),
    }
    assert!(read(&file).contains("\"gateway\""));
}

#[tokio::test]
async fn a_different_owned_key_list_is_treated_as_first_takeover() {
    let e = env("list-changed");
    let file = e.config.join("config.json");
    write_file(&file, "{}\n");
    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();

    // 清单变了（多了一个键）→ 不当冲突，按首次接管（先备份 + 提示）。
    let spec2 = FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Json,
        vec![
            set(&["inferenceProvider"], json!("gateway")),
            set(&["inferenceModels"], json!(["claude-sonnet-4-5"])),
        ],
    );
    match e
        .writer
        .write_one(&spec2, ConflictMode::Abort)
        .await
        .unwrap()
    {
        FileOutcome::Written { notice, .. } => {
            assert!(notice.unwrap().contains("清单与上次记录不符"), "要提示");
        }
        other => panic!("{other:?}"),
    }
}

// ── 备份 + 记账 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn first_write_snapshots_the_original_and_records_state() {
    let e = env("first-snapshot");
    let file = e.config.join("config.json");
    let original = "{\n  \"inferenceProvider\": \"anthropic\"\n}\n";
    write_file(&file, original);

    let out = e
        .writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert!(matches!(
        out,
        FileOutcome::Written {
            first_write: true,
            marker_only: false,
            ..
        }
    ));

    let bks = backups_of(&e.root, "claude-desktop");
    assert_eq!(bks.len(), 1);
    assert_eq!(read(&bks[0]), original, "首写快照 = 写之前的整份文件");
    let name = bks[0].file_name().unwrap().to_string_lossy().to_string();
    let stamp = name
        .strip_prefix("config.json.")
        .expect("命名 <文件名>.<时间戳>");
    assert_eq!(stamp.len(), 15, "{name}");
    assert_eq!(stamp.as_bytes()[8], b'-', "{name}");
    assert!(stamp[..8].chars().all(|c| c.is_ascii_digit()), "{name}");

    let rec = state::load(e.writer.store(), "claude-desktop", "config.json")
        .await
        .unwrap()
        .expect("必须记库");
    assert_eq!(rec.last_op, "write");
    assert!(rec.existed_at_first_write);
    assert_eq!(rec.backup_path.as_deref(), Some(bks[0].as_path()));
    assert_eq!(rec.snapshot.values, vec![json!("gateway")]);

    // 再写一次（多一个接管键 → 清单变了，按首次接管处理），首写快照路径仍不被覆盖。
    let spec3 = FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Json,
        vec![
            set(&["inferenceProvider"], json!("gateway")),
            set(&["inferenceGatewayBaseUrl"], json!("http://127.0.0.1:8787")),
        ],
    );
    e.writer
        .write_one(&spec3, ConflictMode::Abort)
        .await
        .unwrap();
    let rec2 = state::load(e.writer.store(), "claude-desktop", "config.json")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec2.backup_path.as_deref(),
        Some(bks[0].as_path()),
        "首写快照路径记下就不再改"
    );
}

#[tokio::test]
async fn first_write_on_a_missing_file_writes_a_marker_only() {
    let e = env("first-marker");
    let out = e
        .writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert!(matches!(
        out,
        FileOutcome::Written {
            first_write: true,
            marker_only: true,
            ..
        }
    ));

    let file = e.config.join("config.json");
    assert!(file.exists(), "文件该被建出来");
    let bks = backups_of(&e.root, "claude-desktop");
    assert_eq!(bks.len(), 1, "首写时文件不存在：只写 marker，不造空备份");
    let name = bks[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with(".absent"), "{name}");
    assert!(read(&bks[0]).contains("\"absent\":true"));

    let rec = state::load(e.writer.store(), "claude-desktop", "config.json")
        .await
        .unwrap()
        .unwrap();
    assert!(!rec.existed_at_first_write);
}

// ── 还原 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn snip_restore_drops_owned_keys_and_pointer_entries_only() {
    let e = env("restore-snip");
    let file = e.config.join("_meta.json");
    write_file(
        &file,
        "{\n  \"appliedId\": \"ac278ce7-9506-4f19-b147-25637015682c\",\n  \"entries\": [\n    {\n      \"id\": \"ac278ce7-9506-4f19-b147-25637015682c\",\n      \"name\": \"Default\"\n    }\n  ]\n}\n",
    );
    let id = "9f1c2b3a-4d5e-4f60-8a71-000000000001";
    let spec = FileSpec::new(
        "claude-desktop",
        "_meta.json",
        Format::Json,
        vec![
            OwnedOp::SetElement {
                path: p(&["entries"]),
                field: "id".into(),
                value: json!({ "id": id, "name": "Claude Gateway" }),
            },
            set(&["appliedId"], json!(id)),
        ],
    )
    .with_restore(vec![
        OwnedOp::RemoveElement {
            path: p(&["entries"]),
            field: "id".into(),
            matches: json!(id),
        },
        OwnedOp::ReplaceIfEquals {
            path: p(&["appliedId"]),
            equals: json!(id),
            value: None,
        },
    ]);

    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    let written: Json = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(written["appliedId"], json!(id));
    assert_eq!(
        written["entries"].as_array().unwrap().len(),
        2,
        "网关条目 upsert 进去"
    );

    let out = e.writer.restore_one(&spec).await.unwrap();
    assert!(matches!(out, FileOutcome::Written { .. }), "{out:?}");
    let restored: Json = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(
        restored["entries"],
        json!([{ "id": "ac278ce7-9506-4f19-b147-25637015682c", "name": "Default" }]),
        "只摘网关条目，别的 profile 不动"
    );
    assert!(restored.get("appliedId").is_none(), "指针指向网关 → 摘掉");

    let rec = state::load(e.writer.store(), "claude-desktop", "_meta.json")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.last_op, "restore");

    // 再摘一次：已经干净 → 完全不写。
    let before = mtime(&file);
    assert_eq!(
        e.writer.restore_one(&spec).await.unwrap(),
        FileOutcome::Unchanged
    );
    assert_eq!(mtime(&file), before);
}

#[tokio::test]
async fn restore_deletes_a_file_the_gateway_created_and_emptied() {
    let e = env("restore-delete");
    let file = e.config.join("settings.json");
    let spec = FileSpec::new(
        "claude-code",
        "settings.json",
        Format::Json,
        vec![set(
            &["env", "ANTHROPIC_BASE_URL"],
            json!("http://127.0.0.1:8787"),
        )],
    )
    .with_restore(vec![OwnedOp::RemoveIfEmpty {
        path: p(&["env", "ANTHROPIC_BASE_URL"]),
    }]);

    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    assert!(file.exists());
    match e.writer.restore_one(&spec).await.unwrap() {
        FileOutcome::Removed { .. } => {}
        other => panic!("网关新建且只剩网关内容 → 整个删，实得 {other:?}"),
    }
    assert!(!file.exists());
}

#[tokio::test]
async fn restore_keeps_a_file_that_has_user_content() {
    let e = env("restore-keep");
    let file = e.config.join("settings.json");
    let spec = FileSpec::new(
        "claude-code",
        "settings.json",
        Format::Json,
        vec![set(
            &["env", "ANTHROPIC_BASE_URL"],
            json!("http://127.0.0.1:8787"),
        )],
    )
    .with_restore(vec![OwnedOp::RemoveIfEmpty {
        path: p(&["env", "ANTHROPIC_BASE_URL"]),
    }]);

    e.writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap();
    // 用户后来自己加了内容。
    let mut v: Json = serde_json::from_str(&read(&file)).unwrap();
    v["user"] = json!("mine");
    write_file(&file, &serde_json::to_string_pretty(&v).unwrap());

    e.writer.restore_one(&spec).await.unwrap();
    assert!(file.exists(), "还有用户内容 → 留着");
    let v: Json = serde_json::from_str(&read(&file)).unwrap();
    assert!(v.get("env").is_none(), "空壳 env 摘掉");
    assert_eq!(v["user"], json!("mine"));
}

#[tokio::test]
async fn restore_without_restore_ops_is_a_loud_error() {
    let e = env("restore-no-ops");
    write_file(&e.config.join("config.json"), "{}\n");
    // `FileSpec::new` 没配 `with_restore` → 不许悄悄当「无事可做」。
    let no_restore = FileSpec::new(
        "claude-desktop",
        "config.json",
        Format::Json,
        vec![set(&["inferenceProvider"], json!("gateway"))],
    );
    let err = e
        .writer
        .restore_one(&no_restore)
        .await
        .expect_err("没有还原动作必须报错");
    assert!(format!("{err:#}").contains("没有还原动作"), "{err:#}");
}

// ── 整份回滚 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn rollback_restores_the_first_snapshot_byte_for_byte() {
    let e = env("rollback-ok");
    let file = e.config.join("config.json");
    let original = "{\r\n  \"inferenceProvider\": \"anthropic\",\r\n  \"user\": 7\r\n}\r\n";
    write_file(&file, original);
    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert_ne!(read(&file), original);

    match e.writer.rollback(&provider_spec()).await.unwrap() {
        RollbackOutcome::Rolled { deleted, .. } => assert!(!deleted),
        other => panic!("{other:?}"),
    }
    assert_eq!(read(&file), original, "整份回滚 = 首写快照逐字节");
}

#[tokio::test]
async fn rollback_refuses_when_the_file_drifted_after_our_write() {
    let e = env("rollback-drift");
    let file = e.config.join("config.json");
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");
    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    // 外部改动：手改接管键。
    write_file(&file, "{\n  \"inferenceProvider\": \"someone-else\"\n}\n");
    let text = read(&file);

    match e.writer.rollback(&provider_spec()).await.unwrap() {
        RollbackOutcome::Refused { reason } => {
            assert!(reason.contains("外部改过"), "{reason}");
            assert!(reason.contains("inferenceProvider"), "{reason}");
        }
        other => panic!("应当拒绝，实得 {other:?}"),
    }
    assert_eq!(read(&file), text, "拒绝 = 一个字节都不动");
}

#[tokio::test]
async fn rollback_without_any_record_is_refused() {
    let e = env("rollback-norecord");
    write_file(&e.config.join("config.json"), "{}\n");
    match e.writer.rollback(&provider_spec()).await.unwrap() {
        RollbackOutcome::Refused { reason } => assert!(reason.contains("没有网关写入记录")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn rollback_deletes_a_file_the_gateway_created() {
    let e = env("rollback-created");
    let file = e.config.join("config.json");
    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert!(file.exists());
    match e.writer.rollback(&provider_spec()).await.unwrap() {
        RollbackOutcome::Rolled { deleted: true, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(!file.exists());
}

// ── 原子替换 / 占用 / 崩溃窗口 ────────────────────────────────────────────────

#[cfg(windows)]
#[tokio::test]
async fn busy_file_is_retried_then_reported_with_paths_and_hint() {
    use std::os::windows::fs::OpenOptionsExt;
    let e = env("busy");
    let file = e.config.join("config.json");
    write_file(&file, "{\n  \"inferenceProvider\": \"anthropic\"\n}\n");
    let text = read(&file);

    // 别的客户端正占用：只给读共享，写/删一律被拒。
    const FILE_SHARE_READ: u32 = 0x1;
    let _locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&file)
        .unwrap();

    let started = std::time::Instant::now();
    let err = e
        .writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .expect_err("被占用必须报错");
    let msg = format!("{err:#}");
    assert!(msg.contains(&file.display().to_string()), "{msg}");
    assert!(msg.contains("backups"), "错误要带备份路径：{msg}");
    assert!(msg.contains("可能有客户端正占用该文件"), "{msg}");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(3 * 200),
        "必须退避重试 3 次（~200ms）：{msg}"
    );

    assert_eq!(read(&file), text, "写失败不许动目标文件");
    assert!(
        cg_tmp_leftovers(&e.config).is_empty(),
        "替换失败即删临时文件（崩溃窗口不留半截）"
    );
}

#[tokio::test]
async fn stale_temp_files_are_cleaned_and_foreign_temp_files_are_left_alone() {
    let e = env("stale-temp");
    let file = e.config.join("config.json");
    write_file(&file, "{}\n");

    // 崩溃残留：陈旧（>24h）的临时文件。
    let stale = e.config.join("config.json.cg-tmp");
    write_file(&stale, "半截垃圾");
    set_modified(
        &stale,
        std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600),
    );
    // 另一个是别人刚放下的同名候选 → 不许碰。
    let fresh = e.config.join("config.json.cg-tmp.1");
    write_file(&fresh, "别人的");

    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();

    assert!(!stale.exists(), "陈旧临时文件要顺手清掉");
    assert_eq!(read(&fresh), "别人的", "不是陈旧的临时文件不碰");
    let v: Json = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(v["inferenceProvider"], json!("gateway"));
    // 成功替换后不留自己的临时文件（崩溃窗口里不会攒下半截文件）。
    let mine: Vec<PathBuf> = cg_tmp_leftovers(&e.config)
        .into_iter()
        .filter(|p| p != &fresh)
        .collect();
    assert!(mine.is_empty(), "{mine:?}");
}

#[tokio::test]
async fn crash_window_never_leaves_a_half_written_target() {
    let e = env("crash-window");
    let file = e.config.join("config.json");
    // 目标文件「上一轮」的内容完好；同目录里躺着一个写满但没来得及替换的临时文件
    // （= 崩在 fsync 与替换之间的现场）。
    let previous = "{\n  \"inferenceProvider\": \"anthropic\"\n}\n";
    write_file(&file, previous);
    let abandoned = e.config.join("config.json.cg-tmp.7");
    write_file(&abandoned, "{\n  \"inferenceProvider\": \"半截\"\n}\n");

    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();

    // 目标文件只能是「完整的新内容」，绝不能混进临时文件里的半截字节。
    let text = read(&file);
    assert!(!text.contains("半截"), "{text}");
    let v: Json = serde_json::from_str(&text).expect("目标文件必须是完整合法 JSON");
    assert_eq!(v["inferenceProvider"], json!("gateway"));
    // 没被选中的临时文件仍在（不是我们的候选），但也不会污染结果。
    assert_eq!(
        read(&abandoned),
        "{\n  \"inferenceProvider\": \"半截\"\n}\n"
    );
}

fn set_modified(path: &Path, t: std::time::SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

// ── TOML：保序 + 别的段 + 用户注释 ─────────────────────────────────────────────

const CODEX_TOML: &str = "\
# 我的注释：Codex 配置
model = \"gpt-5\"

[model_providers.other]
base_url = \"https://example.com\"

[windows]
sandbox = \"elevated\"

[plugins.\"computer-use@openai-bundled\"]
enabled = true
";

fn codex_spec() -> FileSpec {
    FileSpec::new(
        "codex",
        "config.toml",
        Format::Toml,
        vec![
            set(&["model_provider"], json!("claudegateway")),
            set(&["model"], json!("claude-sonnet-4-5")),
            set(
                &["model_catalog_json"],
                json!("claudegateway-model-catalog.json"),
            ),
            set(
                &["model_providers", "claudegateway", "base_url"],
                json!("http://127.0.0.1:8787/v1"),
            ),
            set(
                &["model_providers", "claudegateway", "wire_api"],
                json!("responses"),
            ),
            set(
                &["model_providers", "claudegateway", "context_window"],
                json!(262144),
            ),
        ],
    )
    .with_restore(vec![
        remove(&["model_provider"]),
        remove(&["model_catalog_json"]),
        remove(&["model"]),
        remove(&["model_providers", "claudegateway"]),
    ])
}

#[tokio::test]
async fn toml_rewrite_keeps_order_comments_and_other_tables() {
    let e = env("toml");
    let file = e.config.join("config.toml");
    write_file(&file, CODEX_TOML);

    let out = e
        .writer
        .write_one(&codex_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert!(matches!(out, FileOutcome::Written { .. }), "{out:?}");
    let text = read(&file);

    let doc: toml_edit::DocumentMut = text.parse().expect("结果仍是合法 TOML");
    assert_eq!(doc["model_provider"].as_str(), Some("claudegateway"));
    assert_eq!(doc["model"].as_str(), Some("claude-sonnet-4-5"));
    assert_eq!(
        doc["model_providers"]["claudegateway"]["wire_api"].as_str(),
        Some("responses")
    );
    // 别人的段与用户注释一个字节不动。
    assert!(text.contains("# 我的注释：Codex 配置"), "{text}");
    assert_eq!(doc["windows"]["sandbox"].as_str(), Some("elevated"));
    assert_eq!(
        doc["plugins"]["computer-use@openai-bundled"]["enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(
        doc["model_providers"]["other"]["base_url"].as_str(),
        Some("https://example.com")
    );
    // 保序：原有内容相对顺序不变。
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("少了 {needle}：{text}"))
    };
    assert!(at("# 我的注释") < at("[model_providers.other]"));
    assert!(at("[model_providers.other]") < at("[windows]"));
    assert!(at("[windows]") < at("[plugins."));

    // 幂等：语义相同 → 不写。
    let before = mtime(&file);
    assert_eq!(
        e.writer
            .write_one(&codex_spec(), ConflictMode::Abort)
            .await
            .unwrap(),
        FileOutcome::Unchanged
    );
    assert_eq!(mtime(&file), before);

    // Codex 桌面应用往同一个文件里插自己的段 → 不算冲突、不被抹掉。
    write_file(&file, &format!("{text}\n[desktop]\nlocale = \"zh-CN\"\n"));
    let out = e
        .writer
        .write_one(&codex_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    assert_eq!(out, FileOutcome::Unchanged, "非接管键变化不算冲突");
    assert!(read(&file).contains("[desktop]"));

    // 摘键还原：只摘网关的键，别的段留着。
    e.writer.restore_one(&codex_spec()).await.unwrap();
    let text = read(&file);
    let doc: toml_edit::DocumentMut = text.parse().expect("还原后仍是合法 TOML");
    assert!(doc.get("model_provider").is_none());
    assert!(doc.get("model_catalog_json").is_none());
    assert!(doc.get("model").is_none());
    assert!(doc["model_providers"].get("claudegateway").is_none());
    assert!(
        text.contains("[windows]") && text.contains("[desktop]"),
        "{text}"
    );
    assert_eq!(
        doc["model_providers"]["other"]["base_url"].as_str(),
        Some("https://example.com")
    );
}

#[tokio::test]
async fn toml_set_to_null_is_rejected_instead_of_corrupting() {
    let e = env("toml-null");
    let file = e.config.join("config.toml");
    write_file(&file, "model = \"gpt-5\"\n");
    let spec = FileSpec::new(
        "codex",
        "config.toml",
        Format::Toml,
        vec![set(&["model_provider"], json!(null))],
    );
    let err = e
        .writer
        .write_one(&spec, ConflictMode::Abort)
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("TOML 没有 null 字面值"),
        "{err:#}"
    );
    assert_eq!(read(&file), "model = \"gpt-5\"\n");
}

// ── 多文件顺序 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_file_write_follows_dependency_order_and_lists_changed_files() {
    let e = env("multi-file");
    let first = FileSpec::new(
        "codex",
        "claudegateway-model-catalog.json",
        Format::Json,
        vec![set(&["models"], json!([{ "slug": "claude-sonnet-4-5" }]))],
    )
    .with_order(0);
    let pointer = FileSpec::new(
        "codex",
        "config.toml",
        Format::Toml,
        vec![set(
            &["model_catalog_json"],
            json!("claudegateway-model-catalog.json"),
        )],
    )
    .with_order(1);
    // 清单里故意把指针放在前面：写序必须按 order。
    let plan = WritePlan::new(vec![pointer.clone(), first]);

    let report = e.writer.write(&plan).await.unwrap();
    assert_eq!(report.files.len(), 2);
    assert_eq!(
        report.files[0].0,
        PathBuf::from("claudegateway-model-catalog.json")
    );
    assert_eq!(report.files[1].0, PathBuf::from("config.toml"));
    assert!(e.config.join("config.toml").exists());

    // 中途失败：报错要列出已改动的文件。
    let catalog2 = FileSpec::new(
        "codex",
        "claudegateway-model-catalog.json",
        Format::Json,
        vec![set(&["models"], json!([{ "slug": "claude-opus-4-5" }]))],
    )
    .with_order(0);
    let broken = FileSpec::new(
        "codex",
        "broken.json",
        Format::Json,
        vec![set(&["a"], json!(1))],
    )
    .with_order(2);
    write_file(&e.config.join("broken.json"), "{ 坏 }");
    let plan = WritePlan::new(vec![catalog2, pointer, broken]);
    let err = e.writer.write(&plan).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("多文件写入中途失败"), "{msg}");
    assert!(msg.contains("已改动"), "{msg}");
    assert!(
        msg.contains("claudegateway-model-catalog.json"),
        "要列出已改动的文件：{msg}"
    );
}

// ── 并发 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_writes_on_the_same_file_serialize() {
    let e = env("concurrent");
    let file = e.config.join("config.json");
    write_file(&file, "{}\n");
    let spec = provider_spec();
    let (a, b) = tokio::join!(
        e.writer.write_one(&spec, ConflictMode::Abort),
        e.writer.write_one(&spec, ConflictMode::Abort)
    );
    let outs = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outs.iter()
            .filter(|o| matches!(o, FileOutcome::Written { .. }))
            .count(),
        1,
        "并发写同一个文件只许真写一次：{outs:?}"
    );
    assert!(outs.contains(&FileOutcome::Unchanged));
    let v: Json = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(v["inferenceProvider"], json!("gateway"));
}

// ── 备份保留 ──────────────────────────────────────────────────────────────────

#[test]
fn backup_retention_keeps_five_rolling_and_never_prunes_the_first_snapshot() {
    let root = tmp_root("retention");
    let dir = root.join("backups").join("codex");
    fs::create_dir_all(&dir).unwrap();
    let first = dir.join("config.toml.20260101-000000");
    fs::write(&first, "首写快照").unwrap();
    for day in 1..=7 {
        let p = dir.join(format!("config.toml.2026010{day}-010000"));
        fs::write(&p, format!("滚动 {day}")).unwrap();
    }
    // 一份超 90 天的滚动备份。
    let ancient = dir.join("config.toml.20250101-000000");
    fs::write(&ancient, "老").unwrap();
    set_modified(
        &ancient,
        std::time::SystemTime::now() - std::time::Duration::from_secs(100 * 24 * 3600),
    );

    super::backup::snapshot(
        &root.join("backups"),
        "codex",
        "config.toml",
        Some("新的滚动备份".as_bytes()),
        Some(&first),
    )
    .unwrap();

    assert!(first.exists(), "首写快照常驻不清理");
    assert!(!ancient.exists(), "超 90 天的滚动备份要清掉");
    let rolling = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path() != first)
        .count();
    assert_eq!(rolling, 5, "滚动备份保留最近 5 份");
}

// ── 记账日志 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn every_write_and_restore_leaves_a_metadata_log_line() {
    let e = env("log");
    let file = e.config.join("config.json");
    write_file(&file, "{}\n");
    e.writer
        .write_one(&provider_spec(), ConflictMode::Abort)
        .await
        .unwrap();
    e.writer.restore_one(&provider_spec()).await.unwrap();

    let logs: Vec<PathBuf> = fs::read_dir(e.root.join("logs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(logs.len(), 1, "按天一个文件：{logs:?}");
    let name = logs[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.starts_with("writer-") && name.ends_with(".log"),
        "{name}"
    );
    let text = read(&logs[0]);
    assert!(
        text.contains("write client=claude-desktop file=config.json"),
        "{text}"
    );
    assert!(
        text.contains("restore client=claude-desktop file=config.json"),
        "{text}"
    );
    assert!(
        !text.contains("inferenceProvider"),
        "日志只记元数据，不记 body：{text}"
    );
}

#[test]
fn log_retention_drops_files_older_than_seven_days() {
    let root = tmp_root("log-retention");
    let logs = root.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let old = logs.join("writer-20200101.log");
    fs::write(&old, "老日志\n").unwrap();
    set_modified(
        &old,
        std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600),
    );
    // 别的文件不归我们管。
    let foreign = logs.join("gateway.log");
    fs::write(&foreign, "外壳的日志\n").unwrap();
    set_modified(
        &foreign,
        std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600),
    );

    super::log::append(&logs, "write client=x file=y").unwrap();
    assert!(!old.exists(), "超 7 天的 writer 日志要清掉");
    assert!(foreign.exists(), "不是 writer-*.log 的文件不碰");
}

// ── 单实例互斥量 ──────────────────────────────────────────────────────────────

#[cfg(windows)]
#[test]
fn single_instance_mutex_tells_a_second_instance_apart() {
    let name = format!("ClaudeGateway.test.{}", std::process::id());
    let first = super::instance::acquire(&name).expect("取互斥量");
    assert!(first.is_some(), "第一个实例应当拿到");
    assert!(
        super::instance::acquire(&name).expect("再取一次").is_none(),
        "已有实例在跑时必须能认出来"
    );
    drop(first);

    // 名字不同 → 互不影响。
    let other = super::instance::acquire(&format!("{name}.other")).expect("取另一个名字");
    assert!(other.is_some());
}

// ── doc 层单测 ────────────────────────────────────────────────────────────────

#[test]
fn fidelity_detects_bom_crlf_indent_and_trailing_newline() {
    let bytes = b"\xEF\xBB\xBF{\r\n    \"a\": 1\r\n}\r\n";
    let f = Fidelity::detect(bytes);
    assert!(f.bom);
    assert!(f.crlf);
    assert_eq!(f.indent, "    ");
    assert!(f.trailing_newline);

    let f = Fidelity::detect(b"{\"a\":1}");
    assert!(!f.bom && !f.crlf && !f.trailing_newline);
    assert_eq!(f.indent, "  ", "认不出缩进就用两空格");
}

#[test]
fn jsonc_stripper_keeps_offsets_and_respects_strings() {
    let src = "{\n  // 注释里的 , ] }\n  \"a\": \"x//y,\",\n  \"b\": 1,\n}\n";
    let doc = Doc::parse(Format::Jsonc, Some(src.as_bytes())).expect("JSONC 应当能解析");
    assert_eq!(doc.get(&p(&["a"])), Some(json!("x//y,")));
    assert_eq!(doc.get(&p(&["b"])), Some(json!(1)));

    // 行列位置仍对应原文件（注释行不算数）。
    let err = Doc::parse(Format::Jsonc, Some(b"{\n  // }\n  \"a\": @,\n}".as_ref()))
        .err()
        .expect("坏 JSONC 必须报错");
    assert!(format!("{err:#}").contains("第 3 行"), "{err:#}");
}

#[test]
fn toml_parse_error_reports_a_line_and_column() {
    let err = Doc::parse(Format::Toml, Some(b"model = \"a\n".as_ref()))
        .err()
        .expect("坏 TOML 必须报错");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("解析失败即停") && msg.contains("行") && msg.contains("列"),
        "{msg}"
    );
}

#[test]
fn numbers_compare_by_value_so_toml_integers_are_idempotent() {
    assert!(json_eq(&json!(262144), &json!(262144.0)));
    assert!(!json_eq(&json!(1), &json!(2)));
    let doc = Doc::parse(Format::Toml, Some(b"context_window = 262144\n".as_ref())).unwrap();
    let op = set(&["context_window"], json!(262144));
    assert!(doc.satisfies(&op), "TOML 整数与 JSON 数字语义相同 → 不写");
}

#[test]
fn toml_arrays_of_inline_tables_upsert_by_field() {
    let doc_src = "entries = [{ id = \"a\", name = \"A\" }]\n";
    let mut doc = Doc::parse(Format::Toml, Some(doc_src.as_bytes())).unwrap();
    doc.apply(&OwnedOp::SetElement {
        path: p(&["entries"]),
        field: "id".into(),
        value: json!({ "id": "b", "name": "B" }),
    })
    .unwrap();
    let text = String::from_utf8(doc.to_bytes().unwrap()).unwrap();
    let reparsed: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(reparsed["entries"].as_array().unwrap().len(), 2);

    doc.apply(&OwnedOp::RemoveElement {
        path: p(&["entries"]),
        field: "id".into(),
        matches: json!("a"),
    })
    .unwrap();
    let text = String::from_utf8(doc.to_bytes().unwrap()).unwrap();
    let reparsed: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(reparsed["entries"].as_array().unwrap().len(), 1);
    assert_eq!(reparsed["entries"][0]["id"].as_str(), Some("b"));
}
