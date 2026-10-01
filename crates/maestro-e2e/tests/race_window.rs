//! R14 竞态回归：WorkerExit 消息晚于 TaskPause/EmergencyStop 到达时，
//! 任务停在 Suspended 但 worker 已死 —— resume 不得产生「无进程的 Working」。
//!
//! 构造方式：先挂起（真 worker 冻结在 metas 里），再注入「假」WorkerExit
//! （Core 信任消息），复现「退出消息迟到于挂起」的消息顺序。

use maestro_daemon::core::CoreMsg;
use maestro_daemon::worker::WorkerExit;
use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::pgid_sandbox;
use serial_test::serial;

fn worker_of(d: &TestDaemon, task: &TaskId) -> (String, u32) {
    let v = d.api(
        Method::TaskGet,
        serde_json::json!({ "task": task.as_str() }),
    );
    let w = v["worker"].as_str().unwrap().to_string();
    let workers = d.api(Method::WorkerList, serde_json::json!({}));
    let pid = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"].as_str() == Some(&w))
        .and_then(|x| x["pid"].as_u64())
        .unwrap() as u32;
    (w, pid)
}

/// 手动 resume 必须重拉进程，而不是对着死 worker 发 Resumed
#[test]
#[serial]
fn resume_dead_suspended_task_respawns() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("late-exit", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    let (w, pid) = worker_of(&d, &t);

    // 挂起（真 worker 冻结）
    d.api(Method::TaskPause, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(d.task_state(&t), WorkerState::Suspended);
    // 注入迟到的 WorkerExit：Suspended 早退，但 metas/pidfile 已清
    d.feed(CoreMsg::WorkerExit(WorkerExit {
        worker: maestro_protocol::WorkerId::new(w.clone()),
        task: t.clone(),
        exit_code: Some(0),
        stderr_tail: String::new(),
    }));
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(d.task_state(&t), WorkerState::Suspended);
    // 清理冻结的真进程（其真实退出消息走同一早退路径，无害）
    pgid_sandbox::kill_group(pid);

    // 手动 resume：旧 bug = Resumed 指向死 worker（无进程 Working）
    d.api(
        Method::TaskResume,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert_eq!(
        d.task_state(&t),
        WorkerState::Working,
        "resume 后应 Working"
    );
    let (w2, _) = worker_of(&d, &t);
    assert_ne!(w2, w, "应重拉新 worker 而非沿用死 worker");
    // 新 worker 是真进程（对它 cancel 应能成功）
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}

/// resume_all 对「worker 已死的 EmergencyStop 挂起」必须重拉进程
#[test]
#[serial]
fn resume_all_respawns_dead_emergency_suspended() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("dead-during-freeze", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    let (w, pid) = worker_of(&d, &t);

    // 急停（真 worker 冻结 + Suspended(EmergencyStop)）
    d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert_eq!(d.task_state(&t), WorkerState::Suspended);
    // 注入迟到的 WorkerExit
    d.feed(CoreMsg::WorkerExit(WorkerExit {
        worker: maestro_protocol::WorkerId::new(w.clone()),
        task: t.clone(),
        exit_code: None,
        stderr_tail: String::new(),
    }));
    std::thread::sleep(std::time::Duration::from_millis(200));
    pgid_sandbox::kill_group(pid);

    // resume_all：旧 bug = metas 已空，任务被完全跳过 → 永久 Suspended
    d.api(Method::ServerResumeAll, serde_json::json!({}));
    assert!(
        d.wait_state(&t, WorkerState::Working, 5000),
        "resume_all 应重拉死掉的急停任务，实际: {:?}",
        d.task_state(&t)
    );
    let (w2, _) = worker_of(&d, &t);
    assert_ne!(w2, w, "应有新 worker");
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}
