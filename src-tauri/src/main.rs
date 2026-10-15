// Windows 发布版不弹控制台窗口；debug 保留，方便看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;

fn main() {
    tauri::Builder::default()
        // 单实例必须最先注册（插件文档要求）：第二次启动走到这个回调，
        // 把已有窗口提出来，自己不建窗口、不建托盘。不开 semver feature（票 18 §2）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::show_main_window(app)
        }))
        .setup(shell::setup)
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
