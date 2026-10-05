// Windows 入口：转发 lib::run（Tauri 2 惯例，便于 dev/build/单测共享）

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    maestro_desktop_lib::run()
}
