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

use tauri::{Emitter, Manager};

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let st = state::AppState::init();
            app.manage(st);
            // 引擎拉起失败只发事件不打断启动（UI 常驻、引擎可后起 —— 与 ui.rs 同原则）
            if let Err(e) = daemon::ensure_daemon(&handle) {
                let _ = handle.emit("maestro://daemon-error", serde_json::json!({ "message": e }));
            }
            events::spawn_bridge(handle.clone());
            events::spawn_poller(handle);
            Ok(())
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
            commands::list_workers,
            commands::list_inbox,
            commands::emergency_stop,
            commands::resume_all,
            commands::list_checkpoints,
            commands::rollback_checkpoint,
            commands::get_settings,
            commands::save_settings,
            commands::probe_worker,
        ])
        .run(tauri::generate_context!())
        .expect("maestro desktop 启动失败");
}
