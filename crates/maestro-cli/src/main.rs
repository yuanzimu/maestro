//! maestro CLI：status/task/worker/inbox/stop/resume/doctor（0.13 + R19 体验）

use clap::{Parser, Subcommand};
use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use serde_json::Value;

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
    /// 收件箱（blocked 任务 + 行动建议）
    Inbox,
    /// 全局急停（FREEZE→SNAPSHOT，现场保留）
    Stop,
    /// 急停恢复
    Resume {
        /// steering 处理：flush 投递积压轻推 | hold 丢弃（发事件不静默）
        #[arg(long, default_value = "flush")]
        steering: String,
    },
    /// 事件流（--follow 持续；--human 可读渲染）
    Events {
        /// 从哪个 seq 开始
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// 持续跟随
        #[arg(short, long)]
        follow: bool,
        /// 人类可读渲染（默认输出原始 JSON，面向管道/jq）
        #[arg(long)]
        human: bool,
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
    List {
        /// 输出原始 JSON
        #[arg(long)]
        json: bool,
    },
    /// 任务详情
    Get { id: String },
    /// 暂停
    Pause { id: String },
    /// 恢复（blocked = 用户确认重试并重新入队）
    Resume { id: String },
    /// 取消
    Cancel { id: String },
    /// 轻推（下一轮生效）
    Steer { id: String, message: String },
    /// 账本：轮数/耗时/成本汇总
    Ledger { id: String },
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
                println!("{}", fmt_create(&v));
            }
            TaskAction::List { json } => {
                let v = client
                    .call("task-list", Method::TaskList, serde_json::json!({}))
                    .map_err(fmt_err)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                } else {
                    print!("{}", fmt_task_list(&v));
                }
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
                if v.get("requeued").and_then(|x| x.as_bool()).unwrap_or(false) {
                    println!("已重新入队：{id}（验收计数已清零）");
                } else {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap());
                }
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
            TaskAction::Ledger { id } => {
                let v = client
                    .call(
                        "task-ledger",
                        Method::TaskLedger,
                        serde_json::json!({ "task": id }),
                    )
                    .map_err(fmt_err)?;
                println!("{}", fmt_ledger(&v));
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
            print!("{}", fmt_inbox(&v));
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
        Cmd::Events {
            from,
            follow,
            human,
        } => {
            client
                .subscribe(from, |env| {
                    if human {
                        println!("{}", fmt_event_human(&env));
                    } else {
                        println!("{}", serde_json::to_string(&env).unwrap());
                    }
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

// ---------------------------------------------------------------------------
// 展示层（纯函数，可单测）
// ---------------------------------------------------------------------------

/// 事件 → 人类可读一行（`maestro events --human`；B1 叙事的 CLI 前菜）。
/// 未知事件 fallback 原始 JSON（前向兼容：daemon 新增事件不破渲染）
fn fmt_event_human(env: &maestro_protocol::events::Envelope) -> String {
    use maestro_protocol::events::Event;
    let ts = env.ts / 1000; // 秒
    let reason_str = |r: &maestro_protocol::types::SuspendReason| {
        serde_json::to_value(r)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{r:?}"))
    };
    let body = match &env.event {
        Event::TaskCreated { task, .. } => format!("{} 「{}」 建立", task.id, task.title),
        Event::WorkerSpawned { task, worker, .. } => format!("{task} → worker {worker}"),
        Event::RoundProgress {
            task,
            round,
            tools_used,
            summary,
            ..
        } => format!("{task} 轮 {round} · [{}] {summary}", tools_used.join(",")),
        Event::LedgerEntry { task, usage, .. } => format!(
            "{task} 入账 {} in / {} out",
            usage.input_tokens, usage.output_tokens
        ),
        Event::CostDrift {
            task,
            ledger_cents,
            cli_cents,
            ..
        } => format!("⚠ {task} 费用对账漂移：账本 {ledger_cents}¢ vs CLI 自报 {cli_cents}¢"),
        Event::ContextCompacted { task, round } => {
            format!("{task} 上下文已压缩（轮 {round}），任务继续")
        }
        Event::RoundsExhausted { task, rounds, .. } => {
            format!("⚑ {task} 轮数预算耗尽（{rounds} 轮）——maestro task resume 续跑")
        }
        Event::Suspended { task, reason, .. } => format!("{task} 挂起（{}）", reason_str(reason)),
        Event::TaskCompleted { task, summary, .. } => format!("✓ {task} 完成：{summary}"),
        Event::TaskFailed { task, error, .. } => format!("✗ {task} 失败：{error}"),
        Event::SteeringQueued { task, .. } => format!("{task} 轻推入队（下一轮生效）"),
        Event::SteeringDropped { task, .. } => format!("{task} 轻推丢弃（任务已终态，不静默）"),
        _ => {
            return format!(
                "[{ts}] {}",
                serde_json::to_string(&env.event).unwrap_or_default()
            )
        }
    };
    format!("[{ts}] {body}")
}

/// task.create 响应 → 友好输出
fn fmt_create(v: &Value) -> String {
    let id = v["task"]["id"].as_str().unwrap_or("?");
    let title = v["task"]["title"].as_str().unwrap_or("");
    if v["queued"].as_bool().unwrap_or(false) {
        let reason = match v["reason"].as_str() {
            Some("emergency_frozen") => "急停冻结中",
            Some("slots_full") => "并发槽位已满",
            Some("workdir_busy") => "工作目录被其他任务占用",
            _ => "排队中",
        };
        format!("任务已入队：{id} 「{title}」（{reason}，稍后自动启动）")
    } else if let Some(w) = v["worker"].as_str() {
        format!("任务已启动：{id} 「{title}」 → worker {w}")
    } else {
        serde_json::to_string_pretty(v).unwrap_or_default()
    }
}

/// task.list 响应 → 表格
fn fmt_task_list(v: &Value) -> String {
    let Some(tasks) = v["tasks"].as_array() else {
        return "(空)".into();
    };
    if tasks.is_empty() {
        return "没有任务。用 maestro task create 建一个。".into();
    }
    let mut out = String::from("ID       STATE      ROUND  FAIL  TITLE\n");
    out.push_str(&"-".repeat(58));
    out.push('\n');
    for t in tasks {
        let id = t["id"].as_str().unwrap_or("?");
        let state = t["state"].as_str().unwrap_or("?");
        let round = t["round"].as_u64().unwrap_or(0);
        let fail = t["acceptance_failures"].as_u64().unwrap_or(0);
        let title = t["title"].as_str().unwrap_or("");
        out.push_str(&format!(
            "{id:<8} {state:<10} {round:<6} {fail:<5} {title}\n"
        ));
    }
    out
}

/// inbox 响应 → 行动建议
fn fmt_inbox(v: &Value) -> String {
    let Some(items) = v["items"].as_array() else {
        return "(空)".into();
    };
    if items.is_empty() {
        return "收件箱是空的 —— 没有需要你处理的任务。".into();
    }
    let mut out = format!("收件箱（{} 项需要处理）\n", items.len());
    out.push_str(&"-".repeat(58));
    out.push('\n');
    for it in items {
        let task = it["task"].as_str().unwrap_or("?");
        let kind = it["kind"].as_str().unwrap_or("?");
        let title = it["title"].as_str().unwrap_or("");
        let (what, hint) = match kind {
            "acceptance_failed" => (
                "连续 3 次假完成（声称完成但无产物）",
                format!("确认重试: maestro task resume {task}   或回滚: maestro task checkpoints {task}"),
            ),
            "infra" => (
                "基础设施故障（网络/供应商），自动恢复已耗尽",
                format!("网络恢复后重试: maestro task resume {task}"),
            ),
            "rounds_exhausted" => (
                "轮数预算耗尽（未输出完成信号，会话现场完整保留）",
                format!(
                    "续跑（从上一完成轮续接）: maestro task resume {task}   或放弃: maestro task cancel {task}"
                ),
            ),
            other => (other, format!("maestro task get {task}")),
        };
        out.push_str(&format!("{task} 「{title}」\n  {what}\n  → {hint}\n"));
    }
    out
}

/// ledger 响应 → 人话（P0 验收项：「这个任务花了多少」一句话回答）
fn fmt_ledger(v: &Value) -> String {
    let rounds = v["rounds"].as_u64().unwrap_or(0);
    let wall_ms = v["wall_ms"].as_u64().unwrap_or(0);
    let secs = wall_ms / 1000;
    let dur = if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    };
    let in_tok = v["input_tokens"].as_u64().unwrap_or(0);
    let out_tok = v["output_tokens"].as_u64().unwrap_or(0);
    let actual = v["actual_cost_cents"].as_u64().unwrap_or(0);
    let cf = v["counterfactual_cost_cents"].as_u64().unwrap_or(0);
    let saved = v["saved_cents"].as_u64().unwrap_or(0);
    let mut out = format!(
        "任务 {}：{} 轮 · 墙钟 {dur}",
        v["task"].as_str().unwrap_or("?"),
        rounds
    );
    if in_tok + out_tok > 0 {
        out.push_str(&format!(
            " · {in_tok} in / {out_tok} out tokens · 实付 {actual}¢"
        ));
        // cache 明细（R24/R28 计量闭环）：命中占比给「前缀是否被破坏」直觉
        if let Some(cr) = v["cache_read_tokens"].as_u64().filter(|c| *c > 0) {
            let pct = cr * 100 / in_tok.max(1);
            out.push_str(&format!(" · cache 命中 {cr}（{pct}%）"));
        }
        if saved > 0 {
            out.push_str(&format!(" · 省 {saved}¢（对比冷跑 {cf}¢）"));
        }
        // 上下文轮转（R37）：长任务的压缩次数 —— 「这任务上下文太大」的信号
        if let Some(n) = v["compactions"].as_u64().filter(|c| *c > 0) {
            out.push_str(&format!(" · 上下文压缩 {n} 次"));
        }
    } else {
        out.push_str(" · 暂无 token 用量（单发 worker / 多轮未入账）");
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_friendly_output() {
        let started = serde_json::json!({
            "task": { "id": "t-1", "title": "修 bug" },
            "worker": "w-1"
        });
        assert!(fmt_create(&started).contains("已启动"));
        let queued = serde_json::json!({
            "task": { "id": "t-2", "title": "排队" },
            "queued": true, "reason": "slots_full"
        });
        let s = fmt_create(&queued);
        assert!(s.contains("已入队") && s.contains("并发槽位已满"), "{s}");
    }

    #[test]
    fn task_list_table_shape() {
        let v = serde_json::json!({ "tasks": [
            { "id": "t-1", "state": "done", "round": 2, "acceptance_failures": 0, "title": "a" },
            { "id": "t-2", "state": "blocked", "round": 0, "acceptance_failures": 3, "title": "b" },
        ]});
        let s = fmt_task_list(&v);
        assert!(s.contains("ID") && s.contains("STATE"), "{s}");
        assert!(s.contains("t-1") && s.contains("blocked"));
        assert!(fmt_task_list(&serde_json::json!({ "tasks": [] })).contains("没有任务"));
    }

    #[test]
    fn inbox_gives_actionable_hints() {
        let v = serde_json::json!({ "items": [
            { "task": "t-9", "kind": "acceptance_failed", "title": "假完成" },
            { "task": "t-10", "kind": "rounds_exhausted", "title": "长活" },
        ]});
        let s = fmt_inbox(&v);
        assert!(s.contains("假完成"), "{s}");
        assert!(s.contains("maestro task resume t-9"), "{s}");
        // 轮数耗尽（R42）：续跑/放弃双建议
        assert!(s.contains("轮数预算耗尽"), "{s}");
        assert!(s.contains("maestro task resume t-10"), "{s}");
        assert!(s.contains("maestro task cancel t-10"), "{s}");
        assert!(fmt_inbox(&serde_json::json!({ "items": [] })).contains("空的"));
    }

    /// events --human：核心事件渲染 + 未知事件前向兼容（R45）
    #[test]
    fn event_human_rendering() {
        use maestro_protocol::events::{Envelope, Event};
        use maestro_protocol::types::*;
        let mk = |event: Event| Envelope {
            seq: 1,
            ts: 1_700_000_000,
            priority: Priority::Info,
            event,
        };
        let s = fmt_event_human(&mk(Event::RoundProgress {
            task: TaskId::new("t-1"),
            round: 3,
            tools_used: vec!["Read".into(), "Bash".into()],
            summary: "读文件".into(),
            tokens_in: 100,
            tokens_out: 20,
        }));
        assert!(s.contains("t-1 轮 3"), "{s}");
        assert!(s.contains("[Read,Bash]"), "{s}");
        let s = fmt_event_human(&mk(Event::CostDrift {
            task: TaskId::new("t-1"),
            round: 3,
            model: "m".into(),
            ledger_cents: 1,
            cli_cents: 50,
        }));
        assert!(s.contains("账本 1¢ vs CLI 自报 50¢"), "{s}");
        let s = fmt_event_human(&mk(Event::RoundsExhausted {
            task: TaskId::new("t-1"),
            worker: WorkerId::new("w-1"),
            rounds: 20,
        }));
        assert!(s.contains("⚑ t-1 轮数预算耗尽（20 轮）"), "{s}");
        let s = fmt_event_human(&mk(Event::Suspended {
            task: TaskId::new("t-1"),
            worker: WorkerId::new("w-1"),
            reason: SuspendReason::NetworkLost,
            session_ref: SessionRef::new(""),
            checkpoint_ref: CheckpointRef::new(""),
            round: 1,
        }));
        assert!(s.contains("挂起（network_lost）"), "{s}");
        // 未知事件：fallback 原始 JSON（daemon 新事件不破渲染）
        let s = fmt_event_human(&mk(Event::TaskStarted {
            task: TaskId::new("t-1"),
            worker: WorkerId::new("w-1"),
        }));
        assert!(s.contains("task_started"), "{s}");
    }

    #[test]
    fn ledger_human_summary() {
        // 多轮计价闭环：token + cache 命中 + 省钱口径
        let v = serde_json::json!({
            "task": "t-1", "rounds": 5, "wall_ms": 83_000,
            "input_tokens": 1000, "output_tokens": 200,
            "cache_read_tokens": 600, "cache_creation_tokens": 150,
            "actual_cost_cents": 1, "counterfactual_cost_cents": 1, "saved_cents": 0
        });
        let s = fmt_ledger(&v);
        assert!(s.contains("5 轮"), "{s}");
        assert!(s.contains("1m23s"), "{s}");
        assert!(s.contains("1000 in / 200 out"), "{s}");
        assert!(s.contains("cache 命中 600（60%）"), "{s}");
        // 无压缩 → 不显示（不制造噪音）
        assert!(!s.contains("压缩"), "{s}");
        // 有压缩 → 显示次数（R37 轮转感知）
        let v = serde_json::json!({
            "task": "t-1", "rounds": 9, "wall_ms": 500_000,
            "input_tokens": 9000, "output_tokens": 900,
            "actual_cost_cents": 10, "counterfactual_cost_cents": 20, "saved_cents": 10,
            "compactions": 2
        });
        let s = fmt_ledger(&v);
        assert!(s.contains("上下文压缩 2 次"), "{s}");
        // 无 token（单发 worker）：不显示待接入字样
        let empty = serde_json::json!({
            "task": "t-2", "rounds": 1, "wall_ms": 500,
            "input_tokens": 0, "output_tokens": 0
        });
        let s2 = fmt_ledger(&empty);
        assert!(s2.contains("暂无 token 用量"), "{s2}");
    }
}
