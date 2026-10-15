// Windows 发布版不弹控制台窗口；debug 保留，方便看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod maint;
mod shell;

fn main() {
    // 无界面维护动作（票 62）：导出 / 导入 / 自启开关 / 卸载时还原客户端配置。
    // 认出来就干完退出，连 Tauri 都不建（卸载器 `ExecWait` 等的是一个会退出的进程）。
    // `--startup`（Run 键拉起来的那一条）不算维护动作：它照样起外壳，只是直接进托盘。
    if let Some(code) = maint::run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        std::process::exit(code);
    }

    tauri::Builder::default()
        // 单实例必须最先注册（插件文档要求）：第二次启动走到这个回调，
        // 把已有窗口提出来，自己不建窗口、不建托盘。不开 semver feature（票 18 §2）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::show_main_window(app)
        }))
        .setup(shell::setup)
        // 票 52：网关没在跑时（`/api/*` 发不出去）前端走这三条 IPC 启动 / 换端口。
        .invoke_handler(tauri::generate_handler![
            shell::gateway_start,
            shell::gateway_probe_port,
            shell::gateway_change_port
        ])
        // 关窗 = 隐藏到托盘，网关继续跑（票 18 §2）。
        .on_window_event(shell::on_window_event)
        .build(tauri::generate_context!())
        .expect("Claude Gateway 外壳启动失败")
        .run(|app, event| {
            // 只有真正退出才停网关，连它一起停。
            if let tauri::RunEvent::Exit = event {
                shell::stop_gateway(app);
            }
        });
}
