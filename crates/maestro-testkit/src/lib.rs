//! Maestro 测试基建：MockClock、脚本 Worker、pgid 沙箱、R3 harness、fixtures。
//! 用例组 A/B 全依赖这里（U10_T6_DESIGN.md §12）。

pub mod mock_clock;
pub mod pgid_sandbox;
pub mod r3;
pub mod script_worker;

pub use mock_clock::MockClock;
