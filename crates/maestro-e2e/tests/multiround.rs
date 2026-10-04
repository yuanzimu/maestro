//! 0.15 多轮驱动 Worker 模式 e2e（rounder + mock CLI）：
//! - U4 验收：任务运行中注入轻推 → 影响下一轮输出（P0 验收项）
//! - 会话续接：daemon kill -9 恢复后 rounder 从上一完成轮续跑

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;

/// rounder 二进制路径（workspace 共享 target）
fn rounder_bin() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/debug/maestro-rounder"
    )
    .to_string()
}

#[test]
#[serial]
fn steering_injected_mid_task_changes_next_round() {
    let (_repo, work) = git_repo();
    let tmp = tempfile::tempdir().unwrap();
    let cli = tmp.path().join("mock-claude");
    write_mock_cli(&cli, &tmp.path().join("state"));

    let rb = rounder_bin();
    let d = TestDaemon::start(&rb, &["--", cli.to_str().unwrap()]);
    let t = d.create_task("multiround", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 运行中注入前缀指示。⚠️ 必须等该轮真正执行（rounds.jsonl 出现注入轮）
    // 再注入「结束」—— 两条轻推若在同一轮边界被一起 drain，拼接 prompt 命中
    // mock 的「补充指示」分支，结束信号永不触发（时序确定化）
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "补充指示：从现在起每句输出加前缀 [S]" }),
    );
    let rounds_path = work.join(".maestro/rounds.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut injected_seen = false;
    while std::time::Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string(&rounds_path) {
            if content.lines().any(|l| l.contains("补充指示")) {
                injected_seen = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(injected_seen, "注入轮应在 10s 内出现在轮账中");
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );

    // 任务收敛完成（DONE 信号 → rounder exit 0 → 验收门读回 out.txt 通过）
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "实际: {:?}",
        d.task_state(&t)
    );

    // 轮账断言：存在注入轮，且其后的轮输出带 [S] 前缀
    let rounds = std::fs::read_to_string(work.join(".maestro/rounds.jsonl")).unwrap();
    let recs: Vec<serde_json::Value> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(!recs.is_empty(), "轮账应有记录");
    let inj_idx = recs
        .iter()
        .position(|r| r["prompt"].as_str().unwrap_or("").contains("补充指示"))
        .expect("应存在注入轮");
    let later_prefixed = recs[inj_idx + 1..]
        .iter()
        .any(|r| r["answer"].as_str().unwrap_or("").contains("[S]"));
    assert!(
        later_prefixed,
        "注入后的轮应带前缀（U4：轻推影响下一轮）: {recs:?}"
    );
    // 同一 session 续接
    let sids: Vec<&str> = recs
        .iter()
        .map(|r| r["session_id"].as_str().unwrap())
        .collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "多轮应共用同一 session: {sids:?}"
    );
    // 账本：轮数 > 1（真正多轮）
    let v = d.api(
        Method::TaskLedger,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(v["rounds"].as_u64().unwrap() >= 2, "多轮任务轮数应 ≥2: {v}");
}

/// kill -9 daemon 后恢复：rounder 读 .maestro/session 续接同一会话完成
#[test]
#[serial]
fn crash_recovery_resumes_session() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let rb = rounder_bin();

    // 实例 1：任务跑起来（rounder 多轮循环中），立即 kill -9
    {
        let d = TestDaemon::start_with(&rb, &["--", cli.to_str().unwrap()], Some(data_dir.clone()));
        let t = d.create_task("crash-mr", &work);
        assert!(d.wait_state(&t, WorkerState::Working, 5000));
        // 等 round 1 落盘（session 建立）
        for _ in 0..50 {
            if work.join(".maestro/session").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(work.join(".maestro/session").exists(), "session 应已建立");
    } // drop = kill -9

    // 实例 2：恢复 → suspended(DaemonCrash) → 手动 resume → 续接完成
    let d2 = TestDaemon::start_with(&rb, &["--", cli.to_str().unwrap()], Some(data_dir.clone()));
    let v = d2.api(Method::TaskList, serde_json::json!({}));
    let tid = v["tasks"][0]["id"].as_str().unwrap().to_string();
    let t = TaskId::new(tid);
    assert_eq!(d2.task_state(&t), WorkerState::Suspended);

    d2.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    d2.api(
        Method::TaskResume,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(
        d2.wait_state(&t, WorkerState::Done, 20000),
        "恢复后应续接完成，实际: {:?}",
        d2.task_state(&t)
    );

    // 轮账：跨实例共用同一 session（续接而非重开）
    let rounds = std::fs::read_to_string(work.join(".maestro/rounds.jsonl")).unwrap();
    let recs: Vec<serde_json::Value> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let sids: Vec<&str> = recs
        .iter()
        .map(|r| r["session_id"].as_str().unwrap())
        .collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "崩溃恢复后应续接同一 session: {sids:?}"
    );
}
