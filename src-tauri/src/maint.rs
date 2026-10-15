//! 无界面的维护动作（票 62）：命令行解析在内核里（`gateway_core::dist::cli`），这里只执行。
//!
//! 为什么走同一个 exe：卸载器的 `ExecWait` 只能叫得动已经装好的东西，而它要调的那件事
//! （按 `backups\` 与 `client_config_state` 还原客户端配置）必须用**同一个版本的写入引擎**。
//! 「导出 / 导入可脚本调用」也顺带落在这里：`claude-gateway.exe --export D:\x.json`。
//!
//! 出口码：`0` 成功、`1` 动作失败、`2` 参数错误。`--restore-clients` 有任何一条失败就是 1，
//! 卸载器据此显示「请手动还原」。

use std::path::Path;

use gateway_core::dist::autostart::{self, HkcuRun, RunKey};
use gateway_core::dist::bundle::{self, ExportOptions, ImportMode};
use gateway_core::dist::cli::{self, Command};
use gateway_core::dist::restore::{self, ClientRoots};
use gateway_core::store::Store;

/// 跑一个无界面动作。返回 `None` = 这不是维护命令，照常启动外壳。
pub fn run(args: &[String]) -> Option<i32> {
    let command = match cli::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("参数错误：{e}");
            return Some(2);
        }
    };

    match command {
        Command::Run { .. } => None,
        Command::Help => {
            println!("{}", cli::USAGE);
            Some(0)
        }
        Command::Version => {
            println!("Claude Gateway {}", env!("CARGO_PKG_VERSION"));
            Some(0)
        }
        Command::Autostart { on } => Some(finish(autostart_cmd(on))),
        Command::RestoreClients { .. } => Some(finish(block_on(restore_clients_cmd()))),
        Command::Export {
            path,
            password,
            with_secrets,
        } => Some(finish(block_on(export_cmd(&path, password, with_secrets)))),
        Command::Import {
            path,
            password,
            overwrite,
        } => Some(finish(block_on(import_cmd(&path, password, overwrite)))),
    }
}

fn finish(result: anyhow::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("失败：{e:#}");
            1
        }
    }
}

/// 维护动作要开库 / 跑异步存储线程：借 Tauri 那个 tokio runtime（外壳本来就用它起网关）。
fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(f: F) -> anyhow::Result<()> {
    tauri::async_runtime::block_on(f)
}

/// `--autostart on|off|status`：写 / 删 / 查 `HKCU\...\Run`。
fn autostart_cmd(on: Option<bool>) -> anyhow::Result<()> {
    let key = HkcuRun::new();
    match on {
        None => {
            // 只查：`on` / `off` 一个词，脚本好判。
            let enabled = autostart::is_enabled(&key)?;
            println!("{}", if enabled { "on" } else { "off" });
            if enabled {
                println!("{}", key.get()?.unwrap_or_default());
            }
        }
        Some(enabled) => {
            let exe = std::env::current_exe()?;
            let now = autostart::set_enabled(&key, enabled, &exe)?;
            println!(
                "开机自启：{}（HKCU\\{} · {}）",
                if now { "已开启" } else { "已关闭" },
                autostart::RUN_KEY_PATH,
                autostart::command_line(&exe)
            );
        }
    }
    Ok(())
}

/// 卸载器调的那条：还原三个客户端的官方配置。**默认还原**就是它的语义。
async fn restore_clients_cmd() -> anyhow::Result<()> {
    let store = Store::open_default()?;
    let data_dir = store.root().to_path_buf();
    let port = restore::configured_port(&store).await;
    let report = restore::restore_clients(&store, &data_dir, port, &ClientRoots::real()).await;
    println!("{}", report.summary());
    if report.failed() {
        anyhow::bail!(
            "有客户端没能还原：{}。请手动把它切回官方配置（Claude Desktop 还要完全退出重启）",
            report.errors.join("；")
        );
    }
    Ok(())
}

async fn export_cmd(
    path: &Path,
    password: Option<String>,
    with_secrets: bool,
) -> anyhow::Result<()> {
    let store = Store::open_default()?;
    let opts = ExportOptions {
        include_secrets: with_secrets,
        password,
    };
    let text = bundle::export(&store, env!("CARGO_PKG_VERSION"), &opts).await?;
    std::fs::write(path, text.as_bytes())
        .map_err(|e| anyhow::anyhow!("写导出文件失败（{}）：{e}", path.display()))?;
    println!(
        "已导出到 {}（{} 字节；明文密钥：{}；加密：{}）",
        path.display(),
        text.len(),
        if opts.include_secrets {
            "有"
        } else {
            "没有"
        },
        if opts.password.is_some() {
            "是"
        } else {
            "否"
        }
    );
    if opts.password.is_none() {
        println!("{}", gateway_core::dist::DPAPI_MIGRATION_WARNING);
    }
    Ok(())
}

async fn import_cmd(path: &Path, password: Option<String>, overwrite: bool) -> anyhow::Result<()> {
    let store = Store::open_default()?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读导入文件失败（{}）：{e}", path.display()))?;
    let mode = if overwrite {
        ImportMode::Replace
    } else {
        ImportMode::SkipExisting
    };
    let report = bundle::import(&store, &text, password.as_deref(), mode).await?;
    println!(
        "已导入 {}（格式 v{}，导出于 {}）：新建上游 {}，覆盖 {}，跳过 {}，重复上游 {}{}，模型 {}，绑定 {}，设置 {}，密钥 {}{}",
        path.display(),
        report.version,
        report.exported_at,
        report.upstreams_created,
        report.upstreams_replaced,
        report.upstreams_skipped,
        report.duplicates.len(),
        if report.duplicates.is_empty() {
            String::new()
        } else {
            format!("（{}）", report.duplicates.join("、"))
        },
        report.upstream_models_imported,
        report.virtual_models_imported,
        report.settings_imported,
        report.credentials_imported,
        if report.credentials_skipped > 0 {
            format!("，另有 {} 条凭据因文件不含密钥而跳过（请重新填写）", report.credentials_skipped)
        } else {
            String::new()
        }
    );
    Ok(())
}
