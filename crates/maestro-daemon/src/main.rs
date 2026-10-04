//! maestro-daemon 入口：双 socket 服务 + Core 主循环。
//!
//! worker 配置（R54 可用性修复）：`--worker <prog> [-- <args...>]` 命令行
//! 形态（优先）或 MAESTRO_WORKER_PROGRAM/MAESTRO_WORKER_ARGS 环境变量。
//! 此前只认环境变量，命令行传入被静默忽略 → 回退 /bin/sh echo →
//! 假完成 3 振出局（真用户首跑即踩，浏览器验证时发现）。

use clap::Parser;
use maestro_client::transport;
use maestro_daemon::core::{Core, CoreConfig};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "maestro-daemon", about = "Maestro 任务守护进程")]
struct Args {
    /// Worker 程序（多轮驱动用 maestro-rounder）
    #[arg(long)]
    worker: Option<String>,
    /// 数据目录（默认 <系统临时目录>/maestro 或 $MAESTRO_DATA_DIR）
    #[arg(long)]
    data_dir: Option<String>,
    /// `--` 之后是 worker 参数（透传，如 `-- /path/to/inner-cli`）
    #[arg(last = true)]
    worker_args: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    // 日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Args::parse();
    let data_dir = cli
        .data_dir
        .map(PathBuf::from)
        .or_else(|| std::env::var("MAESTRO_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(transport::default_data_dir);

    // worker 配置：命令行优先（--worker X -- Y Z），其次环境变量，
    // 默认占位（v0 echo worker）。⚠️ rounder 约定 argv 含 "--" 分隔
    // （-- 之后的才是内层 CLI）—— clap 的 last 参数吃掉了 "--"
    // （实测 worker_args 不含它），这里回补且不得丢首元素（内层 CLI）
    let (worker_program, worker_args) = match &cli.worker {
        Some(prog) => {
            let mut args = vec!["--".to_string()];
            args.extend(cli.worker_args.iter().cloned());
            (prog.clone(), args)
        }
        None => {
            // 默认 echo worker（占位演示用）：平台各自的 shell
            #[cfg(unix)]
            let (dprog, dargs): (&str, Vec<&str>) = ("/bin/sh", vec!["-c", "echo maestro-worker-v0"]);
            #[cfg(windows)]
            let (dprog, dargs): (&str, Vec<&str>) = ("cmd", vec!["/C", "echo maestro-worker-v0"]);
            (
                std::env::var("MAESTRO_WORKER_PROGRAM").unwrap_or_else(|_| dprog.into()),
                std::env::var("MAESTRO_WORKER_ARGS")
                    .map(|s| s.split_whitespace().map(String::from).collect())
                    .unwrap_or_else(|_| dargs.into_iter().map(String::from).collect()),
            )
        }
    };

    // 恢复 or 全新启动：数据目录有事件库则恢复。
    // socket_path 先占位 —— 真实端点 bind 后才确定（Windows TCP 端口），见下方回填
    let cfg = CoreConfig {
        data_dir: data_dir.clone(),
        workdir: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
        worker_program,
        worker_args,
        socket_path: String::new(),
        max_parallel_workers: std::env::var("MAESTRO_MAX_WORKERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(maestro_daemon::core::DEFAULT_MAX_PARALLEL_WORKERS),
        default_model: std::env::var("MAESTRO_MODEL")
            .unwrap_or_else(|_| maestro_daemon::core::DEFAULT_MODEL.into()),
        // Worker 环境白名单透传（R37 上下文轮转阈值 / R42 轮数预算等）
        worker_env: [
            "MAESTRO_CONTEXT_LIMIT",
            "MAESTRO_CLI_DIALECT",
            "MAESTRO_MAX_ROUNDS",
            "MAESTRO_ROUND_GAP_MS",
        ]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v)))
        .collect(),
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
    // ⚠️ run() 持锁到底：sender/hub/store 必须在起 run 线程前全部取出
    let (core_tx, hub, store) = {
        let g = core.lock().unwrap();
        (g.sender(), g.hub_handle(), g.event_store_handle())
    };
    let (_guard, api_addr) = maestro_daemon::server::serve(hub, core_tx, &data_dir, store)?;
    // 回填真实端点（Windows TCP 端口 bind 后才确定；Unix 下即 socket 路径）
    core.lock().unwrap().cfg.socket_path = api_addr.to_env_value();

    tracing::info!("maestro daemon 就绪: {}", api_addr.to_env_value());

    // 主循环（Core 线程内联在主线程 —— 单线程权威）
    core.lock().unwrap().run();
    tracing::info!("daemon 关停");
    Ok(())
}
