//! 系统托盘（Sprint C C3 + C4-1 优雅降级）：关窗最小化到托盘 + 菜单动作 + 状态刷新。
//!
//! 设计对齐 macOS main.swift 的 NSStatusItem（B3 已落地的行为基线）：
//! - 左键单击/菜单「显示窗口」恢复窗口（关窗 = 隐藏，任务照跑）
//! - 菜单：显示窗口 / 新建任务 / 收件箱(N) / 急停↔恢复全部 / 设置 / 退出
//! - 状态随事件刷新（C3-3）：急停态改菜单文本与 tooltip；未读数进收件箱
//!   项文本（Windows 无原生角标，N 嵌入文本是等价呈现；macOS 可后续接
//!   set_badge_count）
//!
//! C4-1 优雅降级（Linux 精简 WM / 无 SNI 宿主）：
//! - build 全链路 Result 化，任一步失败只记日志 + emit `maestro://tray-degraded`
//!   事件，**不 panic**（原 `.expect` 在无 DBus 会话/无 StatusNotifierWatcher 的
//!   环境 = 启动即崩，正是任务点名要消除的报错路径）
//! - `is_available()` 供关窗逻辑分支：托盘不可用时窗口关闭走默认退出
//!   （daemon 是独立 sidecar 进程，「任务照跑」承诺不破；重开 GUI 探活接管）
//! - 找回路径：单实例插件（C4-2）—— 托盘缺失导致窗口被隐藏的场景，
//!   再次启动可唤起已有实例窗口
//!
//! 线程模型：menu/tray 的 muda 底层在 Windows 要求 UI 线程操作 —— 所有
//! 菜单变更经 run_on_main_thread 代理；事件桥/poller 线程安全调用。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
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

/// 托盘是否可用（C4-1 降级标志；关窗行为据此分支）
fn available() -> &'static AtomicBool {
    static A: OnceLock<AtomicBool> = OnceLock::new();
    A.get_or_init(|| AtomicBool::new(false))
}

pub fn is_available() -> bool {
    available().load(Ordering::SeqCst)
}

/// 建托盘（setup 阶段调用一次）。
/// 返回是否成功 —— 失败不 panic，emit 降级事件由前端提示（C4-1）。
pub fn build(app: &AppHandle) -> bool {
    match build_inner(app) {
        Ok(()) => {
            available().store(true, Ordering::SeqCst);
            true
        }
        Err(e) => {
            let msg = format!("系统托盘不可用（将无法最小化到托盘）: {e}");
            eprintln!("[maestro] {msg}");
            let _ = app.emit(
                "maestro://tray-degraded",
                serde_json::json!({ "message": msg }),
            );
            false
        }
    }
}

fn build_inner(app: &AppHandle) -> Result<(), String> {
    let mk = |id: &'static str, text: &str| {
        MenuItem::with_id(app, id, text, true, None::<&str>)
            .map_err(|e| format!("菜单项 {id} 创建失败: {e}"))
    };
    let show = mk(ID_SHOW, "显示窗口")?;
    let new_task = mk(ID_NEW, "新建任务")?;
    let inbox = mk(ID_INBOX, "收件箱")?;
    let emergency = mk(ID_EMERGENCY, "⛔ 急停")?;
    let settings = mk(ID_SETTINGS, "设置")?;
    let quit = mk(ID_QUIT, "退出")?;
    let menu = Menu::with_items(
        app,
        &[&show, &new_task, &inbox, &emergency, &settings, &quit],
    )
    .map_err(|e| format!("托盘菜单创建失败: {e}"))?;

    let icon = app
        .default_window_icon()
        .ok_or_else(|| "缺默认图标（bundle icons 配置）".to_string())?
        .clone();

    let _tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        // Windows/Linux 惯例：左键恢复窗口，右键开菜单（macOS 由 AppKit 接管）
        .show_menu_on_left_click(false)
        .tooltip("Maestro 指挥台")
        .icon(icon)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_SHOW => show_window(app),
            ID_QUIT => app.exit(0),
            // 其余动作交前端：复用既有确认流（急停 confirm / 恢复 flush 语义）
            // 与导航 dispatch，托盘不另起一套业务逻辑
            action => {
                let _ = app.emit("maestro://tray", serde_json::json!({ "action": action }));
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("托盘注册失败（无托盘宿主？）: {e}"))?;
    // 句柄由 tauri 管理（with_id 后可 app.tray_by_id("main")）

    *items().lock().unwrap() = Some(TrayItems { inbox, emergency });
    Ok(())
}

pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Alt+M 全局快捷键（C4-3）：可见→藏（驻留后台），隐藏/最小化→唤起。
/// 与托盘左键共用恢复路径；无托盘环境这是主要唤回手段之一（另一个是单实例）。
/// 最小化先判（C4-3 复检修）：X11 iconic 窗口 is_visible 可能为 true，
/// 不先判会把「想唤起最小化窗口」错误执行成 hide。
pub fn toggle_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_minimized().unwrap_or(false) {
            show_window(app);
            return;
        }
        match w.is_visible() {
            Ok(true) => {
                let _ = w.hide();
            }
            _ => show_window(app),
        }
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
        let _ = inner.tray_by_id("main").map(|tray| {
            tray.set_tooltip(Some(if frozen {
                "Maestro 指挥台 —— 全局急停中"
            } else {
                "Maestro 指挥台"
            }))
        });
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
