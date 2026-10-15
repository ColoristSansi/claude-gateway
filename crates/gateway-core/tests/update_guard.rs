//! 升级的**配置级判据**（票 61）：这三条不许被后面的票顺手改掉。
//!
//! 1. 生产配置里更新端点一律 https、签名校验不可关（`dangerous*` 一个都不许开、
//!    `requireSignedVersion` 必须开）；本地 http 源只允许出现在 `tauri.conf.test.json`。
//! 2. 升级**不改端口、不改数据、不改客户端配置**——机制是 NSIS：`currentUser` 装同一目录、
//!    卸载钩子的还原段被 `$UpdateMode <> 1` 跳过（升级时一个客户端配置都不重写），
//!    端口默认值钉在 8787（升级不换端口，也不碰 `config.json` / `settings.port`）。
//! 3. 装完能被拉起来：更新清单的两把平台键都在（`windows-x86_64-nsis` / `windows-x86_64`）。

use std::path::{Path, PathBuf};

use gateway_core::dist::update::WINDOWS_PLATFORM_KEYS;
use gateway_core::service::DEFAULT_PORT;
use serde_json::Value;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("仓库根")
}

fn read(path: &str) -> String {
    let full = repo_root().join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("读 {} 失败：{e}", full.display()))
}

fn prod_config() -> Value {
    serde_json::from_str(&read("src-tauri/tauri.conf.json"))
        .expect("生产 tauri.conf.json 是合法 JSON")
}

#[test]
fn the_production_updater_config_keeps_every_security_switch_on() {
    let cfg = prod_config();
    let updater = &cfg["plugins"]["updater"];

    let endpoints = updater["endpoints"].as_array().expect("要有 endpoints");
    assert!(!endpoints.is_empty(), "端点不许为空，否则检查更新永远失败");
    for e in endpoints {
        let url = e.as_str().expect("端点要是字符串");
        assert!(
            url.starts_with("https://"),
            "生产端点必须 https（插件在 release 构建里对 http 直接报 InsecureTransportProtocol）：{url}"
        );
    }

    for switch in [
        "dangerousInsecureTransportProtocol",
        "dangerousAcceptInvalidCerts",
        "dangerousAcceptInvalidHostnames",
        // 蛇形别名也算同一件事：两个都禁掉，省得从别名钻进来。
        "dangerous-insecure-transport-protocol",
        "dangerous-accept-invalid-certs",
        "dangerous-accept-invalid-hostnames",
    ] {
        let on = updater
            .get(switch)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            !on,
            "生产配置不许开 {switch}（本地 http 源只走 tauri.conf.test.json）"
        );
    }

    let pubkey = updater["pubkey"]
        .as_str()
        .expect("要有 pubkey（公钥进应用配置）");
    assert!(pubkey.len() > 40, "pubkey 看着不像真的：{pubkey:?}");
    assert!(
        !pubkey.to_uppercase().contains("PLACEHOLDER") && pubkey != "PUBKEY_FROM_KEYS_DIR",
        "pubkey 还是占位：{pubkey:?}"
    );
    // 私钥绝不能出现在配置里（它是用户资产，放仓库外）。
    let whole = read("src-tauri/tauri.conf.json");
    assert!(
        !whole.contains("untrusted comment: rsign encrypted secret key"),
        "私钥内容不许进配置"
    );

    assert_eq!(
        updater["requireSignedVersion"].as_bool(),
        Some(true),
        "签名里必须带它签的那个版本（挡降级攻击）；签名时用 signer sign --app-version"
    );
    assert_eq!(
        updater["windows"]["installMode"].as_str(),
        Some("passive"),
        "Windows 静默装：passive（只有进度条，不弹交互）"
    );
}

#[test]
fn the_local_http_source_lives_only_in_the_test_override() {
    let test_cfg: Value =
        serde_json::from_str(&read("src-tauri/tauri.conf.test.json")).expect("覆盖配置是合法 JSON");
    let updater = &test_cfg["plugins"]["updater"];
    assert_eq!(
        updater["dangerousInsecureTransportProtocol"].as_bool(),
        Some(true),
        "本地源是 http，覆盖配置里必须显式开这个开关——所以它只出现在这份文件里"
    );
    let endpoints = updater["endpoints"].as_array().expect("要有 endpoints");
    let url = endpoints[0].as_str().expect("端点");
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "本地源必须是回环：{url}"
    );

    // 覆盖配置只改端点与那个开关：公钥 / 版本要求 / 安装模式仍来自生产配置（deep merge）。
    for key in ["pubkey", "requireSignedVersion", "windows"] {
        assert!(
            test_cfg["plugins"]["updater"].get(key).is_none(),
            "覆盖配置不该重复 {key}（它只覆盖端点与危险开关）"
        );
    }
}

#[test]
fn upgrading_does_not_touch_port_data_or_client_config() {
    // ① 端口：默认常量还是 8787（升级不换端口；端口来自配置文件 / settings，升级路径不读不写它）
    assert_eq!(DEFAULT_PORT, 8787);

    // ② 数据：NSIS per-user 装回同一个目录，卸载段在 upgrade 模式下跳过数据删除
    let cfg = prod_config();
    assert_eq!(
        cfg["bundle"]["windows"]["nsis"]["installMode"].as_str(),
        Some("currentUser"),
        "per-user：升级不会换到 Program Files，也不会带上管理员"
    );
    assert_eq!(
        cfg["identifier"].as_str(),
        Some("com.claudegateway.desktop")
    );
    assert_eq!(cfg["productName"].as_str(), Some("Claude Gateway"));

    // ③ 客户端配置：卸载钩子的还原段整段被 `$UpdateMode <> 1` 挡在升级之外
    let hooks = read("src-tauri/nsis/hooks.nsh");
    assert!(
        hooks.contains("$UpdateMode <> 1"),
        "卸载钩子的还原动作必须在升级（/UPDATE）时跳过——否则升级会重写三个客户端的配置"
    );
    // 「还原」那条 ExecWait 必须在守卫里面：取守卫之后到宏结束之间的那段文本，它得包含 ExecWait。
    let guarded = hooks
        .split("$UpdateMode <> 1")
        .nth(1)
        .expect("守卫之后还有东西");
    assert!(
        guarded.contains("--restore-clients"),
        "还原调用要在守卫里面（升级不许触发还原）"
    );
}

#[test]
fn the_update_manifest_keys_match_what_the_plugin_looks_up() {
    // 装机版的 bundle_type = nsis → 插件先查 `windows-x86_64-nsis`，再退到 `windows-x86_64`；
    // 免安装 zip 版没有安装器信息，只查后者。两份都写，两种装法都能升。
    assert_eq!(
        WINDOWS_PLATFORM_KEYS,
        ["windows-x86_64-nsis", "windows-x86_64"]
    );
    let pack = read("pack.ps1");
    for key in WINDOWS_PLATFORM_KEYS {
        assert!(
            pack.contains(key),
            "pack.ps1 的 latest.json 少了平台键 {key}"
        );
    }
    assert!(
        pack.contains("--app-version"),
        "pack.ps1 签名必须带 --app-version（生产配置开了 requireSignedVersion）"
    );
}
