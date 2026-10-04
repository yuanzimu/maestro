//! 跨平台 IPC 传输（R57）：Unix domain socket（Linux/macOS）↔ TCP 回环（Windows）。
//!
//! 为什么 Windows 不用 named pipe：零新依赖原则 —— std 的 UnixStream/TcpStream
//! 同为 Read+Write，JSONL 线协议完全不变。发现机制：
//! - Unix：`<data_dir>/maestro.api.sock`（固定路径，直连）
//! - Windows：daemon bind 时写 `<data_dir>/maestro.api.port`（内容 "127.0.0.1:PORT"），
//!   客户端读文件取地址（bind :0 自动分配，避免端口冲突）
//!
//! ENV_SOCKET_PATH（worker 子进程回连 daemon）的 wire 值：
//! Unix = socket 路径；Windows = "127.0.0.1:PORT"。

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// 默认数据目录（三平台）：$MAESTRO_DATA_DIR 或 <系统临时目录>/maestro
pub fn default_data_dir() -> PathBuf {
    std::env::var("MAESTRO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("maestro"))
}

/// 端点地址（平台互斥）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    /// Unix domain socket 文件路径
    #[cfg(unix)]
    Unix(PathBuf),
    /// Windows：TCP 回环地址
    #[cfg(windows)]
    Tcp(std::net::SocketAddr),
}

/// Windows 端口文件缺失时的占位地址（连接必失败，等价于 Unix 的 socket 不存在）。
/// 注意 const 构造：`SocketAddr::from` 非 const fn（E0658），用 V4 variant 直构。
#[cfg(windows)]
const UNRESOLVED: std::net::SocketAddr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    1,
));

impl Addr {
    /// ENV_SOCKET_PATH 的值（Unix=路径；Windows="ip:port"）
    pub fn to_env_value(&self) -> String {
        match self {
            #[cfg(unix)]
            Addr::Unix(p) => p.display().to_string(),
            #[cfg(windows)]
            Addr::Tcp(a) => a.to_string(),
        }
    }

    /// 解析 ENV_SOCKET_PATH（rounder 回连用）。平台即语义：
    /// Unix 上值是路径；Windows 上值是 "ip:port"。
    pub fn from_env_value(s: &str) -> Self {
        #[cfg(unix)]
        {
            Addr::Unix(PathBuf::from(s))
        }
        #[cfg(windows)]
        {
            s.parse().map(Addr::Tcp).unwrap_or(Addr::Tcp(UNRESOLVED))
        }
    }

    /// 从 data_dir 解析端点对 (api, events)（客户端视角）。
    /// Unix：固定 socket 文件名；Windows：读端口文件（缺失 → 占位，连接时报错）。
    pub fn endpoints(data_dir: &Path) -> (Addr, Addr) {
        #[cfg(unix)]
        {
            (
                Addr::Unix(data_dir.join("maestro.api.sock")),
                Addr::Unix(data_dir.join("maestro.events.sock")),
            )
        }
        #[cfg(windows)]
        {
            (
                read_port_file(&data_dir.join("maestro.api.port")),
                read_port_file(&data_dir.join("maestro.events.port")),
            )
        }
    }

    /// api → events 兄弟端点。
    /// Unix：同目录文件名替换；Windows：无法从 api 地址推导目录，
    /// 返回自身占位（唯一消费者 rounder 不订阅事件流，见 from_env_value）。
    pub fn sibling_events(&self) -> Addr {
        match self {
            #[cfg(unix)]
            Addr::Unix(p) => Addr::Unix(p.with_file_name("maestro.events.sock")),
            #[cfg(windows)]
            a => a.clone(),
        }
    }

    pub fn connect(&self) -> io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Addr::Unix(p) => std::os::unix::net::UnixStream::connect(p).map(Stream::Unix),
            #[cfg(windows)]
            Addr::Tcp(a) => std::net::TcpStream::connect(a).map(Stream::Tcp),
        }
    }
}

#[cfg(windows)]
fn read_port_file(path: &Path) -> Addr {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .map(Addr::Tcp)
        .unwrap_or(Addr::Tcp(UNRESOLVED))
}

/// 连接（读 + 写 + try_clone —— 两平台 stream 的最小公共面）
#[derive(Debug)]
pub enum Stream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    #[cfg(windows)]
    Tcp(std::net::TcpStream),
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.try_clone().map(Stream::Unix),
            #[cfg(windows)]
            Stream::Tcp(s) => s.try_clone().map(Stream::Tcp),
        }
    }

    /// 显式设置阻塞模式（accept 后恢复阻塞：macOS 继承 listener 的
    /// O_NONBLOCK，Linux 不继承 —— 详见 daemon server.rs accept_loop）
    pub fn set_nonblocking(&self, v: bool) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.set_nonblocking(v),
            #[cfg(windows)]
            Stream::Tcp(s) => s.set_nonblocking(v),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
            #[cfg(windows)]
            Stream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
            #[cfg(windows)]
            Stream::Tcp(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
            #[cfg(windows)]
            Stream::Tcp(s) => s.flush(),
        }
    }
}

/// 监听器
#[derive(Debug)]
pub enum Listener {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixListener),
    #[cfg(windows)]
    Tcp(std::net::TcpListener),
}

impl Listener {
    pub fn accept(&self) -> io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Listener::Unix(l) => l.accept().map(|(s, _)| Stream::Unix(s)),
            #[cfg(windows)]
            Listener::Tcp(l) => l.accept().map(|(s, _)| Stream::Tcp(s)),
        }
    }

    pub fn set_nonblocking(&self, v: bool) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Listener::Unix(l) => l.set_nonblocking(v),
            #[cfg(windows)]
            Listener::Tcp(l) => l.set_nonblocking(v),
        }
    }
}

/// 服务端绑定端点对（daemon 用）。
/// Unix：stale socket 探活清理（herdr 决策 A9：只删自己的）+ bind + 0600。
/// Windows：bind 127.0.0.1:0 ×2 + 端口文件落盘（客户端发现机制）。
/// 返回 (api_listener, events_listener, api_addr) —— api_addr 用于回填
/// ENV_SOCKET_PATH（Windows 端口 bind 后才确定）。
pub fn bind_endpoints(data_dir: &Path) -> io::Result<(Listener, Listener, Addr)> {
    #[cfg(unix)]
    {
        let (api, events) = Addr::endpoints(data_dir);
        let (Addr::Unix(api_p), Addr::Unix(ev_p)) = (&api, &events);
        if let Some(dir) = api_p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        remove_stale_socket(api_p);
        remove_stale_socket(ev_p);
        let api_l = Listener::Unix(std::os::unix::net::UnixListener::bind(api_p)?);
        let ev_l = Listener::Unix(std::os::unix::net::UnixListener::bind(ev_p)?);
        // 0600：仅属主可访问
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [api_p, ev_p] {
                let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
            }
        }
        Ok((api_l, ev_l, api))
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(data_dir)?;
        let api_l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let ev_l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let api_addr = api_l.local_addr()?;
        let ev_addr = ev_l.local_addr()?;
        std::fs::write(
            data_dir.join("maestro.api.port"),
            format!("{api_addr}\n"),
        )?;
        std::fs::write(
            data_dir.join("maestro.events.port"),
            format!("{ev_addr}\n"),
        )?;
        Ok((
            Listener::Tcp(api_l),
            Listener::Tcp(ev_l),
            Addr::Tcp(api_addr),
        ))
    }
}

/// stale socket 探活（herdr 决策 A9）：只删自己的 socket。
/// 尝试连接：连得上 = 有 daemon 在用（绝不删）；连不上 = stale。
#[cfg(unix)]
fn remove_stale_socket(path: &Path) {
    if !path.exists() {
        return;
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => {
            // 有活的 listener —— 不碰
        }
        Err(_) => {
            // 没人听：stale，清理
            let _ = std::fs::remove_file(path);
        }
    }
}
