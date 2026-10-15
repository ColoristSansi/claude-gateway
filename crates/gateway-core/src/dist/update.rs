//! 自动升级（票 61）：本地留包 + 回退指引 + 更新状态形状。
//!
//! 真正的「检查 / 下载 / 安装」由 Tauri 的 updater 插件做（它在 `src-tauri` 里，要 AppHandle），
//! 本模块只放**不依赖 Tauri** 的那半：
//!
//! - 数据目录下的 `updates\`：升级下来的安装包留一份（票 18 §3 Q11），回退就靠它；
//! - 回退指引的原文（设置页「关于」/帮助里显示，票 59 直接用）；
//! - [`UpdateStatus`]：`/api/update/*` 的响应形状，前端与设置页只认这一种。
//!
//! 外壳通过 [`UpdateSource`] 把插件的动作接进管理 API：没接进来时那几个端点一律 **503**
//! （与票 62 的维护面同一条规矩），不静默假装没有更新。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// 数据目录下留包的地方（票 18 §3：`updates\`）。
pub const UPDATES_DIR: &str = "updates";
/// `updates\` 里放安装包的子目录。
pub const INSTALLERS_DIR: &str = "installers";
/// 上一次升级的回执文件名（`from` / `to` / 包名 / 时间）。
pub const LAST_UPDATE_FILE: &str = "last-update.json";

/// `latest.json` 里 Windows x64 的平台键：插件先找带安装器名的那一个，再退到不带的那一个
/// （`tauri-plugin-updater` 的 `get_urls`，票 61 起两份都写，装机版与免安装版都认）。
pub const WINDOWS_PLATFORM_KEYS: [&str; 2] = ["windows-x86_64-nsis", "windows-x86_64"];

/// 留包 / 回退用的数据目录。
pub fn updates_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(UPDATES_DIR)
}

/// 安装包留在哪里。
pub fn installers_dir(data_dir: &Path) -> PathBuf {
    updates_dir(data_dir).join(INSTALLERS_DIR)
}

/// 更新状态快照（`GET /api/update/status` 的响应体，camelCase 给前端直接用）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// 当前跑着的版本（= `CARGO_PKG_VERSION`）。
    pub current_version: String,
    /// 生效的更新端点（生产是 https，本地验收是 127.0.0.1）。
    pub endpoints: Vec<String>,
    pub checking: bool,
    pub installing: bool,
    /// 上次检查的 UTC 时间（RFC 3339）；没检查过是 `None`。
    pub last_checked_at: Option<String>,
    /// 有可装的更新时才有；界面据此显示「有新版 0.2.0」。
    pub available: Option<AvailableUpdate>,
    /// 下载 / 安装这条路上的进度（字节）。
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    /// 安装阶段：包已下完、正在跑安装器。
    pub installing_package: Option<String>,
    /// 可读的失败原因（原文给人看，不做错误码）。
    pub error: Option<String>,
    /// 回退指引与本地留着的包。
    pub rollback: Rollback,
}

/// 端点宣告的可用更新。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub download_url: String,
}

/// 本地留着的那几个包 + 按它们算出来的回退指引。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rollback {
    /// 可直接显示的回退步骤（原文，含本次该双击哪个文件）。
    pub guide: String,
    /// `updates\installers\` 的绝对路径（界面上「打开目录」用）。
    pub installers_dir: String,
    /// 本地留着的包，版本高的在前。
    pub packages: Vec<LocalPackage>,
    /// 建议回退到的那一个（严格小于当前版本里最高的）；没有就是 `None`。
    pub previous: Option<LocalPackage>,
    /// 上一次升级的回执（从哪版升到哪版）。
    pub last_update: Option<LastUpdate>,
}

/// 本地留着的一个安装包。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalPackage {
    pub file_name: String,
    pub path: String,
    pub bytes: u64,
    /// 从文件名认出来的版本（认不出就是 `None`，不影响它被列出来）。
    pub version: Option<String>,
}

/// 上一次升级的回执（`updates\last-update.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LastUpdate {
    pub from: String,
    pub to: String,
    pub at: String,
    /// 这次升级用的安装包文件名（就在 `installers\` 里）。
    pub package: String,
}

/// 外壳接进管理 API 的更新动作（Tauri 侧实现）。
///
/// 三个方法都返回自家形状的快照 / 错误字符串：管理 API 只做序列化，不做业务判断。
/// `Debug` 是 `GatewayConfig` 需要的（它 derive 了 Debug，`Store` 那种没 Debug 的另说）。
pub trait UpdateSource: Send + Sync + std::fmt::Debug + 'static {
    /// 当前状态（不发网络请求；界面轮询它）。
    fn snapshot(&self) -> UpdateStatus;
    /// 去端点问一次有没有新版。
    fn check(self: Arc<Self>) -> UpdateFuture<Result<UpdateStatus, String>>;
    /// 开始下载 + 验签 + 静默安装。**同步返回**（把 `installing` 置上就给界面回话），
    /// 真正的活在后台任务里：Windows 上安装器起来后本进程会被 `exit(0)`，这里回不了话。
    fn start_install(self: Arc<Self>) -> Result<UpdateStatus, String>;
}

/// 箱起来的 future：内核不引 `async-trait`，动作面就三个方法。
pub type UpdateFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// 把下载下来的安装包留一份到 `updates\installers\`。
///
/// 文件名只取最后一段（URL 里的 `%20` 由调用方先解码）：包名来自远端，**不许**带路径，
/// 否则一个 `..\..\x.exe` 就能写到数据目录外面去。
pub fn save_package(data_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let name = Path::new(file_name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .ok_or_else(|| anyhow!("安装包文件名不合法：{file_name:?}"))?;
    let dir = installers_dir(data_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("建目录失败：{}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).with_context(|| format!("写安装包失败：{}", path.display()))?;
    Ok(path)
}

/// 本地留着的包，版本高的在前；认不出版本的排在最后（按文件名）。
pub fn list_packages(data_dir: &Path) -> Vec<LocalPackage> {
    let dir = installers_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<LocalPackage> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let file_name = path.file_name()?.to_str()?.to_string();
            let bytes = entry.metadata().ok()?.len();
            Some(LocalPackage {
                version: version_from_file_name(&file_name),
                file_name,
                path: path.to_string_lossy().into_owned(),
                bytes,
            })
        })
        .collect();
    out.sort_by(|a, b| match (&b.version, &a.version) {
        (Some(x), Some(y)) => cmp_version(x, y).then_with(|| a.file_name.cmp(&b.file_name)),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.file_name.cmp(&b.file_name),
    });
    out
}

/// 回退目标：版本**严格小于**当前版本里最高的那一个。
pub fn previous_package(data_dir: &Path, current_version: &str) -> Option<LocalPackage> {
    list_packages(data_dir)
        .into_iter()
        .filter(|p| match &p.version {
            Some(v) => cmp_version(v, current_version) == std::cmp::Ordering::Less,
            None => false,
        })
        .max_by(|a, b| cmp_version(a.version.as_deref().unwrap(), b.version.as_deref().unwrap()))
}

/// 上一次升级的回执。
pub fn last_update(data_dir: &Path) -> Option<LastUpdate> {
    let text = std::fs::read_to_string(updates_dir(data_dir).join(LAST_UPDATE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 记下这次升级（`from` → `to`，配哪个包）。
pub fn record_update(
    data_dir: &Path,
    from: &str,
    to: &str,
    package_file: &str,
    at: &str,
) -> Result<LastUpdate> {
    let record = LastUpdate {
        from: from.to_string(),
        to: to.to_string(),
        at: at.to_string(),
        package: package_file.to_string(),
    };
    let dir = updates_dir(data_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("建目录失败：{}", dir.display()))?;
    let path = dir.join(LAST_UPDATE_FILE);
    let text = serde_json::to_string_pretty(&record)?;
    std::fs::write(&path, text).with_context(|| format!("写升级回执失败：{}", path.display()))?;
    Ok(record)
}

/// 回退指引 + 本地留包清单（设置页「关于」/帮助里原文显示）。
pub fn rollback(data_dir: &Path, current_version: &str) -> Rollback {
    let packages = list_packages(data_dir);
    let previous = previous_package(data_dir, current_version);
    Rollback {
        guide: rollback_guide(
            &installers_dir(data_dir).to_string_lossy(),
            previous.as_ref(),
            current_version,
        ),
        installers_dir: installers_dir(data_dir).to_string_lossy().into_owned(),
        packages,
        previous,
        last_update: last_update(data_dir),
    }
}

/// 回退指引原文。**可执行**：每一步都是用户能照着做的动作，不写「请自行处理」。
///
/// 不做一键回退按钮（票 18 §3 Q11）：那一步要跑安装器，做进应用里只会把当前进程与安装器
/// 搅在一起；给文件路径 + 步骤，用户双击即可。
pub fn rollback_guide(
    installers_dir: &str,
    previous: Option<&LocalPackage>,
    current: &str,
) -> String {
    let target = match previous {
        Some(p) => format!("双击这一个安装包：\n     {}", p.path),
        None => format!(
            "本次是**首次安装**（或本地没留更旧的包），回退要用你当初下载的那一份安装包；\n     \
             也可以把旧版安装包拷进 {} 再按下面的步骤做。",
            installers_dir
        ),
    };
    format!(
        "回到上一个版本（当前 {current}）：\n\
         1. 托盘右键「退出」，让 Claude Gateway 完全退出（升级/回退期间程序不能在跑）。\n\
         2. {target}\n\
         3. 安装器会覆盖安装旧版（仍是 per-user，不需要管理员）；装完自动打开，版本回到旧版。\n\
         4. 数据、端口、客户端配置**都不动**：数据目录 %APPDATA%\\com.claudegateway.desktop 一个字节不变，\n   \
            端口还是配置里那个，三个客户端的配置也不会被重写（升级时 NSIS 走 /UPDATE，跳过数据与客户端的清理）。\n\
         5. 想再回到新版：装 {} 里版本最高的那一个（新版的包每次升级都会留一份）。\n\
         说明：只有「升级上去的」版本才会自动留包，手工装的那一版不在 updates\\installers\\ 里。",
        installers_dir
    )
}

/// 从安装包文件名认版本：`Claude Gateway_0.2.0_x64-setup.exe` → `0.2.0`。
///
/// 认法：按 `_` 切，取第一段**只由数字和点组成且含点**的；认不出回 `None`（不猜）。
pub fn version_from_file_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".exe").unwrap_or(file_name);
    stem.split('_')
        .find(|part| {
            part.contains('.')
                && part
                    .split('.')
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|s| s.to_string())
}

/// 从安装包 URL 认文件名：`…/Claude%20Gateway_0.2.0_x64-setup.exe` → `Claude Gateway_0.2.0_x64-setup.exe`。
///
/// 只做路径解码（`%XX`），不认 `+` 当空格（URL 路径里 `+` 就是加号）；末段为空就回一个兜底名。
pub fn file_name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode(last);
    if decoded.is_empty() {
        "claude-gateway-update.exe".to_string()
    } else {
        decoded
    }
}

/// `%XX` → 字节 → UTF-8（解不出的字节按替换字符处理，不 panic）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 比版本大小：按点切、逐段比数字；段数不同时短的按缺 0 算（`1.2` == `1.2.0`）。
fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let nums = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (mut x, mut y) = (nums(a), nums(b));
    let len = x.len().max(y.len());
    x.resize(len, 0);
    y.resize(len, 0);
    x.cmp(&y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cg-update-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn saved_packages_land_in_the_updates_dir_and_cannot_escape_it() {
        let dir = scratch("save");
        let path = save_package(&dir, "Claude Gateway_0.2.0_x64-setup.exe", b"v2").expect("留包");
        assert_eq!(
            path,
            installers_dir(&dir).join("Claude Gateway_0.2.0_x64-setup.exe")
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"v2");

        // 远端给的名字带路径（甚至 `..`）也不许写到数据目录外面：只取最后一段。
        let escaped = save_package(&dir, r"..\..\evil.exe", b"x").expect("留包");
        assert_eq!(escaped, installers_dir(&dir).join("evil.exe"));
        assert!(escaped.starts_with(&dir), "{}", escaped.display());

        assert!(save_package(&dir, "..", b"x").is_err());
        assert!(save_package(&dir, "", b"x").is_err());
    }

    #[test]
    fn packages_are_listed_newest_first_with_the_version_read_from_the_name() {
        let dir = scratch("list");
        for name in [
            "Claude Gateway_0.1.0_x64-setup.exe",
            "Claude Gateway_0.3.0_x64-setup.exe",
            "Claude Gateway_0.2.0_x64-setup.exe",
            "unnamed-download.exe",
        ] {
            save_package(&dir, name, b"x").expect("留包");
        }
        let listed = list_packages(&dir);
        let versions: Vec<_> = listed.iter().map(|p| p.version.clone()).collect();
        assert_eq!(
            versions,
            vec![
                Some("0.3.0".to_string()),
                Some("0.2.0".to_string()),
                Some("0.1.0".to_string()),
                None
            ]
        );
        assert_eq!(listed[0].bytes, 1);
        assert!(listed[0]
            .path
            .ends_with("Claude Gateway_0.3.0_x64-setup.exe"));
    }

    #[test]
    fn previous_package_is_the_highest_one_strictly_below_the_running_version() {
        let dir = scratch("previous");
        for name in [
            "Claude Gateway_0.1.0_x64-setup.exe",
            "Claude Gateway_0.2.0_x64-setup.exe",
            "Claude Gateway_0.3.0_x64-setup.exe",
        ] {
            save_package(&dir, name, b"x").expect("留包");
        }
        // 跑 0.3.0：回退到 0.2.0（不是 0.1.0、也不是自己）
        let prev = previous_package(&dir, "0.3.0").expect("有上一版");
        assert_eq!(prev.version.as_deref(), Some("0.2.0"));
        assert_eq!(previous_package(&dir, "0.1.0"), None);
    }

    #[test]
    fn version_parsing_refuses_to_guess() {
        assert_eq!(
            version_from_file_name("Claude Gateway_0.2.0_x64-setup.exe").as_deref(),
            Some("0.2.0")
        );
        assert_eq!(
            version_from_file_name("Claude-Gateway_1.10.3_x64-portable.zip").as_deref(),
            Some("1.10.3")
        );
        assert_eq!(version_from_file_name("Claude-Gateway_x64-setup.exe"), None);
        assert_eq!(version_from_file_name("readme.txt"), None);
    }

    #[test]
    fn the_rollback_guide_is_executable_and_names_a_real_installer() {
        let dir = scratch("guide");
        save_package(&dir, "Claude Gateway_0.1.0_x64-setup.exe", b"v1").expect("留包");
        let rb = rollback(&dir, "0.2.0");
        assert_eq!(
            rb.previous
                .as_ref()
                .and_then(|p| p.version.clone())
                .as_deref(),
            Some("0.1.0")
        );
        assert!(!rb.guide.contains("请自行"), "{}", rb.guide);
        assert!(rb.guide.contains("退出"), "{}", rb.guide);
        assert!(
            rb.guide.contains(&rb.previous.as_ref().unwrap().path),
            "指引里要写出该双击哪个文件：{}",
            rb.guide
        );
        // 「数据 / 端口 / 客户端配置都不动」这条判据必须在指引里说出来
        for word in ["数据", "端口", "客户端配置"] {
            assert!(rb.guide.contains(word), "指引少了「{word}」：{}", rb.guide);
        }
        assert!(rb.guide.contains("updates\\installers\\"), "{}", rb.guide);

        // 没有旧包时也要给出可执行的做法（用当初下载的那份）
        let empty = rollback(&scratch("guide-empty"), "0.2.0");
        assert!(empty.previous.is_none());
        assert!(empty.guide.contains("当初下载"), "{}", empty.guide);
    }

    #[test]
    fn the_last_update_receipt_round_trips() {
        let dir = scratch("receipt");
        assert!(last_update(&dir).is_none());
        let written = record_update(
            &dir,
            "0.1.0",
            "0.2.0",
            "Claude Gateway_0.2.0_x64-setup.exe",
            "2026-10-15T15:00:00Z",
        )
        .expect("写回执");
        assert_eq!(last_update(&dir).as_ref(), Some(&written));
        assert_eq!(written.from, "0.1.0");
        assert_eq!(written.to, "0.2.0");
    }

    #[test]
    fn the_download_url_gives_the_file_name_we_save_the_package_under() {
        assert_eq!(
            file_name_from_url(
                "https://github.com/o/r/releases/download/v0.2.0/Claude%20Gateway_0.2.0_x64-setup.exe"
            ),
            "Claude Gateway_0.2.0_x64-setup.exe"
        );
        // 本地验收源（带查询串 / 端口）
        assert_eq!(
            file_name_from_url("http://127.0.0.1:8791/Claude-Gateway_0.2.0_x64-setup.exe?sig=1"),
            "Claude-Gateway_0.2.0_x64-setup.exe"
        );
        // 认不出来也不要给一个有路径的名字
        assert_eq!(
            file_name_from_url("http://127.0.0.1:8791/"),
            "claude-gateway-update.exe"
        );
    }

    #[test]
    fn the_status_shape_is_camel_case_and_carries_the_rollback() {
        let status = UpdateStatus {
            current_version: "0.2.0".into(),
            endpoints: vec!["https://example.invalid/latest.json".into()],
            checking: false,
            installing: false,
            last_checked_at: Some("2026-10-15T15:00:00Z".into()),
            available: Some(AvailableUpdate {
                version: "0.3.0".into(),
                notes: Some("修了几处".into()),
                pub_date: None,
                download_url: "https://example.invalid/x.exe".into(),
            }),
            downloaded_bytes: 0,
            total_bytes: None,
            installing_package: None,
            error: None,
            rollback: Rollback {
                guide: "g".into(),
                installers_dir: "d".into(),
                packages: vec![],
                previous: None,
                last_update: None,
            },
        };
        let json = serde_json::to_value(&status).expect("序列化");
        for key in [
            "currentVersion",
            "endpoints",
            "checking",
            "installing",
            "lastCheckedAt",
            "available",
            "downloadedBytes",
            "totalBytes",
            "installingPackage",
            "error",
            "rollback",
        ] {
            assert!(json.get(key).is_some(), "少了字段 {key}：{json}");
        }
        assert_eq!(
            json["available"]["downloadUrl"],
            "https://example.invalid/x.exe"
        );
        assert_eq!(json["rollback"]["installersDir"], "d");
    }

    #[test]
    fn platform_keys_cover_both_the_installer_named_and_the_plain_target() {
        // 装机版（bundle_type = nsis）先查 `windows-x86_64-nsis`；免安装版只查 `windows-x86_64`。
        assert!(WINDOWS_PLATFORM_KEYS.contains(&"windows-x86_64"));
        assert!(WINDOWS_PLATFORM_KEYS.contains(&"windows-x86_64-nsis"));
    }
}
