//! Steering 队列：轻推消息持久化 + 多轮驱动的轮间投递点（0.16）。
//!
//! 关键语义：
//! - 消息入队即持久化（kill -9 不丢 —— P0 验收）
//! - resume 时 flush（按序投递）或 hold（丢弃但每条发 SteeringDropped 事件，不静默）
//! - 同一任务排队，投递发生在轮边界（多轮驱动）

use maestro_protocol::types::TaskId;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// 一条轻推消息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SteeringMsg {
    pub seq: u64,
    pub task: TaskId,
    pub message: String,
    pub queued_at: u64,
}

/// 队列（Core 独占；持久化到 data_dir/steering.jsonl）
pub struct SteeringQueue {
    queues: std::collections::HashMap<TaskId, VecDeque<SteeringMsg>>,
    next_seq: u64,
    file: PathBuf,
}

impl SteeringQueue {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("steering.jsonl");
        let mut q = Self {
            queues: Default::default(),
            next_seq: 1,
            file,
        };
        q.load();
        q
    }

    /// 测试用（不持久化）
    pub fn in_memory() -> Self {
        Self {
            queues: Default::default(),
            next_seq: 1,
            file: PathBuf::from("/dev/null"),
        }
    }

    fn load(&mut self) {
        let Ok(content) = std::fs::read_to_string(&self.file) else {
            return;
        };
        for line in content.lines() {
            if let Ok(msg) = serde_json::from_str::<SteeringMsg>(line) {
                self.queues
                    .entry(msg.task.clone())
                    .or_default()
                    .push_back(msg.clone());
                if msg.seq >= self.next_seq {
                    self.next_seq = msg.seq + 1;
                }
            }
        }
    }

    fn persist_append(&self, msg: &SteeringMsg) {
        if self.file.as_os_str() == "/dev/null" {
            return;
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            let _ = writeln!(f, "{}", serde_json::to_string(msg).unwrap_or_default());
            let _ = f.sync_data();
        }
    }

    fn persist_rewrite(&self) {
        if self.file.as_os_str() == "/dev/null" {
            return;
        }
        let mut lines = vec![];
        for q in self.queues.values() {
            for m in q {
                lines.push(serde_json::to_string(m).unwrap_or_default());
            }
        }
        let tmp = self.file.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, lines.join("\n") + "\n").is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }

    /// 入队（持久化后返回消息）
    pub fn push(&mut self, task: &TaskId, message: String) -> SteeringMsg {
        let msg = SteeringMsg {
            seq: self.next_seq,
            task: task.clone(),
            message,
            queued_at: maestro_protocol::now_ms(),
        };
        self.next_seq += 1;
        self.queues
            .entry(task.clone())
            .or_default()
            .push_back(msg.clone());
        self.persist_append(&msg);
        msg
    }

    /// flush：取走该任务全部积压（按序）
    pub fn drain(&mut self, task: &TaskId) -> Vec<SteeringMsg> {
        let out: Vec<SteeringMsg> = self
            .queues
            .get_mut(task)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default();
        if !out.is_empty() {
            self.persist_rewrite();
        }
        out
    }

    /// hold：丢弃并返回（调用方对每条发 SteeringDropped 事件 —— 不静默，用例 B7）
    pub fn drop_all(&mut self, task: &TaskId) -> Vec<SteeringMsg> {
        self.drain(task) // 同 drain；语义差异在调用方发什么事件
    }

    pub fn pending(&self, task: &TaskId) -> usize {
        self.queues.get(task).map(|q| q.len()).unwrap_or(0)
    }

    pub fn total_pending(&self) -> usize {
        self.queues.values().map(|q| q.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0 验收路径：入队 → 重开（模拟 kill -9）→ 消息还在
    #[test]
    fn persisted_across_restart() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let mut q = SteeringQueue::open(tmp.path());
            q.push(&TaskId::new("t1"), "别动那个文件".into());
            q.push(&TaskId::new("t1"), "先跑测试".into());
            q.push(&TaskId::new("t2"), "other".into());
        }
        let mut q2 = SteeringQueue::open(tmp.path());
        assert_eq!(q2.pending(&TaskId::new("t1")), 2);
        assert_eq!(q2.pending(&TaskId::new("t2")), 1);
        let drained = q2.drain(&TaskId::new("t1"));
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "别动那个文件", "顺序保持");
        assert_eq!(drained[1].message, "先跑测试");
        // drain 后重开：已投递的不复活
        let q3 = SteeringQueue::open(tmp.path());
        assert_eq!(q3.pending(&TaskId::new("t1")), 0);
        assert_eq!(q3.pending(&TaskId::new("t2")), 1);
    }

    /// seq 持续递增（跨重启）
    #[test]
    fn seq_monotonic_across_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let s1 = {
            let mut q = SteeringQueue::open(tmp.path());
            q.push(&TaskId::new("t"), "a".into()).seq
        };
        let mut q2 = SteeringQueue::open(tmp.path());
        let s2 = q2.push(&TaskId::new("t"), "b".into()).seq;
        assert!(s2 > s1, "seq 跨重启递增: {s1} -> {s2}");
    }
}
