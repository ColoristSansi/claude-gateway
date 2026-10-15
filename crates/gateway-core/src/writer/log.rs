//! 元数据级日志（spec §8.4 第 9 条）：每次写入 / 还原一行，**只记元数据**（客户端、文件、
//! 结果、备份路径、时间），不记 body；按天一个文件，保留 7 天（spec §9 的日志保留口径）。
//!
//! 日志写失败不许把写入本身带崩：调用方一律 `let _ = ...`。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::ingress::api::rfc3339_utc;

/// 日志保留天数（spec §9：7 天）。
pub(crate) const KEEP_DAYS: u64 = 7;

/// 追加一行（顺带清掉超过 7 天的旧文件）。
pub(crate) fn append(log_root: &Path, line: &str) -> io::Result<()> {
    fs::create_dir_all(log_root)?;
    retain(log_root);
    let name = format!("writer-{}.log", day_stamp(SystemTime::now()));
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_root.join(name))?;
    writeln!(f, "[{}] {line}", rfc3339_utc(SystemTime::now()))
}

/// 删掉超过 7 天的 `writer-*.log`。
fn retain(log_root: &Path) {
    let Ok(entries) = fs::read_dir(log_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("writer-") || !name.ends_with(".log") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .map(|age| age > Duration::from_secs(KEEP_DAYS * 24 * 60 * 60))
            .unwrap_or(false);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `YYYYMMDD`（UTC，文件名用）。
fn day_stamp(t: SystemTime) -> String {
    let s = rfc3339_utc(t);
    format!("{}{}{}", &s[0..4], &s[5..7], &s[8..10])
}
