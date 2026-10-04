//! maestro-rounder：多轮驱动 Worker（DEV_PLAN 0.15）。
//!
//! daemon 把本程序作为 worker_program 拉起；它持有任务循环：
//! 每轮调一次底层 CLI（单轮 + `--resume` 续接），轮边界拉取轻推
//! （steering）注入下一轮 prompt。会话引用持久化在 workdir/.maestro/<task_id>/，
//! daemon 崩溃恢复后重拉 rounder 能从上一完成轮续接（R3 S7c 语义）；
//! 状态按任务隔离 —— 同 workdir 串行任务不共享会话（R36）。
//!
//! 用法（由 daemon 经 SpawnSpec 拉起）：
//!   maestro-rounder -- <inner-cli> [inner args...]
//! 环境：MAESTRO_TASK_ID / MAESTRO_WORKER_ID / MAESTRO_SOCKET_PATH /
//!       MAESTRO_PROMPT（daemon 注入）
//! 状态：<cwd>/.maestro/<task_id>/session（session id）、
//!       <cwd>/.maestro/<task_id>/rounds.jsonl（轮账）
//! 结束：底层 CLI 回答含 MAESTRO_DONE（结构化完成信号，v0）或达
//!       MAESTRO_MAX_ROUNDS（默认 20）或 CLI 非零退出（透传退出码）。

use maestro_client::MaestroClient;
use maestro_protocol::api::Method;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

const MAX_ROUNDS_DEFAULT: u32 = 20;
const DONE_MARKER: &str = "MAESTRO_DONE";
/// 上下文轮转（R37，R3_PROTOCOL §7.5）：占用 ≥ 阈值 → 轮边界注入压缩指令
/// （`claude -p "/compact ..." --resume` 官方受支持路径；compact 不换
/// session id）。默认 200k ≈ claude 窗口；比 CLI 被动 compact（~85-95%，
/// 时机在 turn 中间）提前可控触发
const CONTEXT_LIMIT_DEFAULT: u64 = 200_000;
const COMPACT_INSTRUCTION: &str = "/compact 保留任务状态、关键结论与文件路径，然后继续当前任务";

#[derive(Serialize)]
struct RoundRecord {
    round: u32,
    session_id: String,
    prompt: String,
    answer: String,
    steering_injected: bool,
    /// 本轮 prompt 是否为压缩指令（上下文轮转触发）
    #[serde(default)]
    compacted: bool,
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
    // 上下文轮转阈值（R37）：经 daemon worker_env 白名单透传
    let context_limit: u64 = std::env::var("MAESTRO_CONTEXT_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(CONTEXT_LIMIT_DEFAULT);
    if task.is_empty() || socket.is_empty() {
        eprintln!("maestro-rounder: 缺少 MAESTRO_TASK_ID/MAESTRO_SOCKET_PATH");
        std::process::exit(2);
    }

    let client = MaestroClient::from_api_socket(std::path::Path::new(&socket));
    // 状态按任务隔离（R36）：session/轮账放 .maestro/<task_id>/ ——
    // 同 workdir 串行任务不得共享会话（跨任务上下文泄漏，混沌⑩证伪后修复）；
    // 同任务 respawn/崩溃恢复读同一路径，续接语义不变。
    // task id 做目录名：非法字符替换 _（防御性，正常 id 为 t-N 形）
    let safe_task: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let state_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".maestro")
        .join(&safe_task);
    let _ = std::fs::create_dir_all(&state_dir);
    let session_file = state_dir.join("session");
    let rounds_file = state_dir.join("rounds.jsonl");

    let mut session: Option<String> = std::fs::read_to_string(&session_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut next_prompt = prompt;
    // at-least-once（R32）：本轮 prompt 里注入的消息 seq —— 轮尝试完成后
    // （成功/CLI 失败/结构化错误，CLI 已跑过该 prompt）经 TaskSteerAck 确认；
    // rounder 在 CLI 启动前被杀 → 未确认 → daemon 重投
    let mut injected_seqs: Vec<u64> = vec![];
    // 方言（0.6 适配器）：默认 claude，MAESTRO_CLI_DIALECT 可换（B7 Codex/Gemini）
    let dialect = maestro_daemon::adapter::dialect_by_name(
        &std::env::var("MAESTRO_CLI_DIALECT").unwrap_or_default(),
    );

    let mut round: u32 = 0;
    // 上下文轮转状态（R37）：compact_pending = 下一轮注入压缩指令；
    // 防抖——压缩轮本身 usage 仍高（输入=全上下文），压缩后跳过一次检测
    // 给压缩生效留一轮（若仍超限，隔轮再压，不是死循环）
    let mut compact_pending = false;
    loop {
        round += 1;
        let this_round_is_compact = compact_pending;
        compact_pending = false;
        // 1. 跑一轮底层 CLI（方言构造参数）
        let mut cmd = Command::new(&cli[0]);
        cmd.args(&cli[1..])
            .args(dialect.round_args(&next_prompt, session.as_deref()));
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                // CLI 未跑（spawn 失败）→ 不 ack（消息未被消费，daemon 会重投）
                eprintln!("maestro-rounder: spawn 内层 CLI 失败: {e}");
                std::process::exit(3);
            }
        };
        if !out.status.success() {
            // CLI 跑过本轮 prompt（注入消息已消费）→ 先 ack 再透传退出码
            ack_steering(&client, &task, &worker, &injected_seqs);
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            std::process::exit(out.status.code().unwrap_or(1));
        }
        let oc = maestro_daemon::adapter::parse_stream_json(&out.stdout);
        // 结构化错误（R28，调研落地）：result.errors/api_error_status 优于解析
        // stderr 文本。可重试（429/5xx 过载限流）→ 合成 "API Error: N" 到
        // stderr 退出，daemon 分类为断连 → Suspended 自动恢复（否则烧光轮数
        // 预算）；不可重试 → 透传结构化错误文本走 Failed。
        // error_max_turns 不算错误（--max-turns 1 的正常出口，轮循环继续）
        if oc.is_error && (oc.api_error_status.is_some() || !oc.errors.is_empty()) {
            ack_steering(&client, &task, &worker, &injected_seqs);
            let msg = oc
                .api_error_status
                .map(|s| format!("API Error: {s}"))
                .unwrap_or_else(|| oc.errors.join("; "));
            eprintln!("maestro-rounder: {msg}");
            std::process::exit(1);
        }
        // schema 漂移检测（R35）：续接凭据/计量/回答捕获任一缺失 = 核心能力
        // 静默失效 —— 显式报错带人话诊断（exit 3 → daemon 判 Fatal），
        // 不让坏 schema 的轮次空转烧预算
        let drifts = maestro_daemon::adapter::schema_drift(&oc);
        if !drifts.is_empty() {
            eprintln!("maestro-rounder: CLI schema 漂移: {}", drifts.join("; "));
            std::process::exit(3);
        }
        let sid = oc
            .session_id
            .clone()
            .expect("schema_drift 已保证 session_id");
        session = Some(sid.clone());
        let _ = std::fs::write(&session_file, &sid);
        let answer = oc.answer.clone();

        // 2. 确认上轮注入消息已消费（在 poll 前 —— 防自己重投自己）
        if !injected_seqs.is_empty() {
            ack_steering(&client, &task, &worker, &injected_seqs);
            injected_seqs.clear();
        }
        // 3. 轮边界拉轻推（U4：注入下一轮；at-least-once 重投未确认）
        let steering = poll_steering(&client, &task, &worker);
        let injected = !steering.is_empty();
        injected_seqs = steering.iter().map(|(s, _)| *s).collect();

        // 3. 轮账落盘
        let rec = RoundRecord {
            round,
            session_id: sid,
            prompt: next_prompt.clone(),
            answer: answer.clone(),
            steering_injected: injected,
            compacted: this_round_is_compact,
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
                "cache_creation_tokens": oc.cache_creation,
                "model": oc.model,
                "tools_used": oc.tools_used,
                // 摘要（U3 叙事）：全文可能很长，rounder 侧先截 200 字符
                "summary": answer.chars().take(200).collect::<String>(),
                // CLI 自报费用（R34 对账；无该字段的 CLI 为 null → daemon 跳过对账）
                "total_cost_usd": oc.total_cost_usd,
                // 上下文轮转（R37）：daemon 发 ContextCompacted 事件（U3 叙事）
                "compacted": this_round_is_compact,
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

        // 6. 上下文占用检测（R37）：输入侧 token（含缓存命中）近似占用。
        //    压缩轮跳过（防抖——压缩生效需一轮，见上）
        let context_used = oc.usage_in + oc.cache_read.unwrap_or(0);
        if !this_round_is_compact && context_used >= context_limit {
            compact_pending = true;
        }

        // 7. 下一轮 prompt：轻推 > 压缩 > 继续。
        //    轻推优先（用户实时指令最急，压缩顺延到下轮检测）；
        //    无内容时按轮间隔歇一拍（防秒回型 CLI 热循环）
        if injected {
            next_prompt = steering
                .iter()
                .map(|(_, m)| m.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        } else if compact_pending {
            eprintln!("maestro-rounder: 上下文占用 {context_used} ≥ {context_limit}，注入压缩指令");
            next_prompt = COMPACT_INSTRUCTION.into();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(round_gap_ms));
            next_prompt = "继续当前任务；没有剩余工作时输出完成信号".into();
        }
    }
}

/// 拉 steering（尽力而为：daemon 不可达时不阻塞任务，下一轮再试）。
/// 返回 (seq, message) —— seq 用于消费后 ack（at-least-once）
fn poll_steering(client: &MaestroClient, task: &str, worker: &str) -> Vec<(u64, String)> {
    match client.call(
        "steer-poll",
        Method::TaskSteerPoll,
        serde_json::json!({ "task": task, "worker": worker }),
    ) {
        Ok(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        let seq = m["seq"].as_u64()?;
                        let msg = m["message"].as_str()?;
                        Some((seq, msg.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => vec![],
    }
}

/// 确认 steering 消费（尽力而为：daemon 不可达时下轮 poll 会重投 —— 可接受）
fn ack_steering(client: &MaestroClient, task: &str, worker: &str, seqs: &[u64]) {
    if seqs.is_empty() {
        return;
    }
    let _ = client.call(
        "steer-ack",
        Method::TaskSteerAck,
        serde_json::json!({ "task": task, "worker": worker, "seqs": seqs }),
    );
}
