//! bench-worker：基准集 mock 模式的确定性 worker（claude 方言子集）。
//!
//! 与 mock-cli（演示模式，只写 demo-result.md）不同：按任务真实应用
//! suite/<id>/solution/ 下的修复文件，使验收命令（cargo test / node --test）
//! 从红转绿 —— 这是基准「验收通过率」的可复现口径。
//!
//! 定位约定（runner 布局）：cwd = <run_root>/tasks/<suite-id>/repo，
//! 故 suite id = cwd 的父目录名；套件根经 MAESTRO_BENCH_SUITE 环境变量
//! 由 daemon worker_env 透传（daemon → rounder → 本进程 env 继承链）。
//! 轮次协议与 mock-cli 一致：第 1 轮分析（不落盘），第 2 轮应用修复并
//! MAESTRO_DONE（验收门比对 workdir 快照 —— 有变更即真完成）。

use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut prompt = String::new();
    let mut resume: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let v = args.get(i + 1).cloned();
        if matches!(a, "-p" | "--prompt") {
            if let Some(v) = v {
                prompt = v;
                i += 1;
            }
        } else if a == "--resume" {
            if let Some(v) = v {
                resume = Some(v);
                i += 1;
            }
        }
        i += 1;
    }

    std::thread::sleep(std::time::Duration::from_millis(150));

    // 轮数口径 = rounder 轮账行数 + 1（与 mock-cli 同款）
    let task = std::env::var("MAESTRO_TASK_ID").unwrap_or_default();
    let dir: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let rounds_file = std::path::Path::new(".maestro")
        .join(&dir)
        .join("rounds.jsonl");
    let done_rounds = std::fs::read_to_string(&rounds_file)
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let round = done_rounds as u32 + 1;

    // suite id = cwd 父目录名（runner 布局约定）
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let suite_id = cwd
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let suite_root = std::env::var("MAESTRO_BENCH_SUITE").unwrap_or_default();
    let solution_dir = Path::new(&suite_root).join(&suite_id).join("solution");

    // 第 1 轮：分析定位（不落盘）；第 ≥2 轮：应用修复 + 完成信号
    let applied = if round >= 2 && solution_dir.is_dir() {
        apply_tree(&solution_dir, &cwd)
    } else {
        Ok(0)
    };
    let finish_now = round >= 2;

    let sid = resume.unwrap_or_else(|| format!("bench-{}", std::process::id()));
    let model = "claude-sonnet-4";
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let _ = writeln!(
        out,
        r#"{{"type":"system","subtype":"init","session_id":"{sid}","model":"{model}"}}"#
    );
    let tool = if round == 1 { "Read" } else { "Edit" };
    let _ = writeln!(
        out,
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{tool}","id":"tu_{round}"}}]}}}}"#
    );

    let mut result = if round == 1 {
        format!("第 1 轮完成：已定位「{}」的缺陷根因", truncate(&prompt, 30))
    } else {
        match applied {
            Ok(n) if n > 0 => format!("第 {round} 轮完成：已应用修复（{n} 个文件变更）"),
            Ok(_) => format!("第 {round} 轮完成：已复核修复内容"),
            Err(e) => format!("第 {round} 轮完成：修复应用异常（{e}）"),
        }
    };
    if finish_now {
        result.push_str(" MAESTRO_DONE");
    }
    let usage = format!(
        r#"{{"input_tokens":{},"output_tokens":{},"cache_read_input_tokens":{},"cache_creation_input_tokens":60}}"#,
        600 + round as u64 * 100,
        120 + round as u64 * 40,
        240 + round as u64 * 60
    );
    let _ = writeln!(
        out,
        r#"{{"type":"result","subtype":"success","is_error":false,"result":{},"session_id":"{sid}","model":"{model}","usage":{usage}}}"#,
        serde_json::to_string(&result).unwrap_or_default()
    );
    let _ = out.flush();
}

/// 递归覆盖 solution/ 到 cwd；返回写入的文件数。
/// 写后把 mtime 拨到当前 —— Windows fs::copy 保留源时间戳，会早于
/// 验收命令（cargo test）的既有编译产物，mtime 缓存判定跳过重编译。
fn apply_tree(src: &Path, dst: &Path) -> Result<usize, String> {
    if !src.is_dir() {
        return Err(format!("solution 目录缺失: {}", src.display()));
    }
    let now = std::time::SystemTime::now();
    let mut n = 0usize;
    let entries = std::fs::read_dir(src).map_err(|e| format!("读 {} 失败: {e}", src.display()))?;
    for ent in entries.flatten() {
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("创建 {} 失败: {e}", to.display()))?;
            n += apply_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("写 {} 失败: {e}", to.display()))?;
            if let Ok(f) = std::fs::File::options().write(true).open(&to) {
                let _ = f.set_modified(now);
            }
            n += 1;
        }
    }
    Ok(n)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
