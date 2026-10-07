#![cfg(unix)]

//! 多轮负载混沌（R44）：8 任务并发（槽位 4），混合四种行为——
//! - 「结束」：首轮 DONE 快速收敛
//! - 「上下文增长」：低压缩阈值 → 轮转触发 → 注入「结束」收敛
//! - 「费用自洽」：单轮 DONE + total_cost_usd 对账静默
//! - 「坏schema」：exit 3 → TaskFailed
//!
//! 压测 R23~R42 全机制（多轮驱动/轮转/对账/漂移检测/槽位调度/会话隔离）
//! 在并发下的稳定性。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;

fn task_state_dir(work: &std::path::Path, task: &TaskId) -> std::path::PathBuf {
    work.join(".maestro").join(task.as_str())
}

#[test]
#[serial]
fn eight_mixed_multiround_tasks_converge() {
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    // 槽位 4 < 任务数 8：验证排队补位 + 并发不串
    let d = TestDaemon::start_limited_with_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        4,
        // 低压缩阈值：上下文增长型任务必触发轮转（200/轮，2 轮即 600）
        vec![("MAESTRO_CONTEXT_LIMIT".into(), "600".into())],
    );

    // 8 任务 × 独立 workdir（会话隔离是断言对象，不能共享）。
    // ⚠️ TempDir 必须保活到测试结束——迭代内 drop 会删掉 workdir
    //（worker cwd 变幽灵目录：轮账/out.txt 写进已摘除的 inode，外部读不到）
    let mut keep: Vec<tempfile::TempDir> = vec![];
    let mut tasks: Vec<(TaskId, std::path::PathBuf, &str)> = vec![];
    for (i, prompt) in [
        "结束",
        "上下文增长：每轮记录新发现",
        "费用自洽",
        "坏schema",
        "结束",
        "上下文增长：每轮记录新发现",
        "费用自洽",
        "坏schema",
    ]
    .iter()
    .enumerate()
    {
        let (repo, work) = git_repo();
        keep.push(repo);
        let t = d.create_task_with_prompt(&format!("load-{i}"), prompt, &work);
        tasks.push((t, work, prompt));
    }

    // 上下文增长型任务需要「结束」轻推收敛（等轮转发生后再推，确保
    // 轮转路径被走到；简单起见：等任意一条增长任务的轮账出现压缩轮）
    let growing: Vec<&(TaskId, std::path::PathBuf, &str)> = tasks
        .iter()
        .filter(|(_, _, p)| *p == "上下文增长：每轮记录新发现")
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    'outer: while std::time::Instant::now() < deadline {
        for (t, work, _) in &growing {
            let p = task_state_dir(work, t).join("rounds.jsonl");
            if let Ok(c) = std::fs::read_to_string(&p) {
                if c.lines().any(|l| l.contains("\"compacted\":true")) {
                    break 'outer;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for (t, _, _) in &growing {
        d.api(
            Method::TaskSteer,
            serde_json::json!({ "task": t.as_str(), "message": "结束" }),
        );
    }

    // 全员收敛：6 Done + 2 Failed（坏schema）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let states: Vec<WorkerState> = tasks.iter().map(|(t, _, _)| d.task_state(t)).collect();
        let done = states.iter().filter(|s| **s == WorkerState::Done).count();
        let failed = states.iter().filter(|s| **s == WorkerState::Failed).count();
        let working = states
            .iter()
            .filter(|s| **s == WorkerState::Working || **s == WorkerState::Queued)
            .count();
        if done == 6 && failed == 2 && working == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            // 诊断 dump：Blocked 任务的轮账 + worker 日志
            for (t, work, _) in &tasks {
                if d.task_state(t) == WorkerState::Blocked {
                    eprintln!("=== {t} rounds.jsonl:");
                    if let Ok(c) =
                        std::fs::read_to_string(task_state_dir(work, t).join("rounds.jsonl"))
                    {
                        eprintln!("{c}");
                    }
                    let logs = std::path::Path::new(&d.data_dir).join("logs");
                    if let Ok(es) = std::fs::read_dir(&logs) {
                        for e in es.flatten() {
                            if let Ok(c) = std::fs::read_to_string(e.path()) {
                                if !c.trim().is_empty() {
                                    eprintln!("--- {:?}: {c}", e.file_name());
                                }
                            }
                        }
                    }
                }
            }
            panic!(
                "60s 未收敛: done={done} failed={failed} states={states:?} details={:?}",
                tasks
                    .iter()
                    .map(|(t, _, _)| {
                        d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }))
                    })
                    .collect::<Vec<_>>()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 会话隔离：每任务轮账内 session 全同 + 各任务互不相同
    let mut all_sids: Vec<String> = vec![];
    for (t, work, prompt) in &tasks {
        if *prompt == "坏schema" {
            continue; // 坏 schema 无轮账（首轮 exit 3 前不落盘）
        }
        let rounds = std::fs::read_to_string(task_state_dir(work, t).join("rounds.jsonl")).unwrap();
        let sids: Vec<String> = rounds
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .map(|r| r["session_id"].as_str().unwrap().to_string())
            .collect();
        assert!(!sids.is_empty(), "{t} 应有轮账");
        assert!(
            sids.windows(2).all(|w| w[0] == w[1]),
            "{t} 任务内 session 应一致: {sids:?}"
        );
        all_sids.push(sids[0].clone());
    }
    let uniq: std::collections::HashSet<&String> = all_sids.iter().collect();
    assert_eq!(
        uniq.len(),
        all_sids.len(),
        "任务间 session 不得重复（跨任务泄漏）: {all_sids:?}"
    );

    // 轮转与对账在并发下正常：增长任务有压缩事件 + CostDrift 仅费用虚报场景
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let compacted = store
        .replay_all()
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::ContextCompacted { .. }
            )
        })
        .count();
    assert!(compacted >= 1, "并发下轮转应发生（低阈值）");
    // 费用自洽任务零 CostDrift（漂移告警不出现在自洽负载里）
    let drifts = store
        .replay_all()
        .iter()
        .filter(|e| matches!(&e.event, maestro_protocol::events::Event::CostDrift { .. }))
        .count();
    assert_eq!(drifts, 0, "自洽任务不得触发 CostDrift");
}
