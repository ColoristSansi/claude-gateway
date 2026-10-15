//! 开机自启开关（票 62；定案见[票 18](../../../../../.scratch/model-gateway/issues/18-grilling-packaging-and-updates.md) §2）。
//!
//! 三条硬规矩：
//!
//! 1. **默认关闭**——不装就写，只有用户显式打开才写值。
//! 2. **写 HKCU 的 Run 键**，不用 Tauri autostart 插件：插件默认 `WindowsEnableMode::Dynamic`
//!    （先试 HKLM），以管理员身份运行时值会落到 HKLM，而卸载器只清 HKCU 同名值 → 卸载残留。
//! 3. **值名固定 `Claude Gateway`**（= `productName`）：NSIS 卸载段按这个名字删
//!    `HKCU\...\Run` 值（tauri-bundler 的 `installer.nsi`）。名字一改，关闭 / 卸载就会留残留。
//!
//! 注册表是这一项设置的**唯一真相**（不进 `settings` 表）：装机时用户的意愿跟数据目录无关，
//! 数据目录被删（卸载勾了「Delete app data」）也不该顺手改掉自启状态。
//!
//! 值的命令行 = `"<exe 全路径>" --autostart`（带空格的路径必须加引号；`--autostart` 让外壳
//! 开机时静默进托盘，见 `src-tauri`）。

use std::path::Path;

use anyhow::{bail, Result};

/// `HKCU` 下的 Run 键路径（**只写 HKCU**，绝不碰 HKLM）。
pub const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Run 值名 = `productName`。卸载器按这个名字清理，改不得。
pub const VALUE_NAME: &str = "Claude Gateway";

/// 自启启动时带的开关：外壳据此直接进托盘、不弹窗口。
///
/// 名字与 [`crate::dist::cli`] 的 `--autostart on|off`（设置开关本身）**故意不同**：
/// 一个是「这次是被 Run 键拉起来的」，一个是「改设置」。
pub const STARTUP_FLAG: &str = "--startup";

/// 写进 Run 值的命令行。
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" {STARTUP_FLAG}", exe.display())
}

/// Run 键的读写面。抽成 trait 是为了让开关逻辑（默认关、关掉即删）能在不碰真实注册表的地方断言；
/// 真实实现只有 [`HkcuRun`]。
pub trait RunKey {
    /// 当前值（`None` = 没开）。
    fn get(&self) -> Result<Option<String>>;
    /// 写值（覆盖已有值）。
    fn set(&self, value: &str) -> Result<()>;
    /// 删值；本来就没有也算成功（幂等）。
    fn delete(&self) -> Result<()>;
}

/// 开着的判据就是「Run 键里有这个值」，不看数据库（换机 / 手改注册表都认）。
pub fn is_enabled(key: &dyn RunKey) -> Result<bool> {
    Ok(key.get()?.is_some())
}

/// 开 / 关。开关本身幂等：`on` 时写当前 exe 的命令行，`off` 时删值。
/// 返回「关掉之后还有没有值」——`false` 就是「关闭后无残留」。
pub fn set_enabled(key: &dyn RunKey, on: bool, exe: &Path) -> Result<bool> {
    if on {
        let value = command_line(exe);
        key.set(&value)?;
        // 写完读回来：写失败被吞掉的话这里能发现。
        if key.get()?.as_deref() != Some(value.as_str()) {
            bail!("写入 HKCU Run 值后读回来不一致（写的是 {value}）");
        }
        Ok(true)
    } else {
        key.delete()?;
        let left = key.get()?;
        if left.is_some() {
            bail!("删 HKCU Run 值后仍在（{left:?}）");
        }
        Ok(false)
    }
}

// ── 真实实现：HKCU ────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{RunKey, RUN_KEY_PATH, VALUE_NAME};
    use anyhow::{anyhow, Context, Result};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    /// `HKCU\...\Run` 上的一个值。`value_name` 可换（测试用一个带进程号的临时名，不留残留）。
    pub struct HkcuRun {
        value_name: String,
    }

    impl HkcuRun {
        pub fn new() -> HkcuRun {
            HkcuRun::named(VALUE_NAME)
        }

        pub fn named(value_name: impl Into<String>) -> HkcuRun {
            HkcuRun {
                value_name: value_name.into(),
            }
        }

        pub fn value_name(&self) -> &str {
            &self.value_name
        }
    }

    impl Default for HkcuRun {
        fn default() -> HkcuRun {
            HkcuRun::new()
        }
    }

    /// 除值为「不存在」之外的失败都是真错误：不许把权限 / 句柄问题当成「没开」。
    fn check(code: WIN32_ERROR, what: &str) -> Result<()> {
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(anyhow!(
                "{what} 失败：Win32 错误码 {code}（{}）",
                std::io::Error::from_raw_os_error(code as i32)
            ))
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 开键。Run 键在每台机器上都有；开不了就报错（不静默当「没开」）。
    fn open(sam: u32) -> Result<HKEY> {
        let mut hkey: HKEY = std::ptr::null_mut();
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY_PATH).as_ptr(),
                0,
                sam,
                &mut hkey,
            )
        };
        check(code, &format!("打开 HKCU\\{RUN_KEY_PATH}"))?;
        Ok(hkey)
    }

    impl RunKey for HkcuRun {
        fn get(&self) -> Result<Option<String>> {
            let hkey = open(KEY_QUERY_VALUE)?;
            let name = wide(&self.value_name);
            let mut kind: u32 = 0;
            let mut size: u32 = 0;
            let code = unsafe {
                RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            if code == ERROR_FILE_NOT_FOUND {
                unsafe { RegCloseKey(hkey) };
                return Ok(None);
            }
            check(code, "读 Run 值长度")?;

            let mut buf = vec![0u8; size as usize];
            let code = unsafe {
                RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    buf.as_mut_ptr(),
                    &mut size,
                )
            };
            unsafe { RegCloseKey(hkey) };
            check(code, "读 Run 值")?;
            if kind != REG_SZ {
                return Err(anyhow!(
                    "Run 值 {} 不是 REG_SZ（类型 {kind}）",
                    self.value_name
                ));
            }

            let (units, _) = buf[..size as usize].as_chunks::<2>();
            let text: Vec<u16> = units
                .iter()
                .map(|pair| u16::from_ne_bytes(*pair))
                .take_while(|u| *u != 0)
                .collect();
            Ok(Some(
                String::from_utf16(&text).context("Run 值不是合法 UTF-16")?,
            ))
        }

        fn set(&self, value: &str) -> Result<()> {
            let hkey = open(KEY_SET_VALUE)?;
            let name = wide(&self.value_name);
            let data = wide(value);
            let bytes = (data.len() * std::mem::size_of::<u16>()) as u32;
            let code = unsafe {
                RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    bytes,
                )
            };
            unsafe { RegCloseKey(hkey) };
            check(code, &format!("写 Run 值 {}", self.value_name))
        }

        fn delete(&self) -> Result<()> {
            let hkey = open(KEY_SET_VALUE)?;
            let name = wide(&self.value_name);
            let code = unsafe { RegDeleteValueW(hkey, name.as_ptr()) };
            unsafe { RegCloseKey(hkey) };
            if code == ERROR_FILE_NOT_FOUND {
                return Ok(()); // 本来就没有 = 已经达成「无残留」
            }
            check(code, &format!("删 Run 值 {}", self.value_name))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{RunKey, VALUE_NAME};
    use anyhow::{bail, Result};

    pub struct HkcuRun {
        value_name: String,
    }

    impl HkcuRun {
        pub fn new() -> HkcuRun {
            HkcuRun::named(VALUE_NAME)
        }
        pub fn named(value_name: impl Into<String>) -> HkcuRun {
            HkcuRun {
                value_name: value_name.into(),
            }
        }
        pub fn value_name(&self) -> &str {
            &self.value_name
        }
    }

    impl Default for HkcuRun {
        fn default() -> HkcuRun {
            HkcuRun::new()
        }
    }

    impl RunKey for HkcuRun {
        fn get(&self) -> Result<Option<String>> {
            bail!("开机自启只有 Windows 有")
        }
        fn set(&self, _value: &str) -> Result<()> {
            bail!("开机自启只有 Windows 有")
        }
        fn delete(&self) -> Result<()> {
            bail!("开机自启只有 Windows 有")
        }
    }
}

pub use imp::HkcuRun;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// 内存里的 Run 键（假实现）：开关逻辑在它上面断言，字节不落注册表。
    #[derive(Default)]
    struct MemoryRun {
        values: Mutex<HashMap<String, String>>,
        name: String,
    }

    impl MemoryRun {
        fn new(name: &str) -> MemoryRun {
            MemoryRun {
                values: Mutex::new(HashMap::new()),
                name: name.to_string(),
            }
        }
    }

    impl RunKey for MemoryRun {
        fn get(&self) -> Result<Option<String>> {
            Ok(self.values.lock().expect("锁").get(&self.name).cloned())
        }
        fn set(&self, value: &str) -> Result<()> {
            self.values
                .lock()
                .expect("锁")
                .insert(self.name.clone(), value.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            self.values.lock().expect("锁").remove(&self.name);
            Ok(())
        }
    }

    #[test]
    fn default_is_off_and_only_an_explicit_switch_writes_a_value() {
        let key = MemoryRun::new(VALUE_NAME);
        // 默认关：没人写过就查不到值。
        assert!(!is_enabled(&key).expect("读开关"), "默认必须是关闭");
        assert_eq!(key.get().expect("读值"), None, "默认不许有 Run 值");
    }

    #[test]
    fn enabling_writes_the_quoted_exe_with_the_autostart_flag_and_disabling_leaves_nothing() {
        let key = MemoryRun::new(VALUE_NAME);
        let exe = Path::new(r"C:\Users\me\AppData\Local\Claude Gateway\claude-gateway.exe");

        assert!(set_enabled(&key, true, exe).expect("开自启"), "开完是开着");
        let value = key.get().expect("读值").expect("开完必须有值");
        assert_eq!(
            value, r#""C:\Users\me\AppData\Local\Claude Gateway\claude-gateway.exe" --startup"#,
            "带空格的路径必须加引号，并带上 --startup"
        );

        // 再开一次幂等（值不变）。
        assert!(set_enabled(&key, true, exe).expect("再开一次"));
        assert_eq!(key.get().expect("读值").as_deref(), Some(value.as_str()));

        assert!(
            !set_enabled(&key, false, exe).expect("关自启"),
            "关完是关着"
        );
        assert_eq!(key.get().expect("读值"), None, "关闭后不许留残留");
    }

    /// 真实注册表：用带进程号的临时值名，跑完删干净（不动用户真正的 `Claude Gateway` 值）。
    #[cfg(windows)]
    #[test]
    fn real_hkcu_run_value_round_trips_and_leaves_no_residue() {
        let probe = HkcuRun::named(format!("Claude Gateway (t62 test {})", std::process::id()));
        let exe = std::env::current_exe().expect("取当前 exe");

        assert!(!is_enabled(&probe).expect("读开关"), "临时值名一开始不存在");
        assert!(set_enabled(&probe, true, &exe).expect("写真实 Run 值"));
        assert!(is_enabled(&probe).expect("读开关"), "写完必须查得到");
        assert_eq!(
            probe.get().expect("读值").as_deref(),
            Some(command_line(&exe).as_str())
        );
        assert!(!set_enabled(&probe, false, &exe).expect("删真实 Run 值"));
        assert_eq!(probe.get().expect("读值"), None, "删完不许留残留");
    }
}
