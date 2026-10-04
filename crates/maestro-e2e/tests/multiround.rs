#![cfg(unix)]

//! 0.15 多轮驱动 Worker 模式 e2e（rounder + mock CLI）：
//! - U4 验收：任务运行中注入轻推 → 影响下一轮输出（P0 验收项）
//! - 会话续接：daemon kill -9 恢复后 rounder 从上一完成轮续跑

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;

/// rounder 二进制路径（workspace 共享 target）

/// 任务级 rounder 状态目录（R36：.maestro/<task_id>/，跨任务会话隔离）
fn task_state_dir(work: &std::path::Path, task: &TaskId) -> std::path::PathBuf {
    work.join(".maestro").join(task.as_str())
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
    let rounds_path = task_state_dir(&work, &t).join("rounds.jsonl");
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
    let rounds = std::fs::read_to_string(task_state_dir(&work, &t).join("rounds.jsonl")).unwrap();
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
            if task_state_dir(&work, &t).join("session").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            task_state_dir(&work, &t).join("session").exists(),
            "session 应已建立"
        );
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
    let rounds = std::fs::read_to_string(task_state_dir(&work, &t).join("rounds.jsonl")).unwrap();
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
    let rounds_path = task_state_dir(&work, &t).join("rounds.jsonl");
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
    assert!(task_state_dir(&work1, &t1).join("rounds.jsonl").exists());
    assert!(task_state_dir(&work2, &t2).join("rounds.jsonl").exists());
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
    assert!(task_state_dir(&work, &t).join("session").exists());
    assert!(task_state_dir(&work, &t).join("rounds.jsonl").exists());

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

/// 混沌⑧ 费用对账（R34，T4 增量对账）：CLI 自报 total_cost_usd 与
/// daemon 牌价计费比对 —— 自洽轮静默；虚报轮（98% 漂移 > 25% 阈）发
/// CostDrift(Warning) 事件且不影响入账金额（daemon 计费为准）
#[test]
#[serial]
fn cost_reconciliation_drift_event() {
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    // 两任务独立 workdir（避免 session/out.txt 串扰）
    let (_r1, work_a) = git_repo();
    let (_r2, work_b) = git_repo();
    // 自洽：mock usage(IN=200 OUT=40 CR=120 CC=30) 按牌价 = 1¢ = $0.01 → 零漂移
    let a = d.create_task_with_prompt("cost-a", "费用自洽", &work_a);
    assert!(
        d.wait_state(&a, WorkerState::Done, 15000),
        "A 实际: {:?}",
        d.task_state(&a)
    );
    // 虚报：同 usage 自报 $0.50 → |1-50|/50 = 98% > 25% → CostDrift
    let b = d.create_task_with_prompt("cost-b", "费用虚报", &work_b);
    assert!(
        d.wait_state(&b, WorkerState::Done, 15000),
        "B 实际: {:?}",
        d.task_state(&b)
    );

    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let all = store.replay_all();
    let drifts: Vec<&maestro_protocol::events::Envelope> = all
        .iter()
        .filter(|e| matches!(&e.event, maestro_protocol::events::Event::CostDrift { .. }))
        .collect();
    // 自洽任务零事件
    assert!(
        drifts.iter().all(|e| !matches!(
            &e.event,
            maestro_protocol::events::Event::CostDrift { task, .. } if task == &a
        )),
        "自洽轮不得发 CostDrift: {:?}",
        drifts.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
    // 虚报任务恰好一条，数值与口径正确
    let b_drifts: Vec<&maestro_protocol::events::Envelope> = drifts
        .iter()
        .copied()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::CostDrift { task, .. } if task == &b
            )
        })
        .collect();
    assert_eq!(b_drifts.len(), 1, "虚报轮应发一条 CostDrift");
    match &b_drifts[0].event {
        maestro_protocol::events::Event::CostDrift {
            ledger_cents,
            cli_cents,
            model,
            ..
        } => {
            assert_eq!(*ledger_cents, 1, "daemon 侧计费（89850 mc → 1¢）");
            assert_eq!(*cli_cents, 50, "CLI 自报 $0.50 → 50¢");
            assert_eq!(model, "claude-sonnet-4");
            assert_eq!(
                b_drifts[0].priority,
                maestro_protocol::types::Priority::Warning
            );
        }
        _ => unreachable!(),
    }
    // 入账金额不受虚报影响（daemon 计费为准）：B 的账本 cents 与 A 相同
    for t in [&a, &b] {
        let v = d.api(
            Method::TaskLedger,
            serde_json::json!({ "task": t.as_str() }),
        );
        assert_eq!(v["actual_cost_cents"].as_u64(), Some(1), "{v}");
    }
}

/// 混沌⑨ CLI schema 漂移检测（R35，R28 调研待办⑤）：坏 schema
/// （system 缺 session_id + result 缺 usage）→ rounder exit 3 带人话诊断 →
/// 任务 Failed 且诊断进事件流，不静默空转烧轮数
#[test]
#[serial]
fn bad_cli_schema_fails_with_diagnosis() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    let t = d.create_task_with_prompt("bad-schema", "坏schema", &work);
    assert!(
        d.wait_state(&t, WorkerState::Failed, 15000),
        "实际: {:?}",
        d.task_state(&t)
    );

    // 诊断人话进事件流（TaskFailed.error 含 stderr 尾部）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let failed = store
        .replay_all()
        .iter()
        .filter_map(|e| match &e.event {
            maestro_protocol::events::Event::TaskFailed { task, error, .. } if task == &t => {
                Some(error.clone())
            }
            _ => None,
        })
        .next_back()
        .expect("应有 TaskFailed 事件");
    assert!(failed.contains("schema"), "诊断应含 schema 字样: {failed}");
    assert!(
        failed.contains("session_id") && failed.contains("usage"),
        "两条漂移都应报出: {failed}"
    );
}

/// 混沌⑩ 跨任务会话隔离（R36）：同 workdir 串行两任务，B 不得继承 A 的
/// session——否则 B 的上下文被 A 污染（跨任务信息泄漏）。证伪素材：mock 的
/// FACT 召回需「通读」前置（会话内 read 标记），A 通读后 B 首轮问 FACT_1：
/// 继承 → 召回成功（错）；隔离 → 「我不知道」（对）
#[test]
#[serial]
fn second_task_in_same_workdir_gets_fresh_session() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    // A：通读建立上下文（会话内 read 标记），经「结束」轻推收敛
    let a = d.create_task_with_prompt("task-a", "通读 README 与 src", &work);
    assert!(d.wait_state(&a, WorkerState::Working, 5000));
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": a.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&a, WorkerState::Done, 20000),
        "A 实际: {:?}",
        d.task_state(&a)
    );

    // B：同 workdir（互斥保证串行，A 已 Done 释放）。首轮问 FACT_1
    let b = d.create_task_with_prompt("task-b", "FACT_1 在哪个文件？", &work);
    assert!(d.wait_state(&b, WorkerState::Working, 5000));
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": b.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&b, WorkerState::Done, 20000),
        "B 实际: {:?}",
        d.task_state(&b)
    );

    // B 首轮 answer 必须是「无上下文」——继承 A 的 session 才能召回
    // （R36 隔离后 B 的轮账在独立目录，不受 A 污染）
    let rounds_path = task_state_dir(&work, &b).join("rounds.jsonl");
    let content = std::fs::read_to_string(&rounds_path).unwrap();
    let b_first = content
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|r| r["prompt"].as_str().unwrap_or("").contains("FACT_1"))
        .expect("B 的 FACT_1 轮应在轮账");
    let ans = b_first["answer"].as_str().unwrap();
    assert!(
        ans.contains("我不知道"),
        "B 首轮不应召回 FACT_1（跨任务 session 泄漏！answer: {ans}）"
    );
}

/// 混沌⑪ 上下文轮转（R37，P1 待办②机制层）：会话线性膨胀（200 token/轮）
/// → 轮边界占用 ≥ MAESTRO_CONTEXT_LIMIT(600) → 注入 /compact 指令轮 →
/// 压缩后 usage 回落 → 继续正常轮。全程同一 session（compact 不换 id）
#[test]
#[serial]
fn context_rotation_compacts_when_over_limit() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_CONTEXT_LIMIT".into(), "600".into())],
    );

    let t = d.create_task_with_prompt("ctx", "上下文增长：每轮记录新发现", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 等压缩轮出现（膨胀 3 轮到 600 → 第 4 轮压缩）
    let rounds_path = task_state_dir(&work, &t).join("rounds.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut compact_rec: Option<serde_json::Value> = None;
    while std::time::Instant::now() < deadline {
        if let Ok(c) = std::fs::read_to_string(&rounds_path) {
            compact_rec = c
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .find(|r| r["compacted"].as_bool().unwrap_or(false));
            if compact_rec.is_some() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let compact = compact_rec.expect("30s 内应出现压缩轮（膨胀 3 轮 ≥600）");

    // 压缩轮断言：prompt 是 /compact 指令、usage 回落（CTX 截为 1 行 → 200）
    let p = compact["prompt"].as_str().unwrap();
    assert!(p.contains("/compact"), "压缩轮 prompt 应为压缩指令: {p}");
    assert_eq!(compact["answer"].as_str().unwrap(), "已压缩上下文");
    // 压缩前恰 3 轮膨胀（200/轮：轮3 达 600 触发），压缩是第 4 轮
    let all = std::fs::read_to_string(&rounds_path).unwrap();
    let recs: Vec<serde_json::Value> = all
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let growth_rounds: Vec<u64> = recs
        .iter()
        .take_while(|r| !r["compacted"].as_bool().unwrap_or(false))
        .map(|r| r["round"].as_u64().unwrap())
        .collect();
    assert_eq!(
        growth_rounds,
        vec![1, 2, 3],
        "膨胀 3 轮（600=200×3）后应触发压缩"
    );

    // 任务经「结束」轻推收敛（压缩后正常轮继续，不是死循环压缩）
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "压缩后应继续任务并收敛，实际: {:?}",
        d.task_state(&t)
    );

    // 全程同一 session（compact 不换 session id —— 调研结论的机制验证）
    let sids: Vec<&str> = recs
        .iter()
        .map(|r| r["session_id"].as_str().unwrap())
        .collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "压缩前后应同一 session: {sids:?}"
    );

    // daemon 感知：ContextCompacted 事件（U3 叙事「上下文已压缩」）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let compacted_events = store
        .replay_all()
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::ContextCompacted { task, .. } if task == &t
            )
        })
        .count();
    assert_eq!(compacted_events, 1, "应恰有一条 ContextCompacted 事件");
    // 账本汇总（R40 轮转感知）：ledger API 带 compactions 计数
    let lv = d.api(
        Method::TaskLedger,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert_eq!(lv["compactions"].as_u64(), Some(1), "{lv}");
}

/// 混沌⑫ 轻推与压缩竞争（R39）：超限轮的边界同时有轻推 —— 轻推优先占
/// 下一轮，压缩意图作废（不得把轻推轮记账成压缩轮）；轻推轮后占用仍
/// 超限 → 压缩在再下一轮重新触发
#[test]
#[serial]
fn steering_beats_compact_and_compact_retriggers() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_CONTEXT_LIMIT".into(), "600".into())],
    );

    let t = d.create_task_with_prompt("ctx-race", "上下文增长：每轮记录新发现", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    let rounds_path = task_state_dir(&work, &t).join("rounds.jsonl");
    let read_rounds = || -> Vec<serde_json::Value> {
        std::fs::read_to_string(&rounds_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    };
    let wait_round = |n: usize| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if read_rounds().len() >= n {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("20s 内未到轮 {n}: {:?}", read_rounds());
    };

    // 轮 2 落盘后注入轻推 → 轮 3 边界 poll 拿到；轮 3（继续指令轮，
    // CTX 3 行 = 600 达阈值）结束时检测超限但下一轮已被轻推占用
    wait_round(2);
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "补充指示：输出加前缀 [Z]" }),
    );

    // 轮 3 = 继续指令轮（600 达阈）；轮 4 = 轻推轮（[Z]）
    wait_round(4);
    // 轮 5：轻推轮后仍超限（CTX 4 行 = 800）→ 压缩重新触发
    wait_round(6);

    // 轮 4（轻推轮）不得记账成压缩轮 —— prompt 与 compacted 标记一致
    let recs = read_rounds();
    let steer_round = recs
        .iter()
        .find(|r| r["prompt"].as_str().unwrap_or("").contains("[Z]"))
        .expect("应存在 [Z] 轻推轮");
    assert!(
        !steer_round["compacted"].as_bool().unwrap_or(false),
        "轻推轮不得记为压缩轮（R39 bug）: {steer_round}"
    );
    // 压缩轮存在且 prompt 真的是 /compact 指令（在轻推轮之后）
    let steer_no = steer_round["round"].as_u64().unwrap();
    let compact_rounds: Vec<&serde_json::Value> = recs
        .iter()
        .filter(|r| r["compacted"].as_bool().unwrap_or(false))
        .collect();
    assert!(!compact_rounds.is_empty(), "轻推轮后应重新触发压缩");
    for c in &compact_rounds {
        assert!(
            c["prompt"].as_str().unwrap_or("").contains("/compact"),
            "压缩轮 prompt 必须是压缩指令: {c}"
        );
        assert!(
            c["round"].as_u64().unwrap() > steer_no,
            "压缩应发生在轻推轮之后: {c}"
        );
    }

    // 收敛
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "实际: {:?}",
        d.task_state(&t)
    );
    // 压缩轮数量与 ContextCompacted 事件一致（每个压缩轮恰一条）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let events = store
        .replay_all()
        .iter()
        .filter(|e| {
            matches!(
                &e.event,
                maestro_protocol::events::Event::ContextCompacted { task, .. } if task == &t
            )
        })
        .count();
    assert_eq!(
        events,
        compact_rounds.len(),
        "ContextCompacted 事件数应等于压缩轮数"
    );
}

/// 混沌⑬ 轮数预算耗尽（R42）：MAX_ROUNDS 到顶但无完成信号 —— 不得标
/// Done（半途任务过验收门是语义缺陷）；应 blocked(rounds_exhausted) 进
/// 收件箱；resume → requeue → respawn 从 session 续接跑完
#[test]
#[serial]
fn rounds_exhausted_blocks_then_resume_continues() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![
            ("MAESTRO_MAX_ROUNDS".into(), "2".into()),
            // 压缩不干扰本测试：阈值拉满
            ("MAESTRO_CONTEXT_LIMIT".into(), "99999999".into()),
        ],
    );

    // 「上下文增长」永不输出 DONE → 2 轮预算耗尽
    let t = d.create_task_with_prompt("re", "上下文增长：每轮记录新发现", &work);
    assert!(
        d.wait_state(&t, WorkerState::Blocked, 20000),
        "轮数耗尽应 Blocked，实际: {:?}",
        d.task_state(&t)
    );
    let g = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(g["blocked_kind"], "rounds_exhausted", "{g}");
    // 收件箱含该项（U9：Critical 进收件箱，CLI 给续跑/放弃建议）
    let inbox = d.api(Method::InboxList, serde_json::json!({}));
    assert!(
        inbox["items"].as_array().is_some_and(|a| a
            .iter()
            .any(|it| it["task"] == *t.as_str() && it["kind"] == "rounds_exhausted")),
        "收件箱应含 rounds_exhausted 项: {inbox}"
    );

    // resume → requeue → respawn（新 rounder 从 .maestro/<task>/session 续接）
    d.api(
        Method::TaskResume,
        serde_json::json!({ "task": t.as_str() }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Working, 10000),
        "resume 应重拉 worker，实际: {:?}",
        d.task_state(&t)
    );
    // 注入「结束」→ 续接的 rounder 轮边界 poll → DONE 收敛
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "续跑后应完成，实际: {:?}",
        d.task_state(&t)
    );

    // 跨 respawn 同一 session（续接而非重开）
    let rounds = std::fs::read_to_string(task_state_dir(&work, &t).join("rounds.jsonl")).unwrap();
    let recs: Vec<serde_json::Value> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(recs.len() >= 3, "至少 2+1 轮: {recs:?}");
    let sids: Vec<&str> = recs
        .iter()
        .map(|r| r["session_id"].as_str().unwrap())
        .collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "respawn 后应续接同一 session: {sids:?}"
    );
}

/// B1 叙事降级模板（R46）：task get 的 narrative 字段 —— 事件流单遍聚合出
/// 「第 N 轮：Read×N，累计 …¢，耗时 …｜最近：…」一句话进度（LLM 缺席的降级路径）
#[test]
#[serial]
fn task_get_narrative_line() {
    let (_repo, work) = git_repo();
    let tmp = tempfile::tempdir().unwrap();
    let cli = tmp.path().join("mock-claude");
    write_mock_cli(&cli, &tmp.path().join("state"));

    let rb = rounder_bin();
    let d = TestDaemon::start(&rb, &["--", cli.to_str().unwrap()]);

    // 零轮任务（确定性）：坏schema 首轮即 exit 3 → TaskFailed，无任何
    // RoundProgress → 降级模板空态分支。用独立 workdir 避开 R13 workdir 互斥
    let idle_work = tmp.path().join("idle-work");
    std::fs::create_dir_all(&idle_work).unwrap();
    let t0 = d.create_task_with_prompt("narrative-idle", "坏schema", &idle_work);
    assert!(
        d.wait_state(&t0, WorkerState::Failed, 5000),
        "坏schema 应终态 Failed，实际: {:?}",
        d.task_state(&t0)
    );
    let v0 = d.api(
        Method::TaskGet,
        serde_json::json!({ "task": t0.as_str() }),
    );
    assert_eq!(v0["narrative"].as_str().unwrap(), "尚未开始（无轮账）");

    // 多轮任务：注入「结束」快速收敛 → narrative 聚合轮数/工具/成本/摘要
    let t = d.create_task("narrative", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "实际: {:?}",
        d.task_state(&t)
    );

    let rounds = std::fs::read_to_string(task_state_dir(&work, &t).join("rounds.jsonl")).unwrap();
    let n = rounds
        .lines()
        .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
        .count();
    assert!(n >= 1, "至少 1 轮: {rounds}");

    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    // R48 修复：TaskRecord.round 随 RoundProgress 推进（此前恒 0）
    assert_eq!(
        v["round"].as_u64().unwrap_or(0),
        n as u64,
        "task get 的 round 应等于完成轮数: {v}"
    );
    let narrative = v["narrative"].as_str().unwrap_or_default();
    assert!(
        narrative.starts_with(&format!("第 {n} 轮")),
        "narrative 应以真实轮数开头: {narrative}"
    );
    assert!(
        narrative.contains(&format!("Read×{n}")),
        "mock 每轮一次 Read，工具频次应等于轮数: {narrative}"
    );
    assert!(narrative.contains("累计"), "成本应入叙事: {narrative}");
    assert!(
        narrative.contains("｜最近："),
        "最近轮摘要应收尾: {narrative}"
    );
}

/// 混沌⑭（R49）：过期/孤儿 rounder 自杀 —— poll -403（易主）→ exit 6 只烧
/// 1 轮；daemon 失联容忍 1 轮（重启窗口）、连续 2 轮 → exit 7。
/// 此前被取代/失联的 rounder 会空转到 MAX_ROUNDS 烧穿预算（pre-pidfile
/// 孤儿连重启后的 reap 都扫不到，只能靠自杀止损）
#[test]
#[serial]
fn stale_or_orphan_rounder_self_exits() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    // 真任务保持 Working（占住所有权；泛化轮不收敛）
    let t = d.create_task("ghost-guard", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    let socket = d.data_dir.join("maestro.api.sock");
    let run_ghost = |task: &str, worker: &str, socket: &std::path::Path, dir: &std::path::Path| {
        std::process::Command::new(rounder_bin())
            .args(["--", cli.to_str().unwrap()])
            .env("MAESTRO_TASK_ID", task)
            .env("MAESTRO_WORKER_ID", worker)
            .env("MAESTRO_SOCKET_PATH", socket)
            .env("MAESTRO_PROMPT", "p")
            .env("MAESTRO_MAX_ROUNDS", "5")
            .env("MAESTRO_ROUND_GAP_MS", "50")
            .current_dir(dir)
            .output()
            .expect("spawn ghost rounder")
    };
    let ghost_rounds = |dir: &std::path::Path| -> usize {
        std::fs::read_to_string(dir.join(".maestro").join(t.as_str()).join("rounds.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
            .count()
    };

    // 场景 A：易主 —— 假 worker id → poll -403 → 立即退场（exit 6）
    let ghost_a = tempfile::tempdir().unwrap();
    let out = run_ghost(t.as_str(), "w-ghost", &socket, ghost_a.path());
    assert_eq!(
        out.status.code(),
        Some(6),
        "易主应 exit 6: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("STALE_WORKER"),
        "stderr 应带人话标记"
    );
    assert_eq!(ghost_rounds(ghost_a.path()), 1, "易主自杀只烧 1 轮");
    assert_eq!(
        d.task_state(&t),
        WorkerState::Working,
        "真任务不受 ghost 影响"
    );

    // 场景 B：daemon 失联 —— socket 不存在；第 1 次 Connect 失败容忍
    // （可能是 daemon 重启窗口），第 2 次自杀（exit 7）
    let ghost_b = tempfile::tempdir().unwrap();
    let dead_sock = ghost_b.path().join("no-such.sock");
    let out = run_ghost(t.as_str(), "w-ghost2", &dead_sock, ghost_b.path());
    assert_eq!(
        out.status.code(),
        Some(7),
        "失联 2 轮应 exit 7: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("DAEMON_LOST"));
    assert_eq!(
        ghost_rounds(ghost_b.path()),
        2,
        "第 1 次失联应容忍，第 2 次才自杀"
    );

    // 收尾：真任务正常完成（ghost 未污染其会话/轮账）
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "实际: {:?}",
        d.task_state(&t)
    );
}

/// R50 状态目录 GC 零惊喜回归：daemon 重启后，近期终态任务（≤ 保留额度）
/// 的 .maestro/<task>/ 轮账/session 必须还在（GC 只清超额旧目录）
#[test]
#[serial]
fn restart_gc_keeps_recent_terminal_state_dirs() {
    let data_tmp = tempfile::tempdir().unwrap();
    let data_dir = data_tmp.path().to_path_buf();
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let rb = rounder_bin();

    // 实例 1：任务跑到 Done（终态，session + 轮账落盘）
    let t = {
        let d = TestDaemon::start_with(&rb, &["--", cli.to_str().unwrap()], Some(data_dir.clone()));
        let t = d.create_task_with_prompt("gc-keep", "结束", &work);
        assert!(
            d.wait_state(&t, WorkerState::Done, 20000),
            "实际: {:?}",
            d.task_state(&t)
        );
        t
    }; // drop = kill -9

    // 实例 2：恢复 → GC 跑一遍（终态 ×1 ≤ KEEP，零删除）
    let d2 = TestDaemon::start_with(&rb, &["--", cli.to_str().unwrap()], Some(data_dir));
    assert_eq!(d2.task_state(&t), WorkerState::Done, "重放后仍 Done");
    assert!(
        task_state_dir(&work, &t).join("rounds.jsonl").exists(),
        "近期终态目录不得被 GC 删除"
    );
    assert!(
        task_state_dir(&work, &t).join("session").exists(),
        "session 文件（审计/续接凭据）保留"
    );
}