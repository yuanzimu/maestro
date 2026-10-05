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
use std::process::Command;
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

// ---- U5 结果卡：变更明细（task_diff）----

/// diff 钳制上限：结果卡 inline 展示，超长截断（完整内容看工作区 git）
const MAX_PATCH_BYTES: usize = 96 * 1024;

/// task_diff：baseline checkpoint → 当前工作区的变更（stat + patch）。
/// 结果卡「变更明细」入口。只读操作，桌面进程直接跑 git ——
/// 仓库布局与 daemon checkpoints.rs 一致（refs/maestro/cp/<task>/<seq>-<label>），
/// baseline 为任务入队时的 seq=1 锚点（缺失则取最早一条 checkpoint）。
#[tauri::command]
pub async fn task_diff(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let unavailable = |reason: String| Ok(json!({ "available": false, "reason": reason }));

    // workdir 从 daemon 拿（task_get 响应的权威字段），防前端传陈旧路径
    let task = call(&state, Method::TaskGet, json!({ "task": id }))?;
    let workdir = task["workdir"].as_str().unwrap_or_default().to_string();
    if workdir.is_empty() {
        return unavailable("任务无 workdir（无变更明细）".into());
    }

    // baseline：version 排序下最早一条（正常即 1-baseline；
    // for-each-ref 前缀匹配到 / 为止，t-1 不会误吞 t-10 的引用）
    let refs = match git_out(
        &workdir,
        &[
            "for-each-ref",
            &format!("refs/maestro/cp/{id}"),
            "--sort=version:refname",
            "--format=%(refname)",
        ],
    ) {
        Ok(r) => r,
        Err(e) => return unavailable(format!("git 不可用：{e}")),
    };
    let Some(baseline) = refs.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return unavailable("无 checkpoint（任务未产生快照）".into());
    };

    // stat：--numstat 每行 `ins\tdel\tpath`，二进制为 `-`；quotepath=false 防
    // CJK 文件名被转义成八进制
    let numstat = git_out(
        &workdir,
        &["-c", "core.quotepath=false", "diff", "--numstat", baseline],
    )?;
    let (files, ins, del) = parse_numstat(&numstat);

    // patch：baseline commit → 工作区（含未提交改动）；按字节钳制 + UTF-8 边界截断
    let patch_bytes = git_bytes(&workdir, &["diff", "--no-color", baseline])?;
    let truncated = patch_bytes.len() > MAX_PATCH_BYTES;
    let patch = clamp_utf8(&patch_bytes, MAX_PATCH_BYTES);

    Ok(json!({
        "available": true,
        "baseline": baseline,
        "files": files,
        "insertions": ins,
        "deletions": del,
        "patch": patch,
        "truncated": truncated,
    }))
}

fn git_out(workdir: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn git_bytes(workdir: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// numstat 解析：`ins\tdel\tpath` 每行一条；二进制文件增删为 `-`（只计文件数）
fn parse_numstat(s: &str) -> (u64, u64, u64) {
    let (mut files, mut ins, mut del) = (0, 0, 0);
    for line in s.lines() {
        if line.is_empty() {
            continue;
        }
        files += 1;
        let mut parts = line.splitn(3, '\t');
        ins += parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        del += parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    }
    (files, ins, del)
}

/// 字节钳制 + UTF-8 字符边界截断（回退越过续字节 0b10xxxxxx，防多字节字符切断）
fn clamp_utf8(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max;
    while end > 0 && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    let mut s = String::from_utf8_lossy(&bytes[..end]).into_owned();
    s.push_str("\n…（diff 过长已截断，完整变更请查看工作区 git）");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_parses_and_skips_binary() {
        let (f, i, d) = parse_numstat("12\t3\tsrc/a.rs\n-\t-\tlogo.png\n\n5\t0\tsrc/b.rs");
        assert_eq!((f, i, d), (3, 17, 3));
    }

    #[test]
    fn clamp_cuts_on_char_boundary() {
        // "中" 3 字节 / "文" 3 字节：max=5 应回退到 3 —— 保留完整一个字符
        let s = clamp_utf8("中文".as_bytes(), 5);
        assert_eq!(s.matches('中').count(), 1);
        assert!(s.contains("截断"));
        // 未超限原样返回
        assert_eq!(clamp_utf8(b"abc", 10), "abc");
    }
}
