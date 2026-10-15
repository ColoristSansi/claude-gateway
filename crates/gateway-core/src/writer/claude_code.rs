//! Claude Code 这一条：`%USERPROFILE%\.claude\settings.json` 的 `env` 块（spec §8 表 + §8.3）。
//!
//! 引擎只认 [`FileSpec`]；这里把 Claude Code 那份清单拼出来（键名、取值、还原动作），
//! 写入根仍由调用方给（[`default_config_root`] 是给真实运行用的那一份，测试一律临时 home）。
//!
//! # 只写 `ANTHROPIC_AUTH_TOKEN`，绝不写 `ANTHROPIC_API_KEY`
//!
//! `ANTHROPIC_AUTH_TOKEN` 直接进 `Authorization: Bearer`；`ANTHROPIC_API_KEY` 需要一次
//! `/config` 交互批准，用户曾拒绝过就**静默失灵**（票 24 硬约束 1、spec §8 表）。
//! 用户自己写在 `env` 里的 `ANTHROPIC_API_KEY` 是**非接管键**，原样保留。
//!
//! # `ANTHROPIC_BASE_URL` 不带 `/v1`（2026-10-15 实测，票 47）
//!
//! 用隔离 `CLAUDE_CONFIG_DIR` + 本机 Claude Code 2.1.299（零官方登录）打一个只记请求行的
//! 本地探针：`ANTHROPIC_BASE_URL=http://127.0.0.1:18787` → `POST /v1/messages?beta=true`；
//! 改成 `http://127.0.0.1:18787/v1` → `POST /v1/v1/messages?beta=true`（同一次实验的负对照）。
//! 即 base 是**根地址**，`/v1/messages` 由客户端自己追加——与 Claude Desktop 同理（spec §8.1 第 1 条）。

use anyhow::{anyhow, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::service::HOST;

use super::plan::{FileSpec, Format, OwnedOp};

/// 客户端标识：备份子目录名 + `client_config_state.client`。
pub const CLIENT: &str = "claude-code";

/// 目标文件，**相对配置根**（配置根 = `%USERPROFILE%\.claude`）。
pub const REL_PATH: &str = "settings.json";

/// 网关根地址（**不带 `/v1`**，见模块文档）。
pub const BASE_URL_KEY: &str = "ANTHROPIC_BASE_URL";
/// 唯一写的凭据键（不写 `ANTHROPIC_API_KEY`）。
pub const AUTH_TOKEN_KEY: &str = "ANTHROPIC_AUTH_TOKEN";
/// 四个家族槽位变量，顺序即写入顺序（票 24 §1.6 的家族名单）。
pub const MODEL_KEYS: [&str; 4] = [
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

/// 网关暴露的四个家族槽位名（spec §7）：Claude Code 侧默认就填它们。
pub const FAMILY_SLOTS: [&str; 4] = [
    "claude-fable",
    "claude-opus",
    "claude-sonnet",
    "claude-haiku",
];

/// 四个 `ANTHROPIC_DEFAULT_*_MODEL` 的取值：默认 = 家族槽位名，可换成该客户端的覆盖名
/// （spec §7「每客户端可覆盖」：同一个 sonnet 槽位可以给 Claude Code 指另一个模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSlots {
    pub fable: String,
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
}

impl Default for ModelSlots {
    fn default() -> ModelSlots {
        ModelSlots {
            fable: FAMILY_SLOTS[0].to_string(),
            opus: FAMILY_SLOTS[1].to_string(),
            sonnet: FAMILY_SLOTS[2].to_string(),
            haiku: FAMILY_SLOTS[3].to_string(),
        }
    }
}

impl ModelSlots {
    /// 按 [`MODEL_KEYS`] 的顺序取值。
    fn values(&self) -> [&str; 4] {
        [&self.fable, &self.opus, &self.sonnet, &self.haiku]
    }
}

/// 网关根地址：`http://127.0.0.1:<port>`，就是 [`BASE_URL_KEY`] 该写的那份值。
pub fn gateway_root_url(port: u16) -> String {
    format!("http://{HOST}:{port}")
}

/// 配置根 = `<home>\.claude`；`home` 由调用方给（真实值是 `%USERPROFILE%`）。
pub fn config_root_from(home: &Path) -> PathBuf {
    home.join(".claude")
}

/// 真实配置根 `%USERPROFILE%\.claude`（**只算路径、不创建**；测试不要调它）。
pub fn default_config_root() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .ok_or_else(|| anyhow!("USERPROFILE 未设置，无法定位 Claude Code 配置目录"))?;
    Ok(config_root_from(Path::new(&home)))
}

/// Claude Code 的「客户端 × 文件」接管键清单。
///
/// 写：`env` 下六个键（[`BASE_URL_KEY`] / [`AUTH_TOKEN_KEY`] / 四个 [`MODEL_KEYS`]）。
/// 还原：同一份清单用 [`OwnedOp::RemoveIfEmpty`] 摘掉——`env` 只剩网关键时连空壳一起摘。
pub fn file_spec(base_url: &str, auth_token: &str, models: &ModelSlots) -> FileSpec {
    let mut write_ops = vec![
        set(BASE_URL_KEY, json!(base_url)),
        set(AUTH_TOKEN_KEY, json!(auth_token)),
    ];
    for (key, value) in MODEL_KEYS.iter().zip(models.values()) {
        write_ops.push(set(key, json!(value)));
    }
    // JSONC：严格 JSON 的超集，用户手写的注释 / 尾逗号不至于让「解析失败即停」挡住写入。
    FileSpec::new(CLIENT, REL_PATH, Format::Jsonc, write_ops).with_restore(
        std::iter::once(BASE_URL_KEY)
            .chain(std::iter::once(AUTH_TOKEN_KEY))
            .chain(MODEL_KEYS)
            .map(|key| OwnedOp::RemoveIfEmpty {
                path: vec!["env".to_string(), key.to_string()],
            })
            .collect(),
    )
}

fn set(key: &str, value: serde_json::Value) -> OwnedOp {
    OwnedOp::Set {
        path: vec!["env".to_string(), key.to_string()],
        value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::writer::{FileOutcome, Format, OwnedOp, WritePlan, WriteRoots, Writer};
    use serde_json::{json, Value as Json};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn tmp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join("cg47-claude-code-tests")
            .join(tag);
        let _ = fs::remove_dir_all(&root);
        root
    }

    struct Env {
        writer: Writer,
        /// 扮演 `%USERPROFILE%`，真实那一个一个字节都不碰。
        home: PathBuf,
    }

    impl Env {
        fn settings(&self) -> PathBuf {
            self.home.join(".claude").join(REL_PATH)
        }

        fn seed(&self, text: &str) {
            let path = self.settings();
            fs::create_dir_all(path.parent().unwrap()).expect("建 .claude 目录");
            fs::write(&path, text).expect("写 settings.json");
        }
    }

    fn env(tag: &str) -> Env {
        let root = tmp_root(tag);
        let store = Store::open(&root.join("data")).expect("开库");
        let home = root.join("home");
        let writer = Writer::new(
            store,
            WriteRoots {
                config_root: config_root_from(&home),
                backup_root: root.join("backups"),
                log_root: root.join("logs"),
            },
        );
        Env { writer, home }
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

    fn p(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    /// 网关根地址：`http://127.0.0.1:8787`，**不带 `/v1`**。
    fn root_url() -> String {
        gateway_root_url(crate::service::DEFAULT_PORT)
    }

    async fn write(env: &Env, models: &ModelSlots) -> FileOutcome {
        let spec = file_spec(&root_url(), "cg-test-token", models);
        env.writer
            .write_one(&spec, Default::default())
            .await
            .expect("写")
    }

    /// 写 → **逐字**核对整份 `settings.json`：网关六个键按写清单顺序追加，别的键原样。
    #[tokio::test]
    async fn write_appends_the_six_env_keys_verbatim_and_keeps_the_rest() {
        let e = env("verbatim");
        e.seed("{\n  \"model\": \"opus\",\n  \"env\": {\n    \"MY_FLAG\": \"1\"\n  }\n}\n");
        let out = write(&e, &ModelSlots::default()).await;
        assert!(
            matches!(
                out,
                FileOutcome::Written {
                    first_write: true,
                    ..
                }
            ),
            "{out:?}"
        );
        assert_eq!(
            read(&e.settings()),
            "{\n  \"model\": \"opus\",\n  \"env\": {\n    \"MY_FLAG\": \"1\",\n    \
             \"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:8787\",\n    \
             \"ANTHROPIC_AUTH_TOKEN\": \"cg-test-token\",\n    \
             \"ANTHROPIC_DEFAULT_FABLE_MODEL\": \"claude-fable\",\n    \
             \"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus\",\n    \
             \"ANTHROPIC_DEFAULT_SONNET_MODEL\": \"claude-sonnet\",\n    \
             \"ANTHROPIC_DEFAULT_HAIKU_MODEL\": \"claude-haiku\"\n  }\n}\n"
        );
    }

    /// 用户自加键（含用户自己的 `ANTHROPIC_API_KEY`）与别的顶层键原样保留。
    #[tokio::test]
    async fn user_keys_and_a_user_owned_api_key_survive() {
        let e = env("user-keys");
        e.seed(
            "{\n  \"model\": \"opus\",\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls:*)\"\n    ]\n  },\n  \
             \"env\": {\n    \"ANTHROPIC_API_KEY\": \"user-owned-key\",\n    \"MY_FLAG\": \"1\"\n  }\n}\n",
        );
        write(&e, &ModelSlots::default()).await;
        let v: Json = serde_json::from_str(&read(&e.settings())).expect("可解析");
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["model", "permissions", "env"], "顶层键序不变");
        assert_eq!(
            v["permissions"],
            json!({ "allow": ["Bash(ls:*)"] }),
            "非接管键原样"
        );
        assert_eq!(v["env"]["MY_FLAG"], json!("1"), "别的 env 键原样");
        assert_eq!(
            v["env"]["ANTHROPIC_API_KEY"],
            json!("user-owned-key"),
            "用户自己的 ANTHROPIC_API_KEY 一个字节没动"
        );
        assert_eq!(v["env"][BASE_URL_KEY], json!(root_url()));
    }

    /// 幂等：第二次语义相同 → `Unchanged`，mtime 与字节都不变。
    #[tokio::test]
    async fn second_write_is_a_noop_and_mtime_does_not_move() {
        let e = env("idempotent");
        e.seed("{\n  \"model\": \"opus\"\n}\n");
        write(&e, &ModelSlots::default()).await;
        let before = (read(&e.settings()), mtime(&e.settings()));
        let out = write(&e, &ModelSlots::default()).await;
        assert_eq!(out, FileOutcome::Unchanged);
        assert_eq!((read(&e.settings()), mtime(&e.settings())), before);
    }

    /// 还原：env 只剩网关键 → 摘键后连空 `env` 壳一起摘掉。
    #[tokio::test]
    async fn restore_drops_the_owned_keys_and_the_emptied_env_shell() {
        let e = env("restore-empty-env");
        let seed = "{\n  \"model\": \"opus\"\n}\n";
        e.seed(seed);
        write(&e, &ModelSlots::default()).await;
        let spec = file_spec(&root_url(), "cg-test-token", &ModelSlots::default());
        e.writer
            .restore(&WritePlan::new(vec![spec]))
            .await
            .expect("还原");
        assert_eq!(read(&e.settings()), seed, "还原后与写前逐字相同（零残留）");
    }

    /// 还原：env 里还有用户自己的键 → 只摘网关键，`env` 留着。
    #[tokio::test]
    async fn restore_keeps_an_env_that_still_has_user_keys() {
        let e = env("restore-user-env");
        let seed = "{\n  \"model\": \"opus\",\n  \"env\": {\n    \"MY_FLAG\": \"1\"\n  }\n}\n";
        e.seed(seed);
        write(&e, &ModelSlots::default()).await;
        let spec = file_spec(&root_url(), "cg-test-token", &ModelSlots::default());
        e.writer
            .restore(&WritePlan::new(vec![spec]))
            .await
            .expect("还原");
        assert_eq!(read(&e.settings()), seed, "只摘网关六键，用户 env 原样");
    }

    /// 接管键清单逐字：就是 `env` 下那六个键，**绝不含** `ANTHROPIC_API_KEY`；还原是同一份清单。
    #[test]
    fn the_owned_key_list_is_the_six_env_keys_and_never_the_api_key() {
        let spec = file_spec(&root_url(), "t", &ModelSlots::default());
        let want = vec![
            p(&["env", "ANTHROPIC_BASE_URL"]),
            p(&["env", "ANTHROPIC_AUTH_TOKEN"]),
            p(&["env", "ANTHROPIC_DEFAULT_FABLE_MODEL"]),
            p(&["env", "ANTHROPIC_DEFAULT_OPUS_MODEL"]),
            p(&["env", "ANTHROPIC_DEFAULT_SONNET_MODEL"]),
            p(&["env", "ANTHROPIC_DEFAULT_HAIKU_MODEL"]),
        ];
        let got: Vec<Vec<String>> = spec.write_ops.iter().map(|o| o.path().to_vec()).collect();
        assert_eq!(got, want, "写清单 = 六个 env 键");
        assert!(
            got.iter().flatten().all(|s| s != "ANTHROPIC_API_KEY"),
            "接管键里不许出现 ANTHROPIC_API_KEY"
        );
        let restore: Vec<Vec<String>> =
            spec.restore_ops.iter().map(|o| o.path().to_vec()).collect();
        assert_eq!(restore, want, "还原 = 同一份清单的摘键");
        assert!(
            spec.restore_ops
                .iter()
                .all(|o| matches!(o, OwnedOp::RemoveIfEmpty { .. })),
            "摘键用 RemoveIfEmpty（env 空了连壳摘）"
        );
        assert_eq!(spec.client, CLIENT);
        assert_eq!(spec.format, Format::Jsonc);
        assert_eq!(spec.rel_path, PathBuf::from(REL_PATH));
        assert!(!spec.rel_path.is_absolute(), "相对路径：真实根由调用方给");
    }

    /// 根地址**不带 `/v1`**：实测带 `/v1` 会得到 `/v1/v1/messages`（见模块文档）。
    #[test]
    fn the_base_url_is_the_gateway_origin_without_a_v1_suffix() {
        assert_eq!(root_url(), "http://127.0.0.1:8787");
        assert!(!root_url().ends_with('/'));
        assert!(!root_url().contains("/v1"));
    }

    /// 四个槽位默认写网关的家族槽位名；该客户端的覆盖名可换。
    #[tokio::test]
    async fn model_values_default_to_the_family_slots_and_overrides_win() {
        let e = env("model-override");
        let models = ModelSlots {
            sonnet: "claude-sonnet-4-5-cheap".to_string(),
            ..ModelSlots::default()
        };
        write(&e, &models).await;
        let v: Json = serde_json::from_str(&read(&e.settings())).expect("可解析");
        assert_eq!(
            v["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            json!("claude-sonnet-4-5-cheap")
        );
        assert_eq!(
            v["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"],
            json!("claude-opus")
        );
        assert_eq!(
            v["env"]["ANTHROPIC_DEFAULT_FABLE_MODEL"],
            json!("claude-fable")
        );
        assert_eq!(
            v["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            json!("claude-haiku")
        );
    }

    /// 配置根 = `%USERPROFILE%\.claude`（真实根由环境变量拼；测试只用临时 home）。
    #[test]
    fn the_config_root_is_dot_claude_under_userprofile() {
        assert_eq!(
            config_root_from(Path::new(r"C:\Users\me")),
            PathBuf::from(r"C:\Users\me\.claude")
        );
        let root = default_config_root().expect("USERPROFILE 在本机应存在");
        let home = std::env::var_os("USERPROFILE").expect("USERPROFILE");
        assert_eq!(root, PathBuf::from(home).join(".claude"));
    }
}
