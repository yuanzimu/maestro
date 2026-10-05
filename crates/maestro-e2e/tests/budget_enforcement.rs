#![cfg(unix)]

//! v2.5 Sprint A：硬预算闸门端到端强制（决策 26，设计借鉴 Foreman）。
//!
//! 场景：一个永不自停的多轮任务（增长模式），花费硬顶设 1 cent ——
//! 第 1 轮累计 1¢（未超），第 2 轮累计 2¢ → daemon 必须在轮账处理中
//! SIGSTOP 冻结 rounder 并挂起为 BudgetExceeded（仅手动恢复），且事件流
//! 留有叙事快照。验证「硬」预算真的会停车，而非只报告。

use maestro_daemon::budget::TaskBudget;
use maestro_daemon::gateway::GatewayConfig;
use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;

#[test]
#[serial]
fn hard_budget_cost_exceeded_freezes_and_suspends() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));

    // 网关禁用（本测试只验预算）；花费硬顶 1 cent，不限时长
    let gateway = GatewayConfig {
        url: String::new(),
        token: String::new(),
    };
    let budget = TaskBudget {
        max_cost_cents: Some(1),
        max_wall_ms: None,
    };
    let d = TestDaemon::start_with_gateway_budget(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        gateway,
        budget,
    );

    // 增长模式任务：rounder 持续循环、永不输出 DONE，逐轮累计花费
    let t = d.create_task_with_prompt("budget-cost", "上下文增长：每轮记录新发现", &work);
    assert!(
        d.wait_state(&t, WorkerState::Suspended, 15000),
        "花费超硬顶应 Suspended，实际: {:?}",
        d.task_state(&t)
    );

    // 挂起原因 = budget_exceeded（手动恢复语义）
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert_eq!(v["suspend_reason"], "budget_exceeded", "{v}");
    assert_eq!(v["state"], "suspended");

    // 至少跑到第 2 轮才触发（第 1 轮 1¢ 未超）
    assert!(v["round"].as_u64().unwrap_or(0) >= 2, "应累计 ≥2 轮: {v}");

    // 叙事快照记录预算超限原因（事件流可审计，U3 呈现）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let narr = store
        .replay_all()
        .iter()
        .filter_map(|e| match &e.event {
            maestro_protocol::events::Event::NarrativeSnapshot { task: tt, milestone, .. }
                if tt == &t =>
            {
                Some(milestone.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        narr.iter().any(|m| m.contains("预算超限") && m.contains("预算")),
        "应有预算超限叙事快照: {narr:?}"
    );

    // rounder 已被 SIGSTOP 冻结：确认其进程组处于停止态（现场保留非杀死）
    let workers = d.api(Method::WorkerList, serde_json::json!({}));
    let cur = workers["workers"][0]["id"].as_str().unwrap().to_string();
    let meta_pid = workers["workers"][0]["pid"].as_u64().unwrap_or(0);
    assert!(meta_pid > 0, "worker 记录应带 pid: {workers}");
    // 跨平台读进程状态：`ps -o state= -p PID`（Linux/macOS 均支持，
    // 原实现读 /proc/{pid}/stat —— macOS 无 /proc，恒报 None）
    let state_out = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &meta_pid.to_string()])
        .output()
        .unwrap_or_else(|_| panic!("ps 调用失败"));
    let state_ch = String::from_utf8_lossy(&state_out.stdout)
        .trim()
        .chars()
        .next();
    assert_eq!(
        state_ch,
        Some('T'),
        "worker 应被 SIGSTOP 停止（T 态），实际: {state_ch:?}"
    );
    let _ = cur;
}

#[test]
#[serial]
fn budget_not_exceeded_task_runs_unaffected() {
    // 对照组：宽松预算（$100）下，普通「结束」任务应正常 Done，不被预算干扰
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));

    let gateway = GatewayConfig {
        url: String::new(),
        token: String::new(),
    };
    let budget = TaskBudget {
        max_cost_cents: Some(10_000),
        max_wall_ms: None,
    };
    let d = TestDaemon::start_with_gateway_budget(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        gateway,
        budget,
    );
    let t = d.create_task_with_prompt("budget-ok", "结束", &work);
    assert!(
        d.wait_state(&t, WorkerState::Done, 15000),
        "预算内任务应正常 Done，实际: {:?}",
        d.task_state(&t)
    );
}
