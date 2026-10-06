//! Maestro 桌面指挥台：Tauri 2 壳 + sidecar daemon + 事件桥。
//!
//! 架构（DEV_PLAN P1）：
//! - daemon 独立子进程（崩溃隔离；「关窗任务照跑」= 不 kill on drop）
//! - Rust 侧两条后台线程：事件桥（subscribe 全量重放+增量）+ 5s 兜底轮询
//! - 前端 React 只做渲染与本地状态 reconcile，权威数据以 RPC 快照为准

pub mod commands;
pub mod daemon;
pub mod events;
pub mod settings;
pub mod state;
pub mod tools;
pub mod tray;

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Alt+M 全局快捷键（C4-3）：X11/Windows 可注册；Wayland 无全局注册协议
/// → 注册失败不静默，emit 降级事件由设置页给系统自定义快捷键引导。
/// 结果记入 shortcut_error 供 shortcut_status command 查询。
fn register_alt_m(app: &tauri::AppHandle) {
    let res = app.global_shortcut().on_shortcut("Alt+M", |app, _s, event| {
        if event.state == ShortcutState::Pressed {
            crate::tray::toggle_window(app);
        }
    });
    match res {
        Ok(()) => {
            crate::commands::set_shortcut_ok();
        }
        Err(e) => {
            let msg = e.to_string();
            eprintln!("[maestro] Alt+M 注册失败: {msg}");
            crate::commands::set_shortcut_degraded(msg.clone());
            let _ = app.emit(
                "maestro://shortcut-degraded",
                serde_json::json!({ "message": msg }),
            );
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        // C4-2 单实例：二次启动唤起既有窗口（菜单/命令行重复点击是 Linux
        // 桌面常态；也是托盘缺失时窗口被藏后的找回路径）
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            crate::tray::show_window(app);
        }))
        // C4-3 全局快捷键载体（on_shortcut 在 setup 注册，失败可捕获）
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            let st = state::AppState::init();
            app.manage(st);
            // 引擎拉起失败只发事件不打断启动（UI 常驻、引擎可后起 —— 与 ui.rs 同原则）
            if let Err(e) = daemon::ensure_daemon(&handle) {
                let _ = handle.emit("maestro://daemon-error", serde_json::json!({ "message": e }));
            }
            events::spawn_bridge(handle.clone());
            events::spawn_poller(handle.clone());
            // C3-1：托盘常驻（关窗最小化 + 菜单 + 状态角标）；
            // C4-1：无托盘宿主（精简 WM）优雅降级 —— 不 panic，关窗分支见下
            tray::build(&handle);
            // C4-3：Alt+M（X11/Windows 注册；Wayland 失败 → 设置页引导）
            register_alt_m(&handle);
            Ok(())
        })
        // C3-2：关窗 = 最小化到托盘（daemon 独立常驻、ensure_daemon 支持接管，
        // 「关窗任务照跑」承诺）；真正退出走托盘菜单「退出」。
        // C4-1 降级：托盘不可用时不拦截关闭 —— 窗口销毁后应用退出，但
        // daemon 是独立 sidecar 进程、任务照跑，重开窗口自动接管现场
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if crate::tray::is_available() {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bridge_ready,
            commands::daemon_status,
            commands::restart_daemon,
            commands::shutdown_daemon,
            commands::create_task,
            commands::list_tasks,
            commands::get_task,
            commands::pause_task,
            commands::resume_task,
            commands::cancel_task,
            commands::steer_task,
            commands::get_ledger,
            commands::task_feedback,
            commands::get_ledger_summary,
            commands::list_workers,
            commands::list_inbox,
            commands::emergency_stop,
            commands::resume_all,
            commands::list_checkpoints,
            commands::rollback_checkpoint,
            commands::task_diff,
            commands::task_diff_revert,
            commands::get_settings,
            commands::save_settings,
            commands::probe_worker,
            commands::tray_status,
            commands::shortcut_status,
        ])
        .run(tauri::generate_context!())
        .expect("maestro desktop 启动失败");
}
