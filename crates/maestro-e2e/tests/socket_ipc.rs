//! server 层集成测试：双 socket（API + 事件流）真实 IPC 路径。
//! 这是 server.rs 唯一的测试覆盖 —— 之前完全没测。

use maestro_client::MaestroClient;
use maestro_daemon::core::{Core, CoreConfig};
use maestro_daemon::server::{self, IpcPaths};
use maestro_protocol::api::Method;
use std::sync::{Arc, Mutex};

/// 拉起带真实 socket 的 daemon
fn spawn_socket_daemon(data_dir: &std::path::Path) -> (Arc<Mutex<Core>>, IpcPaths) {
    let cfg = CoreConfig {
        data_dir: data_dir.to_path_buf(),
        workdir: data_dir.to_path_buf(),
        worker_program: "/bin/sh".into(),
        worker_args: vec!["-c".into(), "sleep 300".into()],
        socket_path: data_dir.join("maestro.api.sock").display().to_string(),
        max_parallel_workers: 4,
        default_model: "claude-sonnet-4".into(),
    };
    let (core, _) = Core::recover(cfg, Arc::new(maestro_protocol::SystemClock));
    let core = Arc::new(Mutex::new(core));
    let paths = IpcPaths::new(data_dir);
    let (core_tx, hub, store) = {
        let g = core.lock().unwrap();
        (g.sender(), g.hub_handle(), g.event_store_handle())
    };
    server::serve(hub, core_tx, &paths, store).expect("serve");
    // Core 主循环线程
    {
        let c = core.clone();
        std::thread::spawn(move || {
            c.lock().unwrap().run();
        });
    }
    (core, paths)
}

/// API socket 全链路：status → task create → workers → stop → resume
#[test]
fn api_socket_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    let (_core, _paths) = spawn_socket_daemon(&dir);

    let client = MaestroClient::new(&dir);
    assert!(client.is_daemon_alive());

    // status
    let v = client
        .call("s1", Method::ServerStatus, serde_json::json!({}))
        .unwrap();
    assert!(v["version"].is_string());
    assert!(v["pid"].as_u64().is_some());

    // task create（workdir 用临时 git 仓）
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.email", "t@t"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.name", "t"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["commit", "--allow-empty", "-q", "-m", "init"])
        .output()
        .unwrap();

    let v = client
        .call(
            "s2",
            Method::TaskCreate,
            serde_json::json!({ "title": "sock-test", "prompt": "p", "workdir": repo.display().to_string() }),
        )
        .unwrap();
    let task_id = v["task"]["id"].as_str().unwrap().to_string();
    assert!(!task_id.is_empty());
    assert!(v["worker"].is_string());

    // workers 列表应有 1 个
    let v = client
        .call("s3", Method::WorkerList, serde_json::json!({}))
        .unwrap();
    assert_eq!(v["workers"].as_array().map(|a| a.len()), Some(1));

    // 急停 + 恢复
    let v = client
        .call("s4", Method::ServerEmergencyStop, serde_json::json!({}))
        .unwrap();
    assert!(v["freeze_ms"].as_u64().is_some());
    let v = client
        .call(
            "s5",
            Method::ServerResumeAll,
            serde_json::json!({ "steering": "flush" }),
        )
        .unwrap();
    assert!(v["resumed"].as_array().is_some());

    // 错误路径：不存在的任务 → Rpc 错误
    let err = client
        .call(
            "s6",
            Method::TaskGet,
            serde_json::json!({ "task": "no-such" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");

    // 关停（Core 循环退出）
    client
        .call("s7", Method::ServerShutdown, serde_json::json!({}))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    // socket 应被清理或 daemon 退出（listener 线程还挂着，但 Core 已停）
}

/// 事件 socket：订阅后能收到新发布的事件（JSONL）
#[test]
fn events_socket_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    let (_core, _paths) = spawn_socket_daemon(&dir);
    let client = MaestroClient::new(&dir);

    // 订阅线程：收 1 个 TaskCreated 就停
    let (tx, rx) = std::sync::mpsc::channel();
    let _sub = std::thread::spawn(move || {
        client
            .subscribe(0, |env| {
                let _ = tx.send(env);
                false // 收到第一个就断
            })
            .unwrap();
    });

    // 等订阅建立，然后建任务
    std::thread::sleep(std::time::Duration::from_millis(200));
    let repo = tmp.path().join("repo2");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.email", "t@t"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.name", "t"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["commit", "--allow-empty", "-q", "-m", "init"])
        .output()
        .unwrap();

    let client2 = MaestroClient::new(&dir);
    client2
        .call(
            "e1",
            Method::TaskCreate,
            serde_json::json!({ "title": "evt", "prompt": "p", "workdir": repo.display().to_string() }),
        )
        .unwrap();

    let env = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("应收到事件");
    if let maestro_protocol::events::Event::TaskCreated { task, .. } = &env.event {
        assert!(task.title.contains("evt"));
    }
    // 订阅可能先收到其他事件（重放起点 0 已含历史）—— 只要收到事件就算通
}

/// stale socket 清理：假 socket 文件不阻碍新 daemon 起服务
#[test]
fn stale_socket_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    // 伪造 stale socket（无 listener）
    std::fs::write(dir.join("maestro.api.sock"), b"stale").unwrap();
    std::fs::write(dir.join("maestro.events.sock"), b"stale").unwrap();

    let (_core, paths) = spawn_socket_daemon(&dir);
    // 起服务成功 = stale 被清掉了
    assert!(paths.api_sock.exists());
    let client = MaestroClient::new(&dir);
    assert!(client.is_daemon_alive(), "新 daemon 应能监听（stale 已清）");
}
