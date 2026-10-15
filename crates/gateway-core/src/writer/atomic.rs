//! 原子替换：同目录临时文件 + fsync + `ReplaceFileW`（spec §8.4 第 1、2 条）。
//!
//! - 崩在替换之前：目标文件一个字节没动，只留一个 `<文件名>.cg-tmp*` 临时文件
//!   （超过 24 小时的陈旧临时文件下次写入时顺手清掉）。
//! - `ReplaceFileW` 遇 `NotFound` / `ERROR_NOT_SUPPORTED` 退 `fs::rename`，**最多 3 轮**；
//!   替换失败即删临时文件、不做前滚。
//! - 被占用 / 权限不足 / 只读：退避重试 **3 次（~200ms 间隔）**，仍失败报错。
//! - 不做父目录 fsync（票 28 第 2 条第 1 点）。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, SystemTime};

/// 占用/权限失败的重试次数（票 28 第 2 条第 3 点）。
pub(crate) const BUSY_RETRIES: u32 = 3;
/// 重试间隔。
pub(crate) const BUSY_BACKOFF: Duration = Duration::from_millis(200);
/// `ReplaceFileW` 的轮数（票 28 第 2 条第 1 点）。
const REPLACE_ROUNDS: u32 = 3;
/// 临时文件后缀（票 28 遗留字面值之一，这里定死）。
const TEMP_SUFFIX: &str = "cg-tmp";
/// 陈旧临时文件的年龄上限：下次写入顺手清掉。
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 临时文件名最多试几个后缀。
const TEMP_CANDIDATES: u32 = 32;

/// 原子替换 `target` 的内容为 `bytes`。
pub(crate) fn replace(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "目标文件没有父目录"))?;
    fs::create_dir_all(dir)?;

    let mut last: Option<io::Error> = None;
    for attempt in 0..=BUSY_RETRIES {
        if attempt > 0 {
            sleep(BUSY_BACKOFF);
        }
        let tmp = write_temp(target, bytes)?;
        match swap(&tmp, target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                if is_busy(&e) {
                    last = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("原子替换失败（原因未知）")))
}

/// 写同目录临时文件并 fsync；文件已存在就换一个后缀（陈旧清掉）。
fn write_temp(target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "目标文件没有文件名"))?
        .to_string_lossy()
        .to_string();
    cleanup_stale(target, &name);

    for n in 0..TEMP_CANDIDATES {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!(".{n}")
        };
        let cand = target.with_file_name(format!("{name}.{TEMP_SUFFIX}{suffix}"));
        match OpenOptions::new().write(true).create_new(true).open(&cand) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                // fsync 文件本体（不做父目录 fsync）。
                f.sync_all()?;
                return Ok(cand);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "同目录已有一堆 <文件名>.{TEMP_SUFFIX}* 临时文件，建不出新的"
    )))
}

/// 清掉超过 24 小时的陈旧临时文件（崩溃残留）。
fn cleanup_stale(target: &Path, name: &str) {
    for n in 0..TEMP_CANDIDATES {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!(".{n}")
        };
        let cand = target.with_file_name(format!("{name}.{TEMP_SUFFIX}{suffix}"));
        let Ok(meta) = fs::metadata(&cand) else {
            continue;
        };
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .map(|age| age > STALE_TEMP_AGE)
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(&cand);
        }
    }
}

/// 临时文件 → 目标文件。Windows 走 `ReplaceFileW`，失败按规则退 `fs::rename`。
fn swap(tmp: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let mut last: Option<io::Error> = None;
        for _ in 0..REPLACE_ROUNDS {
            match replace_file_w(target, tmp) {
                Ok(()) => return Ok(()),
                Err(e) if fallback_to_rename(&e) => {} // 退 rename：这一轮的成败看下面那次
                Err(e) => return Err(e),
            }
            match fs::rename(tmp, target) {
                Ok(()) => return Ok(()),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("替换失败（原因未知）")))
    }
    #[cfg(not(windows))]
    {
        fs::rename(tmp, target)
    }
}

/// `NotFound` / `ERROR_NOT_SUPPORTED` → 退 `fs::rename`（票 28 第 2 条第 1 点）。
#[cfg(windows)]
fn fallback_to_rename(e: &io::Error) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND,
    };
    match e.raw_os_error().map(|c| c as u32) {
        Some(c) => {
            c == ERROR_FILE_NOT_FOUND || c == ERROR_PATH_NOT_FOUND || c == ERROR_NOT_SUPPORTED
        }
        None => false,
    }
}

/// 占用 / 权限不足 / 只读 → 值得退避重试。
fn is_busy(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::PermissionDenied {
        return true;
    }
    // 共享冲突 / 锁冲突 / 只读卷（std 有的映射不到 PermissionDenied）。
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_WRITE_PROTECT: i32 = 19;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    // ReplaceFileW 碰上只读目标时给的两个码（「只读」也在退避重试之列，spec §8.4 第 2 条）。
    const ERROR_UNABLE_TO_REMOVE_REPLACED: i32 = 1175;
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT: i32 = 1176;
    matches!(
        e.raw_os_error(),
        Some(
            ERROR_ACCESS_DENIED
                | ERROR_WRITE_PROTECT
                | ERROR_SHARING_VIOLATION
                | ERROR_LOCK_VIOLATION
                | ERROR_UNABLE_TO_REMOVE_REPLACED
                | ERROR_UNABLE_TO_MOVE_REPLACEMENT
        )
    )
}

#[cfg(windows)]
fn replace_file_w(target: &Path, tmp: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let replaced = wide(target);
    let replacement = wide(tmp);
    // 不设备份名（备份由引擎自己的备份布局负责）、不排除、不保留 ACL 之外的额外语义。
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        return Ok(());
    }
    Err(io::Error::from_raw_os_error(
        unsafe { GetLastError() } as i32
    ))
}
