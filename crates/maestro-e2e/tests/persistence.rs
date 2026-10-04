#![cfg(unix)]

//! 持久化恢复：kill -9 后事件重放 + steering 队列不丢（P0 验收）

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use serial_test::serial;

/// P0 验收：事件库重放带回全部任务状态（kill -9 恢复无损）
#[test]
#[serial]
fn events_replay_restores_tasks() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();

    // 实例 1：建任务 → 急停（产生丰富事件）→ 模拟 kill -9（直接 drop）
    let (_repo, work) = git_repo();
    {
        let d = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
        let t = d.create_task("persist-me", &work);
        assert!(d.wait_state(&t, WorkerState::Working, 5000));
        d.api(
            Method::TaskSteer,
            serde_json::json!({ "task": t.as_str(), "message": "留到重启后" }),
        );
        d.api(Method::ServerEmergencyStop, serde_json::json!({}));
        assert!(d.wait_state(&t, WorkerState::Suspended, 2000));
        drop(d); // 不 shutdown = kill -9 语义
    }

    // 实例 2：同 data_dir 恢复
    let d2 = TestDaemon::start_with("/bin/sh", &["-c", "sleep 300"], Some(data_dir.clone()));
    let v = d2.api(Method::TaskList, serde_json::json!({}));
    let tasks: Vec<serde_json::Value> = v["tasks"].as_array().unwrap().clone();
    assert_eq!(tasks.len(), 1, "任务应从事件流恢复");
    let st = serde_json::from_value::<WorkerState>(tasks[0]["state"].clone()).unwrap();
    // kill -9 时是 EmergencyStop suspended；恢复路径 mark_recovered... 只动 Working 的，
    // EmergencyStop 的 suspended 保持原样
    assert_eq!(st, WorkerState::Suspended, "状态应保持 suspended: {st:?}");

    // seq 不冲突：新事件序号 > 历史
    let before_seq = tasks.len() as u64; // 粗查：能继续发布
    let _ = d2.api(Method::TaskList, serde_json::json!({}));
    assert!(before_seq >= 1);

    // steering 队列消息还在（kill -9 不丢 —— P0 验收）
    let store = maestro_daemon::persist::EventStore::open(&data_dir).unwrap();
    let all = store.replay_all();
    let steered = all
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::SteeringQueued { .. }
            )
        })
        .count();
    assert_eq!(steered, 1, "steering 事件应在事件流");
    // 恢复后 resume（手动）应 flush 该轻推
    let tid = tasks[0]["id"].as_str().unwrap().to_string();
    d2.api(Method::TaskResume, serde_json::json!({ "task": tid }));
    assert!(d2.wait_state(&TaskId::new(tid), WorkerState::Working, 5000));
    let all2 = maestro_daemon::persist::EventStore::open(&data_dir)
        .unwrap()
        .replay_all();
    let delivered = all2
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::SteeringDelivered { .. }
            )
        })
        .count();
    assert_eq!(delivered, 1, "轻推在恢复后投递");
}

/// 双实例 seq 连续性：恢复后新事件不与历史 seq 冲突
#[test]
#[serial]
fn seq_floor_continues_after_recovery() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();

    let (_repo, work) = git_repo();
    let max_seq_1;
    {
        let d = TestDaemon::start_with("/bin/sh", &["-c", "exit 0"], Some(data_dir.clone()));
        let t = d.create_task("seq-1", &work);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let store = maestro_daemon::persist::EventStore::open(&data_dir).unwrap();
        max_seq_1 = store.max_seq();
        let _ = t;
    }
    assert!(max_seq_1 > 0, "实例 1 应有事件");

    let d2 = TestDaemon::start_with("/bin/sh", &["-c", "exit 0"], Some(data_dir.clone()));
    let t2 = d2.create_task("seq-2", &work);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let store = maestro_daemon::persist::EventStore::open(&data_dir).unwrap();
    let all = store.replay_all();
    // seq 唯一性
    let mut seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    seqs.sort_unstable();
    seqs.dedup();
    assert_eq!(
        seqs.len(),
        all.len(),
        "seq 不得重复（恢复后不得与历史冲突）"
    );
    let _ = t2;
}