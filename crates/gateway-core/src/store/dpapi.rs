//! DPAPI：当前用户范围的密钥加解密（spec §9「密钥」）。
//!
//! `CryptProtectData` / `CryptUnprotectData` 默认 CurrentUser 范围，所以密文**换机 / 换用户
//! 解不开**（票 18 已把它写成导出功能的已知边界）。这里不做可选熵：熵一改旧密文全废，
//! 而范围本身已经绑到当前用户。
//!
//! `CRYPTPROTECT_UI_FORBIDDEN`：不弹任何 UI 提示。加密在存储写线程上、解密在读连接那边的
//! blocking 池上，都不占 runtime 线程。

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, Result};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    fn empty() -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        }
    }

    /// 把 LocalAlloc 出来的结果拷走再释放（不 free 就是每次加密泄一块）。
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut core::ffi::c_void);
        bytes
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = blob(plain);
            let mut out = empty();
            let ok = CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err(anyhow!(
                    "CryptProtectData 失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(take(out))
        }
    }

    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>> {
        unsafe {
            let input = blob(cipher);
            let mut out = empty();
            let ok = CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return Err(anyhow!(
                    "CryptUnprotectData 失败（换机 / 换用户后密文解不开）：{}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(take(out))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{bail, Result};

    pub fn protect(_plain: &[u8]) -> Result<Vec<u8>> {
        bail!("DPAPI 只有 Windows 有")
    }

    pub fn unprotect(_cipher: &[u8]) -> Result<Vec<u8>> {
        bail!("DPAPI 只有 Windows 有")
    }
}

pub use imp::{protect, unprotect};
