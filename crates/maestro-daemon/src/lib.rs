//! Maestro 守护进程。
//!
//! 架构（继承 herdr 验证过的模式）：
//! - **单线程权威 Core**：独占全部状态，其他线程经 mpsc 发消息，无锁竞争
//! - **Worker stdout/stderr 落文件**（不接管道）：防 SIGSTOP 后管道满导致的
//!   wait() 死锁（同类项目实测坑：SIGSTOP+pipe full = 永久挂死）
//! - **有界订阅队列**：慢客户端不拖死事件循环（超限断开，客户端凭 seq 重放补齐）
//! - **PID 文件带 start_time**：防 PID 复用误杀

pub mod acceptance;
pub mod adapter;
pub mod checkpoints;
pub mod core;
pub mod emergency;
pub mod eventhub;
pub mod llm;
pub mod narrative;
pub mod persist;
pub mod routing;
pub mod security;
pub mod server;
pub mod state;
pub mod steering;
pub mod suspend;
pub mod worker;

pub use core::{Core, CoreMsg};
pub use state::Authority;
