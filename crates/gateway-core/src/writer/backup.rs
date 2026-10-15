//! 备份布局：`<备份根>\<客户端>\<文件名>.<yyyyMMdd-HHmmss>`（spec §8.4 第 7 条）。
//!
//! - 首写快照**常驻不清理**（路径记在 `client_config_state.backup_path`，清理时跳过它）。
//! - 其余是滚动备份：保留最近 **5** 份、超过 **90 天**（按文件 mtime）删。
//! - 首写时目标文件不存在 → **只写 marker，不造空备份**。
//! - 同一秒内的第二次备份加 `-1` / `-2` 后缀，不互相覆盖。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::ingress::api::rfc3339_utc;

/// 滚动备份保留份数。
pub(crate) const KEEP_ROLLING: usize = 5;
/// 滚动备份最大年龄。
pub(crate) const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// 时间戳长度 `yyyyMMdd-HHmmss`。
const STAMP_LEN: usize = 15;
/// 「首写时文件不存在」的 marker 后缀。
const MARKER_SUFFIX: &str = "absent";

/// 一份备份的落点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Backup {
    pub path: PathBuf,
    /// 文件当时不存在，只落了 marker。
    pub marker_only: bool,
}

/// 写前留一份备份。
///
/// `bytes = None` 表示首写时目标文件不存在（只写 marker）；`keep` = 首写快照路径，清理时永不删它。
pub(crate) fn snapshot(
    backup_root: &Path,
    client: &str,
    file_name: &str,
    bytes: Option<&[u8]>,
    keep: Option<&Path>,
) -> io::Result<Backup> {
    let dir = backup_root.join(client);
    fs::create_dir_all(&dir)?;
    let stamp = stamp_now();
    let marker = bytes.is_none();

    let mut path = None;
    for n in 0..64 {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let extra = if marker {
            format!(".{MARKER_SUFFIX}")
        } else {
            String::new()
        };
        let cand = dir.join(format!("{file_name}.{stamp}{suffix}{extra}"));
        if !cand.exists() {
            path = Some(cand);
            break;
        }
    }
    let path = path.ok_or_else(|| {
        io::Error::other(format!("同一个时间戳下的备份名用完了：{}", dir.display()))
    })?;

    match bytes {
        Some(bytes) => {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        None => {
            // 只写 marker：记下「首写时这个文件本来不存在」。
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            writeln!(
                f,
                "{{\"absent\":true,\"recorded_at\":\"{}\"}}",
                rfc3339_utc(SystemTime::now())
            )?;
            f.sync_all()?;
        }
    }

    retain(&dir, file_name, keep)?;
    Ok(Backup {
        path,
        marker_only: marker,
    })
}

/// 清理：滚动备份留最近 5 份、超 90 天删；`keep`（首写快照）永不删。
fn retain(dir: &Path, file_name: &str, keep: Option<&Path>) -> io::Result<()> {
    let prefix = format!("{file_name}.");
    let mut items: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        if rest.len() < STAMP_LEN {
            continue;
        }
        let stamp = &rest[..STAMP_LEN];
        if !is_stamp(stamp) {
            // 不认识的名字（别的工具留的）一律不碰。
            continue;
        }
        items.push((stamp.to_string(), entry.path()));
    }
    // 新的在前（戳是定长可比较的）
    items.sort_by(|a, b| b.0.cmp(&a.0));

    let now = SystemTime::now();
    let mut rank = 0usize;
    for (_, path) in items {
        if keep.map(|k| k == path.as_path()).unwrap_or(false) {
            continue; // 首写快照常驻
        }
        let too_many = rank >= KEEP_ROLLING;
        let too_old = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|age| age > MAX_AGE)
            .unwrap_or(false);
        if too_many || too_old {
            let _ = fs::remove_file(&path);
        } else {
            rank += 1;
        }
    }
    Ok(())
}

fn is_stamp(s: &str) -> bool {
    s.len() == STAMP_LEN
        && s.as_bytes()[..8].iter().all(u8::is_ascii_digit)
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[9..].iter().all(u8::is_ascii_digit)
}

/// 当前本地时间 → `yyyyMMdd-HHmmss`。
///
/// 本地时间：备份名是给人看的（票 28 第 6 条第 4 点只定了格式，没定 UTC）。
#[cfg(windows)]
pub(crate) fn stamp_now() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

/// 非 Windows（只为编译通过）：用本进程算出的 UTC 戳。
#[cfg(not(windows))]
pub(crate) fn stamp_now() -> String {
    let s = rfc3339_utc(SystemTime::now());
    // "YYYY-MM-DDTHH:MM:SSZ" → "YYYYMMDD-HHMMSS"
    format!(
        "{}{}{}-{}{}{}",
        &s[0..4],
        &s[5..7],
        &s[8..10],
        &s[11..13],
        &s[14..16],
        &s[17..19]
    )
}
