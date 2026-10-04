//! Maestro 测试基建：MockClock、脚本 Worker、pgid 沙箱、R3 harness、fixtures。
//! 用例组 A/B 全依赖这里（U10_T6_DESIGN.md §12）。
//!
//! 平台矩阵（R57）：mock_clock 跨平台；脚本 Worker/pgid 沙箱/mock CLI
//! 依赖 Unix 进程组 + bash → cfg(unix)（Windows 测试面见 CI 矩阵说明）。

pub mod mock_clock;
#[cfg(unix)]
pub mod pgid_sandbox;
#[cfg(unix)]
pub mod r3;
#[cfg(unix)]
pub mod script_worker;

pub use mock_clock::MockClock;
