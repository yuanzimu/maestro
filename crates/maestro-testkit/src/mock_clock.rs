//! MockClock：虚拟时钟。退避/窗口/超时全部虚拟推进，CI 零真实睡眠（用例 A2 等）。
//!
//! 实现 maestro_protocol::Clock；被测代码面向 trait 编程，测试注入本实现。

use maestro_protocol::Clock;
use std::sync::{Arc, Condvar, Mutex, RwLock};

/// 虚拟时钟内部状态
struct Inner {
    /// 虚拟 Unix 毫秒
    now_ms: Mutex<u64>,
    /// 推进通知（与 now_ms 配对）
    cond: Condvar,
    /// 等待者数量（用于断言无泄漏）
    waiters: RwLock<usize>,
}

/// MockClock：`Arc` 共享给被测代码，测试线程持有另一份推进时间。
#[derive(Clone)]
pub struct MockClock {
    inner: Arc<Inner>,
    /// 起始值（默认 2026-01-01，避免 0 附近的时间怪异）
    pub start_ms: u64,
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    pub fn new() -> Self {
        // 2026-01-01T00:00:00Z = 1767225600000
        Self::at(1_767_225_600_000)
    }

    pub fn at(start_ms: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                now_ms: Mutex::new(start_ms),
                cond: Condvar::new(),
                waiters: RwLock::new(0),
            }),
            start_ms,
        }
    }

    /// 推进虚拟时间并唤醒全部等待者（A2 驱动退避的唯一手段）
    pub fn advance_ms(&self, ms: u64) {
        let mut now = self.inner.now_ms.lock().unwrap();
        *now += ms;
        self.inner.cond.notify_all();
    }

    pub fn advance_secs(&self, s: u64) {
        self.advance_ms(s * 1000);
    }

    /// 当前虚拟时间
    pub fn current_ms(&self) -> u64 {
        *self.inner.now_ms.lock().unwrap()
    }

    /// 活跃等待者数量（测试断言用：调度器不该有泄漏的等待者）
    pub fn waiter_count(&self) -> usize {
        *self.inner.waiters.read().unwrap()
    }
}

impl Clock for MockClock {
    fn now_ms(&self) -> u64 {
        self.current_ms()
    }

    /// 虚拟等待：等待者计数 +1，在 condvar 上等到 now >= deadline。
    /// advance 推进时间会唤醒它。注意：等待循环必须重新检查 now（防虚假唤醒）。
    fn sleep_until(&self, deadline_ms: u64) {
        {
            let mut w = self.inner.waiters.write().unwrap();
            *w += 1;
        }
        let mut now = self.inner.now_ms.lock().unwrap();
        while *now < deadline_ms {
            now = self.inner.cond.wait(now).unwrap();
        }
        let mut w = self.inner.waiters.write().unwrap();
        *w -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 虚拟推进：advance 唤醒 sleep_until 等待者，全程零真实睡眠
    #[test]
    fn advance_wakes_sleepers() {
        let clock = MockClock::new();
        let c2 = clock.clone();
        let start = Instant::now();

        let h = std::thread::spawn(move || {
            // 睡到虚拟 now + 30000（模拟退避第一档）
            let deadline = c2.current_ms() + 30_000;
            c2.sleep_until(deadline);
            c2.current_ms()
        });

        std::thread::sleep(std::time::Duration::from_millis(50)); // 让等待者先登记
        assert_eq!(clock.waiter_count(), 1);
        clock.advance_secs(30);
        let woke_at = h.join().unwrap();

        assert_eq!(woke_at, clock.start_ms + 30_000);
        assert!(start.elapsed().as_secs() < 5, "不应有真实长睡眠");
        assert_eq!(clock.waiter_count(), 0, "等待者应清零");
    }

    /// 多个等待者不同 deadline，一次大推进全部唤醒
    #[test]
    fn advance_wakes_all_staggered() {
        let clock = MockClock::new();
        let mut handles = vec![];
        for delta in [10u64, 60, 120] {
            let c = clock.clone();
            handles.push(std::thread::spawn(move || {
                let d = c.current_ms() + delta * 1000;
                c.sleep_until(d);
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(clock.waiter_count(), 3);
        clock.advance_secs(120);
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(clock.waiter_count(), 0);
    }

    /// deadline 已过：立即返回不阻塞
    #[test]
    fn past_deadline_returns_immediately() {
        let clock = MockClock::new();
        clock.sleep_until(clock.current_ms());
        clock.sleep_until(clock.current_ms() - 1); // 过期 deadline
        assert_eq!(clock.waiter_count(), 0);
    }
}
