#![cfg(unix)]

//! 0.11 安全基线 e2e：敏感 workdir / 任意 ref 回滚 / 超长输入在 API 层被拒。

use maestro_e2e::*;
use maestro_protocol::api::Method;
use serial_test::serial;

/// 敏感目录禁止作为 workdir；正常目录放行
#[test]
#[serial]
fn sensitive_workdir_rejected() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    // /etc：敏感
    let r = d.try_api(
        Method::TaskCreate,
        serde_json::json!({ "title": "evil", "prompt": "p", "workdir": "/etc" }),
    );
    assert!(r.is_err(), "/etc 应被拒绝: {r:?}");
    // 穿越
    let r = d.try_api(
        Method::TaskCreate,
        serde_json::json!({ "title": "evil", "prompt": "p", "workdir": "/tmp/../etc" }),
    );
    assert!(r.is_err(), "穿越应被拒绝");
    // 不存在
    let r = d.try_api(
        Method::TaskCreate,
        serde_json::json!({ "title": "evil", "prompt": "p", "workdir": "/nonexistent/xyz" }),
    );
    assert!(r.is_err(), "不存在路径应被拒绝");
    // 正常目录放行
    let r = d.try_api(
        Method::TaskCreate,
        serde_json::json!({ "title": "ok", "prompt": "p", "workdir": work.display().to_string() }),
    );
    assert!(r.is_ok(), "正常 workdir 应放行: {r:?}");
}

/// rollback 只允许 refs/maestro/cp/ 命名空间（防任意 ref 重写）
#[test]
#[serial]
fn rollback_ref_namespace_locked() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let t = d.create_task("rb-sec", &work);
    assert!(d.wait_state(&t, maestro_protocol::types::WorkerState::Working, 5000));
    // 试图回滚到主分支 ref：必须拒绝
    let r = d.try_api(
        Method::CheckpointRollback,
        serde_json::json!({ "task": t.as_str(), "to": "refs/heads/main" }),
    );
    assert!(r.is_err(), "refs/heads/main 必须拒绝: {r:?}");
    let r = d.try_api(
        Method::CheckpointRollback,
        serde_json::json!({ "task": t.as_str(), "to": "refs/maestro/cp/../../evil" }),
    );
    assert!(r.is_err(), "穿越 ref 必须拒绝");
    // 命名空间内的合法 ref：可通过校验（不存在会由 git 报错，但不是 403）
    let r = d.try_api(
        Method::CheckpointRollback,
        serde_json::json!({ "task": t.as_str(), "to": "refs/maestro/cp/t-1/0-baseline" }),
    );
    if let Err(code_msg) = r {
        let (code, _) = code_msg;
        assert_ne!(code, -403, "命名空间内不应被安全层拒绝: {code}");
    }
}

/// 超长输入拒绝（防事件库膨胀）
#[test]
#[serial]
fn oversized_inputs_rejected() {
    let (_repo, work) = git_repo();
    let d = TestDaemon::start("/bin/sh", &["-c", "sleep 300"]);
    let r = d.try_api(
        Method::TaskCreate,
        serde_json::json!({ "title": "x".repeat(500), "prompt": "p", "workdir": work.display().to_string() }),
    );
    assert!(r.is_err(), "超长 title 应拒绝");
    let t = d.create_task("steer-sec", &work);
    let r = d.try_api(
        Method::TaskSteer,
        serde_json::json!({ "task": t.as_str(), "message": "x".repeat(20_000) }),
    );
    assert!(r.is_err(), "超长 steer 消息应拒绝");
}