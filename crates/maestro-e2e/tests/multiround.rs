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
    // 账本：轮数 > 1（真正多轮）+ token 计量闭环（每轮 mock IN=200 OUT=40）
    let v = d.api(
        Method::TaskLedger,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(v["rounds"].as_u64().unwrap() >= 2, "多轮任务轮数应 ≥2: {v}");
    let n_rounds = recs.len() as u64;
    assert_eq!(
        v["input_tokens"].as_u64().unwrap(),
        200 * n_rounds,
        "input_tokens 应按轮汇总（R24 计量闭环）: {v}"
    );
    assert_eq!(
        v["output_tokens"].as_u64().unwrap(),
        40 * n_rounds,
        "output_tokens 应按轮汇总: {v}"
    );
    // 计价闭环（R24）：默认模型 claude-sonnet-4 在牌价表 → cents 必有值；
    // actual ≤ counterfactual（cache 只省不亏）
    assert!(
        v["actual_cost_cents"].as_u64().unwrap_or(0)
            <= v["counterfactual_cost_cents"].as_u64().unwrap_or(0),
        "actual ≤ counterfactual（cache 计价）: {v}"
    );
    assert!(
        v["counterfactual_cost_cents"].as_u64().unwrap_or(0) > 0,
        "已知模型应完成计价（cents > 0）: {v}"
    );
    // 轮进度事件（U3 叙事）：每轮一条，带工具调用名与摘要
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let progresses = store
        .replay_all()
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::RoundProgress { task, tools_used, .. }
                    if task == &t && tools_used.iter().any(|t| t == "Read")
            )
        })
        .count();
    assert_eq!(
        progresses, n_rounds as usize,
        "每轮应有一条 RoundProgress 事件（含工具调用）: 实际 {progresses} / 轮 {n_rounds}"
    );
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

/// 混沌①：过期驱动进程防护 —— 旧 rounder 复活后不得拉走轻推/灌轮账（-403）
#[test]
#[serial]
fn stale_worker_cannot_poll_or_report() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);
    let t = d.create_task("stale-guard", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    let workers = d.api(Method::WorkerList, serde_json::json!({}));
    let cur = workers["workers"][0]["id"].as_str().unwrap().to_string();

    // 过期 worker 拉轻推 → -403（防抽走当前 worker 的消息）
    let e = d.try_api(
        Method::TaskSteerPoll,
        serde_json::json!({ "task": t.as_str(), "worker": "w-stale" }),
    );
    assert_eq!(e.unwrap_err().0, -403, "过期 worker 不得拉轻推");

    // 过期 worker 灌轮账 → -403（防账本被幽灵进程污染）
    let e = d.try_api(
        Method::TaskRoundReport,
        serde_json::json!({
            "task": t.as_str(), "worker": "w-stale", "round": 99,
            "input_tokens": 12345, "output_tokens": 6789
        }),
    );
    assert_eq!(e.unwrap_err().0, -403, "过期 worker 不得灌轮账");

    // 当前 worker：poll 空 + report 正常入账
    let v = d
        .try_api(
            Method::TaskSteerPoll,
            serde_json::json!({ "task": t.as_str(), "worker": cur }),
        )
        .unwrap();
    assert_eq!(v["messages"].as_array().map(|a| a.len()), Some(0));
    // 清理：注入结束收敛
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(d.wait_state(&t, WorkerState::Done, 20000));
}

/// 混沌②：任务终态时未投递轻推不静默丢失（SteeringDropped 事件可审计）。
/// 单发 worker 同样适用（投递语义 v0.15 的终态兜底）
#[test]
#[serial]
fn steering_dropped_not_silent_on_done() {
    let (_repo, work) = git_repo();
    // 500ms worker：确保轻推在退出前入队
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 0.5; echo hi > out.txt"]);
    let t = d.create_task("drop-on-done", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "现在就停下" }),
    );
    assert!(d.wait_state(&t, WorkerState::Done, 5000));

    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let dropped = store
        .replay_all()
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::SteeringDropped { task, .. }
                    if task == &t
            )
        })
        .count();
    assert_eq!(dropped, 1, "终态时未投递轻推应有 SteeringDropped（不静默）");
}

/// 混沌③：多轮任务中途断连（adapter::classify_exit 全链路）——
/// 内层 CLI 网络错误 → rounder 透传 → daemon Suspended(NetworkLost) →
/// 退避到点自动 respawn → session 续接 → 注入结束收敛
#[test]
#[serial]
fn disconnect_mid_task_recovers_session() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);
    let t = d.create_task("net-mr", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 等 round 1 落盘（保证故障指令注入到后续轮而非首轮）
    let rounds_path = work.join(".maestro/rounds.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(c) = std::fs::read_to_string(&rounds_path) {
            if c.lines().count() >= 1 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // 注入网络故障指令 → 下一轮 prompt 命中 mock 的网络故障分支 → exit 1
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "网络故障模拟：本轮触发连接失败" }),
    );
    // 断连映射 suspended（≠ failed）+ 退避调度
    assert!(
        d.wait_state(&t, WorkerState::Suspended, 10000),
        "断连应 Suspended，实际: {:?}",
        d.task_state(&t)
    );
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "network_lost");

    // 推进虚拟时钟过第一档退避（30s）→ 自动恢复 respawn rounder
    d.clock.advance_secs(31);
    assert!(
        d.wait_state(&t, WorkerState::Working, 5000),
        "退避到点应自动恢复"
    );

    // 续接 session 完成收敛
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "断连恢复后应续接完成，实际: {:?}",
        d.task_state(&t)
    );

    // 全程同一 session（断连前的轮 + 恢复后的轮）
    let rounds = std::fs::read_to_string(&rounds_path).unwrap();
    let sids: Vec<String> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|r| r["session_id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "断连恢复应续接同一 session: {sids:?}"
    );
}

/// 混沌④：两个多轮任务并发 —— 轮账/轻推/session 各自独立，互不串扰
#[test]
#[serial]
fn two_concurrent_multiround_tasks() {
    let (_repo1, work1) = git_repo();
    let (_repo2, work2) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    let t1 = d.create_task("mr-a", &work1);
    let t2 = d.create_task("mr-b", &work2);
    assert!(d.wait_state(&t1, WorkerState::Working, 5000));
    assert!(d.wait_state(&t2, WorkerState::Working, 5000));

    // 各自注入结束
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t1.as_str(), "message": "结束" }),
    );
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t2.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t1, WorkerState::Done, 20000),
        "{:?}",
        d.task_state(&t1)
    );
    assert!(
        d.wait_state(&t2, WorkerState::Done, 20000),
        "{:?}",
        d.task_state(&t2)
    );

    // 轮账各自落在各自 workdir
    assert!(work1.join(".maestro/rounds.jsonl").exists());
    assert!(work2.join(".maestro/rounds.jsonl").exists());
    // ledger 独立且都 ≥1 轮
    for t in [&t1, &t2] {
        let v = d.api(
            Method::TaskLedger,
            serde_json::json!({ "task": t.as_str() }),
        );
        assert!(v["rounds"].as_u64().unwrap() >= 1, "ledger: {v}");
    }
}

/// 混沌⑤：多轮任务的验收门 3 振出局 —— DONE 信号但无产物 → 反思回喂 →
/// respawn 续跑仍假完成 → blocked(AcceptanceFailed)。验证多轮模式的
/// 假完成检测闭环（.maestro/ 状态目录不算产物，门不误判）
#[test]
#[serial]
fn multiround_fake_completion_three_strikes() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);
    // prompt 含「无产物模拟」→ 每次 respawn 的第 1 轮即 DONE 且跳过产物落盘
    let v = d.api(
        Method::TaskCreate,
        serde_json::json!({
            "title": "mr-fake",
            "prompt": "无产物模拟：完成但无产物",
            "workdir": work.display().to_string()
        }),
    );
    let t = TaskId::new(v["task"]["id"].as_str().unwrap().to_string());
    assert!(
        d.wait_state(&t, WorkerState::Blocked, 15000),
        "3 次假完成应 blocked，实际: {:?}",
        d.task_state(&t)
    );
    let g = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(g["blocked_kind"], "acceptance_failed", "{g}");
    assert_eq!(g["acceptance_failures"], 3, "{g}");

    // .maestro 状态目录不算产物：session/轮账存在但门不误判
    assert!(work.join(".maestro/session").exists());
    assert!(work.join(".maestro/rounds.jsonl").exists());

    // 轮账：3 次 respawn 各 1 轮（假完成轮也计量 token —— 钱真花了）
    let l = d.api(
        Method::TaskLedger,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert_eq!(
        l["ledger_entries"].as_u64().unwrap(),
        3,
        "3 次假完成各应入账一轮: {l}"
    );
}

/// 混沌⑥：结构化过载错误（R28 调研落地）—— 错误经 stdout 的 result 事件
/// （is_error + api_error_status 529，非 stderr）→ rounder 转译退出 →
/// daemon 分类断连 → Suspended 自动恢复语义。与混沌③（stderr 路径）互补，
/// 防止过载烧光轮数预算
#[test]
#[serial]
fn structured_overload_maps_to_suspended() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);
    let t = d.create_task("overload-mr", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 注入过载模拟 → 下一轮 prompt 命中 mock 的结构化错误分支
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "过载模拟：触发服务端过载" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Suspended, 15000),
        "结构化过载应 Suspended（≠烧光轮数/≠Failed），实际: {:?}",
        d.task_state(&t)
    );
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "network_lost", "{v}");

    // 清理（Suspended 任务可 cancel）
    d.api(
        Method::TaskCancel,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(d.wait_state(&t, WorkerState::Cancelled, 5000));
}

/// 混沌⑦：steering at-least-once（R32）—— poll 未确认重投、ack 后清空、
/// 过期 worker ack 拒绝。模拟「rounder 在 poll 与下一轮之间被杀」的消息不丢
#[test]
#[serial]
fn steering_at_least_once_semantics() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);
    let t = d.create_task("alo", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    let workers = d.api(Method::WorkerList, serde_json::json!({}));
    let cur = workers["workers"][0]["id"].as_str().unwrap().to_string();

    // 轻推入队
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "补充指示：输出加前缀 [A]" }),
    );
    // 第一次 poll：取走（进 inflight）
    let v1 = d
        .try_api(
            Method::TaskSteerPoll,
            serde_json::json!({ "task": t.as_str(), "worker": cur }),
        )
        .unwrap();
    let seq1 = v1["messages"][0]["seq"].as_u64().unwrap();
    assert_eq!(v1["messages"].as_array().map(|a| a.len()), Some(1));

    // ⚠️ 不 ack —— 再次 poll 应重投同一条（rounder 被杀后新实例的视角）
    let v2 = d
        .try_api(
            Method::TaskSteerPoll,
            serde_json::json!({ "task": t.as_str(), "worker": cur }),
        )
        .unwrap();
    assert_eq!(
        v2["messages"][0]["seq"].as_u64().unwrap(),
        seq1,
        "未确认消息应重投（at-least-once）: {v2}"
    );

    // 过期 worker 不得 ack（防幽灵进程吞消息）
    let e = d.try_api(
        Method::TaskSteerAck,
        serde_json::json!({ "task": t.as_str(), "worker": "w-ghost", "seqs": [seq1] }),
    );
    assert_eq!(e.unwrap_err().0, -403, "过期 worker 不得 ack");

    // 当前 worker ack → 清空；再 poll 为空
    let acked = d
        .try_api(
            Method::TaskSteerAck,
            serde_json::json!({ "task": t.as_str(), "worker": cur, "seqs": [seq1] }),
        )
        .unwrap();
    assert_eq!(acked["acked"].as_u64().unwrap(), 1, "{acked}");
    let v3 = d
        .try_api(
            Method::TaskSteerPoll,
            serde_json::json!({ "task": t.as_str(), "worker": cur }),
        )
        .unwrap();
    assert_eq!(
        v3["messages"].as_array().map(|a| a.len()),
        Some(0),
        "ack 后不再投"
    );

    // 清理
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(d.wait_state(&t, WorkerState::Done, 20000));
}
