// EventStream.swift — 事件流订阅（events socket 常驻连接 + 自动重连）
// 协议：connect 后发 {"from_seq": N}\n，随后每行一个 Envelope {seq, ts, priority, event:{type,...}}
import Foundation

struct MaestroEvent: Identifiable {
    let seq: UInt64
    let ts: UInt64          // Unix 毫秒
    let priority: String    // info / warning / critical
    let type: String
    let payload: [String: Any]

    var id: UInt64 { seq }
    var taskID: String? { payload["task"] as? String }

    var timeString: String {
        let date = Date(timeIntervalSince1970: Double(ts) / 1000)
        let fmt = DateFormatter()
        fmt.dateFormat = "HH:mm:ss"
        return fmt.string(from: date)
    }

    var typeText: String {
        MaestroEvent.typeNames[type] ?? type
    }

    /// 从 payload 里挑最有人话价值的一句
    var detailText: String {
        for key in ["summary", "message", "error", "milestone", "reason", "cause", "output", "model"] {
            if let s = payload[key] as? String, !s.isEmpty {
                return String(s.prefix(90))
            }
        }
        if let r = payload["round"] as? Int { return "第 \(r) 轮" }
        if let workers = payload["workers"] as? [Any] { return "冻结 \(workers.count) 个 worker" }
        return ""
    }

    static let typeNames: [String: String] = [
        "task_created": "任务创建",
        "task_started": "开始执行",
        "task_completed": "✅ 完成",
        "task_failed": "❌ 失败",
        "task_cancelled": "已取消",
        "task_requeued": "重新入队",
        "worker_spawned": "Worker 启动",
        "worker_died": "Worker 退出",
        "goal_progress": "目标轮进展",
        "acceptance_gate_passed": "验收通过",
        "acceptance_gate_failed": "验收失败",
        "suspended": "💤 挂起",
        "resumed": "已恢复",
        "resume_attempt": "恢复尝试",
        "auto_recovery_exhausted": "自动恢复耗尽",
        "emergency_stopped": "🛑 急停",
        "emergency_snapshotted": "急停快照",
        "steering_dropped": "轻推被丢弃",
        "steering_queued": "轻推已排队",
        "steering_delivered": "轻推已投递",
        "checkpoint_created": "检查点",
        "checkpoint_rolled_back": "已回滚",
        "narrative_snapshot": "进展摘要",
        "feedback_recorded": "反馈记录",
        "ledger_entry": "计费入账",
        "round_progress": "轮进度",
        "cost_drift": "费用漂移",
        "context_compacted": "上下文压缩",
        "rounds_exhausted": "轮数耗尽",
        "provider_switched": "切换供应商",
    ]
}

final class EventStream {
    var onEvent: ((MaestroEvent) -> Void)?
    var onDisconnect: (() -> Void)?

    private let socketPath: String
    private var thread: Thread?
    private var stopped = false
    private var lastSeq: UInt64 = 0

    init(socketPath: String, fromSeq: UInt64 = 0) {
        self.socketPath = socketPath
        self.lastSeq = fromSeq
    }

    func start() {
        stopped = false
        let t = Thread { [weak self] in self?.runLoopBody() }
        t.name = "maestro-events"
        t.start()
        thread = t
    }

    func stop() {
        stopped = true
    }

    private func runLoopBody() {
        while !stopped {
            do {
                let conn = try UnixSocketConnection(path: socketPath)
                let hello = try JSONSerialization.data(withJSONObject: ["from_seq": lastSeq])
                try conn.writeAll(hello)
                try conn.writeAll(Data([0x0A]))
                while !stopped, let line = try conn.readLine() {
                    guard !line.isEmpty else { continue }
                    guard let obj = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
                          let seq = obj["seq"] as? UInt64 else { continue }
                    lastSeq = max(lastSeq, seq)
                    let ev = obj["event"] as? [String: Any] ?? [:]
                    let event = MaestroEvent(
                        seq: seq,
                        ts: obj["ts"] as? UInt64 ?? 0,
                        priority: obj["priority"] as? String ?? "info",
                        type: ev["type"] as? String ?? "?",
                        payload: ev
                    )
                    if stopped { break }
                    onEvent?(event)
                }
            } catch {
                // 断开：走重连
            }
            if stopped { break }
            onDisconnect?()
            Thread.sleep(forTimeInterval: 1.5)
        }
    }
}
