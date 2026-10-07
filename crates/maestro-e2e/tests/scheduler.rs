#![cfg(unix)]

//! R13 队列调度器 e2e：并发上限 + FIFO 补位 + 取消释放槽位。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

/// max=1：任务串行执行；第二个任务先入队，第一个完成后补位启动
#[test]
#[serial]
fn max_one_serializes_execution() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start_limited(
        "/bin/sh",
        &[
            "-c",
            "sleep 0.3; echo $MAESTRO_TASK_ID >> order.txt; exit 0",
        ],
        None,
        1,
    );
    let a = d.create_task("a", &work);
    let b = d.create_task("b", &work);

    // 槽位被 A 占满：B 应为 Queued
    assert!(d.wait_state(&a, WorkerState::Working, 5000), "A 先启动");
    assert_eq!(
        d.task_state(&b),
        WorkerState::Queued,
        "并发满时 B 应入队而非启动"
    );

    // A 完成（读回校验过）→ B 补位
    assert!(d.wait_state(&b, WorkerState::Done, 8000), "B 应补位完成");
    assert_eq!(d.task_state(&a), WorkerState::Done);
    let order = std::fs::read_to_string(work.join("order.txt")).unwrap();
    assert_eq!(
        order.trim(),
        format!("{a}\n{b}"),
        "执行顺序应为 A→B（FIFO）: {order}"
    );
}

/// 取消运行中任务 → 槽位立即释放 → 排队任务补位
#[test]
#[serial]
fn cancel_frees_slot_for_next() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start_limited("/bin/sh", &["-c", "sleep 300"], None, 1);
    let a = d.create_task("blocker", &work);
    let b = d.create_task("waiter", &work);
    assert!(d.wait_state(&a, WorkerState::Working, 5000));
    assert_eq!(d.task_state(&b), WorkerState::Queued);

    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": a.as_str() }),
    );
    assert!(d.wait_state(&a, WorkerState::Cancelled, 5000));
    assert!(
        d.wait_state(&b, WorkerState::Working, 5000),
        "取消应释放槽位，B 补位启动，实际: {:?}",
        d.task_state(&b)
    );
}

/// FIFO 确定性：3 个任务按创建顺序执行（虚拟时钟下 created_at 相同，
/// 靠 queue_seq 消歧）
#[test]
#[serial]
fn fifo_order_is_deterministic() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start_limited(
        "/bin/sh",
        &["-c", "echo $MAESTRO_TASK_ID >> order.txt; exit 0"],
        None,
        1,
    );
    let ids: Vec<TaskId> = (0..3)
        .map(|i| d.create_task(&format!("t{i}"), &work))
        .collect();
    for t in &ids {
        assert!(d.wait_state(t, WorkerState::Done, 10000), "全部应完成");
    }
    let order = std::fs::read_to_string(work.join("order.txt")).unwrap();
    let want: String = ids.iter().map(|i| format!("{i}\n")).collect();
    assert_eq!(order, want, "严格 FIFO（queue_seq 消歧）: {order}");
}

/// 急停冻结期入队的任务：resume_all 后按槽位补位（B12 × R13 联动）
#[test]
#[serial]
fn frozen_queue_resumes_within_slot_limit() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start_limited("/bin/sh", &["-c", "sleep 300"], None, 1);
    let a = d.create_task("frozen-runner", &work);
    assert!(d.wait_state(&a, WorkerState::Working, 5000));

    // 急停：A 冻结
    d.api(Method::ServerEmergencyStop, serde_json::json!({}));
    assert!(d.wait_state(&a, WorkerState::Suspended, 3000));
    // 冻结期建 B：入队
    let b = d.create_task("frozen-queued", &work);
    assert_eq!(d.task_state(&b), WorkerState::Queued, "冻结期只入队");

    // 解冻：A 恢复继续占槽，B 仍排队
    d.api(Method::ServerResumeAll, serde_json::json!({}));
    assert!(d.wait_state(&a, WorkerState::Working, 5000), "A 恢复执行");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        d.task_state(&b),
        WorkerState::Queued,
        "A 仍占槽（SIGSTOP 恢复后继续跑），B 应继续等"
    );
}

/// workdir 互斥：同 workdir 第二个任务入队；不同 workdir 的任务可并行
#[test]
#[serial]
fn same_workdir_exclusive_but_other_workdir_runs() {
    let (_repo, work1) = git_repo();
    let (_repo2, work2) = git_repo();
    // 槽位 2：同 workdir 互斥不占用第二个槽
    let d = TestDaemon::start_limited("/bin/sh", &["-c", "sleep 300"], None, 2);
    let a = d.create_task("w1-a", &work1);
    assert!(d.wait_state(&a, WorkerState::Working, 5000));

    // 同 workdir 的 B：有空闲槽也必须入队（互斥）
    let b = d.create_task("w1-b", &work1);
    assert_eq!(
        d.task_state(&b),
        WorkerState::Queued,
        "同 workdir 被占用应入队（互斥），即使槽位有空"
    );

    // 不同 workdir 的 C：应立即启动
    let c = d.create_task("w2-c", &work2);
    assert!(
        d.wait_state(&c, WorkerState::Working, 5000),
        "不同 workdir 不受互斥影响"
    );

    // A 取消 → 释放 workdir1 → B 补位
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": a.as_str() }),
    );
    assert!(d.wait_state(&b, WorkerState::Working, 5000), "B 应补位");
}
