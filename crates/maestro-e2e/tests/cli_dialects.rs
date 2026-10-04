#![cfg(unix)]

//! R2/R51 方言矩阵 e2e：codex / gemini 方言全链（参数构造 → 事件解析 →
//! session 续接 → 轮账计量 → 收敛）。mock CLI 按参数形态自动切换输出格式
//! （exec → codex；-p 且无 --max-turns → gemini），usage 数值三方言同源 ——
//! 「同一任务换个 CLI，账本口径不变」是本组测试的核心断言。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use maestro_protocol::types::*;
use maestro_testkit::r3::write_mock_cli;
use serial_test::serial;

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

/// 读取轮账记录（JSONL）
fn round_records(work: &std::path::Path, task: &TaskId) -> Vec<serde_json::Value> {
    std::fs::read_to_string(task_state_dir(work, task).join("rounds.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// codex 方言全链：多轮 + `exec resume <tid>` 续接 + 拆桶计量对齐 claude 口径
/// （mock 合成 input_tokens = IN+CR，解析侧拆回 in/CR 两桶）
#[test]
#[serial]
fn codex_dialect_full_loop() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));

    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_CLI_DIALECT".into(), "codex".into())],
    );
    let t = d.create_task("codex-mr", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));

    // 多轮后收敛
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    if !d.wait_state(&t, WorkerState::Done, 20000) {
        // 诊断：读事件库拿 TaskFailed 的 exit/stderr
        if let Ok(store) = maestro_daemon::persist::EventStore::open(&d.data_dir) {
            for env in store.replay_all() {
                if let maestro_protocol::events::Event::TaskFailed { error, .. } = env.event {
                    panic!("codex 任务未完成，TaskFailed: {error}");
                }
            }
        }
        panic!("codex 任务未完成，实际: {:?}（无 TaskFailed 事件）", d.task_state(&t));
    }

    let recs = round_records(&work, &t);
    assert!(recs.len() >= 2, "多轮: {recs:?}");
    // 全程同一 thread id（codex resume 续接）
    let sids: Vec<&str> = recs.iter().map(|r| r["session_id"].as_str().unwrap()).collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "codex exec resume 应续接同一 thread: {sids:?}"
    );
    // 拆桶计量对齐：每轮 in=200 out=40 cache_read=120（与 claude 方言同任务同账）
    let v = d.api(Method::TaskLedger, serde_json::json!({ "task": t.as_str() }));
    let n = recs.len() as u64;
    assert_eq!(v["input_tokens"].as_u64().unwrap(), 200 * n, "账本: {v}");
    assert_eq!(v["output_tokens"].as_u64().unwrap(), 40 * n, "账本: {v}");
    assert_eq!(v["cache_read_tokens"].as_u64().unwrap(), 120 * n, "拆桶后 cache_read: {v}");
    // 工具叙事：codex 的 command_execution 计入 tools
    let g = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert!(
        g["narrative"].as_str().unwrap_or_default().contains("command_execution"),
        "narrative 应含 codex 工具名: {g}"
    );
}

/// gemini 方言全链：退出码 53（轮次上限）不杀任务 —— 轮循环继续；
/// `--resume` 续接 + stats 计量（cached 合并值 → cache_read）
#[test]
#[serial]
fn gemini_dialect_full_loop_with_exit53() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));

    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_CLI_DIALECT".into(), "gemini".into())],
    );
    // 首轮即触发 53（轮次上限）→ rounder 视为正常轮出口继续循环
    let t = d.create_task_with_prompt("gemini-53", "gemini轮次上限：继续探索", &work);
    assert!(d.wait_state(&t, WorkerState::Working, 5000));
    // 53 轮之后注入结束信号收敛（证明轮循环活着）
    d.api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "结束" }),
    );
    assert!(
        d.wait_state(&t, WorkerState::Done, 20000),
        "53 不应杀死任务，实际: {:?}",
        d.task_state(&t)
    );

    let recs = round_records(&work, &t);
    assert!(recs.len() >= 2, "53 轮之后还有续轮: {recs:?}");
    let sids: Vec<&str> = recs.iter().map(|r| r["session_id"].as_str().unwrap()).collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "gemini --resume 应续接同一 session: {sids:?}"
    );
    // 计量：stats.input_tokens=200/output=40/cached=120（每轮）
    let v = d.api(Method::TaskLedger, serde_json::json!({ "task": t.as_str() }));
    let n = recs.len() as u64;
    assert_eq!(v["input_tokens"].as_u64().unwrap(), 200 * n, "账本: {v}");
    assert_eq!(v["output_tokens"].as_u64().unwrap(), 40 * n, "账本: {v}");
    assert_eq!(v["cache_read_tokens"].as_u64().unwrap(), 120 * n, "cached 合并值入 cache_read: {v}");
    // 工具叙事：gemini 的 tool_use 计入
    let g = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert!(
        g["narrative"].as_str().unwrap_or_default().contains("Read×"),
        "narrative 应含 gemini 工具: {g}"
    );
}

/// 方言不匹配的漂移防护（R51）：gemini 方言 + 只会吐 claude 格式的 worker
/// → 缺 init/result stats 要素 → exit 3 → TaskFailed（不静默空转烧预算）
#[test]
#[serial]
fn gemini_dialect_schema_mismatch_fails_loudly() {
    let (_repo, work) = git_repo();
    let tmp = tempfile::tempdir().unwrap();
    // 手写「永远输出 claude stream-json」的假 CLI
    let fake = tmp.path().join("fake-claude-only");
    std::fs::write(
        &fake,
        "#!/bin/bash\nprintf '%s\\n' '{\"type\":\"system\",\"session_id\":\"s1\"}' '{\"type\":\"result\",\"result\":\"ok\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", fake.to_str().unwrap()],
        vec![("MAESTRO_CLI_DIALECT".into(), "gemini".into())],
    );
    let t = d.create_task("gemini-mismatch", &work);
    assert!(
        d.wait_state(&t, WorkerState::Failed, 10000),
        "schema 漂移应 exit 3 → Failed，实际: {:?}",
        d.task_state(&t)
    );
    // 诊断可见：TaskFailed 的 error 应带人话漂移描述（事件库直读）
    let store = maestro_daemon::persist::EventStore::open(&d.data_dir).unwrap();
    let errors: Vec<String> = store
        .replay_all()
        .into_iter()
        .filter_map(|env| match env.event {
            maestro_protocol::events::Event::TaskFailed { error, .. } => Some(error),
            _ => None,
        })
        .collect();
    assert!(
        errors.iter().any(|e| e.contains("schema 漂移")),
        "TaskFailed 应带漂移诊断: {errors:?}"
    );
}

/// OpenCode 方言全链（R52）：`run -s <sid>` 续接 + step_finish 计量 +
/// 自报 cost（四方言唯一）—— 自报费用经 daemon 对账路径（模型不在牌价
/// 表 → cents 留空，对账跳过，不阻塞计量）
#[test]
#[serial]
fn opencode_dialect_full_loop() {
    let (_repo, work) = git_repo();
    let mock_tmp = tempfile::tempdir().unwrap();
    let cli = mock_tmp.path().join("mock-claude");
    write_mock_cli(&cli, &mock_tmp.path().join("state"));

    let d = TestDaemon::start_with_worker_env(
        &rounder_bin(),
        &["--", cli.to_str().unwrap()],
        vec![("MAESTRO_CLI_DIALECT".into(), "opencode".into())],
    );
    let t = d.create_task("oc-mr", &work);
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

    let recs = round_records(&work, &t);
    assert!(recs.len() >= 2, "多轮: {recs:?}");
    // 全程同一 sessionID（run -s 续接）
    let sids: Vec<&str> = recs.iter().map(|r| r["session_id"].as_str().unwrap()).collect();
    assert!(
        sids.windows(2).all(|w| w[0] == w[1]),
        "opencode run -s 应续接同一 session: {sids:?}"
    );
    // 计量：text/tool/step_finish 解析（无 cache 桶 → cache_read 0）
    let v = d.api(Method::TaskLedger, serde_json::json!({ "task": t.as_str() }));
    let n = recs.len() as u64;
    assert_eq!(v["input_tokens"].as_u64().unwrap(), 200 * n, "账本: {v}");
    assert_eq!(v["output_tokens"].as_u64().unwrap(), 40 * n, "账本: {v}");
    assert_eq!(v["cache_read_tokens"].as_u64().unwrap_or(0), 0, "opencode 无 cache 细分: {v}");
    // 工具叙事：opencode 的 read 工具计入
    let g = d.api(Method::TaskGet, serde_json::json!({ "task": t.as_str() }));
    assert!(
        g["narrative"].as_str().unwrap_or_default().contains("read×"),
        "narrative 应含 opencode 工具: {g}"
    );
}