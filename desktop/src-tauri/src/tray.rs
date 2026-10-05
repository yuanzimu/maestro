//! 系统托盘（Sprint C C3）：关窗最小化到托盘 + 菜单动作 + 状态刷新。
//!
//! 设计对齐 macOS main.swift 的 NSStatusItem（B3 已落地的行为基线）：
//! - 左键单击/菜单「显示窗口」恢复窗口（关窗 = 隐藏，任务照跑）
//! - 菜单：显示窗口 / 新建任务 / 收件箱(N) / 急停↔恢复全部 / 设置 / 退出
//! - 状态随事件刷新（C3-3）：急停态改菜单文本与 tooltip；未读数进收件箱
//!   项文本（Windows 无原生角标，N 嵌入文本是等价呈现；macOS 可后续接
//!   set_badge_count）
//!
//! 线程模型：menu/tray 的 muda 底层在 Windows 要求 UI 线程操作 —— 所有
//! 菜单变更经 run_on_main_thread 代理；事件桥/poller 线程安全调用。

use std::sync::{Mutex, OnceLock};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

const ID_SHOW: &str = "tray-show";
const ID_NEW: &str = "tray-new-task";
const ID_INBOX: &str = "tray-inbox";
const ID_EMERGENCY: &str = "tray-emergency";
const ID_SETTINGS: &str = "tray-settings";
const ID_QUIT: &str = "tray-quit";

/// 动态文本项的句柄（run_on_main_thread 里变更用；MenuItem: Clone）
struct TrayItems {
    inbox: MenuItem<tauri::Wry>,
    emergency: MenuItem<tauri::Wry>,
}

fn items() -> &'static Mutex<Option<TrayItems>> {
    static T: OnceLock<Mutex<Option<TrayItems>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}

/// 建托盘（setup 阶段调用一次）
pub fn build(app: &AppHandle) {
    let show = MenuItem::with_id(app, ID_SHOW, "显示窗口", true, None::<&str>)
        .expect("menu item show");
    let new_task = MenuItem::with_id(app, ID_NEW, "新建任务", true, None::<&str>)
        .expect("menu item new-task");
    let inbox = MenuItem::with_id(app, ID_INBOX, "收件箱", true, None::<&str>)
        .expect("menu item inbox");
    let emergency = MenuItem::with_id(app, ID_EMERGENCY, "⛔ 急停", true, None::<&str>)
        .expect("menu item emergency");
    let settings = MenuItem::with_id(app, ID_SETTINGS, "设置", true, None::<&str>)
        .expect("menu item settings");
    let quit = MenuItem::with_id(app, ID_QUIT, "退出", true, None::<&str>)
        .expect("menu item quit");
    let menu = Menu::with_items(
        app,
        &[&show, &new_task, &inbox, &emergency, &settings, &quit],
    )
    .expect("tray menu");

    let tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        // Windows/Linux 惯例：左键恢复窗口，右键开菜单（macOS 由 AppKit 接管）
        .show_menu_on_left_click(false)
        .tooltip("Maestro 指挥台")
        .icon(app.default_window_icon().expect("default icon").clone())
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_SHOW => show_window(app),
            ID_QUIT => app.exit(0),
            // 其余动作交前端：复用既有确认流（急停 confirm / 恢复 flush 语义）
            // 与导航 dispatch，托盘不另起一套业务逻辑
            action => {
                let _ = app.emit(
                    "maestro://tray",
                    serde_json::json!({ "action": action }),
                );
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        })
        .build(app)
        .expect("tray build");

    let _ = tray; // 句柄由 tauri 管理（with_id 后可 app.tray_by_id("main")）
    *items().lock().unwrap() = Some(TrayItems { inbox, emergency });
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 急停态切换（C3-3）：菜单「急停」↔「恢复全部」+ tooltip 提示。
/// 事件桥线程调用；muda 要求 UI 线程 —— run_on_main_thread 代理。
pub fn set_emergency(app: &AppHandle, frozen: bool) {
    let inner = app.clone();
    let _ = app.run_on_main_thread(move || {
        let guard = items().lock().unwrap();
        let Some(t) = guard.as_ref() else { return };
        let _ = t.emergency.set_text(if frozen {
            "▶ 恢复全部"
        } else {
            "⛔ 急停"
        });
        let _ = inner
            .tray_by_id("main")
            .map(|tray| tray.set_tooltip(Some(if frozen {
                "Maestro 指挥台 —— 全局急停中"
            } else {
                "Maestro 指挥台"
            })));
    });
}

/// 未读（待审批/待决策）计数（C3-3）：收件箱菜单项带 N。poller 线程调用。
pub fn set_unread(app: &AppHandle, n: usize) {
    let app = app.clone();
    let _ = app.run_on_main_thread(move || {
        let guard = items().lock().unwrap();
        let Some(t) = guard.as_ref() else { return };
        let text = if n > 0 {
            format!("收件箱（{n} 项待处理）")
        } else {
            "收件箱".to_string()
        };
        let _ = t.inbox.set_text(text);
    });
}
