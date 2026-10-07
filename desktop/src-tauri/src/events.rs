//! 事件桥 + 兜底轮询（双数据源合流）：
//!
//! 1. bridge 线程：`subscribe(from_seq)` —— 首连 from_seq=1 触发 daemon 全量
//!    重放（hub 语义 from_seq>0 才重放），断线 1s 重连、last_seq+1 断点续订；
//!    每条 Envelope emit `maestro://event`
//! 2. poller 线程：5s 一次 server_status + task_list + inbox + workers 快照
//!    emit `maestro://sync` —— 前端以此为权威 reconcile（事件丢包自愈路径）

use crate::state::AppState;
use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::Emitter;
use tauri::Manager;

pub fn spawn_bridge(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let data_dir = state.data_dir.clone();
        let last_seq = state.last_seq.clone();
        // 等前端监听器就绪再首订阅：daemon 存活时 subscribe(from_seq=1) 的
        // 全量重放是瞬时爆发，早于 React listen() 注册的事件会全部丢失
        while !state.bridge_start.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        loop {
            let client = MaestroClient::new(&data_dir);
            if !client.is_daemon_alive() {
                let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": false }));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": true }));
            // 存活即清陈旧失败原因（外部拉起接管 / 恢复路径）——
            // sync 载荷的 engine_error 以此为准，避免常驻误导
            state.clear_engine_error();

            // 首连（last_seq=0）→ 1 = 全量重放；重连 → last_seq+1 续订
            let from = {
                let cur = last_seq.load(Ordering::SeqCst);
                if cur == 0 {
                    1
                } else {
                    cur + 1
                }
            };
            let app2 = app.clone();
            let seq_cell = last_seq.clone();
            // R59 起 subscribe 增 follow 形参（false=回放完即返）：桌面事件桥
            // 需要长连接实时推送 → true
            let _ = client.subscribe(from, true, move |env| {
                seq_cell.fetch_max(env.seq, Ordering::SeqCst);
                // C3-3：托盘随事件刷新急停态（重放亦覆盖 —— 重启后直接对齐）
                match &env.event {
                    maestro_protocol::events::Event::EmergencyStopped { .. } => {
                        crate::tray::set_emergency(&app2, true);
                    }
                    maestro_protocol::events::Event::Resumed {
                        via: maestro_protocol::types::ResumeVia::ResumeAll,
                        ..
                    } => {
                        crate::tray::set_emergency(&app2, false);
                    }
                    _ => {}
                }
                let _ = app2.emit("maestro://event", &env);
                true
            });
            // subscribe 返回 = 断流（daemon 重启/退出）→ 回循环顶部探活重连
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

pub fn spawn_poller(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let state = app.state::<AppState>();
        let client = MaestroClient::new(&state.data_dir);
        if !client.is_daemon_alive() {
            let _ = app.emit(
                "maestro://sync",
                serde_json::json!({
                    "status": null,
                    "tasks": [],
                    "inbox": [],
                    "workers": [],
                    // 引擎未在线的原因（5s 兜底通道；前端 StatusBar/设置页呈现）
                    "engine_error": state.engine_error(),
                }),
            );
        } else {
            let status = client
                .call("poll-status", Method::ServerStatus, serde_json::json!({}))
                .ok();
            let tasks = client
                .call("poll-tasks", Method::TaskList, serde_json::json!({}))
                .map(|v| v["tasks"].clone())
                .unwrap_or(serde_json::json!([]));
            let inbox = client
                .call("poll-inbox", Method::InboxList, serde_json::json!({}))
                .map(|v| v["items"].clone())
                .unwrap_or(serde_json::json!([]));
            // C3-3：未读数进托盘收件箱项（复用本就存在的 5s 轮询，不新增）
            crate::tray::set_unread(&app, inbox.as_array().map(|a| a.len()).unwrap_or(0));
            let workers = client
                .call("poll-workers", Method::WorkerList, serde_json::json!({}))
                .map(|v| v["workers"].clone())
                .unwrap_or(serde_json::json!([]));
            // 注意：不动 last_seq —— 它是事件桥的消费游标，poller 乱推进会
            // 导致 bridge 重连时跳过未消费事件（sync 快照已兜住 UI 状态）
            let _ = app.emit(
                "maestro://sync",
                serde_json::json!({
                    "status": status,
                    "tasks": tasks,
                    "inbox": inbox,
                    "workers": workers,
                    // 前端 managed 仅此通道更新 —— 否则设置页引擎永远误显
                    // 「外部启动（接管）」（daemon_status command 无人轮询）
                    "managed": state.is_managed(),
                    "engine_error": null,
                }),
            );
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}
