//! 全链冒烟：daemon + rounder + bench-worker 真实跑通一个基准任务，
//! 断言终态 done + 验收通过 + 账本计量闭环。跨平台（mock 模式不依赖
//! bash / API key），进 CI 三平台硬门。

use maestro_bench::manifest::load_suite;
use maestro_bench::runner::{ensure_bin, BenchDaemon, RunConfig};
use std::path::PathBuf;

fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("suite")
}

#[test]
fn mock_mode_full_run_one_task() {
    let tasks = load_suite(&suite_dir()).expect("套件加载");
    // 选一个 node 任务：验收零编译、CI 三平台快而稳
    let task = tasks
        .iter()
        .find(|t| t.manifest.id == "slugify-collapse")
        .expect("套件应含 slugify-collapse");

    let rounder = ensure_bin("maestro-rounder", "maestro-daemon").expect("rounder");
    let bench_worker = PathBuf::from(env!("CARGO_BIN_EXE_bench-worker"));

    let tmp = tempfile::tempdir().unwrap();
    let run_root = tmp.path().join("run");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&run_root).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    let cfg = RunConfig {
        suite_dir: suite_dir(),
        rounder,
        bench_worker,
        run_root,
    };
    let daemon = BenchDaemon::start(&cfg, &data_dir);
    let out = daemon.run_task(task, &cfg);

    // C6 汇总口径（本 daemon 数据目录内）：完成数应 ≥1 —— 须在 shutdown 前取
    let v = daemon.ledger_summary();
    daemon.shutdown();

    assert_eq!(out.state, "done", "任务应完成：{out:?}");
    assert!(out.accept_pass, "验收应通过：{out:?}");
    assert!(out.rounds >= 2, "mock 模式至少 2 轮：{out:?}");
    assert!(out.input_tokens > 0, "账本应计量到 token：{out:?}");

    let done = v["all_time"]["tasks_completed"].as_u64().unwrap_or(0);
    assert!(done >= 1, "ledger_summary 应统计到已完成任务: {v}");
}
