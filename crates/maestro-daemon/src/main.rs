//! maestro-daemon 入口：setsid 脱离终端 + 双 socket 服务 + Core 主循环。

use maestro_daemon::core::{Core, CoreConfig};
use maestro_daemon::server::{self, IpcPaths};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() -> anyhow::Result<()> {
    // 日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = std::env::var("MAESTRO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/maestro"));

    // 恢复 or 全新启动：数据目录有事件库则恢复
    let cfg = CoreConfig {
        data_dir: data_dir.clone(),
        workdir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
        worker_program: std::env::var("MAESTRO_WORKER_PROGRAM")
            .unwrap_or_else(|_| "/bin/sh".into()),
        worker_args: std::env::var("MAESTRO_WORKER_ARGS")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_else(|_| vec!["-c".into(), "echo maestro-worker-v0".into()]),
        socket_path: data_dir.join("maestro.api.sock").display().to_string(),
    };

    let clock = Arc::new(maestro_protocol::SystemClock);
    let (core, report) = Core::recover(cfg, clock);
    if !report.killed.is_empty()
        || !report.cleaned_files.is_empty()
        || !report.pid_reused.is_empty()
    {
        tracing::info!(
            "孤儿清理: killed={} cleaned={} pid_reused={}",
            report.killed.len(),
            report.cleaned_files.len(),
            report.pid_reused.len()
        );
    }

    let core = Arc::new(Mutex::new(core));
    let paths = IpcPaths::new(&data_dir);
    // ⚠️ run() 持锁到底：sender/hub/store 必须在起 run 线程前全部取出
    let (core_tx, hub, store) = {
        let g = core.lock().unwrap();
        (g.sender(), g.hub_handle(), g.event_store_handle())
    };
    let _handles = server::serve(hub, core_tx, &paths, store)?;

    tracing::info!("maestro daemon 就绪: {}", paths.api_sock.display());

    // 主循环（Core 线程内联在主线程 —— 单线程权威）
    core.lock().unwrap().run();
    tracing::info!("daemon 关停");
    Ok(())
}
