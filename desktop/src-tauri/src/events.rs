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
        loop {
            let client = MaestroClient::new(&data_dir);
            if !client.is_daemon_alive() {
                let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": false }));
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": true }));

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
                }),
            );
        } else {
            let status = client
                .call("poll-status", Method::ServerStatus, serde_json::json!({}))
                .ok();
            let tasks = client
                .call("poll-tasks", Method::TaskList, serde_json::json!({}))
                .and_then(|v| Ok(v["tasks"].clone()))
                .unwrap_or(serde_json::json!([]));
            let inbox = client
                .call("poll-inbox", Method::InboxList, serde_json::json!({}))
                .and_then(|v| Ok(v["items"].clone()))
                .unwrap_or(serde_json::json!([]));
            let workers = client
                .call("poll-workers", Method::WorkerList, serde_json::json!({}))
                .and_then(|v| Ok(v["workers"].clone()))
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
                }),
            );
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}
