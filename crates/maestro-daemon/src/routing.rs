//! 模型路由框架骨架（DEV_PLAN 0.8 / T1）。
//!
//! 分级思路（nginx upstream）：
//! - L1 小模型池：分类/摘要/预估/简单轮（T4 省 token 主力）
//! - L2 中档：常规编码轮
//! - L3 大模型：只经「升级路径」进入（L1 连续失败 / 复杂度信号 / 用户显式）
//!
//! P0 交付：trait + 规则分类器（确定性、可单测）。P1-A6 接 LLM client
//! 实测降级学习；Provider 可用性 failover（A7）复用同一决策结构。

use maestro_protocol::types::TaskId;

/// 模型档位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// 小模型（分类/摘要/预估/简单轮）
    L1,
    /// 中档（常规编码轮）
    L2,
    /// 大模型（升级路径专用）
    L3,
}

/// 路由决策上下文（多轮驱动 Worker 每轮构造一次）
#[derive(Debug, Clone)]
pub struct RouteCtx {
    pub task: TaskId,
    pub round: u32,
    /// 本轮输入规模（prompt + 上下文字符数，粗粒度）
    pub input_chars: usize,
    /// 同档连续失败次数（升级路径触发器）
    pub consecutive_failures: u32,
    /// 用户显式指定档位（越过一切规则）
    pub forced_tier: Option<Tier>,
    /// 任务类型标记（0.15 起 Worker 上报；None = 未知按编码任务处理）
    pub kind: Option<TaskKind>,
}

/// 任务类型（影响默认档位）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// 分类/摘要/预估等轻量轮
    Lightweight,
    /// 常规编码
    Coding,
    /// 深度推理/架构
    Deep,
}

/// 路由决策
#[derive(Debug, Clone, PartialEq)]
pub struct RouteDecision {
    pub tier: Tier,
    /// 决策原因（进事件流，U3 叙事「为省成本先走小模型」）
    pub reason: String,
}

/// 路由器抽象（P1 可换实现：规则/学习型/混合）
pub trait ModelRouter: Send + Sync {
    fn route(&self, ctx: &RouteCtx) -> RouteDecision;
}

/// 规则分类器 v0（确定性）：
/// 1. 用户显式指定 → 直接采用
/// 2. 连续失败 ≥2 → 升一级（L1→L2→L3）
/// 3. 轻量类型 → L1
/// 4. 深度类型 → L3
/// 5. 超长输入（>200k 字符）→ L2 起（小模型上下文装不下）
/// 6. 其余编码轮 → L2
pub struct RuleRouter {
    /// 升级触发阈值（连续失败次数）
    pub escalate_after: u32,
    /// 超长输入阈值（字符）
    pub long_input_chars: usize,
}

impl Default for RuleRouter {
    fn default() -> Self {
        Self {
            escalate_after: 2,
            long_input_chars: 200_000,
        }
    }
}

impl RuleRouter {
    fn escalate(t: Tier) -> Tier {
        match t {
            Tier::L1 => Tier::L2,
            Tier::L2 | Tier::L3 => Tier::L3,
        }
    }

    fn base_tier(ctx: &RouteCtx) -> Tier {
        match ctx.kind {
            Some(TaskKind::Lightweight) => Tier::L1,
            // 常规编码轮 / 未知类型：L2 起（L1 有上下文风险，L3 浪费）
            Some(TaskKind::Coding) | None => Tier::L2,
            Some(TaskKind::Deep) => Tier::L3,
        }
    }
}

impl ModelRouter for RuleRouter {
    fn route(&self, ctx: &RouteCtx) -> RouteDecision {
        if let Some(t) = ctx.forced_tier {
            return RouteDecision {
                tier: t,
                reason: "user_forced".into(),
            };
        }
        let mut base = Self::base_tier(ctx);
        if ctx.consecutive_failures >= self.escalate_after && base != Tier::L3 {
            let up = Self::escalate(base);
            return RouteDecision {
                tier: up,
                reason: format!("escalated_after_{}_failures", ctx.consecutive_failures),
            };
        }
        // 超长输入规则（规则 5）：轻量轮同样可能携带 >200k 字符，L1 装不下
        // 会溢出/截断、白烧一轮再靠失败升级。旧实现只在 Coding/None 判断，
        // Lightweight 被跳过。
        // 注意区分两件事：
        // - 升档：仅当基线是 L1 时抬到 L2（Coding 基线已是 L2，无需再升）
        // - reason：只要输入超长且基线 <L3，就标 long_input（含 Coding L2，
        //   保留原有「Coding 500k -> long_input」语义；Deep L3 维持 deep_task）
        let too_long = ctx.input_chars > self.long_input_chars;
        let long_input = too_long && base != Tier::L3;
        if too_long && base == Tier::L1 {
            base = Tier::L2;
        }
        let reason = if long_input {
            "long_input".to_string()
        } else {
            match ctx.kind {
                Some(TaskKind::Lightweight) => "lightweight_round".to_string(),
                Some(TaskKind::Deep) => "deep_task".to_string(),
                Some(TaskKind::Coding) | None => "default_coding".to_string(),
            }
        };
        RouteDecision { tier: base, reason }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(kind: Option<TaskKind>, fails: u32, chars: usize) -> RouteCtx {
        RouteCtx {
            task: TaskId::new("t-1"),
            round: 1,
            input_chars: chars,
            consecutive_failures: fails,
            forced_tier: None,
            kind,
        }
    }

    #[test]
    fn forced_tier_wins() {
        let r = RuleRouter::default();
        let mut c = ctx(None, 0, 0);
        c.forced_tier = Some(Tier::L1);
        let d = r.route(&c);
        assert_eq!(d.tier, Tier::L1);
        assert_eq!(d.reason, "user_forced");
    }

    #[test]
    fn lightweight_goes_l1() {
        let r = RuleRouter::default();
        assert_eq!(
            r.route(&ctx(Some(TaskKind::Lightweight), 0, 100)).tier,
            Tier::L1
        );
    }

    #[test]
    fn deep_goes_l3() {
        let r = RuleRouter::default();
        assert_eq!(r.route(&ctx(Some(TaskKind::Deep), 0, 100)).tier, Tier::L3);
    }

    #[test]
    fn failures_escalate() {
        let r = RuleRouter::default();
        // L1 轻量轮失败 2 次 → L2
        let d = r.route(&ctx(Some(TaskKind::Lightweight), 2, 100));
        assert_eq!(d.tier, Tier::L2);
        assert!(d.reason.contains("escalated"));
        // L3 不再升
        let d = r.route(&ctx(Some(TaskKind::Deep), 5, 100));
        assert_eq!(d.tier, Tier::L3);
    }

    #[test]
    fn lightweight_long_input_escalates_to_l2() {
        // 规则 5 对轻量轮同样生效：>200k 字符时 L1 装不下 → L2
        let r = RuleRouter::default();
        let d = r.route(&ctx(Some(TaskKind::Lightweight), 0, 250_000));
        assert_eq!(d.tier, Tier::L2);
        assert_eq!(d.reason, "long_input");
        // 临界内仍走 L1
        let d = r.route(&ctx(Some(TaskKind::Lightweight), 0, 200_000));
        assert_eq!(d.tier, Tier::L1);
    }

    #[test]
    fn coding_defaults_l2() {
        let r = RuleRouter::default();
        let d = r.route(&ctx(Some(TaskKind::Coding), 0, 50_000));
        assert_eq!(d.tier, Tier::L2);
        assert_eq!(d.reason, "default_coding");
        let d = r.route(&ctx(Some(TaskKind::Coding), 0, 500_000));
        assert_eq!(d.reason, "long_input");
    }
}
