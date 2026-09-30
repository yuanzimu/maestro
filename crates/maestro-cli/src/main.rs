//! maestro CLI：status/task/worker/inbox/stop/resume/doctor（0.13）

use clap::{Parser, Subcommand};
use maestro_client::MaestroClient;
use maestro_protocol::api::Method;

#[derive(Parser)]
#[command(name = "maestro", version, about = "Maestro — AI 任务指挥台 CLI")]
struct Cli {
    /// 数据目录（默认 /tmp/maestro 或 $MAESTRO_DATA_DIR）
    #[arg(long, global = true)]
    data_dir: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// daemon 状态
    Status,
    /// 创建任务
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Worker 列表
    Workers,
    /// 收件箱（blocked 任务）
    Inbox,
    /// 全局急停（FREEZE→SNAPSHOT，现场保留）
    Stop,
    /// 急停恢复
    Resume {
        /// steering 处理：flush 投递积压轻推 | hold 丢弃（发事件不静默）
        #[arg(long, default_value = "flush")]
        steering: String,
    },
    /// 事件流（--follow 持续）
    Events {
        /// 从哪个 seq 开始
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// 持续跟随
        #[arg(short, long)]
        follow: bool,
    },
    /// 自检：daemon/socket/版本/环境
    Doctor,
    /// 关停 daemon
    Shutdown,
}

#[derive(Subcommand)]
enum TaskAction {
    /// 创建并启动
    Create {
        /// 任务标题
        #[arg(short, long)]
        title: String,
        /// 任务 prompt
        #[arg(short, long)]
        prompt: String,
        /// 工作目录
        #[arg(short, long)]
        workdir: Option<String>,
    },
    /// 任务列表
    List,
    /// 任务详情
    Get { id: String },
    /// 暂停
    Pause { id: String },
    /// 恢复
    Resume { id: String },
    /// 取消
    Cancel { id: String },
    /// 轻推（下一轮生效）
    Steer { id: String, message: String },
    /// checkpoint 列表
    Checkpoints { id: String },
    /// 回滚到 checkpoint
    Rollback { id: String, to: String },
}

fn main() {
    let cli = Cli::parse();
    let client = match &cli.data_dir {
        Some(d) => MaestroClient::new(std::path::Path::new(d)),
        None => MaestroClient::connect_default(),
    };
    let code = match run(&client, cli.command) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("错误: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn run(client: &MaestroClient, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Status => {
            let v = client
                .call("status", Method::ServerStatus, serde_json::json!({}))
                .map_err(fmt_err)?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        Cmd::Task { action } => match action {
            TaskAction::Create {
                title,
                prompt,
                workdir,
            } => {
                let v = client
                    .call(
                        "task-create",
                        Method::TaskCreate,
                        serde_json::json!({ "title": title, "prompt": prompt, "workdir": workdir }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::List => {
                let v = client
                    .call("task-list", Method::TaskList, serde_json::json!({}))
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Get { id } => {
                let v = client
                    .call(
                        "task-get",
                        Method::TaskGet,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Pause { id } => {
                let v = client
                    .call(
                        "task-pause",
                        Method::TaskPause,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Resume { id } => {
                let v = client
                    .call(
                        "task-resume",
                        Method::TaskResume,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Cancel { id } => {
                let v = client
                    .call(
                        "task-cancel",
                        Method::TaskCancel,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Steer { id, message } => {
                let v = client
                    .call(
                        "task-steer",
                        Method::TaskSteer,
                        serde_json::json!({ "task": id, "message": message }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Checkpoints { id } => {
                let v = client
                    .call(
                        "cp-list",
                        Method::CheckpointList,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            TaskAction::Rollback { id, to } => {
                let v = client
                    .call(
                        "cp-rollback",
                        Method::CheckpointRollback,
                        serde_json::json!({ "task": id, "to": to }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
        },
        Cmd::Workers => {
            let v = client
                .call("workers", Method::WorkerList, serde_json::json!({}))
                .map_err(fmt_err)?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        Cmd::Inbox => {
            let v = client
                .call("inbox", Method::InboxList, serde_json::json!({}))
                .map_err(fmt_err)?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        Cmd::Stop => {
            let v = client
                .call(
                    "stop",
                    Method::ServerEmergencyStop,
                    serde_json::json!({ "reason": "cli" }),
                )
                .map_err(fmt_err)?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            println!("已冻结。现场保留。resume 可恢复。");
        }
        Cmd::Resume { steering } => {
            let mode = if steering == "hold" { "hold" } else { "flush" };
            let v = client
                .call(
                    "resume",
                    Method::ServerResumeAll,
                    serde_json::json!({ "steering": mode }),
                )
                .map_err(fmt_err)?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        Cmd::Events { from, follow } => {
            client
                .subscribe(from, |env| {
                    println!("{}", serde_json::to_string(&env).unwrap());
                    follow
                })
                .map_err(|e| e.to_string())?;
        }
        Cmd::Doctor => doctor(client),
        Cmd::Shutdown => {
            let _ = client.call("shutdown", Method::ServerShutdown, serde_json::json!({}));
            println!("已请求关停");
        }
    }
    Ok(())
}

/// doctor v0：daemon/socket/版本/git（0.13 核心命令）
fn doctor(client: &MaestroClient) {
    let mut fail = 0;
    println!("maestro doctor");
    println!("──────────────────────────────");

    // 1. daemon socket
    if client.is_daemon_alive() {
        println!("[ok]   daemon 在监听");
        // 2. 版本/状态
        match client.call("doc", Method::ServerStatus, serde_json::json!({})) {
            Ok(v) => {
                let ver = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
                let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0);
                println!("[ok]   daemon v{ver} (pid {pid})");
            }
            Err(e) => {
                println!("[fail] daemon 响应失败: {e}");
                fail += 1;
            }
        }
    } else {
        println!("[fail] daemon 未监听 —— 先运行: maestro-daemon");
        fail += 1;
    }

    // 3. git 可用（checkpoint 原语依赖）
    match std::process::Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => {
            println!("[ok]   {}", String::from_utf8_lossy(&o.stdout).trim());
        }
        _ => {
            println!("[fail] git 不可用（checkpoint 依赖）");
            fail += 1;
        }
    }

    // 4. 数据目录可写
    let data_dir = maestro_client::default_data_dir();
    match std::fs::create_dir_all(&data_dir) {
        Ok(()) => println!("[ok]   数据目录可写: {}", data_dir.display()),
        Err(e) => {
            println!("[fail] 数据目录不可写: {e}");
            fail += 1;
        }
    }

    println!("──────────────────────────────");
    if fail == 0 {
        println!("全部通过");
    } else {
        println!("{fail} 项失败");
    }
}

fn fmt_err(e: maestro_client::ClientError) -> String {
    e.to_string()
}
