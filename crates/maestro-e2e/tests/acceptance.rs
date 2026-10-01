//! 验收门 v0 e2e（DEV_PLAN 0.10 / U10 设计「Goal 3 轮」）：
//! - 有真实产物 → AcceptanceGatePassed → Done + 永久 checkpoint
//! - 假完成（exit 0 无产物）→ 3 振出局 blocked(AcceptanceFailed)
//! - 中途产出 → 重试通过
//! - blocked → 用户 resume → 重新入队（计数清零）→ 完成

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::events::Event;
use maestro_protocol::types::*;
use serial_test::serial;

/// 退出码 0 且 worktree 有真实产物 → Done
#[test]
#[serial]
fn gate_passes_with_artifact() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "echo hi > result.txt; exit 0"]);
    let t = d.create_task("real-work", &work);
    assert!(
        d.wait_state(&t, WorkerState::Done, 5000),
        "有产物应完成，实际: {:?}",
        d.task_state(&t)
    );

    // 事件断言：AcceptanceGatePassed 先于 TaskCompleted
    let events = replay(&d);
    let passed = events
        .iter()
        .filter(|e| matches!(&e.event, Event::AcceptanceGatePassed { task, .. } if task == &t))
        .count();
    assert_eq!(passed, 1, "应有验收通过事件: {events:?}");
    assert!(std::fs::read_to_string(work.join("result.txt")).is_ok_and(|s| s.contains("hi")));

    // 永久 checkpoint（CpReason::AcceptancePassed）
    let cps = d.api(
        Method::CheckpointList,
        serde_json::json!({ "task": t.as_str() }),
    );
    let reasons: Vec<&str> = cps["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.contains(&"acceptance_passed"),
        "验收通过点应有 pinned checkpoint: {reasons:?}"
    );
}

/// 假完成：exit 0 但无任何产物 → 重试 3 次后 blocked(AcceptanceFailed)
#[test]
#[serial]
fn fake_completion_three_strikes() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "echo 'all done'; exit 0"]);
    let t = d.create_task("fake-done", &work);
    assert!(
        d.wait_state(&t, WorkerState::Blocked, 8000),
        "3 次假完成应 blocked，实际: {:?}",
        d.task_state(&t)
    );
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["blocked_kind"], "acceptance_failed", "{v}");
    assert_eq!(v["acceptance_failures"], 3, "{v}");

    // 恰好 3 次尝试（不无限重试）
    let events = replay(&d);
    let spawns = events
        .iter()
        .filter(|e| matches!(&e.event, Event::WorkerSpawned { task, .. } if task == &t))
        .count();
    assert_eq!(spawns, 3, "应恰好重试 3 次");
    let fails = events
        .iter()
        .filter(|e| matches!(&e.event, Event::AcceptanceGateFailed { task, failures, .. } if task == &t && failures == &3))
        .count();
    assert_eq!(fails, 1, "第 3 次失败应只发一次事件");
    // Goal 3 轮语义：出局时上报阻塞条件（UI Critical 通知源）
    let goal = events
        .iter()
        .filter(|e| matches!(&e.event, Event::GoalProgress { task, blocked_condition: Some(_), .. } if task == &t))
        .count();
    assert_eq!(goal, 1, "3 振出局应发 GoalProgress(blocked)");
    // 反思回喂（aider 模式）：前两次失败的差异进 steering 且在重试轮投递
    let feedback_q = events
        .iter()
        .filter(|e| matches!(&e.event, Event::SteeringQueued { task, message } if task == &t && message.contains("验收门")))
        .count();
    assert_eq!(feedback_q, 2, "失败 1、2 次应各注入一条反馈");
    let feedback_d = events
        .iter()
        .filter(|e| matches!(&e.event, Event::SteeringDelivered { task, message, .. } if task == &t && message.contains("验收门")))
        .count();
    assert_eq!(feedback_d, 2, "重试轮开工前应投递反馈");
}

/// 前 2 轮假完成、第 3 轮产出 → 重试后通过（计数器放 .maestro/ —— 门忽略该目录）
#[test]
#[serial]
fn retry_recovers_when_worker_eventually_produces() {
    let (_repo, work) = git_repo();
    let script = r#"mkdir -p .maestro; n=$(cat .maestro/runs 2>/dev/null || echo 0); n=$((n+1)); echo $n > .maestro/runs; [ "$n" -ge 3 ] && echo ok > result.txt; exit 0"#;
    let d = TestDaemon::start("/bin/sh", &["-c", script]);
    let t = d.create_task("slow-learner", &work);
    assert!(
        d.wait_state(&t, WorkerState::Done, 8000),
        "第 3 轮应通过验收，实际: {:?}",
        d.task_state(&t)
    );
    let events = replay(&d);
    let fails = events
        .iter()
        .filter(|e| matches!(&e.event, Event::AcceptanceGateFailed { task, .. } if task == &t))
        .count();
    assert_eq!(fails, 2, "前两轮应失败");
    assert!(std::fs::read_to_string(work.join("result.txt")).is_ok());
}

/// 3 振出局后：TaskResume = 用户确认重试 → 重新入队（计数清零）→ 完成
#[test]
#[serial]
fn blocked_task_requeues_via_resume() {
    let (_repo, work) = git_repo();
    let script = r#"mkdir -p .maestro; n=$(cat .maestro/runs 2>/dev/null || echo 0); n=$((n+1)); echo $n > .maestro/runs; [ "$n" -ge 4 ] && echo ok > result.txt; exit 0"#;
    let d = TestDaemon::start("/bin/sh", &["-c", script]);
    let t = d.create_task("requeue-me", &work);
    assert!(d.wait_state(&t, WorkerState::Blocked, 8000), "先 3 振出局");

    // 用户看到收件箱条目后确认重试
    let inbox = d.api(Method::InboxList, serde_json::json!({}));
    assert_eq!(inbox["items"][0]["task"], t.as_str(), "{inbox}");

    d.api(
        Method::TaskResume,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 8000),
        "重入队后第 4 轮应完成，实际: {:?}",
        d.task_state(&t)
    );
    let events = replay(&d);
    let requeued = events
        .iter()
        .filter(|e| matches!(&e.event, Event::TaskRequeued { task, .. } if task == &t))
        .count();
    assert_eq!(requeued, 1, "应有 TaskRequeued 事件");
}

/// workdir 与 data_dir 重叠 → 读回不可观测 → 门退化为仅退出码（不误杀）
#[test]
#[serial]
fn unverifiable_workdir_degrades_to_exit_code_gate() {
    // workdir 即 data_dir（CoreConfig::default 的形态）
    let d = TestDaemon::start("/bin/sh", &["-c", "exit 0"]);
    let work = d.data_dir.clone();
    let t = d.create_task("degraded", &work);
    assert!(
        d.wait_state(&t, WorkerState::Done, 5000),
        "不可观测 workdir 不应 3 振出局，实际: {:?}",
        d.task_state(&t)
    );
    let events = replay(&d);
    let passed = events
        .iter()
        .filter(|e| matches!(&e.event, Event::AcceptanceGatePassed { task, output, .. } if task == &t && output.contains("exit code only")))
        .count();
    assert_eq!(passed, 1, "应标记为仅退出码门: {events:?}");
}

/// 账本 API（0.9）：轮数/墙钟/token/成本汇总
#[test]
#[serial]
fn ledger_reports_rounds_and_wallclock() {
    let (_repo, work) = git_repo();
    let script = r#"mkdir -p .maestro; n=$(cat .maestro/runs 2>/dev/null || echo 0); n=$((n+1)); echo $n > .maestro/runs; [ "$n" -ge 2 ] && echo ok > result.txt; exit 0"#;
    let d = TestDaemon::start("/bin/sh", &["-c", script]);
    let t = d.create_task("ledger-me", &work);
    assert!(
        d.wait_state(&t, WorkerState::Done, 8000),
        "实际: {:?}",
        d.task_state(&t)
    );
    let v = d.api(
        Method::TaskLedger,
        serde_json::json!({ "task": t.as_str() }),
    );
    // 2 轮（第 1 轮假完成重试）
    assert_eq!(v["rounds"].as_u64().unwrap(), 2, "{v}");
    assert!(v["wall_ms"].as_u64().unwrap() >= 0);
    // token 字段就位（值待 0.15 多轮驱动接入）
    assert!(v["input_tokens"].is_u64());
    assert_eq!(v["ledger_entries"].as_u64().unwrap(), 0);
    // 不存在的任务
    let r = d.try_api(Method::TaskLedger, serde_json::json!({ "task": "t-nope" }));
    assert_eq!(r.unwrap_err().0, -404);
}

/// 读事件流（WAL 支持并发读，无需停 daemon）
fn replay(d: &TestDaemon) -> Vec<maestro_protocol::events::Envelope> {
    maestro_daemon::persist::EventStore::open(&d.data_dir)
        .unwrap()
        .replay_all()
}
