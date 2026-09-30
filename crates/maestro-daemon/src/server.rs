//! IPC server：双 socket（JSON API + 事件流）。
//!
//! - API socket：UnixStream，每连接一线程，请求→CoreMsg::Api→响应
//! - 事件 socket：UnixStream，每连接一线程，订阅后持续推送 Envelope JSONL
//! 慢客户端由 EventHub 的有界队列兜底（满即断开，客户端凭 seq 重连重放）

use crate::core::{Core, CoreMsg};
use maestro_protocol::api::{Method, Request, Response};
use maestro_protocol::events::Envelope;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// IPC 路径约定
pub struct IpcPaths {
    pub api_sock: PathBuf,
    pub events_sock: PathBuf,
}

impl IpcPaths {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            api_sock: data_dir.join("maestro.api.sock"),
            events_sock: data_dir.join("maestro.events.sock"),
        }
    }
}

/// stale socket 探活（herdr 决策 A9）：只删自己的 socket。
/// 通过尝试连接：连得上=有 daemon 在用（绝不删）；连不上=stale。
fn remove_stale_socket(path: &Path) {
    if !path.exists() {
        return;
    }
    match UnixStream::connect(path) {
        Ok(_) => {
            // 有活的 listener —— 不碰
        }
        Err(_) => {
            // 没人听：stale，清理
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 启动双 socket 服务（返回各 listener 的 join handles）
pub fn serve(
    core: &Arc<std::sync::Mutex<Core>>,
    core_tx: Sender<CoreMsg>,
    paths: &IpcPaths,
    store: Option<Arc<std::sync::Mutex<crate::persist::EventStore>>>,
) -> std::io::Result<Vec<std::thread::JoinHandle<()>>> {
    std::fs::create_dir_all(paths.api_sock.parent().unwrap())?;
    remove_stale_socket(&paths.api_sock);
    remove_stale_socket(&paths.events_sock);

    let api_listener = UnixListener::bind(&paths.api_sock)?;
    let events_listener = UnixListener::bind(&paths.events_sock)?;
    // 0600：仅属主可访问
    for p in [&paths.api_sock, &paths.events_sock] {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }

    let mut handles = vec![];

    // ---- API socket：每连接一线程 ----
    {
        let core = core.clone();
        let tx = core_tx.clone();
        handles.push(std::thread::spawn(move || {
            for stream in api_listener.incoming() {
                let Ok(stream) = stream else { continue };
                let core = core.clone();
                let tx = tx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_api_conn(stream, &core, &tx) {
                        tracing::debug!("api conn closed: {e}");
                    }
                });
            }
        }));
    }

    // ---- 事件 socket：每连接一线程，推送 JSONL ----
    {
        let core = core.clone();
        handles.push(std::thread::spawn(move || {
            for stream in events_listener.incoming() {
                let Ok(stream) = stream else { continue };
                let core = core.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_events_conn(stream, &core) {
                        tracing::debug!("events conn closed: {e}");
                    }
                });
            }
        }));
    }

    Ok(handles)
}

/// API 连接：逐行读 JSON-RPC 请求 → Core → 回响应
fn handle_api_conn(
    stream: UnixStream,
    core: &Arc<std::sync::Mutex<Core>>,
    tx: &Sender<CoreMsg>,
) -> std::io::Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => {
                let resp = Response::Err {
                    id: "?".into(),
                    error: maestro_protocol::api::RpcError {
                        code: -400,
                        message: "bad request".into(),
                    },
                };
                writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
                writer.flush()?;
                continue;
            }
        };
        // 订阅类方法走事件 socket；其余转 Core
        let resp = if req.method == Method::EventsSubscribe {
            Response::Ok {
                id: req.id.clone(),
                result: serde_json::json!({"hint": "connect events socket, send {\"from_seq\":N}"}),
            }
        } else {
            let (rtx, rrx) = std::sync::mpsc::channel();
            if tx.send(CoreMsg::Api(req, rtx)).is_ok() {
                rrx.recv().unwrap_or(Response::Err {
                    id: "?".into(),
                    error: maestro_protocol::api::RpcError {
                        code: -500,
                        message: "core unreachable".into(),
                    },
                })
            } else {
                Response::Err {
                    id: "?".into(),
                    error: maestro_protocol::api::RpcError {
                        code: -500,
                        message: "core dead".into(),
                    },
                }
            }
        };
        writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
        writer.flush()?;
    }
    Ok(())
}

/// 事件连接：首行可选 {"from_seq":N} 订阅，之后持续推送
fn handle_events_conn(stream: UnixStream, core: &Arc<std::sync::Mutex<Core>>) -> std::io::Result<()> {
    let mut stream = stream;
    let mut reader = BufReader::new(stream.try_clone()?);

    // 首行：订阅游标（默认 0=从现在开始）
    let mut from_seq = 0u64;
    let mut first = String::new();
    if reader.read_line(&mut first)? > 0 {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) {
            if let Some(s) = v.get("from_seq").and_then(|x| x.as_u64()) {
                from_seq = s;
            }
        }
    }

    // 在 Core 的 hub 上订阅（带重放）
    let subscription = {
        let core_guard = core.lock().unwrap();
        let store = core_guard.event_store_handle();
        let replay: Option<Box<dyn Fn(u64) -> Vec<Envelope> + '_>> = store.map(|s| {
            Box::new(move |from: u64| s.lock().unwrap().replay_from(from)) as Box<dyn Fn(u64) -> Vec<Envelope>>
        });
        if let Some(r) = replay {
            core_guard.ctx.hub.subscribe(Some(&*r), from_seq)
        } else {
            core_guard.ctx.hub.subscribe(None, from_seq)
        }
    };

    // 推送循环
    loop {
        match subscription.rx.recv() {
            Ok(env) => {
                let line = serde_json::to_string(&env).unwrap_or_default();
                if writeln!(stream, "{line}").and_then(|_| stream.flush()).is_err() {
                    break; // 客户端断开
                }
            }
            Err(_) => break, // hub 侧断开（背压）
        }
    }
    // 清理订阅
    {
        let core_guard = core.lock().unwrap();
        core_guard.ctx.hub.unsubscribe(subscription.id);
    }
    Ok(())
}
