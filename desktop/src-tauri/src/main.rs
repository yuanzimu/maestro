// Windows 入口：转发 lib::run（Tauri 2 惯例，便于 dev/build/单测共享）

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // C4-2/CI 冒烟：--version 短路 —— 无显示服务器（CI/SSH headless）环境
    // 直接 run() 会在 GTK 初始化处失败；版本自检是安装验证的最小探针
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("Maestro 指挥台 v{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    maestro_desktop_lib::run()
}
