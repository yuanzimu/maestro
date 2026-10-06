// EventStream.swift — 事件流订阅（events socket 常驻连接 + 自动重连）
// 协议：connect 后发 {"from_seq": N}\n，随后每行一个 Envelope {seq, ts, priority, event:{type,...}}
import Foundation

public struct MaestroEvent: Identifiable {
    public let seq: UInt64
    public let ts: UInt64          // Unix 毫秒
    public let priority: String    // info / warning / critical
    public let type: String
    public let payload: [String: Any]

    public init(seq: UInt64, ts: UInt64, priority: String, type: String, payload: [String: Any]) {
        self.seq = seq
        self.ts = ts
        self.priority = priority
        self.type = type
        self.payload = payload
    }

    public var id: UInt64 { seq }
    /// 事件归属任务：多数事件 task 是字符串 id；task_created/worker_died 等
    /// 携带完整对象的取其 id 字段（否则 GUI 事件列对不上任务）
    public var taskID: String? {
        if let s = payload["task"] as? String { return s }
        if let obj = payload["task"] as? [String: Any] { return obj["id"] as? String }
        return nil
    }

    public var timeString: String {
        let date = Date(timeIntervalSince1970: Double(ts) / 1000)
        let fmt = DateFormatter()
        fmt.dateFormat = "HH:mm:ss"
        return fmt.string(from: date)
    }

    public var typeText: String {
        MaestroEvent.typeNames[type] ?? type
    }

    /// 从 payload 里挑最有人话价值的一句
    public var detailText: String {
        switch type {
        // 反馈：👍/👎 + 理由
        case "feedback_recorded":
            let pos = payload["positive"] as? Bool ?? false
            if let reason = payload["reason"] as? String, !reason.isEmpty {
                return (pos ? "👍 " : "👎 ") + String(reason.prefix(80))
            }
            return pos ? "👍 满意" : "👎 不满意"
        // 急停解除：恢复的任务数
        case "emergency_resumed":
            let n = (payload["resumed"] as? [Any])?.count ?? 0
            return n == 0 ? "无冻结任务" : "恢复 \(n) 个任务"
        // 急停冻结：worker 数
        case "emergency_stopped":
            if let workers = payload["workers"] as? [Any] { return "冻结 \(workers.count) 个 worker" }
        // 上下文压缩
        case "context_compacted":
            if let r = payload["round"] as? Int { return "第 \(r) 轮后已压缩" }
        // 断点续跑重排队
        case "task_requeued":
            if let r = payload["round"] as? Int { return "回到第 \(r) 轮锚点重跑" }
        // 批次失败
        case "batch_failed":
            if let n = payload["failed_items"] as? Int {
                let base = "\(n) 项失败"
                return (payload["reason"] as? String).map { "\(base)：\($0)" } ?? base
            }
        default: break
        }
        for key in ["summary", "message", "error", "milestone", "reason", "cause", "output", "model"] {
            if let s = payload[key] as? String, !s.isEmpty {
                return String(s.prefix(90))
            }
        }
        if let r = payload["round"] as? Int { return "第 \(r) 轮" }
        if let workers = payload["workers"] as? [Any] { return "冻结 \(workers.count) 个 worker" }
        return ""
    }

    public static let typeNames: [String: String] = [
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
        "emergency_resumed": "✅ 急停解除",
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
        "batch_marked_suspended": "批次挂起",
        "batch_failed": "批次失败",
    ]
}

public final class EventStream {
    public var onEvent: ((MaestroEvent) -> Void)?
    public var onDisconnect: (() -> Void)?

    private let socketPath: String
    private var thread: Thread?
    private var stopped = false
    private var lastSeq: UInt64 = 0
    private var currentConn: UnixSocketConnection?

    public init(socketPath: String, fromSeq: UInt64 = 0) {
        self.socketPath = socketPath
        self.lastSeq = fromSeq
    }

    public func start() {
        stopped = false
        let t = Thread { [weak self] in self?.runLoopBody() }
        t.name = "maestro-events"
        t.start()
        thread = t
    }

    /// 停止：关断当前连接（打断阻塞 read），线程 ≤ 重连间隔内退出
    public func stop() {
        stopped = true
        currentConn?.close()
    }

    /// 线程是否已退出（测试用：stop 后应在重连间隔内变 true）
    public var hasFinished: Bool { thread?.isFinished ?? true }

    private func runLoopBody() {
        while !stopped {
            do {
                let conn = try UnixSocketConnection(path: socketPath)
                currentConn = conn
                // 续订游标：0 = 从现在开始（协议约定）；断线重连时 seq >= from 为
                // **含**语义 —— 用 lastSeq+1 才不重放最后一条（否则 GUI 里出现重复行）
                let fromSeq = lastSeq == 0 ? 0 : lastSeq + 1
                let hello = try JSONSerialization.data(withJSONObject: ["from_seq": fromSeq])
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
            currentConn = nil
            if stopped { break }
            onDisconnect?()
            Thread.sleep(forTimeInterval: 1.5)
        }
    }
}
