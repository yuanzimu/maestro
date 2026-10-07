//! Steering 队列：轻推消息持久化 + 多轮驱动的轮间投递点（0.16）。
//!
//! 关键语义：
//! - 消息入队即持久化（kill -9 不丢 —— P0 验收）
//! - **at-least-once 投递（R32，调研落地 opencode-queue 语义）**：
//!   poll 取走消息进入 inflight（未确认）；worker 用过（下一轮跑完）后经
//!   TaskSteerAck 确认。未确认的消息：再次 poll 重投、respawn 时随
//!   prompt 前置注入（daemon 确认的投递）、终态时 SteeringDropped。
//!   轮边界 poll 与下一轮启动之间 worker 被杀 → 消息重投，不丢。
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
    /// 已投递未确认（at-least-once）；持久化跨重启
    #[serde(default)]
    pub inflight: bool,
}

/// 队列（Core 独占；持久化到 data_dir/steering.jsonl）
pub struct SteeringQueue {
    queues: std::collections::HashMap<TaskId, VecDeque<SteeringMsg>>,
    inflight: std::collections::HashMap<TaskId, Vec<SteeringMsg>>,
    next_seq: u64,
    file: PathBuf,
}

impl SteeringQueue {
    pub fn open(data_dir: &Path) -> Self {
        let file = data_dir.join("steering.jsonl");
        let mut q = Self {
            queues: Default::default(),
            inflight: Default::default(),
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
            inflight: Default::default(),
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
                if msg.inflight {
                    self.inflight
                        .entry(msg.task.clone())
                        .or_default()
                        .push(msg.clone());
                } else {
                    self.queues
                        .entry(msg.task.clone())
                        .or_default()
                        .push_back(msg.clone());
                }
                if msg.seq >= self.next_seq {
                    self.next_seq = msg.seq + 1;
                }
            }
        }
    }

    fn persist_append(&self, msg: &SteeringMsg) -> std::io::Result<()> {
        if self.file.as_os_str() == "/dev/null" {
            return Ok(());
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?;
        writeln!(f, "{}", serde_json::to_string(msg).unwrap_or_default())?;
        f.sync_data()
    }

    fn persist_rewrite(&self) -> std::io::Result<()> {
        if self.file.as_os_str() == "/dev/null" {
            return Ok(());
        }
        let mut lines = vec![];
        for q in self.queues.values() {
            for m in q {
                lines.push(serde_json::to_string(m).unwrap_or_default());
            }
        }
        for list in self.inflight.values() {
            for m in list {
                lines.push(serde_json::to_string(m).unwrap_or_default());
            }
        }
        let tmp = self.file.with_extension("jsonl.tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, &self.file)
    }

    /// 入队并持久化。持久化失败时返回 Err（不破坏 kill -9 不丢的承诺：
    /// 磁盘满/不可写时必须让上层知道，而非静默吞掉让用户误以为已落盘）。
    /// 消息已先入内存队列；持久化失败时保留在内存（本进程内仍可投递）。
    pub fn push(&mut self, task: &TaskId, message: String) -> Result<SteeringMsg, String> {
        let msg = SteeringMsg {
            seq: self.next_seq,
            task: task.clone(),
            message,
            queued_at: maestro_protocol::now_ms(),
            inflight: false,
        };
        self.next_seq += 1;
        self.queues
            .entry(task.clone())
            .or_default()
            .push_back(msg.clone());
        self.persist_append(&msg)
            .map_err(|e| format!("轻推持久化失败: {e}"))?;
        Ok(msg)
    }

    /// poll：取走积压（进入 inflight）+ 重投未确认的（at-least-once）
    pub fn poll(&mut self, task: &TaskId) -> Vec<SteeringMsg> {
        let queued: Vec<SteeringMsg> = self
            .queues
            .get_mut(task)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default();
        let inflight = self.inflight.entry(task.clone()).or_default();
        for mut m in queued {
            m.inflight = true;
            inflight.push(m);
        }
        let out = inflight.clone();
        if !out.is_empty() {
            // 尽力而为：内存队列是本进程权威，重写失败不影响本轮投递
            let _ = self.persist_rewrite();
        }
        out
    }

    /// 确认消费（worker 用过后上报）：从 inflight 移除。返回确认数。
    pub fn ack(&mut self, task: &TaskId, seqs: &[u64]) -> usize {
        let Some(list) = self.inflight.get_mut(task) else {
            return 0;
        };
        let before = list.len();
        list.retain(|m| !seqs.contains(&m.seq));
        let n = before - list.len();
        if list.is_empty() {
            self.inflight.remove(task);
        }
        if n > 0 {
            let _ = self.persist_rewrite();
        }
        n
    }

    /// respawn / 重试注入：取走全部（积压 + 未确认）。
    /// daemon 侧直接进 prompt = daemon 确认的投递（不要求 worker 再 ack）
    pub fn take_all(&mut self, task: &TaskId) -> Vec<SteeringMsg> {
        let mut out: Vec<SteeringMsg> = self
            .queues
            .get_mut(task)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default();
        out.extend(self.inflight.remove(task).unwrap_or_default());
        if !out.is_empty() {
            let _ = self.persist_rewrite();
        }
        out
    }

    /// hold/终态：丢弃积压 + 未确认（调用方对每条发 SteeringDropped —— 不静默）
    pub fn drop_all(&mut self, task: &TaskId) -> Vec<SteeringMsg> {
        self.take_all(task) // 同 take_all；语义差异在调用方发什么事件
    }

    pub fn pending(&self, task: &TaskId) -> usize {
        self.queues.get(task).map(|q| q.len()).unwrap_or(0)
            + self.inflight.get(task).map(|l| l.len()).unwrap_or(0)
    }

    pub fn total_pending(&self) -> usize {
        self.queues.values().map(|q| q.len()).sum::<usize>()
            + self.inflight.values().map(|l| l.len()).sum::<usize>()
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
            q.push(&TaskId::new("t1"), "别动那个文件".into()).unwrap();
            q.push(&TaskId::new("t1"), "先跑测试".into()).unwrap();
            q.push(&TaskId::new("t2"), "other".into()).unwrap();
        }
        let mut q2 = SteeringQueue::open(tmp.path());
        assert_eq!(q2.pending(&TaskId::new("t1")), 2);
        assert_eq!(q2.pending(&TaskId::new("t2")), 1);
        let drained = q2.poll(&TaskId::new("t1"));
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "别动那个文件", "顺序保持");
        assert_eq!(drained[1].message, "先跑测试", "顺序保持");
        // poll 后未确认：pending 仍计入（at-least-once）
        assert_eq!(q2.pending(&TaskId::new("t1")), 2, "未确认应仍在册");
        // take_all（respawn）取走后重开：不复活
        let _ = q2.take_all(&TaskId::new("t1"));
        let q3 = SteeringQueue::open(tmp.path());
        assert_eq!(q3.pending(&TaskId::new("t1")), 0);
        assert_eq!(q3.pending(&TaskId::new("t2")), 1);
    }

    /// at-least-once：poll 未确认 → 重开重投；ack 后不再投
    #[test]
    fn inflight_redelivered_until_acked() {
        let tmp = tempfile::tempdir().unwrap();
        let mut q = SteeringQueue::open(tmp.path());
        let m = q.push(&TaskId::new("t"), "重要指示".into()).unwrap();
        // 第一次 poll：取走
        let got = q.poll(&TaskId::new("t"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seq, m.seq);
        // 未确认：再次 poll 重投同一条
        let again = q.poll(&TaskId::new("t"));
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].seq, m.seq, "未确认消息应重投");
        // kill -9 重开：inflight 仍在（持久化）
        drop(q);
        let mut q2 = SteeringQueue::open(tmp.path());
        assert_eq!(q2.pending(&TaskId::new("t")), 1, "inflight 应持久化");
        let after = q2.poll(&TaskId::new("t"));
        assert_eq!(after.len(), 1, "重启后未确认消息仍可重投");
        // ack 后清空
        assert_eq!(q2.ack(&TaskId::new("t"), &[m.seq]), 1);
        assert_eq!(q2.pending(&TaskId::new("t")), 0);
        assert!(q2.poll(&TaskId::new("t")).is_empty(), "ack 后不再投");
        // 重复 ack：无效果无报错
        assert_eq!(q2.ack(&TaskId::new("t"), &[m.seq]), 0);
    }

    /// ack 多条 + 混合积压：只清 ack 的，新积压保留
    #[test]
    fn ack_partial_keeps_new_queue() {
        let mut q = SteeringQueue::in_memory();
        let t = TaskId::new("t");
        let a = q.push(&t, "a".into()).unwrap().seq;
        let polled = q.poll(&t);
        assert_eq!(polled.len(), 1);
        let _ = q.push(&t, "b".into()).unwrap(); // poll 后新入队
        assert_eq!(q.ack(&t, &[a]), 1);
        let next = q.poll(&t);
        assert_eq!(next.len(), 1, "新积压应可投");
        assert_eq!(next[0].message, "b");
    }

    /// seq 持续递增（跨重启）
    #[test]
    fn seq_monotonic_across_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let s1 = {
            let mut q = SteeringQueue::open(tmp.path());
            q.push(&TaskId::new("t"), "a".into()).unwrap().seq
        };
        let mut q2 = SteeringQueue::open(tmp.path());
        let s2 = q2.push(&TaskId::new("t"), "b".into()).unwrap().seq;
        assert!(s2 > s1, "seq 跨重启递增: {s1} -> {s2}");
    }
}
