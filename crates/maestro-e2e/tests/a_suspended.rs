//! 用例组 A：suspended 状态机 e2e（U10_T6_DESIGN.md §10）
//! 运行环境：真实子进程 + MockClock 虚拟推进退避

use maestro_daemon::core::CoreMsg;
use maestro_daemon::worker::WorkerExit;
use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

/// A6：人为挂起（UserPause）后，网络恢复事件不得唤醒它（仅手动恢复）
#[test]
#[serial]
fn a6_user_pause_never_auto_resumes() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("pause-test", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000), "应先进入 Working");

    // 用户暂停
    d.api(Method::TaskPause, serde_json::json!({ "task": t.as_str() }));
    assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "user_pause");

    // 大幅推进虚拟时钟（模拟几小时过去）+ 喂网络恢复事件
    d.clock.advance_secs(3600 * 5);
    d.feed(CoreMsg::NetworkProbe(true));
    std::thread::sleep(std::time::Duration::from_millis(300));

    // 仍 Suspended（UserPause 仅手动）
    assert_eq!(d.task_state(&t), WorkerState::Suspended, "UserPause 不得自动恢复");

    // 手动恢复 → Working
    d.api(Method::TaskResume, serde_json::json!({ "task": t.as_str() }));
    assert!(d.wait_state(&t, WorkerState::Working, 2000), "手动恢复应生效");

    // 清理：取消任务
    d.api(Method::TaskCancel, serde_json::json!({ "task": t.as_str() }));
    assert!(d.wait_state(&t, WorkerState::Cancelled, 3000));
}

/// A10（e2e 部分）：CLI 退出 + 断连 stderr → Suspended(NetworkLost)，永不 Failed
#[test]
#[serial]
fn a10_disconnect_stderr_maps_to_suspended() {
    let (_repo, work) = git_repo();
    // worker 跑 2 秒后以断连错误退出
    let d = TestDaemon::start(
        "/bin/sh",
        &["-c", "sleep 1; echo 'Error: connection reset by peer' >&2; exit 1"],
    );
    let t = d.create_task("disc-test", &work);
    assert!(d.wait_state(&t, WorkerState::Suspended, 8000), "断连应映射 Suspended 而非 Failed");
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "network_lost", "断连原因: {v}");
    assert_ne!(d.task_state(&t), WorkerState::Failed);
}

/// 断连后（自动恢复策略）真实成功路径：探测恢复 → 自动 resume → 完成
#[test]
#[serial]
fn disconnect_auto_resumes_when_network_returns() {
    let (_repo, work) = git_repo();
    // 第一次跑 1 秒断连；daemon 会自动重排退避
    let d = TestDaemon::start(
        "/bin/sh",
        &["-c", "sleep 1; echo 'fetch failed: network error' >&2; exit 1"],
    );
    let t = d.create_task("auto-resume", &work);
    assert!(d.wait_state(&t, WorkerState::Suspended, 8000));
    // 网络恢复（v0: network_ok 默认 true，等待退避第一档 30s —— MockClock 推进唤醒 timer）
    d.clock.advance_secs(31);
    // timer 线程被唤醒 → CoreMsg::TimerFired → 自动 resume（重新 spawn worker）
    // 新 worker 又跑同样的脚本 → 又断连 → 再次 Suspended；断言它回到过 Working
    // （真实 --resume 续接在多轮驱动任务实现；此处验证调度闭环）
    let ok = d.wait_state(&t, WorkerState::Working, 5000);
    assert!(ok, "退避到点应自动恢复（重跑 worker）");
}

/// 孤儿清理（A8 e2e 部分）：Core 恢复时杀掉残留 worker 并标 suspended(DaemonCrash)
#[test]
#[serial]
fn recover_reaps_and_marks_daemon_crash() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();
    {
        let (_repo, work) = git_repo();
        let d = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
        let t = d.create_task("crash-test", &work);
        assert!(d.wait_state(&t, WorkerState::Working, 5000));
        // 模拟 daemon kill -9：直接丢掉实例（不 Shutdown），worker 留守
        drop(d);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // pidfile 应还在（worker 没被清理）
    assert!(
        !maestro_daemon::worker::scan_pidfiles(&data_dir.join("workers")).is_empty(),
        "kill -9 后 pidfile 应残留"
    );

    // 恢复：孤儿被杀 + 任务标 suspended(DaemonCrash)
    let (_repo2, work2) = git_repo();
    let d2 = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
    std::thread::sleep(std::time::Duration::from_millis(500));
    // pidfile 已清
    assert!(
        maestro_daemon::worker::scan_pidfiles(&data_dir.join("workers")).is_empty(),
        "恢复后 pidfile 应被清理"
    );
    // 任务状态：suspended(DaemonCrash)（经 API 查询 —— 恢复实例里 task 还在）
    let v = d2.api(Method::TaskList, serde_json::json!({}));
    let tasks = v["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty(), "事件重放应带回任务");
    let state = serde_json::from_value::<WorkerState>(tasks[0]["state"].clone()).unwrap();
    assert_eq!(state, WorkerState::Suspended, "DaemonCrash 应标 suspended: {state:?}");
    let _ = work2;
}
