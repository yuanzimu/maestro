//! mock-cli：桌面端「演示模式」的内置 worker（跨平台、零依赖）。
//!
//! 模拟 claude CLI headless 子集（对齐 adapter.rs ClaudeDialect 三要素：
//! system.session_id / result.result / result.usage）：
//! - 参数：`-p <prompt> [--resume <sid>] --output-format stream-json --verbose --max-turns 1`
//!   （未识别参数忽略 —— rounder 经方言构造的完整参数都能吃下）
//! - 每次调用 = 一轮；向 cwd 追加 `demo-result.md` 一节（验收门 v0 比对
//!   workdir 快照 —— 不写产物会被判「假完成」3 振出局）
//! - 完成信号：第 3 轮或 prompt 含「结束」→ result 带 `MAESTRO_DONE`
//! - prompt 含「无产物」→ 故意不写文件（演示收件箱 blocked 流程用）
//! - 轮数判定：读 demo-result.md 已有节数 +1（同 workdir 多任务并发时
//!   近似为各自轮数，演示场景足够）

use std::io::Write;

const RESULT_FILE: &str = "demo-result.md";

fn main() {
    // --- 解析参数（只关心 -p 与 --resume）---
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut prompt = String::new();
    let mut resume: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--prompt" => {
                if i + 1 < args.len() {
                    prompt = args[i + 1].clone();
                    i += 1;
                }
            }
            "--resume" => {
                resume = (i + 1 < args.len()).then(|| args[i + 1].clone());
                if resume.is_some() {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let prompt = if prompt.is_empty() {
        "继续任务".to_string()
    } else {
        prompt
    };

    // --- 模拟干活耗时 ---
    std::thread::sleep(std::time::Duration::from_millis(800));

    // --- 轮数 = 本任务已完成轮数 + 1 ---
    // 口径 = rounder 轮账 .maestro/<task>/rounds.jsonl 行数（与 daemon GC 同一
    // dir_name 消毒逻辑）。旧口径按 demo-result.md 节数推算 —— 同 workdir 跑过
    // 的任务互相累计，第 1 轮就算出 round≥3 直接 MAESTRO_DONE（叙事错乱
    // 「第 1 轮：第 2 轮完成」+ steer 来不及注入就终态）
    let task = std::env::var("MAESTRO_TASK_ID").unwrap_or_default();
    let dir: String = task
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect();
    let rounds_file = std::path::Path::new(".maestro").join(&dir).join("rounds.jsonl");
    let done_rounds = std::fs::read_to_string(&rounds_file)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let round = done_rounds as u32 + 1;

    let no_output = prompt.contains("无产物");
    let finish_now = round >= 3 || prompt.contains("结束");

    // --- 写产物（验收门比对源）---
    if !no_output {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(RESULT_FILE)
            .expect("mock-cli: 打不开 demo-result.md（workdir 不可写？）");
        let _ = writeln!(
            f,
            "## Round {round}\n\n- 指令：{}\n- 本轮产出：mock 数据 #{round}\n",
            truncate(&prompt, 80)
        );
    }

    // --- session（续接凭据：有 --resume 就沿用）---
    let sid = resume.unwrap_or_else(|| format!("mock-{}", std::process::id()));
    let model = "claude-sonnet-4";

    // --- 输出 claude stream-json 子集 ---
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    let _ = writeln!(
        out,
        r#"{{"type":"system","subtype":"init","session_id":"{sid}","model":"{model}"}}"#
    );
    // 工具调用（U3 叙事素材：RoundProgress.tools_used）
    let tool = if no_output { "Think" } else if round.is_multiple_of(2) { "Edit" } else { "Write" };
    let _ = writeln!(
        out,
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{tool}","id":"tu_{round}"}}]}}}}"#
    );

    let mut result = format!("第 {round} 轮完成：已处理「{}」", truncate(&prompt, 30));
    if no_output {
        result.push_str("（本轮故意未产出文件）");
    }
    if finish_now {
        result.push_str(" MAESTRO_DONE");
    }
    let usage = format!(
        r#"{{"input_tokens":{},"output_tokens":{},"cache_read_input_tokens":120,"cache_creation_input_tokens":30}}"#,
        200 + round * 50,
        40 + round * 10
    );
    // 注：故意不带 total_cost_usd —— 演示模式的拍脑袋费用与 daemon 牌价表
    // 必然对不上（>25% 漂移），每轮刷「费用对账漂移」告警纯属噪声；
    // 缺省 None 时 daemon 跳过对账（R34 设计内行为）
    let _ = writeln!(
        out,
        r#"{{"type":"result","subtype":"success","is_error":false,"result":{},"session_id":"{sid}","model":"{model}","usage":{usage}}}"#,
        serde_json::to_string(&result).unwrap_or_default()
    );
    let _ = out.flush();
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
