//! B1 里程碑叙事：降级模板层（LLM 不可用 / P0 阶段的进度人话化）。
//!
//! 调研结论（R45）：Gemini「Topic & Update」是 LLM 压缩叙事的直接先例，
//! 但 LLM 缺席时各家都退化到原始事件流。Maestro 的降级路径：纯函数把
//! RoundProgress + LedgerEntry 聚合压成一句「第 N 轮：{工具}×{次数}，
//! 累计 {成本}，耗时 {t}」—— P1 接 LLM client 后本层变为 fallback。

/// 任务执行概况（事件流单遍聚合的产物）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskVitals {
    /// 已完成轮数（RoundProgress 计数）
    pub rounds: u32,
    /// 工具调用名 → 次数（按次数降序取前 3）
    pub tools: Vec<(String, u32)>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// 累计实付（美分）
    pub cost_cents: u64,
    pub wall_ms: u64,
    /// 上下文压缩次数（ContextCompacted）
    pub compactions: u32,
    /// 最近一轮的摘要（RoundProgress.summary，钳 40 字）
    pub last_summary: String,
}

/// 一句话进度（`maestro task get` 的 narrative 字段）。
/// 结构对齐调研建议的降级模板：「第 N 轮：{最频繁工具}×{次数}，累计 ${cost}，耗时 {t}」
pub fn progress_line(v: &TaskVitals) -> String {
    if v.rounds == 0 {
        return "尚未开始（无轮账）".into();
    }
    let mut parts = vec![format!("第 {} 轮", v.rounds)];
    if v.tools.is_empty() {
        parts.push("无工具调用".into());
    } else {
        let top: Vec<String> = v
            .tools
            .iter()
            .take(3)
            .map(|(name, n)| format!("{name}×{n}"))
            .collect();
        parts.push(top.join(" · "));
    }
    if v.compactions > 0 {
        parts.push(format!("上下文压缩 {} 次", v.compactions));
    }
    if v.cost_cents > 0 {
        parts.push(format!(
            "累计 {}¢（{}k in / {}k out）",
            v.cost_cents,
            v.tokens_in / 1000,
            v.tokens_out / 1000
        ));
    }
    parts.push(format!("耗时 {}", fmt_duration(v.wall_ms)));
    let mut line = parts.join("，");
    if !v.last_summary.is_empty() {
        line.push_str(&format!("｜最近：{}", v.last_summary));
    }
    line
}

/// 带任务状态的进度叙事（R58）：单发 worker（无轮账）的任务终态后，
/// 「尚未开始」语义错误 —— 按实际状态给终态叙事。
pub fn progress_line_with_state(
    v: &TaskVitals,
    state: &maestro_protocol::types::WorkerState,
) -> String {
    if v.rounds == 0 {
        use maestro_protocol::types::WorkerState;
        return match state {
            WorkerState::Done => "已完成（单发 worker，无轮账）".into(),
            WorkerState::Failed | WorkerState::Cancelled => "已结束（无轮账）".into(),
            WorkerState::Blocked => "阻塞中（无轮账）".into(),
            _ => "尚未开始（无轮账）".into(),
        };
    }
    progress_line(v)
}

fn fmt_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vitals() {
        assert_eq!(progress_line(&TaskVitals::default()), "尚未开始（无轮账）");
    }

    /// R58：无轮账的终态任务不再误报「尚未开始」
    #[test]
    fn terminal_state_without_rounds() {
        use maestro_protocol::types::WorkerState;
        let v = TaskVitals::default();
        assert_eq!(
            progress_line_with_state(&v, &WorkerState::Done),
            "已完成（单发 worker，无轮账）"
        );
        assert_eq!(
            progress_line_with_state(&v, &WorkerState::Failed),
            "已结束（无轮账）"
        );
        assert_eq!(
            progress_line_with_state(&v, &WorkerState::Blocked),
            "阻塞中（无轮账）"
        );
        // 非终态（排队/进行中）保持原叙事
        assert_eq!(
            progress_line_with_state(&v, &WorkerState::Queued),
            "尚未开始（无轮账）"
        );
        // 有轮账时与 progress_line 一致
        let full = TaskVitals {
            rounds: 2,
            wall_ms: 4_000,
            ..Default::default()
        };
        assert_eq!(
            progress_line_with_state(&full, &WorkerState::Done),
            progress_line(&full)
        );
    }

    #[test]
    fn full_vitals_line() {
        let v = TaskVitals {
            rounds: 5,
            tools: vec![
                ("Read".into(), 7),
                ("Bash".into(), 3),
                ("Grep".into(), 2),
                ("Write".into(), 1), // 第 4 名不进 top3
            ],
            tokens_in: 12_000,
            tokens_out: 2_400,
            cost_cents: 12,
            wall_ms: 83_000,
            compactions: 1,
            last_summary: "重构 auth 模块并补测试".into(),
        };
        let s = progress_line(&v);
        assert!(s.starts_with("第 5 轮，Read×7 · Bash×3 · Grep×2"), "{s}");
        assert!(s.contains("上下文压缩 1 次"), "{s}");
        assert!(s.contains("累计 12¢（12k in / 2k out）"), "{s}");
        assert!(s.contains("耗时 1m23s"), "{s}");
        assert!(s.contains("｜最近：重构 auth 模块并补测试"), "{s}");
    }

    #[test]
    fn minimal_vitals_no_noise() {
        // 零成本/零压缩/无摘要 → 不产生空片段
        let v = TaskVitals {
            rounds: 1,
            tools: vec![],
            wall_ms: 4_000,
            ..Default::default()
        };
        assert_eq!(progress_line(&v), "第 1 轮，无工具调用，耗时 4s");
    }
}
