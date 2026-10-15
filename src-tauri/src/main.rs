// Windows 发布版不弹控制台窗口；debug 保留，方便看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("Claude Gateway 外壳启动失败");
}
