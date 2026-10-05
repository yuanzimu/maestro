//! daemon sidecar 生命周期：拉起 / 就绪等待 / 重启 / 关停。
//!
//! 关键点：
//! - worker 配置走 `MAESTRO_WORKER_PROGRAM/ARGS` 环境变量（daemon 的
//!   `--worker` 命令行形态曾有 off-by-one 丢参 bug；环境变量路径按空白
//!   切分语义稳定，且对含空格的 rounder 绝对路径经 ARGS 单值传入安全）
//! - spawn 不 kill on drop：关掉 GUI 窗口任务照跑（产品承诺），重开时
//!   `is_daemon_alive` 探活接管、不重复拉起
//! - 日志落 `%APPDATA%\maestro-desktop\logs\daemon.log`（排障）

use crate::settings::WorkerMode;
use crate::state::AppState;
use crate::tools;
use maestro_client::MaestroClient;
use std::process::Command;
use std::time::Duration;
use tauri::Emitter;
use tauri::Manager;

/// 组装 worker 配置（演示 / claude / 自定义），返回 (PROGRAM, ARGS)
fn worker_config(state: &AppState) -> Result<(String, String), String> {
    let rounder = tools::resolve_tool("maestro-rounder")
        .ok_or("未找到 maestro-rounder（多轮驱动器）—— 安装不完整")?;
    let rounder = rounder.to_string_lossy().into_owned();

    let s = state.settings.lock().unwrap().clone();
    // 注意：MAESTRO_WORKER_ARGS 由 daemon 按空白切分后作为 argv 直传
    // （不经 shell），路径**不能加引号** —— 引号会成为字面字符导致
    // CreateProcess os error 123。安装目录无空格，此约定安全
    match s.worker_mode {
        WorkerMode::Demo => {
            let mock = tools::resolve_tool("mock-cli")
                .ok_or("未找到演示 worker（mock-cli）—— 安装不完整")?;
            Ok((rounder, format!("-- {}", mock.to_string_lossy())))
        }
        WorkerMode::Claude => Ok((rounder, "-- claude".to_string())),
        WorkerMode::Custom => {
            if s.custom_program.trim().is_empty() {
                return Err("自定义 worker 未配置程序路径（设置 → 自定义命令）".into());
            }
            let mut args = format!("-- {}", s.custom_program.trim());
            let extra = s.custom_args.trim();
            if !extra.is_empty() {
                args.push(' ');
                args.push_str(extra);
            }
            Ok((rounder, args))
        }
    }
}

/// 确保引擎在线：已在线则接管；否则拉起 sidecar 并等待就绪
pub fn ensure_daemon(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let client = MaestroClient::new(&state.data_dir);
    if client.is_daemon_alive() {
        state.managed.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": true }));
        return Ok(());
    }

    let daemon_exe = tools::resolve_tool("maestro-daemon")
        .ok_or("未找到 maestro-daemon 引擎 —— 安装不完整")?;
    let (worker_program, worker_args) = worker_config(&state)?;

    // 日志文件（append）
    let log_dir = crate::state::app_root().join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| format!("建日志目录失败: {e}"))?;
    let log_path = log_dir.join("daemon.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打开日志失败: {e}"))?;

    let mut cmd = Command::new(&daemon_exe);
    cmd.arg("--data-dir").arg(&state.data_dir)
        .env("MAESTRO_WORKER_PROGRAM", &worker_program)
        .env("MAESTRO_WORKER_ARGS", &worker_args)
        .env("MAESTRO_ROUND_GAP_MS", "1200") // 演示节奏（rounder 轮间隔）
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);

    let child = cmd
        .spawn()
        .map_err(|e| format!("引擎启动失败（{}）: {e}", daemon_exe.display()))?;
    *state.child.lock().unwrap() = Some(child);
    state
        .managed
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // 就绪等待：端口文件 bind 后 is_daemon_alive 才为真（200ms 轮询，15s 超时）
    let mut waited = 0u64;
    while waited < 15_000 {
        if client.is_daemon_alive() {
            let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": true }));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
        waited += 200;
    }

    // 超时：收集诊断信息（子进程是否已退 + 日志尾部）
    state.kill_child();
    let tail = tools::log_tail(2048);
    let msg = format!(
        "引擎 15s 内未就绪。{}",
        if tail.is_empty() { String::new() } else { format!("日志尾部：\n{tail}") },
    );
    let _ = app.emit("maestro://daemon-error", serde_json::json!({ "message": msg }));
    Err(msg)
}

/// 关停（RPC 优雅关停 → 超时强杀托管子进程）
pub fn shutdown(app: &tauri::AppHandle, grace_ms: u64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let client = MaestroClient::new(&state.data_dir);
    if client.is_daemon_alive() {
        let _ = client.call("shutdown", maestro_protocol::api::Method::ServerShutdown, serde_json::json!({}));
    }
    if !state.wait_daemon_down(grace_ms) {
        state.kill_child();
    } else {
        // 优雅退出：回收子进程句柄
        if let Some(mut c) = state.child.lock().unwrap().take() {
            let _ = c.wait();
        }
        state.managed.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    let _ = app.emit("maestro://daemon", serde_json::json!({ "alive": false }));
    Ok(())
}

/// 重启 = 关停 + 重新拉起
pub fn restart(app: &tauri::AppHandle) -> Result<(), String> {
    shutdown(app, 5_000)?;
    std::thread::sleep(Duration::from_millis(300));
    ensure_daemon(app)
}
