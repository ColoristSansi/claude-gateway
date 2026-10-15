//! 单实例互斥量（ADR-0006 第 8 条）：外壳在启动时取一次，保证进程内只有一个写入方。
//!
//! 引擎自己只用**每文件一把锁**（[`super::engine`]）；跨进程的第二个写入方由这里的命名
//! 互斥量挡住，真漏进来也还有冲突检测兜底。**不做跨进程锁文件**。
//!
//! 外壳接法（票 43 的 `setup` 里一行）：取不到就唤起已有窗口并退出本进程。

/// 持有它 = 本进程是唯一实例；进程退出（句柄关闭）即释放。
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// 取命名互斥量。`Ok(None)` = 已经有实例在跑（应当唤起它的窗口后退出）。
///
/// 名字由调用方给（如 `ClaudeGateway.SingleInstance`），免得内核对象名烤在引擎里。
#[cfg(windows)]
pub fn acquire(name: &str) -> anyhow::Result<Option<InstanceGuard>> {
    use anyhow::Context;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error()).context("建单实例互斥量失败");
    }
    let last = unsafe { GetLastError() };
    if last == ERROR_ALREADY_EXISTS {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        return Ok(None);
    }
    Ok(Some(InstanceGuard { handle }))
}

/// 非 Windows：不装门（本项目只发 Windows 版，这里只为编译通过）。
#[cfg(not(windows))]
pub fn acquire(_name: &str) -> anyhow::Result<Option<InstanceGuard>> {
    Ok(Some(InstanceGuard {}))
}
