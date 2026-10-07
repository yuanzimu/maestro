//! 硬预算闸门（DEV_PLAN v2.5 / Sprint A，决策 26；设计借鉴 Foreman）。
//!
//! 与「软预算（T4 只报告）」不同，硬预算在每轮轮账后由 Core 强制执行：
//! 一旦超限，立即 SIGSTOP 冻结 worker 并挂起为 `BudgetExceeded`（仅手动
//! 恢复 —— 提额或批准），现场完整保留。这是「敢放手」的硬保障。
//!
//! 本模块只做纯判定；冻结/发事件的副作用在 Core 侧。

/// 任务硬预算（任一项为 None = 该维度不限）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskBudget {
    /// 累计实际花费上限（美分）
    pub max_cost_cents: Option<u64>,
    /// 挂钟时长上限（毫秒，从任务首个事件起算）
    pub max_wall_ms: Option<u64>,
}

/// 命中的预算上限
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetLimit {
    CostExceeded { spent: u64, max: u64 },
    WallTimeExceeded { elapsed: u64, max: u64 },
}

impl BudgetLimit {
    /// 人可读描述（进事件流/U3 叙事）
    pub fn describe(self) -> String {
        match self {
            BudgetLimit::CostExceeded { spent, max } => {
                format!(
                    "花费 ${:.2} 超预算上限 ${:.2}",
                    spent as f64 / 100.0,
                    max as f64 / 100.0
                )
            }
            BudgetLimit::WallTimeExceeded { elapsed, max } => format!(
                "耗时 {:.1} 分钟超预算上限 {:.1} 分钟",
                elapsed as f64 / 60_000.0,
                max as f64 / 60_000.0
            ),
        }
    }
}

impl Default for TaskBudget {
    fn default() -> Self {
        // 安全网默认：$5.00 / 30 分钟（均可经环境覆盖；设 0 = 不限）
        Self {
            max_cost_cents: Some(500),
            max_wall_ms: Some(30 * 60 * 1000),
        }
    }
}

impl TaskBudget {
    /// 从环境构造（0 = 该维度不限；缺省走默认安全网）：
    /// - `MAESTRO_BUDGET_CENTS`
    /// - `MAESTRO_BUDGET_WALL_MS`
    pub fn from_env() -> Self {
        // 区分四种结果，避免非法值被静默解释成「不限」（拆除安全网）：
        // 缺省 → 默认；"0" → None(不限)；合法 >0 → Some；
        // 非数字/负数 → 告警后回退默认（而非 None）。
        let resolve = |v: Result<String, std::env::VarError>, name, default| match v {
            Err(_) => default,
            Ok(s) => match s.trim().parse::<u64>() {
                Ok(0) => None,
                Ok(n) => Some(n),
                Err(_) => {
                    tracing::warn!("非法 {name}={}（应为非负整数），回退默认安全网", s.trim());
                    default
                }
            },
        };
        let d = Self::default();
        Self {
            max_cost_cents: resolve(
                std::env::var("MAESTRO_BUDGET_CENTS"),
                "MAESTRO_BUDGET_CENTS",
                d.max_cost_cents,
            ),
            max_wall_ms: resolve(
                std::env::var("MAESTRO_BUDGET_WALL_MS"),
                "MAESTRO_BUDGET_WALL_MS",
                d.max_wall_ms,
            ),
        }
    }

    /// 判定：花费/耗时任一超限即返回命中项。
    pub fn check(&self, spent_cents: u64, elapsed_ms: u64) -> Option<BudgetLimit> {
        if let Some(max) = self.max_cost_cents {
            if spent_cents > max {
                return Some(BudgetLimit::CostExceeded {
                    spent: spent_cents,
                    max,
                });
            }
        }
        if let Some(max) = self.max_wall_ms {
            if elapsed_ms > max {
                return Some(BudgetLimit::WallTimeExceeded {
                    elapsed: elapsed_ms,
                    max,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_budget_passes() {
        let b = TaskBudget::default();
        assert!(b.check(100, 60_000).is_none());
    }

    #[test]
    fn cost_over_limit() {
        let b = TaskBudget {
            max_cost_cents: Some(500),
            max_wall_ms: None,
        };
        let hit = b.check(501, 0).unwrap();
        assert!(matches!(
            hit,
            BudgetLimit::CostExceeded {
                spent: 501,
                max: 500
            }
        ));
        // 恰好等于上限不算超
        assert!(b.check(500, 0).is_none());
    }

    #[test]
    fn wall_over_limit() {
        let b = TaskBudget {
            max_cost_cents: None,
            max_wall_ms: Some(1_800_000),
        };
        let hit = b.check(0, 1_800_001).unwrap();
        assert!(matches!(hit, BudgetLimit::WallTimeExceeded { .. }));
    }

    #[test]
    fn unlimited_dimensions_never_violate() {
        let b = TaskBudget {
            max_cost_cents: None,
            max_wall_ms: None,
        };
        assert!(b.check(u64::MAX, u64::MAX).is_none());
    }

    #[test]
    fn env_zero_means_unlimited() {
        std::env::set_var("MAESTRO_BUDGET_CENTS", "0");
        std::env::set_var("MAESTRO_BUDGET_WALL_MS", "0");
        let b = TaskBudget::from_env();
        assert_eq!(b.max_cost_cents, None);
        assert_eq!(b.max_wall_ms, None);
        std::env::remove_var("MAESTRO_BUDGET_CENTS");
        std::env::remove_var("MAESTRO_BUDGET_WALL_MS");
    }
}
