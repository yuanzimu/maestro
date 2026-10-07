//! EventHub：事件总线。序号分配、持久化挂钩、订阅（有界队列防背压）。

use maestro_protocol::events::Envelope;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// 每订阅者队列容量：慢客户端超此容量即被断开（背压保护，调研坑 5）
pub const SUBSCRIBER_QUEUE_CAP: usize = 1024;

/// 订阅者句柄
pub struct Subscription {
    pub id: u64,
    pub rx: Receiver<Envelope>,
    /// 已送达的最大 seq（断开后客户端凭它重放补齐）
    pub last_seq: Arc<AtomicU64>,
}

/// 事件总线
/// 持久化回调类型（EventStore 挂这里；测试可挂 no-op）
pub type Sink = Box<dyn FnMut(&Envelope) + Send>;

pub struct EventHub {
    next_seq: AtomicU64,
    subscribers: Mutex<Subs>,
    next_sub_id: AtomicU64,
    /// 广播水位：已完成广播的最大 seq（订阅注册时取快照做重放/live 分界）
    broadcast_seq: AtomicU64,
    /// 持久化回调
    sink: Mutex<Option<Sink>>,
}

struct Subs {
    map: HashMap<u64, (SyncSender<Envelope>, Arc<AtomicU64>)>,
    /// 被背压断开的订阅者（诊断用）
    dropped: Vec<u64>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            next_seq: AtomicU64::new(1),
            subscribers: Mutex::new(Subs {
                map: HashMap::new(),
                dropped: vec![],
            }),
            next_sub_id: AtomicU64::new(1),
            broadcast_seq: AtomicU64::new(0),
            sink: Mutex::new(None),
        }
    }

    /// 挂持久化 sink（追加即写）
    pub fn set_sink(&self, f: Sink) {
        *self.sink.lock().unwrap() = Some(f);
    }

    /// 发布事件：分配 seq → 持久化 → 广播。
    /// 返回带 seq 的信封。慢订阅者队列满时**断开它**而不是阻塞发布。
    pub fn publish(&self, event: maestro_protocol::events::Event) -> Envelope {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let env = Envelope::new(seq, event);

        // 1. 持久化（先落盘再广播，崩溃时最多少送不丢账）
        if let Some(sink) = self.sink.lock().unwrap().as_mut() {
            sink(&env);
        }

        // 2. 广播（非阻塞；满即断开该订阅者）
        let mut to_drop = vec![];
        {
            let mut subs = self.subscribers.lock().unwrap();
            for (id, (tx, last_seq)) in subs.map.iter() {
                match tx.try_send(env.clone()) {
                    Ok(()) => {
                        last_seq.store(env.seq, Ordering::Release);
                    }
                    Err(TrySendError::Full(_)) => {
                        to_drop.push(*id);
                    }
                    Err(TrySendError::Disconnected(_)) => to_drop.push(*id),
                }
            }
            for id in to_drop {
                subs.map.remove(&id);
                subs.dropped.push(id);
            }
            // 广播水位推进（在锁内 —— 订阅注册读到的水位必含全部已完成广播）
            self.broadcast_seq.fetch_max(env.seq, Ordering::Release);
        }

        env
    }

    /// 订阅：from_seq>0 时先重放历史（由调用方注入重放闭包）
    pub fn subscribe(
        &self,
        replay: Option<&dyn Fn(u64) -> Vec<Envelope>>,
        from_seq: u64,
    ) -> Subscription {
        let (tx, rx) = std::sync::mpsc::sync_channel(SUBSCRIBER_QUEUE_CAP);
        let id = self.next_sub_id.fetch_add(1, Ordering::SeqCst);
        let last_seq = Arc::new(AtomicU64::new(0));
        // 先登记再重放（防重放期间丢新事件）。同一锁内取广播水位：
        // ≤ 水位的事件必然在注册前完成广播（没 live 给我们）→ 重放补；
        // > 水位的必然（或即将）走 live 投递 → 重放跳过。
        // 没有这个分界，「入库早于注册、广播晚于注册」的窗口事件会双份投递
        // （G90 负载场景抓到）。
        let floor = {
            let mut subs = self.subscribers.lock().unwrap();
            subs.map.insert(id, (tx, last_seq.clone()));
            self.broadcast_seq.load(Ordering::Acquire)
        };

        let sub = Subscription { id, rx, last_seq };

        // 重放历史补齐 [from_seq, floor]
        if from_seq > 0 && from_seq <= floor {
            if let Some(f) = replay {
                for env in f(from_seq) {
                    if env.seq > floor {
                        continue; // live 投递负责
                    }
                    // 重放走 try_send：慢客户端此时满队列同样断开
                    let mut subs = self.subscribers.lock().unwrap();
                    match subs.map.get(&id) {
                        Some((tx, _)) => {
                            if tx.try_send(env).is_err() {
                                subs.map.remove(&id);
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        sub
    }

    /// 恢复模式：把广播水位抬到 seq。重启后历史事件视为「已广播」——
    /// 否则 broadcast_seq=0 会让订阅的重放窗口 [from_seq, floor] 为空，
    /// 新客户端首连（from_seq=1）收不到任何历史事件（GUI 重启后事件流空白）
    pub fn set_broadcast_floor(&self, seq: u64) {
        self.broadcast_seq.fetch_max(seq, Ordering::SeqCst);
    }

    /// 恢复模式：把序号地板抬到 seq（重放历史后接着分配，不与历史冲突）
    pub fn set_seq_floor(&self, floor: u64) {
        let mut cur = self.next_seq.load(Ordering::SeqCst);
        while cur < floor {
            match self
                .next_seq
                .compare_exchange(cur, floor, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(now) => cur = now,
            }
        }
    }

    /// 退订
    pub fn unsubscribe(&self, id: u64) {
        self.subscribers.lock().unwrap().map.remove(&id);
    }

    /// 当前序号（下一个将分配的）
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst)
    }

    /// 最新已发布 seq（无事件时 0）。status.event_seq 用这个：
    /// 协议其余 seq 全是「含语义」（from_seq=N 重放含 N）—— 报 next 会让
    /// 客户端拿 status 做续订游标时漏掉最后一条（G63/G65 场景抓到）
    pub fn last_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst).saturating_sub(1)
    }

    /// 被背压断开的订阅者列表（诊断/测试）
    pub fn dropped_subscribers(&self) -> Vec<u64> {
        self.subscribers.lock().unwrap().dropped.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_protocol::events::Event;
    use maestro_protocol::types::TaskId;
    use maestro_protocol::Priority;

    fn ev(i: u64) -> Event {
        Event::TaskCreated {
            task: maestro_protocol::events::Task {
                id: TaskId::new(format!("t{i}")),
                title: "t".into(),
                workdir: "/tmp".into(),
                created_at: 0,
            },
            prompt: "p".into(),
        }
    }

    #[test]
    fn seq_is_monotonic() {
        let hub = EventHub::new();
        let e1 = hub.publish(ev(1));
        let e2 = hub.publish(ev(2));
        assert_eq!(e2.seq, e1.seq + 1);
        assert!(e2.ts >= e1.ts);
    }

    #[test]
    fn subscribers_receive_events() {
        let hub = EventHub::new();
        let sub = hub.subscribe(None, 0);
        hub.publish(ev(1));
        hub.publish(ev(2));
        let got = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(got.seq, 1);
        let got2 = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(got2.seq, 2);
        hub.unsubscribe(sub.id);
    }

    /// 订阅注册与重放查询之间发布的事件：恰好投递一次（不得 live + 重放双份）。
    /// 确定性复现：把「窗口内发布」做进重放闭包 —— f 执行时先 publish（live
    /// 投递给刚注册的订阅者）再把同一事件包含进重放结果。
    #[test]
    fn subscribe_replay_dedups_live_delivered() {
        let hub = EventHub::new();
        let history: std::sync::Arc<std::sync::Mutex<Vec<Envelope>>> =
            std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let h2 = history.clone();
        hub.set_sink(Box::new(move |env| {
            h2.lock().unwrap().push(env.clone());
        }));
        hub.publish(ev(1));
        hub.publish(ev(2));
        hub.publish(ev(3));

        let h3 = history.clone();
        let hub_ref = &hub;
        let replay = move |from: u64| -> Vec<Envelope> {
            // 模拟竞态窗口内的新发布：注册已完成 → live 投递；入库 → 重放可见
            let env = hub_ref.publish(ev(4));
            h3.lock().unwrap().push(env.clone());
            h3.lock()
                .unwrap()
                .iter()
                .filter(|e| e.seq >= from)
                .cloned()
                .collect()
        };
        let sub = hub.subscribe(Some(&replay), 1);

        let mut got: Vec<u64> = vec![];
        while let Ok(e) = sub.rx.recv_timeout(std::time::Duration::from_millis(300)) {
            got.push(e.seq);
        }
        got.sort();
        assert_eq!(got, vec![1, 2, 3, 4], "seq=4 恰好一次（live 或重放）");
        hub.unsubscribe(sub.id);
    }

    /// 调研坑 5 的验证：慢订阅者不拖死发布
    #[test]
    fn slow_subscriber_gets_disconnected_not_blocking() {
        let hub = EventHub::new();
        // 不消费的订阅者
        let sub = hub.subscribe(None, 0);
        // 灌满队列（容量 1024）
        let start = std::time::Instant::now();
        for i in 0..(SUBSCRIBER_QUEUE_CAP as u64 + 100) {
            hub.publish(ev(i));
        }
        let elapsed = start.elapsed();
        // 全部发布不应阻塞超过 1s（慢者被断开后继续）
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "publish 被拖死: {elapsed:?}"
        );
        assert!(!hub.dropped_subscribers().is_empty(), "慢订阅者应被断开");
        // 断开的订阅者 rx 应耗尽（剩余的旧事件）后无新事件
        drop(sub);
    }

    /// 重放补齐：from_seq 起的历史先于新事件送达
    #[test]
    fn replay_from_seq() {
        let hub = EventHub::new();
        let history = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let h2 = history.clone();
        hub.set_sink(Box::new(move |env| {
            h2.lock().unwrap().push(env.clone());
        }));
        hub.publish(ev(1));
        hub.publish(ev(2));
        hub.publish(ev(3));

        let h3 = history.clone();
        let replay = move |from: u64| -> Vec<Envelope> {
            h3.lock()
                .unwrap()
                .iter()
                .filter(|e| e.seq >= from)
                .cloned()
                .collect()
        };
        let sub = hub.subscribe(Some(&replay), 2);
        // 先收 seq=2,3（重放），再收 4（新发布）
        let a = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let b = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        hub.publish(ev(4));
        let c = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!((a.seq, b.seq, c.seq), (2, 3, 4));
    }

    /// 重启恢复场景：历史事件已入库但本次会话未广播（broadcast_seq=0）。
    /// set_broadcast_floor(max_seq) 后，新订阅 from_seq=1 必须重放全部历史；
    /// 恢复后新发布的事件（seq > max_seq）只走 live，不重放（无双份）。
    #[test]
    fn replay_after_restart_recovery() {
        let hub = EventHub::new();
        // 模拟重启恢复：历史 seq 1..=3 已在事件库（上次会话），本次未广播
        let history: Vec<Envelope> = (1..=3u64).map(|s| Envelope::new(s, ev(s))).collect();
        hub.set_seq_floor(4);
        hub.set_broadcast_floor(3);

        let replay = move |from: u64| -> Vec<Envelope> {
            history.iter().filter(|e| e.seq >= from).cloned().collect()
        };
        let sub = hub.subscribe(Some(&replay), 1);
        let got: Vec<u64> = (0..3)
            .map(|_| {
                sub.rx
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap()
                    .seq
            })
            .collect();
        assert_eq!(got, vec![1, 2, 3], "首连应重放全部历史");

        // 新事件 seq=4 走 live（且不会触发重放窗口 → 仅一份）
        hub.publish(ev(4));
        let d = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(d.seq, 4);
        assert!(
            sub.rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "不应有重复投递"
        );
    }

    /// 事件优先级透传（U9 埋点验证）
    #[test]
    fn priority_passes_through() {
        let hub = EventHub::new();
        let sub = hub.subscribe(None, 0);
        hub.publish(Event::TaskFailed {
            task: TaskId::new("t"),
            worker: maestro_protocol::WorkerId::new("w"),
            error: "x".into(),
        });
        let env = sub
            .rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(env.priority, Priority::Critical);
    }
}
