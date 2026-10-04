#![cfg(unix)]

//! 应用测试场景矩阵（R55）：以「用户旅程」组织的全链验收 —— 每个场景
//! 模拟一类真实使用方式，断言用户可见的行为（而不是内部状态）。
//!
//! S1 首次使用：doctor 语义 → 建任务 → 看进度 → 收到完成
//! S2 长任务监控：网页指挥台看实时进度 + 中途补充指示改变方向
//! S3 出岔子：轮数预算耗尽 → 界面给「恢复/放弃」两条路 → 恢复续跑成功
//! S4 网络抖动：任务挂起 → 自动恢复 → 用户无感
//! S5 多任务并行：网页列表一目了然（每任务独立会话/账本）
//! S6 查账：一个任务花了多少 token/钱，一句话回答
//! S7 Web UI 基础设施：页面可达 + API 形状 + daemon 停止时不白屏
//!
//! HTTP 断言用裸 TcpStream（零新依赖）；UI server 经 maestro_client::ui::spawn
//! 起在随机端口 —— 与浏览器行为完全同路径。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;
use std::io::{Read, Write};
use std::net::TcpStream;

fn rounder_bin() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/debug/maestro-rounder"
    )
    .to_string()
}

fn task_state_dir(work: &std::path::Path, task: &TaskId) -> std::path::PathBuf {
    work.join(".maestro").join(task.as_str())
}

// ---------------------------------------------------------------------------
// HTTP 帮手（裸 socket：GET/POST → (status, body)）
// ---------------------------------------------------------------------------

fn http_get(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect ui");
    write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
    read_response(&mut s)
}

fn http_post_json(port: u16, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect ui");
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
    read_response(&mut s)
}

fn read_response(s: &mut TcpStream) -> (u16, String) {
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    // body = 头部空行后
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// 起 daemon + UI，返回 (daemon, ui_port)
fn start_with_ui(worker_program: &str, worker_args: &[&str]) -> (TestDaemon, u16) {
    let d = TestDaemon::start(worker_program, worker_args);
    let client = maestro_client::MaestroClient::new(&d.data_dir);
    let port = maestro_client::ui::spawn(client).expect("spawn ui");
    (d, port)
}

// ---------------------------------------------------------------------------
// S1 首次使用：建任务 → 进度可见 → 完成
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s1_first_task_lifecycle_visible() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let (d, port) = start_with_ui(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    // 建任务（用户唯一的动作）
    let t = d.create_task_with_prompt("修登录 bug", "通读项目后输出结论", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 进度可见：task get 的 narrative 说出「它在干什么」
    let v = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    let narrative = v["narrative"].as_str().unwrap_or_default();
    assert!(!narrative.is_empty(), "运行中应有进度行: {v}");
    assert!(
        narrative.contains("第") || narrative.contains("尚未"),
        "进度行应含轮次信息: {narrative}"
    );

    // 完成
    d.api(Method::TaskSteer, serde_json::json!({ "task": t.as_str(), "message": "结束" }));
    assert!(d.wait_state(&t, WorkerState::Done, 20000), "实际: {:?}", d.task_state(&t));

    // 界面同步终态
    let (code, body) = http_get(port, "/api/tasks");
    assert_eq!(code, 200);
    let tasks: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(tasks["tasks"][0]["state"], "done", "界面应显示完成: {body}");
    assert!(
        tasks["tasks"][0]["narrative"].as_str().is_some_and(|n| n.contains("轮")),
        "界面任务卡应带进度叙事: {body}"
    );
}

// ---------------------------------------------------------------------------
// S2 长任务监控：网页看实时进度 + 中途改方向
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s2_web_dashboard_steer_mid_flight() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let (d, port) = start_with_ui(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    let t = d.create_task("重构模块", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 事件流可见（UI 右栏的数据源）：创建事件（重放保证）与轮进度（首轮完成）
    // 都用等待式断言 —— UI 订阅握手是异步的（毫秒级），立即断言是竞态
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let (code, body) = http_get(port, "/api/events?from=0");
        assert_eq!(code, 200);
        if body.contains("task_created") && body.contains("round_progress") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "15s 内应见到创建+轮进度事件: {body}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // 网页按钮背后的 API：轻推（改变方向）
    let (code, body) = http_post_json(
        port,
        "/api/steer",
        &format!("{{\"task\":\"{}\",\"message\":\"补充指示：从现在起每句输出加前缀 [W]\"}}", t.as_str()),
    );
    assert_eq!(code, 200, "{body}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["queued"], true);

    // 注入轮真正执行后再发结束（时序确定化：与 U4 测试同法）
    let rounds_path = task_state_dir(&work, &t).join("rounds.jsonl");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if let Ok(c) = std::fs::read_to_string(&rounds_path) {
            if c.contains("补充指示") {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (code, body) = http_post_json(
        port,
        "/api/steer",
        &format!("{{\"task\":\"{}\",\"message\":\"结束\"}}", t.as_str()),
    );
    assert_eq!(code, 200, "{body}");
    assert!(d.wait_state(&t, WorkerState::Done, 20000), "实际: {:?}", d.task_state(&t));

    // 轻推影响了下一轮输出（用户看得见方向被改了）
    let rounds = std::fs::read_to_string(&rounds_path).unwrap();
    let recs: Vec<serde_json::Value> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let inj = recs
        .iter()
        .position(|r| r["prompt"].as_str().unwrap_or("").contains("补充指示"))
        .expect("应有注入轮");
    assert!(
        recs[inj + 1..].iter().any(|r| r["answer"].as_str().unwrap_or("").contains("[W]")),
        "注入后轮输出应带 [W] 前缀"
    );
}

// ---------------------------------------------------------------------------
// S3 出岔子：预算耗尽 → 界面给两条路 → 恢复成功
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s3_rounds_exhausted_ui_recovery_path() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    // 2 轮预算（快速耗尽）
    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_MAX_ROUNDS".into(), "2".into())],
    );
    let client = maestro_client::MaestroClient::new(&d.data_dir);
    let port = maestro_client::ui::spawn(client).unwrap();

    let t = d.create_task("长活", &work);
    assert!(
        d.wait_state(&t, WorkerState::Blocked, 20000),
        "预算耗尽应 blocked，实际: {:?}",
        d.task_state(&t)
    );

    // 界面明示「待处理」+ 分类（用户知道该干什么）
    let (code, body) = http_get(port, "/api/tasks");
    assert_eq!(code, 200);
    let tasks: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(tasks["tasks"][0]["state"], "blocked");
    assert_eq!(tasks["tasks"][0]["blocked_kind"], "rounds_exhausted", "{body}");

    // 收件箱 API（行动建议数据源）
    let (code, body) = http_get(port, "/api/inbox");
    assert_eq!(code, 200);
    let inbox: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        inbox["items"].as_array().is_some_and(|a| !a.is_empty()),
        "收件箱应有项: {body}"
    );

    // 用户选择「恢复」→ 续接跑完
    let (code, body) = http_post_json(port, "/api/task/resume", &format!("{{\"task\":\"{}\"}}", t.as_str()));
    assert_eq!(code, 200, "{body}");
    d.api(Method::TaskSteer, serde_json::json!({ "task": t.as_str(), "message": "结束" }));
    assert!(d.wait_state(&t, WorkerState::Done, 20000), "实际: {:?}", d.task_state(&t));

    // 续接不换会话
    let rounds = std::fs::read_to_string(task_state_dir(&work, &t).join("rounds.jsonl")).unwrap();
    let sids: Vec<String> = rounds
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|r| r["session_id"].as_str().unwrap().to_string())
        .collect();
    assert!(sids.windows(2).all(|w| w[0] == w[1]), "恢复应续接: {sids:?}");
}

// ---------------------------------------------------------------------------
// S4 网络抖动：挂起 → 自动恢复（用户无感）
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s4_disconnect_auto_recovers_via_ui_view() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let (d, port) = start_with_ui(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    let t = d.create_task("联网查资料", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 注入网络故障 → 挂起（界面黄点，用户被告知原因）
    d.api(Method::TaskSteer, serde_json::json!({ "task": t.as_str(), "message": "网络故障模拟：本轮触发连接失败" }));
    assert!(d.wait_state(&t, WorkerState::Suspended, 10000));
    let (code, body) = http_get(port, "/api/tasks");
    assert_eq!(code, 200);
    let tasks: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(tasks["tasks"][0]["state"], "suspended", "{body}");
    assert_eq!(tasks["tasks"][0]["suspend_reason"], "network_lost", "界面应说原因: {body}");

    // 自动恢复（推时钟过退避）→ 完成 —— 用户全程没动手
    d.clock.advance_secs(31);
    assert!(d.wait_state(&t, WorkerState::Working, 5000), "退避到点自动恢复");
    d.api(Method::TaskSteer, serde_json::json!({ "task": t.as_str(), "message": "结束" }));
    assert!(d.wait_state(&t, WorkerState::Done, 20000), "实际: {:?}", d.task_state(&t));
}

// ---------------------------------------------------------------------------
// S5 多任务并行：列表一目了然、互不串扰
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s5_multi_task_dashboard_overview() {
    // TempDir 必须保活（R44 幽灵目录教训：drop 即删 workdir）
    let keeps: Vec<tempfile::TempDir> = (0..3).map(|_| git_repo().0).collect();
    let repos: Vec<std::path::PathBuf> = keeps.iter().map(|t| t.path().to_path_buf()).collect();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let (d, port) = start_with_ui(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    // 1 个完成 + 1 个失败 + 1 个进行中（用户视角的三色面板）
    let t_done = d.create_task_with_prompt("a-完成", "结束", &repos[0]);
    let t_fail = {
        let w = repos[1].join("idle");
        std::fs::create_dir_all(&w).unwrap();
        d.create_task_with_prompt("b-失败", "坏schema", &w)
    };
    let t_run = d.create_task("c-进行中", &repos[2]);
    assert!(d.wait_state(&t_done, WorkerState::Done, 20000));
    assert!(d.wait_state(&t_fail, WorkerState::Failed, 20000));
    assert!(d.wait_state(&t_run, WorkerState::Working, 5000));

    let (code, body) = http_get(port, "/api/tasks");
    assert_eq!(code, 200);
    let tasks: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arr = tasks["tasks"].as_array().unwrap();
    assert_eq!(arr.len(), 3, "列表应有三项: {body}");
    let states: Vec<&str> = arr.iter().map(|t| t["state"].as_str().unwrap()).collect();
    for want in ["done", "failed", "working"] {
        assert!(states.contains(&want), "应含 {want}: {states:?}");
    }

    // 单任务详情（点开一张卡）
    let (code, body) = http_get(port, &format!("/api/task?id={}", t_done.as_str()));
    assert_eq!(code, 200);
    let detail: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(detail["id"], t_done.as_str());
    assert!(detail["narrative"].as_str().is_some(), "详情应带进度: {body}");
}

// ---------------------------------------------------------------------------
// S6 查账：一句话回答「花了多少」
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s6_ledger_answers_cost_question() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let (d, _port) = start_with_ui(&rounder_bin(), &["--", cli.to_str().unwrap()]);

    let t = d.create_task_with_prompt("对账任务", "费用自洽", &work);
    assert!(d.wait_state(&t, WorkerState::Done, 20000), "实际: {:?}", d.task_state(&t));

    let v = d.api(Method::TaskLedger, serde_json::json!({ "task": t.as_str() }));
    let rounds = v["rounds"].as_u64().unwrap();
    assert!(rounds >= 1);
    // token 计量闭环：每轮 in=200 out=40
    assert_eq!(v["input_tokens"].as_u64().unwrap(), 200 * rounds, "{v}");
    assert_eq!(v["output_tokens"].as_u64().unwrap(), 40 * rounds, "{v}");
    // 费用两口径都有（actual vs 冷跑 counterfactual）
    assert!(v["actual_cost_cents"].as_u64().is_some(), "实付应计价: {v}");
    assert!(v["counterfactual_cost_cents"].as_u64().is_some(), "{v}");
    // 费用自洽（自报 $0.01 = 牌价 1¢）→ 无 CostDrift 事件
    let (code, body) = http_get(_port, "/api/events?from=0");
    assert_eq!(code, 200);
    assert!(!body.contains("cost_drift"), "对账一致不应有漂移事件: {body}");
}

// ---------------------------------------------------------------------------
// S7 Web UI 基础设施：页面可用 + daemon 停止不白屏
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn s7_ui_page_and_daemon_down_graceful() {
    // 先起 UI、daemon 不存在（用户先开界面的顺序）
    let tmp = tempfile::tempdir().unwrap();
    let client = maestro_client::MaestroClient::new(tmp.path());
    let port = maestro_client::ui::spawn(client).expect("ui 应能先于 daemon 启动");

    // 页面可达且是中文指挥台
    let (code, body) = http_get(port, "/");
    assert_eq!(code, 200);
    assert!(body.contains("Maestro 指挥台"), "应是指挥台页面");
    assert!(body.contains("任务"), "页面应有关键元素");
    assert!(body.contains("实时动态"));

    // daemon 不在 → API 优雅降级（502 JSON，前端据此显示「未连接」）
    let (code, body) = http_get(port, "/api/tasks");
    assert_eq!(code, 502, "daemon 缺席应 502: {body}");
    assert!(body.contains("error"), "{body}");

    // 事件端点空缓冲不崩（返回空数组）
    let (code, body) = http_get(port, "/api/events?from=0");
    assert_eq!(code, 200);
    assert!(body.contains("\"events\":[]"), "{body}");

    // 404 / 405
    let (code, _) = http_get(port, "/nope");
    assert_eq!(code, 404);
    let (code, _) = http_post_json(port, "/api/tasks", "{}");
    assert_eq!(code, 405);

    // daemon 起来后 UI 自动接上（缓冲线程重连）
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));
    let d = TestDaemon::start_with(&rounder_bin(), &["--", cli.to_str().unwrap()], Some(tmp.path().to_path_buf()));
    let t = d.create_task_with_prompt("后起任务", "结束", &work);
    assert!(d.wait_state(&t, WorkerState::Done, 20000));
    // 等缓冲线程的下一次重连周期（≤1s + 轮询余量）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    loop {
        let (code, body) = http_get(port, "/api/tasks");
        if code == 200 && body.contains("\"done\"") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "UI 应在 daemon 起后自动接上: {code} {body}");
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}