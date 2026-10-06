//! 基准报告：JSON（机器读）+ Markdown（人读）+ 验收率门禁。

use crate::runner::TaskOutcome;
use serde::Serialize;

/// 套件运行总报告。
#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub mode: String,
    pub generated_at: String,
    pub tasks: Vec<TaskOutcome>,
    pub summary: Summary,
}

/// 汇总口径（Sprint C 出口断言的素材）。
#[derive(Debug, Serialize)]
pub struct Summary {
    pub total: usize,
    pub done: usize,
    pub accept_pass: usize,
    /// 验收通过率（Sprint C 出口断言 ≥ 0.98）
    pub accept_rate: f64,
    pub rounds: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub actual_cost_cents: u64,
    pub counterfactual_cost_cents: u64,
    pub saved_cents: u64,
}

impl BenchReport {
    pub fn build(mode: &str, tasks: Vec<TaskOutcome>) -> Self {
        let total = tasks.len();
        let done = tasks.iter().filter(|t| t.state == "done").count();
        let accept_pass = tasks.iter().filter(|t| t.accept_pass).count();
        let sum = |f: fn(&TaskOutcome) -> u64| tasks.iter().map(f).sum();
        let accept_rate = if total == 0 {
            0.0
        } else {
            accept_pass as f64 / total as f64
        };
        let summary = Summary {
            total,
            done,
            accept_pass,
            accept_rate,
            rounds: sum(|t| t.rounds),
            input_tokens: sum(|t| t.input_tokens),
            output_tokens: sum(|t| t.output_tokens),
            cache_read_tokens: sum(|t| t.cache_read_tokens),
            actual_cost_cents: sum(|t| t.actual_cost_cents),
            counterfactual_cost_cents: sum(|t| t.counterfactual_cost_cents),
            saved_cents: sum(|t| t.saved_cents),
        };
        Self {
            mode: mode.to_string(),
            generated_at: now_iso(),
            tasks,
            summary,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn to_markdown(&self) -> String {
        let s = &self.summary;
        let mut md = String::new();
        md.push_str(&format!(
            "# Maestro 基准报告（{}）\n\n生成于 {}\n\n",
            self.mode, self.generated_at
        ));
        md.push_str(
            "| 任务 | 类型 | 终态 | 轮数 | 输入 tok | 输出 tok | 实际成本¢ | 省¢ | 验收 |\n",
        );
        md.push_str("|---|---|---|---|---|---|---|---|---|\n");
        for t in &self.tasks {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                t.id,
                t.kind,
                t.state,
                t.rounds,
                t.input_tokens,
                t.output_tokens,
                t.actual_cost_cents,
                t.saved_cents,
                if t.accept_pass { "PASS" } else { "FAIL" }
            ));
        }
        md.push_str(&format!(
            "\n**汇总**：验收通过率 **{:.1}%**（{}/{}，门禁 ≥98%）；任务完成 {}/{}；\
             总轮数 {}；输入 {} tok / 输出 {} tok / 缓存命中读 {} tok；\
             实际成本 {}¢ vs 反事实 {}¢（省 {}¢）\n",
            s.accept_rate * 100.0,
            s.accept_pass,
            s.total,
            s.done,
            s.total,
            s.rounds,
            s.input_tokens,
            s.output_tokens,
            s.cache_read_tokens,
            s.actual_cost_cents,
            s.counterfactual_cost_cents,
            s.saved_cents
        ));
        md
    }
}

fn now_iso() -> String {
    // 无 chrono 依赖：用 UNIX 秒（报告为机器产物，人读靠 markdown 时间戳同理）
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}
