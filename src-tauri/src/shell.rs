//! Tauri 外壳：托盘、窗口与网关生命周期（票 43；spec §10 托盘段 / 故障态、票 18 §2）。
//!
//! 分工：网关本体在 `gateway_core::service`（不依赖 Tauri），本模块只决定
//! 「什么时候起、什么时候停、托盘上怎么写」。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Window, WindowEvent, Wry};

use gateway_core::clients::ClientEnv;
use gateway_core::ingress::api::{rfc3339_utc, AdminCtx, GatewayControl};
use gateway_core::ingress::proxy;
use gateway_core::service::{self, Gateway, GatewayConfig};
use gateway_core::store::{self, Store};

/// 外壳的常驻状态：网关句柄 + 「网关状态」菜单项（要改文案的那一条）。
#[derive(Default)]
pub struct Shell {
    gateway: Mutex<Option<Gateway>>,
    status_item: Mutex<Option<MenuItem<Wry>>>,
}

/// 托盘图标：两种状态色（运行 / 停止）。票 43 只做两态，够表达「在不在跑」。
pub fn tray_icon_bytes(running: bool) -> &'static [u8] {
    if running {
        include_bytes!("../icons/tray-running.png")
    } else {
        include_bytes!("../icons/tray-stopped.png")
    }
}

/// 图标字节都必须是合法 PNG，供 `TrayIconBuilder::icon` / `set_icon` 用。
fn tray_icon(running: bool) -> Image<'static> {
    Image::from_bytes(tray_icon_bytes(running)).expect("内嵌托盘图标必须是合法 PNG")
}

/// 托盘「网关状态」那一行的文案（spec §10：运行中 · 端口）。
pub fn status_label(running: bool, port: u16) -> String {
    if running {
        format!("网关状态：运行中 · {}:{port}", service::HOST)
    } else {
        format!("网关状态：未运行（{}:{port} 未监听）", service::HOST)
    }
}

/// 端口配置：`<数据目录>\config.json` 里的 `port`；缺省、非法、越界一律回默认 8787。
///
/// shortcut: 端口暂读 JSON 文件，而不是票 42 库里的 `settings` 表 `settings_keys::PORT`
/// （spec §9 要求落库）；`store` 还没有设置项读写接口、设置页（票 52）也还没写它。
/// 接上后这里改读库，`config.json` 只留迁移用（默认值仍是 8787，行为不变）。
pub fn load_port(data_dir: &Path) -> u16 {
    let Ok(text) = fs::read_to_string(data_dir.join("config.json")) else {
        return service::DEFAULT_PORT;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("port").and_then(|p| p.as_u64()))
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .unwrap_or(service::DEFAULT_PORT)
}

/// 库里的端口（票 52 起 `settings.port` 是唯一真相）；读不到回 `None`，由 [`load_port`] 兜底。
pub fn port_from_store(store: &Store) -> Option<u16> {
    let value = tauri::async_runtime::block_on(store.read(|c| {
        Ok(
            c.query_row("SELECT value FROM settings WHERE key='port'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok(),
        )
    }))
    .ok()
    .flatten()?;
    value.trim().parse::<u16>().ok().filter(|p| *p != 0)
}

/// 数据目录下的日志目录（票 18 §5：`logs\` 放外壳 / 网关自身运行日志）。
pub fn logs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// 往 `<日志目录>\gateway.log` 追加一行（带 UTC 时间戳）。
pub fn append_log(logs_dir: &Path, line: &str) -> std::io::Result<()> {
    fs::create_dir_all(logs_dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("gateway.log"))?;
    writeln!(file, "[{}] {line}", rfc3339_utc(SystemTime::now()))
}

/// 外壳替网关做的两件事（管理 API 跑在网关进程里，重启自己得靠外壳）。
///
/// 两个方法都**立刻返回**：真正的启停在后台任务里做——请求正跑在网关自己的 runtime 上，
/// 同步等自己停会死锁。
struct TauriControl {
    app: AppHandle,
}

impl GatewayControl for TauriControl {
    fn start(&self) -> Result<(), String> {
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            let port = current_port();
            if let Err(e) = start_gateway(&app, port).await {
                eprintln!("[外壳] 启动网关失败：{e}");
            }
        });
        Ok(())
    }

    fn restart(&self, port: u16) -> Result<(), String> {
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            stop_gateway(&app);
            if let Err(e) = start_gateway(&app, port).await {
                eprintln!("[外壳] 按新端口 {port} 重启网关失败：{e}");
                update_tray_status(&app, false, port);
            }
        });
        Ok(())
    }
}

/// 当前端口：库优先，其次 `config.json`，再其次默认。
fn current_port() -> u16 {
    let Ok(data_dir) = store::default_data_dir() else {
        return service::DEFAULT_PORT;
    };
    match Store::open(&data_dir) {
        Ok(store) => port_from_store(&store).unwrap_or_else(|| load_port(&data_dir)),
        Err(_) => load_port(&data_dir),
    }
}

/// 读库里的上游 / 目录 / 绑定表，装出转发面（起不来就只挂管理面）。
async fn build_forwarding(store: &Store) -> Option<Arc<proxy::Gateway>> {
    let upstreams = gateway_core::egress::provider::list_upstreams(store).await;
    let catalog = gateway_core::egress::provider::catalog_map(store).await;
    let bindings = gateway_core::binding::load(store).await;
    match (upstreams, catalog, bindings) {
        (Ok(upstreams), Ok(catalog), Ok(bindings)) => {
            let upstreams = upstreams
                .into_iter()
                .map(|u| (u.id.clone(), u))
                .collect::<HashMap<_, _>>();
            let gw = proxy::Gateway::new(store.clone(), upstreams, bindings);
            gw.set_catalog(catalog);
            Some(Arc::new(gw))
        }
        (u, c, b) => {
            let e = u.err().or(c.err()).or(b.err()).map(|e| e.to_string());
            eprintln!("[外壳] 上游 / 绑定表读不出，转发面未挂载：{e:?}");
            None
        }
    }
}

/// 管理面上下文（票 52）：库 + 三个客户端的配置根 + 外壳句柄。
fn build_admin(
    app: &AppHandle,
    data_dir: &Path,
    store: Store,
    db_path: &Path,
    proxy: Option<Arc<proxy::Gateway>>,
) -> Option<Arc<AdminCtx>> {
    match ClientEnv::production(data_dir) {
        Ok(env) => {
            let ctx = AdminCtx::new(
                store,
                data_dir.to_path_buf(),
                env,
                Arc::new(TauriControl { app: app.clone() }),
                env!("CARGO_PKG_VERSION"),
                db_path.to_string_lossy().into_owned(),
            );
            Some(Arc::new(match proxy {
                Some(gw) => ctx.with_proxy(gw),
                None => ctx,
            }))
        }
        Err(e) => {
            eprintln!("[外壳] 客户端配置根算不出来，管理面聚合端点将不可用：{e}");
            None
        }
    }
}

/// 起网关（`setup` 与「启动网关」按钮共用一条路径）。
async fn start_gateway(app: &AppHandle, port: u16) -> Result<(), String> {
    let data_dir = store::default_data_dir().map_err(|e| e.to_string())?;
    let logs = logs_dir(&data_dir);
    let _ = fs::create_dir_all(&logs);
    let db_path = data_dir.join(store::DB_FILE_NAME);
    let store = Store::open(&data_dir).map_err(|e| e.to_string())?;
    let forwarding = build_forwarding(&store).await;
    let admin = build_admin(app, &data_dir, store, &db_path, forwarding.clone());
    let config = GatewayConfig {
        port,
        db_path: db_path.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        forwarding,
        admin,
    };
    let gateway = service::start(config).await.map_err(|e| {
        let line = format!("网关启动失败：{e}");
        eprintln!("{line}");
        let _ = append_log(&logs, &line);
        line
    })?;
    let real = gateway.port();
    let line = format!(
        "网关已启动：http://{}:{}（库：{}）",
        service::HOST,
        real,
        db_path.display()
    );
    println!("{line}");
    let _ = append_log(&logs, &line);
    let shell = app.state::<Shell>();
    if let Ok(mut slot) = shell.gateway.lock() {
        *slot = Some(gateway);
    }
    update_tray_status(app, true, real);
    Ok(())
}

/// 托盘「网关状态」那一行与图标跟着走。
fn update_tray_status(app: &AppHandle, running: bool, port: u16) {
    let shell = app.state::<Shell>();
    if let Ok(item) = shell.status_item.lock() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(status_label(running, port));
        }
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(tray_icon(running)));
        let _ = tray.set_tooltip(Some(format!(
            "Claude Gateway · {}",
            status_label(running, port)
        )));
    }
}

/// `setup`：备目录 → 起网关 → 建托盘。网关起不来也**继续建托盘**，
/// 好让用户看到「未运行」并去改端口（spec §10 故障态）。
pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    // 数据目录只有一处定义（票 42 的 `store::default_data_dir`）：`%APPDATA%\com.claudegateway.desktop\`。
    let data_dir = gateway_core::store::default_data_dir()?;
    let logs = logs_dir(&data_dir);
    fs::create_dir_all(&logs)?;

    let db_path = data_dir.join(store::DB_FILE_NAME);

    // 库先开：端口、管理面上下文、转发面装配都从它来（票 52 起 `settings.port` 是唯一真相，
    // `config.json` 只当老配置的迁移兜底）。
    let store = match Store::open(&data_dir) {
        Ok(store) => Some(store),
        Err(e) => {
            let line = format!("库打开失败，转发面与管理面聚合未挂载：{e}");
            eprintln!("{line}");
            let _ = append_log(&logs, &line);
            None
        }
    };
    let port = store
        .as_ref()
        .and_then(port_from_store)
        .unwrap_or_else(|| load_port(&data_dir));

    // 转发面 `/v1/*`：装配路径**从库里读**（票 51）——空库下三张表都是空的，
    // 未知模型按入站协议回 `not_found_error` 信封，而不是 axum 的裸 404，所以空库也照样挂上。
    let forwarding = match store.as_ref() {
        Some(store) => tauri::async_runtime::block_on(build_forwarding(store)),
        None => None,
    };
    let admin = match (store.as_ref(), forwarding.as_ref()) {
        (Some(store), Some(gw)) => build_admin(
            app.handle(),
            &data_dir,
            store.clone(),
            &db_path,
            Some(gw.clone()),
        ),
        (Some(store), None) => build_admin(app.handle(), &data_dir, store.clone(), &db_path, None),
        (None, _) => None,
    };

    let config = GatewayConfig {
        port,
        db_path: db_path.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        forwarding,
        admin,
    };

    // 网关与外壳同进程（票 18 §2 定案：不是 sidecar）。
    let (gateway, running) = match tauri::async_runtime::block_on(service::start(config)) {
        Ok(gateway) => {
            let line = format!(
                "网关已启动：http://{}:{}（库：{}）",
                service::HOST,
                gateway.port(),
                db_path.display()
            );
            println!("{line}");
            let _ = append_log(&logs, &line);
            (Some(gateway), true)
        }
        Err(e) => {
            let line = format!("网关启动失败：{e}");
            eprintln!("{line}");
            let _ = append_log(&logs, &line);
            (None, false)
        }
    };

    app.manage(Shell {
        gateway: Mutex::new(gateway),
        status_item: Mutex::new(None),
    });
    build_tray(app.handle(), running, port)?;
    Ok(())
}

/// 前端在**网关根本没在跑**时走的三条 IPC（`/api/*` 那时发不出去）。
///
/// 管理面自己那三条 `/api/gateway/*` 是网关在跑时的路径；这两条是同几个动作的无头版。
#[tauri::command]
pub async fn gateway_start(app: AppHandle) -> Result<serde_json::Value, String> {
    let port = current_port();
    start_gateway(&app, port).await?;
    Ok(serde_json::json!({ "started": true, "port": port }))
}

/// 探一个空闲端口（只预填，不写入；std 监听器够用，不必进 runtime）。
#[tauri::command]
pub fn gateway_probe_port() -> Result<serde_json::Value, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let suggested = listener.local_addr().map_err(|e| e.to_string())?.port();
    let current = current_port();
    let free = std::net::TcpListener::bind(("127.0.0.1", current)).is_ok();
    Ok(serde_json::json!({
        "currentPort": current,
        "currentPortFree": free,
        "suggestedPort": suggested,
        "note": "只是预填：确认之后才写入，不静默换端口",
    }))
}

/// 用户确认后的换端口：写库 + 同步已生效客户端 + 按新端口重启。
#[tauri::command]
pub async fn gateway_change_port(app: AppHandle, port: u16) -> Result<serde_json::Value, String> {
    let data_dir = store::default_data_dir().map_err(|e| e.to_string())?;
    let store = Store::open(&data_dir).map_err(|e| e.to_string())?;
    let env = ClientEnv::production(&data_dir).map_err(|e| e.to_string())?;
    let outcome = gateway_core::ingress::api::gateway::apply_port_change(
        &store,
        &data_dir,
        &env,
        current_port(),
        port,
    )
    .await
    .map_err(|e| e.to_string())?;
    stop_gateway(&app);
    start_gateway(&app, port).await?;
    Ok(serde_json::json!({
        "port": outcome.port,
        "previousPort": outcome.previous_port,
        "synced": outcome.synced,
        "failed": outcome.failed,
        "message": format!("已换到端口 {}，已同步 {} 个客户端", outcome.port, outcome.synced.len()),
    }))
}

fn build_tray(app: &AppHandle, running: bool, port: u16) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show-window", "显示主窗口", true, None::<&str>)?;
    // 占位（票 43 明确要求）：1P↔3P 快切要动客户端配置，归票 45 / 46 的写入引擎，先禁用。
    let toggle = MenuItem::with_id(
        app,
        "desktop-1p3p",
        "Claude Desktop 1P↔3P 快切（待配置写入引擎）",
        false,
        None::<&str>,
    )?;
    let status = MenuItem::with_id(
        app,
        "gateway-status",
        status_label(running, port),
        false,
        None::<&str>,
    )?;
    let open_logs = MenuItem::with_id(app, "open-logs", "打开日志目录", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &status, &open_logs, &quit])?;

    *app.state::<Shell>().status_item.lock().expect("状态项锁") = Some(status);

    TrayIconBuilder::with_id("main")
        .icon(tray_icon(running))
        .tooltip(format!("Claude Gateway · {}", status_label(running, port)))
        .menu(&menu)
        // 左键单击开窗口（spec §10），菜单留给右键。
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "show-window" => show_main_window(app),
        "open-logs" => open_logs_dir(),
        "quit" => quit(app),
        // "desktop-1p3p" 是禁用占位项，点不出事件；其余忽略。
        _ => {}
    }
}

/// 把主窗口提出来：从托盘唤起与第二次启动都走这里。
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn open_logs_dir() {
    let Ok(data_dir) = gateway_core::store::default_data_dir() else {
        return;
    };
    let dir = logs_dir(&data_dir);
    let _ = fs::create_dir_all(&dir);
    // 资源管理器开目录：Rust 侧一行搞定，不为它引 opener 插件（也不多一个 IPC 权限面）。
    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
}

fn quit(app: &AppHandle) {
    stop_gateway(app);
    app.exit(0);
}

/// 真正停网关：关监听、等 serve 收尾。托盘「退出」与进程退出都走这里，重复调用是空操作。
pub fn stop_gateway(app: &AppHandle) {
    let shell = app.state::<Shell>();
    let gateway = shell.gateway.lock().ok().and_then(|mut slot| slot.take());
    if let Some(gateway) = gateway {
        tauri::async_runtime::block_on(gateway.stop());
    }
}

/// 关窗 = 隐藏到托盘，网关继续跑；只有托盘「退出」才真停（票 18 §2）。
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cg-shell-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn port_defaults_to_8787_and_reads_config_json_otherwise() {
        let dir = scratch("port");

        // 没有配置文件 → 默认端口
        assert_eq!(load_port(&dir), 8787);

        fs::write(dir.join("config.json"), r#"{"port": 9001}"#).expect("写配置");
        assert_eq!(load_port(&dir), 9001);

        // 非法值不许悄悄变成 0 或别的端口，一律回默认
        for bad in [r#"{"port": 0}"#, r#"{"port": 70000}"#, r#"{}"#, "not json"] {
            fs::write(dir.join("config.json"), bad).expect("写坏配置");
            assert_eq!(load_port(&dir), 8787, "坏配置 {bad} 应当回默认端口");
        }
    }

    #[test]
    fn status_label_says_running_with_port_or_stopped() {
        let up = status_label(true, 8787);
        assert!(up.contains("运行中"), "{up}");
        assert!(up.contains("127.0.0.1:8787"), "{up}");

        let down = status_label(false, 8787);
        assert!(down.contains("未运行"), "{down}");
    }

    #[test]
    fn the_two_state_icons_are_different_pngs() {
        let running = tray_icon_bytes(true);
        let stopped = tray_icon_bytes(false);
        assert_ne!(running, stopped, "两种状态必须换图");
        for bytes in [running, stopped] {
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "托盘图标必须是 PNG");
        }
    }

    #[test]
    fn append_log_creates_the_directory_and_appends() {
        let dir = scratch("log");
        let logs = logs_dir(&dir);
        append_log(&logs, "第一行").expect("写日志");
        append_log(&logs, "第二行").expect("写日志");
        let text = fs::read_to_string(logs.join("gateway.log")).expect("读日志");
        assert!(text.contains("第一行") && text.contains("第二行"), "{text}");
    }

    #[test]
    fn tauri_tray_icon_decodes() {
        let image = Image::from_bytes(tray_icon_bytes(true)).expect("内嵌图标能被 tauri 解码");
        assert!(image.width() > 0 && image.height() > 0);
    }
}
