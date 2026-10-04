//! IPC server：双 socket（JSON API + 事件流）。
//!
//! - API socket：每连接一线程，请求→CoreMsg::Api→响应
//! - 事件 socket：每连接一线程，订阅后持续推送 Envelope JSONL
//!
//! 传输层跨平台（R57）：Unix domain socket（Linux/macOS）↔ TCP 回环（Windows），
//! 统一抽象在 maestro_client::transport —— 线协议（JSONL）不变。
//!
//! 慢客户端由 EventHub 的有界队列兜底（满即断开，客户端凭 seq 重连重放）

use crate::core::CoreMsg;
use crate::eventhub::EventHub;
use maestro_client::transport::{self, Addr, Listener, Stream};
use maestro_protocol::api::{Method, Request, Response};
use maestro_protocol::events::Envelope;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// socket 服务的停机句柄：stop() 退出 accept 循环并回收线程。
/// 用途：e2e 同 data_dir 起第二实例前释放 socket 路径
/// （生产 daemon 随进程退出，不需要调用）。
pub struct ServerGuard {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl ServerGuard {
    /// 停止 accept 循环（≤ 一个轮询周期）并 join 全部 listener 线程。
    /// 已建立的连接线程随客户端断开自然退出。
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

/// accept 循环：非阻塞 + 停机标志轮询（20ms 粒度）。
/// ⚠️ accept 后显式恢复阻塞：Linux 的 accept 不继承 listener 的 O_NONBLOCK，
/// 但 macOS/BSD 会继承 —— 不显式设回的话 macOS 上连接流读到 WouldBlock
/// 即断开（客户端 Broken pipe，CI 实测教训）。
fn accept_loop(
    listener: Listener,
    stop: Arc<AtomicBool>,
    on_conn: impl Fn(Stream) + Send + Sync + 'static,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok(stream) => {
                let _ = stream.set_nonblocking(false);
                on_conn(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

/// 启动双 socket 服务（绑定端点 + 返回停机句柄与 api 端点）。
/// api 端点用于回填 ENV_SOCKET_PATH —— Windows 下 TCP 端口 bind 后才确定。
pub fn serve(
    hub: Arc<EventHub>,
    core_tx: Sender<CoreMsg>,
    data_dir: &Path,
    store: Option<Arc<std::sync::Mutex<crate::persist::EventStore>>>,
) -> std::io::Result<(ServerGuard, Addr)> {
    let (api_listener, events_listener, api_addr) = transport::bind_endpoints(data_dir)?;
    // 非阻塞 accept（停机靠标志轮询）
    api_listener.set_nonblocking(true)?;
    events_listener.set_nonblocking(true)?;

    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = vec![];

    // ---- API socket：每连接一线程（经 channel，不碰 Core 锁）----
    {
        let tx = core_tx.clone();
        let stop = stop.clone();
        handles.push(std::thread::spawn(move || {
            let on_conn = move |stream: Stream| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_api_conn(stream, &tx) {
                        tracing::debug!("api conn closed: {e}");
                    }
                });
            };
            accept_loop(api_listener, stop, on_conn);
        }));
    }

    // ---- 事件 socket：每连接一线程（直接用 hub Arc，不需要 Core 锁）----
    {
        let hub = hub.clone();
        let store = store.clone();
        let stop = stop.clone();
        handles.push(std::thread::spawn(move || {
            let on_conn = move |stream: Stream| {
                let hub = hub.clone();
                let store = store.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_events_conn(stream, &hub, store.as_ref()) {
                        tracing::debug!("events conn closed: {e}");
                    }
                });
            };
            accept_loop(events_listener, stop, on_conn);
        }));
    }

    Ok((ServerGuard { stop, handles }, api_addr))
}

/// API 连接：逐行读 JSON-RPC 请求 → Core → 回响应
fn handle_api_conn(stream: Stream, tx: &Sender<CoreMsg>) -> std::io::Result<()> {
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
fn handle_events_conn(
    stream: Stream,
    hub: &EventHub,
    store: Option<&Arc<std::sync::Mutex<crate::persist::EventStore>>>,
) -> std::io::Result<()> {
    let mut stream = stream;
    let mut reader = BufReader::new(stream.try_clone()?);

    // 首行：订阅游标（默认 0=从现在开始）+ 是否跟随（默认 true，兼容老客户端）
    let mut from_seq = 0u64;
    let mut live = true;
    let mut first = String::new();
    if reader.read_line(&mut first)? > 0 {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&first) {
            if let Some(s) = v.get("from_seq").and_then(|x| x.as_u64()) {
                from_seq = s;
            }
            if let Some(l) = v.get("live").and_then(|x| x.as_bool()) {
                live = l;
            }
        }
    }

    // 订阅（带重放 —— hub 线程安全，不经 Core 锁）
    let replay = store.map(|s| {
        let s = s.clone();
        move |from: u64| -> Vec<Envelope> { s.lock().unwrap().replay_from(from) }
    });
    let subscription = if let Some(r) = &replay {
        hub.subscribe(Some(r), from_seq)
    } else {
        hub.subscribe(None, from_seq)
    };

    // 非跟随模式（看历史）：drain 重放（含临界新事件）后即关闭
    if !live {
        while let Ok(env) = subscription.rx.try_recv() {
            let line = serde_json::to_string(&env).unwrap_or_default();
            if writeln!(stream, "{line}").and_then(|_| stream.flush()).is_err() {
                break; // 客户端已断开
            }
        }
        hub.unsubscribe(subscription.id);
        return Ok(());
    }

    // 推送循环
    while let Ok(env) = subscription.rx.recv() {
        let line = serde_json::to_string(&env).unwrap_or_default();
        if writeln!(stream, "{line}")
            .and_then(|_| stream.flush())
            .is_err()
        {
            break; // 客户端断开
        }
    }
    // 清理订阅（hub 线程安全）
    hub.unsubscribe(subscription.id);
    Ok(())
}
