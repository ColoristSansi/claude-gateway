//! 命令行面（票 62）：导出 / 导入 / 自启 / 卸载时还原都**可脚本调用**。
//!
//! 解析放在内核里（不依赖 Tauri），外壳的 bin 只负责把 [`Command`] 执行掉；因此参数面能被
//! 单测钉住，卸载器的 `claude-gateway.exe --restore-clients --silent` 也不会「只在装机后才验得了」。
//!
//! 认不出来的 `--xxx` 一律回落成 [`Command::Run`]：Windows 拉起 GUI 时会带自己的参数
//! （shell 打开文件等），不能因为一个不认识的开关就不给用户开界面。真正的维护动作必须**显式**
//! 写对参数（写错了就是启动界面，不是静默改配置）。

use std::path::PathBuf;

use anyhow::{bail, Result};

use super::autostart::STARTUP_FLAG;

/// 外壳该干什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// 正常启动：建托盘、起网关、开窗口。
    Run {
        /// 从 Run 键拉起来的（带了 `--startup`）：直接进托盘，不弹窗口。
        from_startup: bool,
    },
    /// 开机自启开关：`Some(true/false)` = 开 / 关，`None` = 只查（打印 `on` / `off`）。
    Autostart { on: Option<bool> },
    /// 还原三个客户端的官方配置（卸载器在删 `$INSTDIR` 之前调它）。`--silent` 只是为了
    /// 让卸载器那条命令行读起来自解释——这条路本来就没有界面。
    RestoreClients { silent: bool },
    /// 导出配置到 JSON 文件。默认不含明文密钥；`--password` = 加密导出（可跨机）。
    Export {
        path: PathBuf,
        password: Option<String>,
        with_secrets: bool,
    },
    /// 从 JSON 文件导入。重复上游默认跳过，`--overwrite` 才覆盖。
    Import {
        path: PathBuf,
        password: Option<String>,
        overwrite: bool,
    },
    /// 打印版本。
    Version,
    /// 打印用法。
    Help,
}

/// 用法说明（`--help` 打这份；票 59 的设置页文案也照它）。
pub const USAGE: &str = "\
Claude Gateway
  不带参数                        启动界面与网关（正常用法）
  --startup                       开机自启拉起时用：直接进托盘，不弹窗口（Run 键里写的就是它）
  --autostart on|off|status       开机自启开关（写 / 删 HKCU Run 值；status 只打印）
  --restore-clients [--silent]    还原三个客户端的官方配置（卸载器删程序目录之前调这个）
  --export <文件> [--password <口令>] [--with-secrets]
                                  导出配置；默认**不含明文密钥**；给口令 = 加密导出（可跨机）
  --import <文件> [--password <口令>] [--overwrite]
                                  导入配置；重复上游默认跳过（不覆盖），--overwrite 才覆盖
  --version                       打印版本
  --help                          这份说明
";

/// 解析参数（不含 argv[0]）。参数错误一律报错并带用法，不猜。
pub fn parse(args: &[String]) -> Result<Command> {
    // `--help` / `--version` 先看：它们压过其它一切（`--export a.json --version` 打版本）。
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Command::Help);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        return Ok(Command::Version);
    }

    let mut from_startup = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            STARTUP_FLAG => from_startup = true,
            "--silent" => {
                // 单独出现没有意义：它是 --restore-clients 的伴生开关。
                if !args.iter().any(|a| a == "--restore-clients") {
                    bail!("--silent 只能配 --restore-clients 用\n{USAGE}");
                }
            }
            "--restore-clients" => {
                let silent = args.iter().any(|a| a == "--silent");
                return Ok(Command::RestoreClients { silent });
            }
            "--autostart" => {
                let value = args.get(i + 1).map(String::as_str);
                return Ok(Command::Autostart {
                    on: match value {
                        Some("on") | Some("enable") | Some("1") => Some(true),
                        Some("off") | Some("disable") | Some("0") => Some(false),
                        Some("status") | None => None,
                        Some(other) => {
                            bail!("--autostart 只认 on / off / status（给的是 {other:?}）\n{USAGE}")
                        }
                    },
                });
            }
            "--export" => {
                let path = take_value(args, i, "--export")?;
                let password = optional_value(args, "--password")?;
                return Ok(Command::Export {
                    path: PathBuf::from(path),
                    password,
                    with_secrets: args.iter().any(|a| a == "--with-secrets"),
                });
            }
            "--import" => {
                let path = take_value(args, i, "--import")?;
                let password = optional_value(args, "--password")?;
                return Ok(Command::Import {
                    path: PathBuf::from(path),
                    password,
                    overwrite: args.iter().any(|a| a == "--overwrite"),
                });
            }
            // 不认识的开关/参数：当普通启动（见模块文档）。
            _ => {}
        }
        i += 1;
    }
    Ok(Command::Run { from_startup })
}

/// `--export` / `--import` 后面那个值。缺了就报错（不猜当前目录）。
fn take_value(args: &[String], i: usize, flag: &str) -> Result<String> {
    match args.get(i + 1) {
        Some(v) if !v.starts_with("--") => Ok(v.clone()),
        _ => bail!("{flag} 后面要跟一个文件路径\n{USAGE}"),
    }
}

/// `--password <口令>`；没给就是 `None`。
fn optional_value(args: &[String], flag: &str) -> Result<Option<String>> {
    match args.iter().position(|a| a == flag) {
        None => Ok(None),
        Some(i) => match args.get(i + 1) {
            Some(v) if !v.starts_with("--") => Ok(Some(v.clone())),
            _ => bail!("{flag} 后面要跟口令\n{USAGE}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn no_arguments_is_a_normal_run() {
        assert_eq!(
            parse(&[]).expect("空参数"),
            Command::Run {
                from_startup: false
            }
        );
    }

    #[test]
    fn startup_flag_only_changes_whether_the_window_pops() {
        assert_eq!(
            parse(&argv(&[STARTUP_FLAG])).expect("自启拉起"),
            Command::Run { from_startup: true }
        );
        // 认不出来的开关（Windows 会给 GUI 带）照样启动，不当参数错误。
        assert_eq!(
            parse(&argv(&["--no-sandbox", "file.txt"])).expect("未知参数"),
            Command::Run {
                from_startup: false
            }
        );
    }

    #[test]
    fn autostart_takes_on_off_or_status_and_rejects_anything_else() {
        assert_eq!(
            parse(&argv(&["--autostart", "on"])).expect("开"),
            Command::Autostart { on: Some(true) }
        );
        assert_eq!(
            parse(&argv(&["--autostart", "off"])).expect("关"),
            Command::Autostart { on: Some(false) }
        );
        assert_eq!(
            parse(&argv(&["--autostart"])).expect("不带值 = 查"),
            Command::Autostart { on: None }
        );
        assert_eq!(
            parse(&argv(&["--autostart", "status"])).expect("status = 查"),
            Command::Autostart { on: None }
        );
        let err = parse(&argv(&["--autostart", "yes"])).expect_err("乱给值必须报错");
        assert!(err.to_string().contains("on / off / status"), "{err}");
    }

    #[test]
    fn restore_clients_is_recognised_with_and_without_silent() {
        assert_eq!(
            parse(&argv(&["--restore-clients"])).expect("还原"),
            Command::RestoreClients { silent: false }
        );
        // 卸载器用的那条命令行，逐字。
        assert_eq!(
            parse(&argv(&["--restore-clients", "--silent"])).expect("静默还原"),
            Command::RestoreClients { silent: true }
        );
        let err = parse(&argv(&["--silent"])).expect_err("孤立的 --silent 必须报错");
        assert!(err.to_string().contains("--restore-clients"), "{err}");
    }

    #[test]
    fn export_defaults_to_no_secrets_and_takes_password_and_flag() {
        assert_eq!(
            parse(&argv(&["--export", r"C:\tmp\a.json"])).expect("默认导出"),
            Command::Export {
                path: PathBuf::from(r"C:\tmp\a.json"),
                password: None,
                with_secrets: false
            }
        );
        assert_eq!(
            parse(&argv(&[
                "--export",
                r"C:\tmp\b.json",
                "--password",
                "口令",
                "--with-secrets"
            ]))
            .expect("加密导出"),
            Command::Export {
                path: PathBuf::from(r"C:\tmp\b.json"),
                password: Some("口令".into()),
                with_secrets: true
            }
        );
        // 缺路径 / 缺口令都不许猜。
        assert!(parse(&argv(&["--export"])).is_err());
        assert!(parse(&argv(&["--export", "--password", "x"])).is_err());
        assert!(parse(&argv(&["--export", "a.json", "--password"])).is_err());
    }

    #[test]
    fn import_takes_overwrite_and_password() {
        assert_eq!(
            parse(&argv(&["--import", "a.json"])).expect("导入"),
            Command::Import {
                path: PathBuf::from("a.json"),
                password: None,
                overwrite: false
            }
        );
        assert_eq!(
            parse(&argv(&[
                "--import",
                "a.json",
                "--password",
                "p",
                "--overwrite"
            ]))
            .expect("覆盖导入"),
            Command::Import {
                path: PathBuf::from("a.json"),
                password: Some("p".into()),
                overwrite: true
            }
        );
        assert!(parse(&argv(&["--import"])).is_err());
    }

    #[test]
    fn help_and_version_win_over_everything() {
        assert_eq!(parse(&argv(&["--help"])).expect("help"), Command::Help);
        assert_eq!(
            parse(&argv(&["--export", "a.json", "--version"])).expect("version"),
            Command::Version
        );
        assert!(USAGE.contains("--restore-clients"));
        assert!(USAGE.contains("不含明文密钥"));
    }
}
