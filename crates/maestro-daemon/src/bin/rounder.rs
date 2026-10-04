//! maestro-rounder：多轮驱动 Worker（DEV_PLAN 0.15）。
//!
//! daemon 把本程序作为 worker_program 拉起；它持有任务循环：
//! 每轮调一次底层 CLI（单轮 + `--resume` 续接），轮边界拉取轻推
//! （steering）注入下一轮 prompt。会话引用持久化在 workdir/.maestro/，
//! daemon 崩溃恢复后重拉 rounder 能从上一完成轮续接（R3 S7c 语义）。
//!
//! 用法（由 daemon 经 SpawnSpec 拉起）：
//!   maestro-rounder -- <inner-cli> [inner args...]
//! 环境：MAESTRO_TASK_ID / MAESTRO_WORKER_ID / MAESTRO_SOCKET_PATH /
//!       MAESTRO_PROMPT（daemon 注入）
//! 状态：<cwd>/.maestro/session（session id）、.maestro/rounds.jsonl（轮账）
//! 结束：底层 CLI 回答含 MAESTRO_DONE（结构化完成信号，v0）或达
//!       MAESTRO_MAX_ROUNDS（默认 20）或 CLI 非零退出（透传退出码）。

use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

const MAX_ROUNDS_DEFAULT: u32 = 20;
const DONE_MARKER: &str = "MAESTRO_DONE";

#[derive(Serialize)]
struct RoundRecord {
    round: u32,
    session_id: String,
    prompt: String,
    answer: String,
    steering_injected: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // "--" 之后是底层 CLI
    let split = args.iter().position(|a| a == "--").unwrap_or(0);
    let (mine, cli) = args.split_at(split);
    let cli = if cli.is_empty() {
        eprintln!("maestro-rounder: 需要 `-- <inner-cli> [args...]`");
        std::process::exit(2);
    } else {
        &cli[1..]
    };
    let _ = mine; // 预留（未来 --budget 等）

    let task = std::env::var("MAESTRO_TASK_ID").unwrap_or_default();
    let worker = std::env::var("MAESTRO_WORKER_ID").unwrap_or_default();
    let socket = std::env::var("MAESTRO_SOCKET_PATH").unwrap_or_default();
    let prompt = std::env::var("MAESTRO_PROMPT").unwrap_or_else(|_| "继续任务".into());
    let max_rounds: u32 = std::env::var("MAESTRO_MAX_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(MAX_ROUNDS_DEFAULT);
    // 无轻推时的轮间隔（毫秒）：真实 CLI 每轮秒级，此间隔只为防止
    // 「秒回型 CLI」热循环烧穿轮数预算；经 daemon 环境继承可调
    let round_gap_ms: u64 = std::env::var("MAESTRO_ROUND_GAP_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    if task.is_empty() || socket.is_empty() {
        eprintln!("maestro-rounder: 缺少 MAESTRO_TASK_ID/MAESTRO_SOCKET_PATH");
        std::process::exit(2);
    }

    let client = MaestroClient::from_api_socket(std::path::Path::new(&socket));
    let state_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".maestro");
    let _ = std::fs::create_dir_all(&state_dir);
    let session_file = state_dir.join("session");
    let rounds_file = state_dir.join("rounds.jsonl");

    let mut session: Option<String> = std::fs::read_to_string(&session_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut next_prompt = prompt;
    // 方言（0.6 适配器）：默认 claude，MAESTRO_CLI_DIALECT 可换（B7 Codex/Gemini）
    let dialect = maestro_daemon::adapter::dialect_by_name(
        &std::env::var("MAESTRO_CLI_DIALECT").unwrap_or_default(),
    );

    let mut round: u32 = 0;
    loop {
        round += 1;
        // 1. 跑一轮底层 CLI（方言构造参数）
        let mut cmd = Command::new(&cli[0]);
        cmd.args(&cli[1..])
            .args(dialect.round_args(&next_prompt, session.as_deref()));
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("maestro-rounder: spawn 内层 CLI 失败: {e}");
                std::process::exit(3);
            }
        };
        if !out.status.success() {
            // 透传退出码：daemon 按 adapter::classify_exit 分类（断连 → suspended 等）
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(out.status.code().unwrap_or(1));
        }
        let oc = maestro_daemon::adapter::parse_stream_json(&out.stdout);
        let Some(sid) = oc.session_id.clone() else {
            eprintln!("maestro-rounder: stream-json 缺 session_id");
            std::process::exit(3);
        };
        session = Some(sid.clone());
        let _ = std::fs::write(&session_file, &sid);
        let answer = oc.answer.clone();

        // 2. 轮边界拉轻推（U4：注入下一轮）
        let steering = poll_steering(&client, &task, &worker);
        let injected = !steering.is_empty();

        // 3. 轮账落盘
        let rec = RoundRecord {
            round,
            session_id: sid,
            prompt: next_prompt.clone(),
            answer: answer.clone(),
            steering_injected: injected,
        };
        if let Ok(line) = serde_json::to_string(&rec) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&rounds_file)
            {
                let _ = writeln!(f, "{line}");
            }
        }

        // 4. 轮账上报（尽力而为：daemon 不可达/已易主不阻塞 —— 少记一轮可接受）
        let _ = client.call(
            "round-report",
            Method::TaskRoundReport,
            serde_json::json!({
                "task": task, "worker": worker, "round": round,
                "input_tokens": oc.usage_in, "output_tokens": oc.usage_out,
                "cache_read_tokens": oc.cache_read,
                "model": oc.model,
            }),
        );

        // 5. 结束判定
        if answer.contains(DONE_MARKER) {
            std::process::exit(0);
        }
        if round >= max_rounds {
            eprintln!("maestro-rounder: 达最大轮数 {max_rounds}");
            std::process::exit(0);
        }

        // 6. 下一轮 prompt：有轻推 → 轻推内容；无 → 继续指令。
        //    无轻推时按轮间隔歇一拍（防秒回型 CLI 热循环）
        if injected {
            next_prompt = steering.join("\n");
        } else {
            std::thread::sleep(std::time::Duration::from_millis(round_gap_ms));
            next_prompt = "继续当前任务；没有剩余工作时输出完成信号".into();
        }
    }
}

/// 拉 steering（尽力而为：daemon 不可达时不阻塞任务，下一轮再试）
fn poll_steering(client: &MaestroClient, task: &str, worker: &str) -> Vec<String> {
    match client.call(
        "steer-poll",
        Method::TaskSteerPoll,
        serde_json::json!({ "task": task, "worker": worker }),
    ) {
        Ok(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["message"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => vec![],
    }
}
