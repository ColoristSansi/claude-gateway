//! Codex 写入器的单测（票 48）：接管键 / 哨兵逐行认领 / 模型目录 / 还原。
//!
//! 一律指向临时 `CODEX_HOME`：真实 `%USERPROFILE%\.codex` 一个字节不碰（要真机走查用
//! `codex_live` 那两条 `#[ignore]` 用例，得显式给 `CG_CODEX_HOME`）。

use super::codex::{
    catalog_value, check_catalog, home_from, mark_managed, strip_managed, CodexWriter,
    CATALOG_FILE, CLIENT, CONFIG_FILE, PROVIDER_ID,
};
use super::plan::{FileOutcome, WriteRoots};
use super::Writer;
use crate::binding::{Binding, ExposedModel, Profile};
use crate::egress::provider::Wire;
use crate::store::Store;
use serde_json::{json, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};

/// 一份「像真的」`~/.codex/config.toml`：桌面应用写的段都在，注释与空行也在。
/// 内容取自本机实测（2026-10-15），删掉了与本票无关的插件段。
const REALISH: &str = r#"notify = [ "C:\\x\\y.exe", "turn-ended" ]
[desktop]
followUpQueueMode = "steer"
conversationDetailMode = "STEPS_PROSE"

[mcp_servers.node_repl]
command = 'C:\x\node_repl.exe'
startup_timeout_sec = 120

# 用户自己的注释，别人不许洗掉
[windows]
sandbox = "elevated"
"#;

/// 模拟桌面应用启动时插进我们 provider 表后面的一张表（票 32 实测的写法）。
const INTRUDER: &str = "[plugins.\"computer-use@openai-bundled\"]\nenabled = true\n";

fn tmp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join("cg48-codex-tests").join(tag);
    let _ = fs::remove_dir_all(&root);
    root
}

struct Env {
    codex: CodexWriter,
    /// 临时 `CODEX_HOME`（= 写入根，参数化）。
    home: PathBuf,
    root: PathBuf,
}

fn env(tag: &str) -> Env {
    let root = tmp_root(tag);
    let store = Store::open(&root.join("data")).expect("开库");
    let home = root.join("codex-home");
    fs::create_dir_all(&home).expect("建 CODEX_HOME");
    let writer = Writer::new(
        store,
        WriteRoots {
            config_root: home.clone(),
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        },
    );
    Env {
        codex: CodexWriter::new(writer, "http://127.0.0.1:8787"),
        home,
        root,
    }
}

/// 六个虚拟模型：四个家族（乱序进来）+ 两个普通（乱序进来）。
/// 入参是网关暴露的清单（票 51 的 `binding::ExposedModel`：名字 + 它在某档案下解析到的三元组）。
fn routes() -> Vec<ExposedModel> {
    let row = |name: &str, up: &str, real: &str, wire: Wire| ExposedModel {
        name: name.to_string(),
        is_family: crate::binding::is_family(name),
        binding: Some(Binding::new(Profile::Global, name, up, real, wire)),
    };
    vec![
        row(
            "zeta",
            "commandcode",
            "deepseek/deepseek-v4.1-flash",
            Wire::Anthropic,
        ),
        row(
            "claude-sonnet",
            "commandcode",
            "deepseek/deepseek-v4.1-flash",
            Wire::Anthropic,
        ),
        row("alpha", "opencode", "qwen/qwen3.5", Wire::OpenAiChat),
        row("claude-fable", "opencode", "qwen/qwen3.5", Wire::OpenAiChat),
        row(
            "claude-opus",
            "commandcode",
            "deepseek/deepseek-v4.1-pro",
            Wire::Anthropic,
        ),
        row(
            "claude-haiku",
            "opencode",
            "qwen/qwen3.5-mini",
            Wire::OpenAiChat,
        ),
    ]
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("读文件")
}

fn mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("mtime")
}

// ── 写入根参数化 ───────────────────────────────────────────────────────────────

#[test]
fn home_honors_codex_home_and_falls_back_to_userprofile() {
    assert_eq!(
        home_from(Some(r"D:\isolated"), Some(r"C:\Users\x")).expect("CODEX_HOME"),
        PathBuf::from(r"D:\isolated")
    );
    assert_eq!(
        home_from(None, Some(r"C:\Users\x")).expect("USERPROFILE"),
        PathBuf::from(r"C:\Users\x").join(".codex")
    );
    // 空白当没设（环境变量被设成空串是常见的坑）。
    assert_eq!(
        home_from(Some("  "), Some(r"C:\Users\x")).expect("空白回落"),
        PathBuf::from(r"C:\Users\x").join(".codex")
    );
    assert!(home_from(None, None).is_err(), "两个都没有就不猜路径");
}

// ── 模型目录：内容、顺序、模板 ──────────────────────────────────────────────────

#[test]
fn catalog_puts_family_slots_first_then_others_by_name() {
    let catalog = catalog_value(&routes());
    check_catalog(&catalog).expect("自检应过");
    let models = catalog["models"].as_array().expect("models 数组");
    let slugs: Vec<&str> = models.iter().map(|m| m["slug"].as_str().unwrap()).collect();
    assert_eq!(
        slugs,
        vec![
            "claude-fable",
            "claude-opus",
            "claude-sonnet",
            "claude-haiku",
            "alpha",
            "zeta"
        ]
    );
    // 每个虚拟模型一条，不抄官方条目（票 31 第 1、2 条）。
    assert_eq!(models.len(), routes().len());
}

#[test]
fn catalog_rows_use_the_neutral_template_defaults() {
    let catalog = catalog_value(&routes());
    let models = catalog["models"].as_array().expect("models");
    for (i, m) in models.iter().enumerate() {
        let slug = m["slug"].as_str().expect("slug");
        assert_eq!(m["display_name"], json!(slug));
        assert_eq!(m["visibility"], json!("list"));
        assert_eq!(m["context_window"], json!(262144));
        assert_eq!(m["max_context_window"], json!(262144));
        assert_eq!(m["shell_type"], json!("shell_command"));
        assert_eq!(m["priority"], json!(1000 + i as u64));
        let route = routes().into_iter().find(|r| r.name == slug).expect("路由");
        let bound = route.binding.expect("夹具里每条都绑好了");
        let description = m["description"].as_str().expect("description");
        assert!(description.contains(&bound.upstream_id), "{description}");
        assert!(description.contains(&bound.real_model), "{description}");
    }
    let text = serde_json::to_string(&catalog).expect("序列化");
    // 中性模板：没有 freeform apply_patch / web_search（票 31 第 5 条、票 32 第 3 条）。
    assert!(!text.contains("apply_patch"), "中性模板不带 apply_patch");
    assert!(!text.contains("\"search\""), "中性模板不带内置 web_search");
    // 解析必需字段全齐（票 31 第 7 条的事实 2）。
    for m in models {
        assert!(m["base_instructions"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(m["supports_reasoning_summaries"].is_boolean());
        assert!(m["supports_parallel_tool_calls"].is_boolean());
    }
}

#[test]
fn catalog_self_check_rejects_bad_input() {
    let good = catalog_value(&routes());
    check_catalog(&good).expect("好的应过");

    let cases: Vec<(&str, Json)> = vec![
        ("models 非数组", json!({ "models": {} })),
        ("models 为空", json!({ "models": [] })),
        (
            "没有一条 visibility=list",
            with_models(&good, |m| {
                m["visibility"] = json!("hide");
            }),
        ),
        (
            "slug 为空",
            with_model(&good, |m| {
                m["slug"] = json!("");
            }),
        ),
        (
            "slug 重复",
            with_model_at(&good, 1, |m| {
                m["slug"] = json!("claude-fable");
            }),
        ),
        (
            "缺 supports_parallel_tool_calls",
            with_model(&good, |m| {
                m.as_object_mut()
                    .unwrap()
                    .remove("supports_parallel_tool_calls");
            }),
        ),
        (
            "缺 base_instructions",
            with_model(&good, |m| {
                m.as_object_mut().unwrap().remove("base_instructions");
            }),
        ),
    ];
    for (why, bad) in cases {
        assert!(check_catalog(&bad).is_err(), "自检应挡住：{why}");
    }
}

/// 对第一个条目做一次改动。
fn with_model(good: &Json, f: impl FnOnce(&mut Json)) -> Json {
    with_model_at(good, 0, f)
}

/// 对每个条目都做一次改动。
fn with_models(good: &Json, f: impl Fn(&mut Json)) -> Json {
    let mut v = good.clone();
    for i in 0..v["models"].as_array().map(Vec::len).unwrap_or(0) {
        f(&mut v["models"][i]);
    }
    v
}

fn with_model_at(good: &Json, i: usize, f: impl FnOnce(&mut Json)) -> Json {
    let mut v = good.clone();
    f(&mut v["models"][i]);
    v
}

// ── 哨兵：逐行认领，不按区域删 ─────────────────────────────────────────────────

#[test]
fn sentinels_wrap_the_owned_blocks_and_rewrapping_is_idempotent() {
    let written = format!(
        "notify = [ \"x\" ]\n\
         model_provider = \"{PROVIDER_ID}\"\n\
         model = \"claude-sonnet\"\n\
         model_catalog_json = \"{CATALOG_FILE}\"\n\
         [desktop]\nfollowUpQueueMode = \"steer\"\n\
         [model_providers.{PROVIDER_ID}]\nname = \"Claude Gateway\"\nwire_api = \"responses\"\n"
    );
    let marked = mark_managed(&written);
    let lines: Vec<&str> = marked.lines().collect();
    assert_eq!(lines[0], "notify = [ \"x\" ]");
    assert_eq!(lines[1], super::codex::TOP_BEGIN);
    assert_eq!(lines[2], format!("model_provider = \"{PROVIDER_ID}\""));
    assert_eq!(lines[4], format!("model_catalog_json = \"{CATALOG_FILE}\""));
    assert_eq!(lines[5], super::codex::TOP_END);
    assert_eq!(lines[6], "[desktop]");
    assert_eq!(lines[8], super::codex::TBL_BEGIN);
    assert_eq!(lines[9], format!("[model_providers.{PROVIDER_ID}]"));
    assert_eq!(lines[12], super::codex::TBL_END);
    assert_eq!(mark_managed(&marked), marked, "重写幂等");
    assert_eq!(strip_managed(&marked), written, "摘哨兵行回到原样");
}

// ── 写：接管键 / 目录 / 幂等 ───────────────────────────────────────────────────

#[tokio::test]
async fn write_owns_the_codex_keys_and_never_writes_env_key_or_auth_json() {
    let e = env("write-owns");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    let auth = e.home.join("auth.json");
    fs::write(&auth, "{\"OPENAI_API_KEY\":\"sk-keep-me\"}").expect("铺 auth.json");
    let auth_before = fs::read(&auth).expect("读 auth");

    let report = e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    assert_eq!(report.conflicts().len(), 0);

    let toml = read(&e.home.join(CONFIG_FILE));
    assert!(toml.contains("model_provider = \"claudegateway\""));
    assert!(toml.contains("model = \"claude-sonnet\""));
    assert!(toml.contains("model_catalog_json = \"claudegateway-model-catalog.json\""));
    assert!(toml.contains("[model_providers.claudegateway]"));
    assert!(toml.contains("name = \"Claude Gateway\""));
    assert!(toml.contains("base_url = \"http://127.0.0.1:8787/v1\""));
    assert!(toml.contains("wire_api = \"responses\""));
    // 票 48/24/25：不写 env_key（声明了就必须真存在）、不写 requires_openai_auth、不碰 auth.json。
    assert!(!toml.contains("env_key"), "不写 env_key");
    assert!(
        !toml.contains("requires_openai_auth"),
        "不写 requires_openai_auth"
    );
    assert_eq!(fs::read(&auth).expect("读 auth"), auth_before);

    // 非接管内容一个字节不动。
    for keep in [
        "notify = [",
        "[desktop]",
        "[mcp_servers.node_repl]",
        "[windows]",
        "# 用户自己的注释",
    ] {
        assert!(toml.contains(keep), "保住了 {keep}");
    }

    // 目录文件落在 CODEX_HOME，指针是相对文件名。
    let catalog = read(&e.home.join(CATALOG_FILE));
    let value: Json = serde_json::from_str(&catalog).expect("目录是合法 JSON");
    assert_eq!(
        value["models"].as_array().expect("models").len(),
        routes().len()
    );
}

#[tokio::test]
async fn rewrite_is_idempotent_and_keeps_the_sentinels() {
    let e = env("idempotent");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    let path = e.home.join(CONFIG_FILE);
    let first = fs::read(&path).expect("读");
    let first_mtime = mtime(&path);

    let again = e
        .codex
        .write(&routes(), "claude-sonnet")
        .await
        .expect("重写");
    assert!(
        again
            .files
            .iter()
            .all(|(_, o)| *o == FileOutcome::Unchanged),
        "语义相同应完全不写：{:?}",
        again.files
    );
    assert_eq!(fs::read(&path).expect("读"), first, "连续两次字节相同");
    assert_eq!(mtime(&path), first_mtime, "幂等不写连 mtime 都不变");

    let toml = read(&path);
    for s in [
        super::codex::TOP_BEGIN,
        super::codex::TOP_END,
        super::codex::TBL_BEGIN,
        super::codex::TBL_END,
    ] {
        assert!(toml.contains(s), "哨兵行在：{s}");
    }
    // 空壳父表 `[model_providers]` 也在哨兵区域内（引擎建它、`RemoveIfEmpty` 摘它）。
    let lines: Vec<&str> = toml.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == super::codex::TBL_BEGIN)
        .expect("provider 哨兵");
    assert_eq!(lines[at + 1].trim(), "[model_providers]");
    let next = lines[at + 2..]
        .iter()
        .find(|l| !l.trim().is_empty())
        .expect("provider 表头");
    assert_eq!(next.trim(), "[model_providers.claudegateway]");
}

#[tokio::test]
async fn catalog_self_check_blocks_the_whole_write() {
    let e = env("blocked");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    let mut dup = routes();
    dup[1].name = "zeta".to_string(); // 两个 zeta → slug 不唯一
    let err = e
        .codex
        .write(&dup, "claude-sonnet")
        .await
        .expect_err("自检不过就不写");
    assert!(format!("{err}").contains("slug"), "报错点明 slug：{err}");
    assert_eq!(
        read(&e.home.join(CONFIG_FILE)),
        REALISH,
        "config.toml 一个字节没动"
    );
    assert!(!e.home.join(CATALOG_FILE).exists(), "目录文件没造出来");
}

// ── 还原：摘键 / 保住别人的表 / 目录文件 ────────────────────────────────────────

#[tokio::test]
async fn restore_returns_the_file_to_its_bytes_when_nobody_else_wrote() {
    let e = env("restore-exact");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    e.codex.restore().await.expect("还原");
    assert_eq!(read(&e.home.join(CONFIG_FILE)), REALISH, "逐字节回到原样");
    assert!(
        !e.home.join(CATALOG_FILE).exists(),
        "网关新建的目录文件已删"
    );
}

#[tokio::test]
async fn restore_keeps_a_table_inserted_between_the_provider_sentinels() {
    let e = env("restore-intruder");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");

    // 桌面应用那一幕（票 32 实测）：把它的表插进我们 provider 哨兵之间
    // （插在 provider 表最后一行之后、`# <<< provider <<<` 之前）。
    let path = e.home.join(CONFIG_FILE);
    let toml = read(&path);
    let anchor = "http_headers = { X-Claude-Gateway-Client = \"codex\" }\n";
    let at = toml.find(anchor).expect("有 http_headers 行") + anchor.len();
    fs::write(&path, format!("{}{INTRUDER}{}", &toml[..at], &toml[at..])).expect("插表");
    assert!(read(&path).contains("computer-use@openai-bundled"));

    e.codex.restore().await.expect("还原");

    let after = read(&path);
    assert!(!after.contains("claudegateway"), "网关零残留：\n{after}");
    assert!(!after.contains("model_provider"), "顶层键已摘：\n{after}");
    assert!(!after.contains("managed ("), "哨兵行已摘：\n{after}");
    assert!(
        after.contains("[plugins.\"computer-use@openai-bundled\"]\nenabled = true"),
        "别人的表保住了：\n{after}"
    );
    assert_eq!(after, format!("{REALISH}{INTRUDER}"), "回到原样 + 别人的表");
}

#[tokio::test]
async fn catalog_is_deleted_on_restore_only_while_it_is_still_the_gateway_bytes() {
    // ① 没被外部改过：删。
    let e = env("catalog-delete");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    e.codex.restore().await.expect("还原");
    assert!(!e.home.join(CATALOG_FILE).exists(), "还是我们的那份 → 删");

    // ② 外部改过（哪怕语义相同、只换了排版）：留着不动。
    let e = env("catalog-keep");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    let catalog = e.home.join(CATALOG_FILE);
    let value: Json = serde_json::from_str(&read(&catalog)).expect("目录");
    fs::write(&catalog, serde_json::to_string(&value).expect("紧凑")).expect("改排版");
    let edited = fs::read(&catalog).expect("读");
    let report = e.codex.restore().await.expect("还原");
    assert_eq!(
        fs::read(&catalog).expect("读"),
        edited,
        "被改过的目录一个字节不动"
    );
    assert!(
        report
            .files
            .iter()
            .any(|(p, o)| p == Path::new(CATALOG_FILE) && matches!(o, FileOutcome::Conflict { .. })),
        "报告里要能看出目录被外部改过：{:?}",
        report.files
    );
    assert_eq!(
        read(&e.home.join(CONFIG_FILE)),
        REALISH,
        "config.toml 照常摘键"
    );
}

#[tokio::test]
async fn rollback_puts_the_first_snapshot_back_byte_for_byte() {
    let e = env("rollback");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    let outcomes = e.codex.rollback().await.expect("整份回滚");
    assert!(outcomes
        .iter()
        .any(|o| matches!(o, super::plan::RollbackOutcome::Rolled { .. })));
    assert_eq!(
        read(&e.home.join(CONFIG_FILE)),
        REALISH,
        "逐字节回到首写快照"
    );
    assert!(
        !e.home.join(CATALOG_FILE).exists(),
        "网关新建的目录文件滚掉了"
    );
}

#[tokio::test]
async fn conflict_gives_up_and_the_explicit_claim_takes_over() {
    let e = env("conflict");
    fs::write(e.home.join(CONFIG_FILE), REALISH).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    // 首写留了快照（票 28 第 7 条）。
    let snapshots = fs::read_dir(e.root.join("backups").join(CLIENT)).expect("备份目录");
    assert!(snapshots.count() >= 1, "首写要留快照");

    // 用户手改接管键（§8.4 第 3 条）→ 写要放弃本次。
    let path = e.home.join(CONFIG_FILE);
    let edited = read(&path).replace("model = \"claude-sonnet\"", "model = \"claude-opus\"");
    fs::write(&path, &edited).expect("手改");
    let report = e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    assert!(
        report
            .files
            .iter()
            .any(|(p, o)| p == Path::new(CONFIG_FILE) && matches!(o, FileOutcome::Conflict { .. })),
        "应报冲突：{:?}",
        report.files
    );
    assert_eq!(read(&path), edited, "冲突时 config.toml 一个字节不动");

    // 用户点「认领并覆盖」→ 接管键以网关为准（ADR-0006 第 4 条）。
    let report = e
        .codex
        .claim(&routes(), "claude-sonnet")
        .await
        .expect("认领");
    assert_eq!(report.conflicts().len(), 0, "{:?}", report.files);
    assert!(read(&path).contains("model = \"claude-sonnet\""));
}

#[tokio::test]
async fn fresh_install_creates_both_files_then_removes_them_on_restore() {
    let e = env("fresh");
    fs::remove_dir_all(&e.home).expect("挪走 CODEX_HOME，模拟全新机器");
    let report = e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    assert!(e.home.join(CONFIG_FILE).exists(), "config.toml 建出来了");
    assert!(e.home.join(CATALOG_FILE).exists(), "目录文件建出来了");
    assert!(
        report.files.iter().any(|(_, o)| matches!(
            o,
            FileOutcome::Written {
                marker_only: true,
                ..
            }
        )),
        "首写时文件不存在 → 只留 marker（票 28 第 7 条）：{:?}",
        report.files
    );
    e.codex.restore().await.expect("还原");
    assert!(
        !e.home.join(CONFIG_FILE).exists(),
        "网关新建且摘空的 config.toml 整个删"
    );
    assert!(!e.home.join(CATALOG_FILE).exists(), "目录文件也删");
}

#[tokio::test]
async fn another_provider_in_the_same_parent_table_survives_restore() {
    let e = env("other-provider");
    let original = format!(
        "{REALISH}\n[model_providers.other]\nname = \"Other\"\nbase_url = \"http://127.0.0.1:9999/v1\"\n"
    );
    fs::write(e.home.join(CONFIG_FILE), &original).expect("铺原文件");
    e.codex.write(&routes(), "claude-sonnet").await.expect("写");
    e.codex.restore().await.expect("还原");
    let after = read(&e.home.join(CONFIG_FILE));
    assert!(!after.contains("claudegateway"), "网关零残留：\n{after}");
    assert_eq!(
        after, original,
        "别人的 provider 与父表 `[model_providers]` 都留着"
    );
}

// ── 真机走查（票 48 验收 2）：只有人手跑 ────────────────────────────────────────

/// 对 `CG_CODEX_HOME` 指向的**真实** `~/.codex` 走一遍：写 → `codex` 读配置 → 还原 → 逐字节比。
///
/// ```text
/// $env:CG_CODEX_HOME = "$env:USERPROFILE\.codex"
/// cargo test -p gateway-core --lib codex_live -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "真机走查（票 48 验收 2）：要显式设 CG_CODEX_HOME 才会碰真实 ~/.codex"]
async fn codex_live_write_read_restore() {
    use crate::store::Store;
    let Ok(home) = std::env::var("CG_CODEX_HOME") else {
        panic!("先设 CG_CODEX_HOME（真实 ~/.codex 的绝对路径）");
    };
    let home = PathBuf::from(home);
    let config = home.join(CONFIG_FILE);
    let catalog = home.join(CATALOG_FILE);
    let before = fs::read(&config).expect("读写入前的 config.toml");
    let auth_before = fs::read(home.join("auth.json")).ok();
    assert!(!catalog.exists(), "走查前不该已经有网关目录文件");

    let root = tmp_root("live");
    let store = Store::open(&root.join("data")).expect("开库");
    let writer = Writer::new(
        store,
        WriteRoots {
            config_root: home.clone(),
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        },
    );
    let codex = CodexWriter::new(writer, "http://127.0.0.1:8787");
    codex.write(&routes(), "claude-sonnet").await.expect("写");
    println!("--- 写入后的 config.toml ---\n{}", read(&config));

    // 交给真实的 `codex` 读一遍配置：先看模型目录解析出来的模型（写没写进选择器看这一步），
    // 再看 `doctor --summary` 的配置 / 凭据结论。
    let models = std::process::Command::new("cmd")
        .args(["/C", "codex debug models"])
        .current_dir(&home)
        .env("CODEX_HOME", &home)
        .output();
    match models {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            println!(
                "codex debug models → status={:?} bytes={} 条目数={} 四家族名在目录里：{} {} {} {}",
                o.status.code(),
                text.len(),
                text.matches("\"slug\"").count(),
                text.contains("claude-fable"),
                text.contains("claude-opus"),
                text.contains("claude-sonnet"),
                text.contains("claude-haiku")
            );
        }
        Err(e) => println!("起不了 codex debug models（跳过）：{e}"),
    }
    let out = std::process::Command::new("cmd")
        .args(["/C", "codex doctor --summary"])
        .current_dir(&home)
        .env("CODEX_HOME", &home)
        .output();
    match out {
        Ok(o) => println!(
            "codex doctor --summary → status={:?}\n{}{}",
            o.status.code(),
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => println!("起不了 codex（跳过 CLI 那一步）：{e}"),
    }

    codex.restore().await.expect("还原");
    assert_eq!(
        fs::read(&config).expect("读还原后"),
        before,
        "还原后逐字节相同"
    );
    assert!(!catalog.exists(), "目录文件已删");
    assert_eq!(
        fs::read(home.join("auth.json")).ok(),
        auth_before,
        "auth.json 一个字节没动"
    );
    println!("真机走查通过：{}", home.display());
}

/// 只做还原那一步：写走查中途被打断时用它把机器收回原状。
#[tokio::test]
#[ignore = "真机走查（票 48 验收 2）：要显式设 CG_CODEX_HOME 才会碰真实 ~/.codex"]
async fn codex_live_restore_only() {
    use crate::store::Store;
    let home = PathBuf::from(std::env::var("CG_CODEX_HOME").expect("先设 CG_CODEX_HOME"));
    let root = tmp_root("live-restore");
    let store = Store::open(&root.join("data")).expect("开库");
    let writer = Writer::new(
        store,
        WriteRoots {
            config_root: home,
            backup_root: root.join("backups"),
            log_root: root.join("logs"),
        },
    );
    let codex = CodexWriter::new(writer, "http://127.0.0.1:8787");
    let report = codex.restore().await.expect("还原");
    println!("还原报告：{:?}", report.files);
    println!("客户端常量：{CLIENT}");
}
