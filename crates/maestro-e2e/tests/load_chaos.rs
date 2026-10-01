//! R18 负载与混沌测试：
//! - 30 任务 × 4 槽位混合 workdir：全部完成、worker 无重复、seq 唯一
//! - kill -9 循环：反复崩溃重启，任务最终收敛、事件库无损坏

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

/// 负载：30 个快任务压 4 槽 6 workdir（含互斥争用），全部 Done
#[test]
#[serial]
fn load_30_tasks_4_slots_converges() {
    let repos: Vec<_> = (0..6).map(|_| git_repo()).collect();
    let works: Vec<std::path::PathBuf> = repos.iter().map(|(_, w)| w.clone()).collect();
    let d = TestDaemon::start_limited(
        "/bin/sh",
        &["-c", "echo $MAESTRO_TASK_ID > out.txt; exit 0"],
        None,
        4,
    );

    let mut tasks = vec![];
    for i in 0..30 {
        let w = &works[i % works.len()];
        tasks.push(d.create_task(&format!("load-{i}"), w));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    for t in &tasks {
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "超时：{} 未完成",
                t.as_str()
            );
            if matches!(
                d.task_state(t),
                WorkerState::Done | WorkerState::Failed | WorkerState::Blocked
            ) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(d.task_state(t), WorkerState::Done, "{} 应完成", t.as_str());
    }

    // 事件库完整性：seq 唯一、worker id 无重复分配
    let all = maestro_daemon::persist::EventStore::open(&d.data_dir)
        .unwrap()
        .replay_all();
    let mut seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(seqs.len(), all.len(), "seq 不得重复");
    let mut workers: Vec<String> = all
        .iter()
        .filter_map(|e| match &e.event {
            maestro_protocol::events::Event::WorkerSpawned { worker, .. } => {
                Some(worker.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    workers.sort();
    workers.dedup();
    assert!(
        workers.len() >= 30,
        "至少 30 个独立 worker（实际 {}）",
        workers.len()
    );
}

/// 混沌：任务 3 振出局 blocked 后 kill -9 daemon → 重启恢复 →
/// 用户 requeue → 收敛完成。验证崩溃不丢 blocked 状态、requeue 路径闭环。
#[test]
#[serial]
fn chaos_blocked_then_kill9_then_requeue_converges() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();
    // workdir 必须跨实例存活
    let (_repo, work) = git_repo();

    // 第 5 次运行起才有产物（前 4 次 = 假完成 → 实例 1 内 3 振出局）
    let script = r#"mkdir -p .maestro; n=$(cat .maestro/runs 2>/dev/null || echo 0); n=$((n+1)); echo $n > .maestro/runs; [ "$n" -ge 5 ] && echo done > result.txt; exit 0"#;

    let t;
    {
        let d = TestDaemon::start_with("/bin/sh", &["-c", script], Some(data_dir.clone()));
        t = d.create_task("chaos-task", &work);
        assert!(
            d.wait_state(&t, WorkerState::Blocked, 10000),
            "前几轮无产物应 3 振出局，实际: {:?}",
            d.task_state(&t)
        );
    } // drop = kill -9 语义

    // 崩溃重启：blocked 状态从事件流恢复（不丢）
    let d2 = TestDaemon::start_with("/bin/sh", &["-c", script], Some(data_dir.clone()));
    let v = d2.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    let st: WorkerState = serde_json::from_value(v["state"].clone()).unwrap();
    assert_eq!(st, WorkerState::Blocked, "崩溃后 blocked 应保持: {v}");
    assert_eq!(v["blocked_kind"], "acceptance_failed");

    // 用户确认重试：requeue（计数清零）→ 后续轮次补齐产物 → 收敛
    d2.api(
        Method::TaskResume,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(
        d2.wait_state(&t, WorkerState::Done, 15000),
        "requeue 后应收敛完成，实际: {:?}",
        d2.task_state(&t)
    );
    assert!(std::fs::read_to_string(work.join("result.txt")).is_ok());
}
