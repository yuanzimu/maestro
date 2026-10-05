//! tauri commands：RPC 薄透传（daemon 的响应 JSON 原样给前端，
//! TS 侧 types.ts 与 wire 形状对齐）+ 引擎/设置的本机操作。
//!
//! 约定：错误统一 `Err(String)`，格式 "RPC -409: xxx" / "连接失败: xxx"。

use crate::daemon;
use crate::settings::{self, Settings};
use crate::state::AppState;
use crate::tools;
use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

fn client(state: &AppState) -> MaestroClient {
    MaestroClient::new(&state.data_dir)
}

fn rpc_err(e: maestro_client::ClientError) -> String {
    match e {
        maestro_client::ClientError::Rpc { code, message } => format!("RPC {code}: {message}"),
        other => other.to_string(),
    }
}

fn call(state: &AppState, method: Method, params: Value) -> Result<Value, String> {
    client(state)
        .call("desktop", method, params)
        .map_err(rpc_err)
}

// ---- 引擎 ----

/// 前端监听器已注册（store 的 listen() 全部就位）→ 放行事件桥首订阅
#[tauri::command]
pub async fn bridge_ready(state: State<'_, AppState>) -> Result<(), String> {
    state
        .bridge_start
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn daemon_status(state: State<'_, AppState>) -> Result<Value, String> {
    let c = client(&state);
    if !c.is_daemon_alive() {
        return Ok(json!({
            "running": false,
            "managed": state.is_managed(),
            "version": "",
            "pid": 0,
            "uptime_secs": 0,
            "event_seq": 0,
        }));
    }
    let status = c
        .call("status", Method::ServerStatus, json!({}))
        .map_err(rpc_err)?;
    Ok(json!({
        "running": true,
        "managed": state.is_managed(),
        "version": status["version"],
        "pid": status["pid"],
        "uptime_secs": status["uptime_secs"],
        "event_seq": status["event_seq"],
    }))
}

#[tauri::command]
pub async fn restart_daemon(app: AppHandle) -> Result<(), String> {
    daemon::restart(&app)
}

#[tauri::command]
pub async fn shutdown_daemon(app: AppHandle) -> Result<(), String> {
    daemon::shutdown(&app, 5_000)
}

// ---- 任务 ----

#[tauri::command]
pub async fn create_task(
    state: State<'_, AppState>,
    title: String,
    prompt: String,
    workdir: Option<String>,
) -> Result<Value, String> {
    let params = json!({ "title": title, "prompt": prompt, "workdir": workdir });
    call(&state, Method::TaskCreate, params)
}

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::TaskList, json!({}))
}

#[tauri::command]
pub async fn get_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskGet, json!({ "task": id }))
}

#[tauri::command]
pub async fn pause_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskPause, json!({ "task": id }))
}

#[tauri::command]
pub async fn resume_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskResume, json!({ "task": id }))
}

#[tauri::command]
pub async fn cancel_task(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskCancel, json!({ "task": id }))
}

#[tauri::command]
pub async fn steer_task(
    state: State<'_, AppState>,
    id: String,
    message: String,
) -> Result<Value, String> {
    call(&state, Method::TaskSteer, json!({ "task": id, "message": message }))
}

#[tauri::command]
pub async fn get_ledger(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    call(&state, Method::TaskLedger, json!({ "task": id }))
}

#[tauri::command]
pub async fn list_workers(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::WorkerList, json!({}))
}

#[tauri::command]
pub async fn list_inbox(state: State<'_, AppState>) -> Result<Value, String> {
    call(&state, Method::InboxList, json!({}))
}

// ---- 急停 / 恢复 ----

#[tauri::command]
pub async fn emergency_stop(state: State<'_, AppState>, reason: Option<String>) -> Result<Value, String> {
    call(&state, Method::ServerEmergencyStop, json!({ "reason": reason.unwrap_or_else(|| "desktop-ui".into()) }))
}

#[tauri::command]
pub async fn resume_all(state: State<'_, AppState>, steering: String) -> Result<Value, String> {
    let s = if steering == "hold" { "hold" } else { "flush" };
    call(&state, Method::ServerResumeAll, json!({ "steering": s }))
}

// ---- checkpoint ----

#[tauri::command]
pub async fn list_checkpoints(state: State<'_, AppState>, task: String) -> Result<Value, String> {
    call(&state, Method::CheckpointList, json!({ "task": task }))
}

#[tauri::command]
pub async fn rollback_checkpoint(
    state: State<'_, AppState>,
    task: String,
    to: String,
) -> Result<Value, String> {
    call(&state, Method::CheckpointRollback, json!({ "task": task, "to": to }))
}

// ---- 设置 / 探测 ----

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    serde_json::to_value(s).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    settings: Value,
) -> Result<(), String> {
    let s: Settings =
        serde_json::from_value(settings).map_err(|e| format!("设置格式错误: {e}"))?;
    settings::save(&s).map_err(|e| format!("保存失败: {e}"))?;
    let state = app.state::<AppState>();
    *state.settings.lock().unwrap() = s;
    Ok(())
}

#[tauri::command]
pub async fn probe_worker(state: State<'_, AppState>) -> Result<Value, String> {
    let s = state.settings.lock().unwrap().clone();
    let p = |name: &str| {
        tools::resolve_tool(name)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    Ok(json!({
        "mode": serde_json::to_value(s.worker_mode).unwrap_or(json!("demo")),
        "claude_found": tools::detect_claude(),
        "daemon_path": p("maestro-daemon"),
        "rounder_path": p("maestro-rounder"),
        "mock_path": p("mock-cli"),
        "data_dir": state.data_dir.to_string_lossy(),
    }))
}
