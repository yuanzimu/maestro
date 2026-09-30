//! 时钟抽象（协议层承载，供 daemon/testkit 共用）。
//! 生产 SystemClock；测试 MockClock（testkit crate 实现，虚拟推进零真实睡眠）。

use serde::{Deserialize, Serialize};

/// 时钟契约
pub trait Clock: Send + Sync {
    /// 当前 Unix 毫秒
    fn now_ms(&self) -> u64;
    /// 睡到 deadline_ms（虚拟时钟=登记唤醒；真实时钟=thread::sleep）
    fn sleep_until(&self, deadline_ms: u64);
}

/// 真实系统时钟
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        crate::types::now_ms()
    }

    fn sleep_until(&self, deadline_ms: u64) {
        let now = self.now_ms();
        if deadline_ms > now {
            std::thread::sleep(std::time::Duration::from_millis(deadline_ms - now));
        }
    }
}
