//! 全局共享状态（tauri manage）

use crate::settings;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 应用数据根（C4-1 前置，Linux XDG 修复）：
/// - Windows：`%APPDATA%\maestro-desktop\`
/// - Linux：`$XDG_DATA_HOME/maestro-desktop`，未设则 `~/.local/share/maestro-desktop`
///   （原实现只有 APPDATA 分支，Linux 上落到系统临时目录 —— 磁盘清理会
///   清空事件库，正是 data_dir 注释里点名要避开的场景）
/// - macOS：`~/Library/Application Support/maestro-desktop`
///
/// dev 构建加 `-dev` 隔离。
pub fn app_root() -> PathBuf {
    let name = if cfg!(debug_assertions) {
        "maestro-desktop-dev"
    } else {
        "maestro-desktop"
    };
    if cfg!(target_os = "macos") {
        let base = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(std::env::temp_dir);
        return base.join(name);
    }
    if cfg!(windows) {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        return base.join(name);
    }
    // Linux（及其余 Unix）：XDG Base Directory
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join(name)
}

/// daemon 数据目录（事件库/账本/端口文件落这里）
/// 不用 daemon 默认（系统临时目录会被磁盘清理清空 —— 事件库会丢）
pub fn data_dir() -> PathBuf {
    app_root().join("data")
}

pub struct AppState {
    pub data_dir: PathBuf,
    /// 事件桥已推进到的 seq（断线重连用 last_seq+1 续订）
    pub last_seq: Arc<AtomicU64>,
    /// 由本应用拉起的 daemon 子进程句柄（重启/关停用）
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    /// daemon 是否由本应用托管（false = 外部启动，接管）
    pub managed: Arc<AtomicBool>,
    /// 前端监听器已就绪（bridge 收到此信号才开始订阅，避免重放爆发在
    /// React listen() 注册前被 emit 而丢失 —— daemon 存活的接管路径下
    /// 首订阅是瞬时的，必现竞态）
    pub bridge_start: Arc<AtomicBool>,
    pub settings: Arc<Mutex<settings::Settings>>,
}

impl AppState {
    pub fn init() -> Self {
        let data_dir = data_dir();
        let _ = std::fs::create_dir_all(&data_dir);
        let settings = settings::load();
        Self {
            data_dir,
            last_seq: Arc::new(AtomicU64::new(0)),
            child: Arc::new(Mutex::new(None)),
            managed: Arc::new(AtomicBool::new(false)),
            bridge_start: Arc::new(AtomicBool::new(false)),
            settings: Arc::new(Mutex::new(settings)),
        }
    }

    pub fn is_managed(&self) -> bool {
        self.managed.load(Ordering::SeqCst)
    }

    /// 等待 daemon 下线（关停/重启用）；返回是否等到
    pub fn wait_daemon_down(&self, timeout_ms: u64) -> bool {
        let client = maestro_client::MaestroClient::new(&self.data_dir);
        let step = 200u64;
        let mut waited = 0u64;
        while waited < timeout_ms {
            if !client.is_daemon_alive() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(step));
            waited += step;
        }
        !client.is_daemon_alive()
    }

    /// 杀掉托管的子进程（超时强杀路径）
    pub fn kill_child(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.managed.store(false, Ordering::SeqCst);
    }
}
