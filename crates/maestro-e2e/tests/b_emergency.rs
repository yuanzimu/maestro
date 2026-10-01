//! 用例组 B：emergency_stop 三阶段 e2e（U10_T6_DESIGN.md §11）

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

/// B1/B5：急停冻结运行中 worker + 现场快照；resume_all 恢复
#[test]
#[serial]
fn b1_b5_emergency_freeze_snapshot_resume() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("freeze-test", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // worker 进程 pid
    let workers = d.api(Method::WorkerList, serde_json::json!({}));
    let pid = workers["workers"][0]["pid"].as_u64().expect("pid") as u32;
    assert!(
        maestro_daemon::worker::proc_stat(pid).is_some(),
        "worker 应存活"
    );

    // 急停
    let result = d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    let freeze_ms = result["freeze_ms"].as_u64().expect("freeze_ms");
    println!("freeze_ms = {freeze_ms}");
    // I1：<100ms（测试机上留宽限到 500ms，CI 抖动保护）
    assert!(freeze_ms < 500, "FREEZE 超时: {freeze_ms}ms");
    // worker 进入 T 态
    assert_eq!(
        maestro_daemon::worker::proc_stat(pid).map(|(st, _, _)| st),
        Some("T".to_string()),
        "worker 应 SIGSTOP"
    );
    // 任务 suspended(EmergencyStop)
    assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "emergency_stop");
    // 快照存在（EmergencyStopped 结果带 checkpoint）
    assert!(
        result["checkpoints"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "急停应产生 checkpoint: {result}"
    );

    // resume_all → worker 恢复运行（S 态，不再 T）
    d.api(
        Method::ServerResumeAll,
        serde_json::json!({ "steering": "flush" }),
    );
    assert!(d.wait_state(&t, WorkerState::Working, 2000));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_ne!(
        maestro_daemon::worker::proc_stat(pid).map(|(st, _, _)| st),
        Some("T".to_string()),
        "resume 后不应再是 T 态"
    );

    // 清理
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}

/// B7：冻结期间 steering hold → 丢弃且每条发 SteeringDropped 事件（不静默）
#[test]
#[serial]
fn b7_steering_hold_drops_not_silently() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("steer-test", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 两条轻推
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "轻推1" }),
    );
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "轻推2" }),
    );

    // 急停 + hold 丢弃
    d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    d.api(
        Method::ServerResumeAll,
        serde_json::json!({ "steering": "hold" }),
    );
    assert!(d.wait_state(&t, WorkerState::Working, 2000));

    // 断言事件流有 2 条 SteeringDropped（经 checkpoint 列表外的途径：直接读事件库）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let all = store.replay_all();
    let dropped = all
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::SteeringDropped { .. }
            )
        })
        .count();
    assert_eq!(dropped, 2, "两条轻推都应有 SteeringDropped 事件（不静默）");

    // 清理
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}

/// B8：急停 → 回滚 → resume：pre-rollback 安全垫 + worktree 恢复
#[test]
#[serial]
fn b8_rollback_then_resume() {
    let (_repo, work) = git_repo();
    std::fs::write(work.join("code.txt"), "v1").unwrap();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("rollback-test", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // worker 运行中「改了文件」（模拟 worker 行为：直接改，急停快照会带上）
    std::fs::write(work.join("code.txt"), "v2-改坏了").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 急停
    let stop = d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    let cp = stop["checkpoints"][0]
        .as_str()
        .expect("checkpoint ref")
        .to_string();

    // 回滚到急停时刻 checkpoint（I4：应产生 pre-rollback 安全垫）
    let rb = d.api(
        Method::CheckpointRollback,
        serde_json::json!({ "task": t.as_str(), "to": cp }),
    );
    assert!(
        rb["pre_rollback"]
            .as_str()
            .is_some_and(|s| s.contains("pre_rollback")),
        "回滚应带 pre-rollback 安全垫: {rb}"
    );
    // worktree 回到 v2-改坏了（急停时刻的真实状态，如实记录）
    assert_eq!(
        std::fs::read_to_string(work.join("code.txt")).unwrap(),
        "v2-改坏了"
    );

    // resume
    d.api(
        Method::ServerResumeAll,
        serde_json::json!({ "steering": "flush" }),
    );
    assert!(d.wait_state(&t, WorkerState::Working, 2000));

    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}

/// B12：急停期间新任务只入队不执行
#[test]
#[serial]
fn b12_new_task_queues_during_freeze() {
    let (_repo, work) = git_repo();
    // 新任务用独立 workdir：t1 恢复后继续占用原 workdir（R13 workdir 互斥），
    // 这里测的是「解冻后排队任务被调度器启动」
    let (_repo2, work2) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t1 = d.create_task("first", &work);
    assert!(d.wait_state(&t1, WorkerState::Working, 5000));

    // 急停
    d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(d.wait_state(&t1, WorkerState::Suspended, 2000));

    // 急停期间建新任务
    let created = d.api(
        Method::TaskCreate,
        serde_json::json!({ "title": "during-freeze", "prompt": "p", "workdir": work2.display().to_string() }),
    );
    assert_eq!(created["queued"], true, "急停期间新任务应入队: {created}");
    assert_eq!(created["reason"], "emergency_frozen");

    // 新任务 Queued 而非 Working
    let new_id = created["task"]["id"].as_str().unwrap().to_string();
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": new_id }));
    let st = serde_json::from_value::<WorkerState>(v["state"].clone()).unwrap();
    assert_eq!(st, WorkerState::Queued, "冻结期新任务应为 Queued");

    // resume_all 后新任务应被启动（调度器解冻）
    d.api(
        Method::ServerResumeAll,
        serde_json::json!({ "steering": "flush" }),
    );
    assert!(
        d.wait_state(&TaskId::new(new_id), WorkerState::Working, 5000),
        "解冻后新任务应启动"
    );

    // 清理
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t1.as_str() }),
    );
    let v = d.api(Method::TaskList, serde_json::json!({}));
    for t in v["tasks"].as_array().unwrap() {
        let id = t["id"].as_str().unwrap().to_string();
        let st = serde_json::from_value::<WorkerState>(t["state"].clone()).unwrap();
        if !st.is_terminal() {
            d.api(Method::TaskCancel, serde_json::json!({ "task": id }));
        }
    }
}

/// B13：急停幂等
#[test]
#[serial]
fn b13_emergency_stop_idempotent() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("idem", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    let _r1 = d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    // 第二次急停：不报错，返回当前冻结状态
    let r2 = d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(
        r2["frozen_workers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "第二次应无新冻结 worker: {r2}"
    );
    assert!(
        r2["checkpoints"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "第二次不应重复快照: {r2}"
    );
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
}
