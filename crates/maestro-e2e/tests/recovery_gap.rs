//! R11 审计回归：崩溃重启后，自动恢复退避中的任务必须继续调度。
//! 之前：recover() 不重排 auto_resume_candidates → NetworkLost 挂起任务
//! 在 daemon 重启后永久滞留 Suspended。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

#[test]
#[serial]
fn recover_reschedules_auto_resume_backoff() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();
    // workdir 必须跨两个 daemon 实例存活（任务记录里存的是绝对路径）
    let (_repo, work) = git_repo();

    // 实例 1：断连挂起（NetworkLost，auto 策略），退避 timer 虚拟等待中
    {
        let d = TestDaemon::start_with(
            "/bin/sh",
            &["-c", "sleep 1; echo 'network error' >&2; exit 1"],
            Some(data_dir.clone()),
        );
        let t = d.create_task("resume-after-crash", &work);
        assert!(d.wait_state(&t, WorkerState::Suspended, 8000));
        // 不推进时钟：退避 timer 未到点就崩溃
    }

    // 实例 2：同 data_dir 恢复 —— recover() 应重排自动恢复
    let d2 = TestDaemon::start_with(
        "/bin/sh",
        &["-c", "echo ok > done.txt; exit 0"],
        Some(data_dir.clone()),
    );
    let v = d2.api(Method::TaskList, serde_json::json!({}));
    let tid = v["tasks"][0]["id"].as_str().unwrap().to_string();
    let t = TaskId::new(tid);

    // 推进虚拟时钟越过第一档退避（30s）→ TimerFired → 重启 worker
    d2.clock.advance_secs(31);
    assert!(
        d2.wait_state(&t, WorkerState::Done, 8000),
        "重启后自动恢复应继续（worker 重跑并完成），实际: {:?}",
        d2.task_state(&t)
    );
    assert!(std::fs::read_to_string(work.join("done.txt")).is_ok());
}

/// 崩溃重启后 UserPause 挂起的任务不得被自动唤醒（仅手动）
#[test]
#[serial]
fn recover_does_not_wake_user_pause() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();

    {
        let (_repo, work) = git_repo();
        let d = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
        let t = d.create_task("stay-paused", &work);
        assert!(d.wait_state(&t, WorkerState::Working, 5000));
        d.api(Method::TaskPause, serde_json::json!({ "task": t.as_str() }));
        assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
    }

    let d2 = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
    let v = d2.api(Method::TaskList, serde_json::json!({}));
    let tid = v["tasks"][0]["id"].as_str().unwrap().to_string();
    let t = TaskId::new(tid);
    d2.clock.advance_secs(3600);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        d2.task_state(&t),
        WorkerState::Suspended,
        "UserPause 不得自动恢复"
    );
}
