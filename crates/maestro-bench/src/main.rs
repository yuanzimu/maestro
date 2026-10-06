//! maestro-bench：C7 效果基准集 runner。
//!
//! 用法：cargo run -p maestro-bench -- [--suite <dir>] [--task <id>]...
//!       [--report-dir <dir>] [--threshold 0.98]
//! 退出码：0 = 验收通过率达标；1 = 未达标 / 有任务失败；2 = 用法错。

use maestro_bench::manifest::load_suite;
use maestro_bench::report::BenchReport;
use maestro_bench::runner::{ensure_bin, BenchDaemon, RunConfig};
use std::path::PathBuf;

struct Args {
    suite: Option<PathBuf>,
    tasks: Vec<String>,
    report_dir: Option<PathBuf>,
    threshold: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        suite: None,
        tasks: vec![],
        report_dir: None,
        threshold: 0.98,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--suite" => {
                i += 1;
                args.suite = Some(PathBuf::from(argv.get(i).ok_or("--suite 缺参数")?));
            }
            "--task" => {
                i += 1;
                args.tasks.push(argv.get(i).ok_or("--task 缺参数")?.clone());
            }
            "--report-dir" => {
                i += 1;
                args.report_dir = Some(PathBuf::from(argv.get(i).ok_or("--report-dir 缺参数")?));
            }
            "--threshold" => {
                i += 1;
                args.threshold = argv
                    .get(i)
                    .ok_or("--threshold 缺参数")?
                    .parse()
                    .map_err(|_| "threshold 须为 0~1 小数")?;
            }
            other => return Err(format!("未知参数 {other}")),
        }
        i += 1;
    }
    Ok(args)
}

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("参数错误: {e}");
            return 2;
        }
    };
    // v0 仅 mock（确定性）；real 模式（真实 provider 对比）留 C7 后续
    let mode = "mock";

    let suite_dir = args
        .suite
        .clone()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("suite"));
    let tasks = match load_suite(&suite_dir) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("套件加载失败: {e}");
            return 2;
        }
    };
    let selected: Vec<_> = if args.tasks.is_empty() {
        tasks
    } else {
        tasks
            .into_iter()
            .filter(|t| args.tasks.iter().any(|id| id == &t.manifest.id))
            .collect()
    };
    if selected.is_empty() {
        eprintln!("没有匹配的任务（--task 过滤后为空）");
        return 2;
    }

    let rounder = match ensure_bin("maestro-rounder", "maestro-daemon") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let bench_worker = match ensure_bin("bench-worker", "maestro-bench") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let run_root = std::env::temp_dir().join(format!("maestro-bench-{ts}"));
    let data_dir = run_root.join("data");
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("创建运行目录失败: {e}");
        return 2;
    }

    println!(
        "maestro-bench: mode={mode} suite={} tasks={} run_root={}",
        suite_dir.display(),
        selected.len(),
        run_root.display()
    );

    let cfg = RunConfig {
        suite_dir: suite_dir.clone(),
        rounder,
        bench_worker,
        run_root: run_root.clone(),
    };
    let daemon = BenchDaemon::start(&cfg, &data_dir);

    let mut outcomes = Vec::new();
    for t in &selected {
        println!("→ [{}] {} ...", t.manifest.kind, t.manifest.id);
        let o = daemon.run_task(t, &cfg);
        println!(
            "  终态={} 轮数={} 验收={}",
            o.state,
            o.rounds,
            if o.accept_pass { "PASS" } else { "FAIL" }
        );
        outcomes.push(o);
    }
    let summary_ledger = daemon.ledger_summary();
    daemon.shutdown();

    let report = BenchReport::build(mode, outcomes);
    let report_dir = args
        .report_dir
        .clone()
        .unwrap_or_else(|| run_root.join("report"));
    let _ = std::fs::create_dir_all(&report_dir);
    let json_path = report_dir.join("bench-report.json");
    let md_path = report_dir.join("bench-report.md");
    let _ = std::fs::write(&json_path, report.to_json());
    let _ = std::fs::write(&md_path, report.to_markdown());
    println!("\n{}", report.to_markdown());
    println!("JSON: {}", json_path.display());

    if let Some(all) = summary_ledger["all_time"].as_object() {
        if let Some(v) = all.get("cache_hit_pct") {
            println!("daemon 账本全量口径 cache_hit_pct = {v:?}（C6 链路验证）");
        }
    }

    let rate = report.summary.accept_rate;
    if rate >= args.threshold && report.summary.total > 0 {
        println!(
            "验收通过率 {:.1}% ≥ 门禁 {:.1}% —— PASS",
            rate * 100.0,
            args.threshold * 100.0
        );
        0
    } else {
        println!(
            "验收通过率 {:.1}% < 门禁 {:.1}% —— FAIL",
            rate * 100.0,
            args.threshold * 100.0
        );
        1
    }
}
