// MaestroAPI.swift — daemon 的 JSON-RPC 客户端 + 数据模型
// 协议与 crates/maestro-protocol 对齐：{"id","method","params"}\n → {"ok":{...}}\n / {"err":{...}}\n
import Foundation

enum MaestroError: Error, CustomStringConvertible {
    case connect(String)
    case rpc(code: Int, message: String)
    case badResponse(String)

    var description: String {
        switch self {
        case .connect(let m): return "连接失败（daemon 未启动？）\(m)"
        case .rpc(let code, let message): return "RPC 错误 \(code): \(message)"
        case .badResponse(let m): return "协议错误: \(m)"
        }
    }
}

// MARK: - 模型（task_list / task_get / inbox_list / server_status / worker_list 的返回形状）

struct TaskSummary: Identifiable, Equatable {
    let id: String
    let title: String
    let state: String        // queued/planning/working/blocked/suspended/done/failed/cancelled
    let round: Int
    let narrative: String?
    let blockedKind: String?
    let suspendReason: String?
    let acceptanceFailures: Int

    var isTerminal: Bool {
        state == "done" || state == "failed" || state == "cancelled"
    }

    static func from(_ d: [String: Any]) -> TaskSummary? {
        guard let id = d["id"] as? String else { return nil }
        return TaskSummary(
            id: id,
            title: d["title"] as? String ?? "(无标题)",
            state: d["state"] as? String ?? "?",
            round: d["round"] as? Int ?? 0,
            narrative: d["narrative"] as? String,
            blockedKind: d["blocked_kind"] as? String,
            suspendReason: d["suspend_reason"] as? String,
            acceptanceFailures: d["acceptance_failures"] as? Int ?? 0
        )
    }
}

struct ServerStatus {
    let version: String
    let pid: Int
    let uptimeSecs: Int
    let tasksTotal: Int
    let workersActive: Int
    let eventSeq: UInt64

    static func from(_ d: [String: Any]) -> ServerStatus? {
        guard let version = d["version"] as? String else { return nil }
        return ServerStatus(
            version: version,
            pid: d["pid"] as? Int ?? 0,
            uptimeSecs: d["uptime_secs"] as? Int ?? 0,
            tasksTotal: d["tasks_total"] as? Int ?? 0,
            workersActive: d["workers_active"] as? Int ?? 0,
            eventSeq: d["event_seq"] as? UInt64 ?? 0
        )
    }
}

struct InboxItem: Identifiable {
    let task: String
    let kind: String?
    let title: String
    var id: String { task }

    var kindText: String {
        switch kind {
        case "permission": return "等权限批准"
        case "plan_approval": return "方案待审"
        case "goal_stalled": return "无进展 3 轮"
        case "acceptance_failed": return "验收 3 次失败"
        case "infra": return "基建故障"
        case "rounds_exhausted": return "轮数耗尽"
        default: return kind ?? "待决策"
        }
    }

    static func from(_ d: [String: Any]) -> InboxItem? {
        guard let task = d["task"] as? String else { return nil }
        return InboxItem(task: task,
                         kind: d["kind"] as? String,
                         title: d["title"] as? String ?? task)
    }
}

// MARK: - API 客户端

final class MaestroAPI {
    let dataDir: URL
    private var reqCounter = 0

    init(dataDir: URL) { self.dataDir = dataDir }

    var apiSocketPath: String { dataDir.appendingPathComponent("maestro.api.sock").path }
    var eventsSocketPath: String { dataDir.appendingPathComponent("maestro.events.sock").path }

    /// 与 Rust 侧 transport::default_data_dir() 语义一致：
    /// $MAESTRO_DATA_DIR > $TMPDIR/maestro（macOS 上 std::env::temp_dir() 即 $TMPDIR）
    static func defaultDataDir() -> URL {
        let env = ProcessInfo.processInfo.environment
        if let dir = env["MAESTRO_DATA_DIR"] { return URL(fileURLWithPath: dir) }
        let tmp = env["TMPDIR"] ?? "/tmp"
        return URL(fileURLWithPath: tmp).appendingPathComponent("maestro")
    }

    var isDaemonAlive: Bool {
        UnixSocketConnection.canConnect(path: apiSocketPath)
    }

    /// 同步调用（在后台线程使用）。每次新建连接 —— 与 maestro-cli 同款行为。
    func call(_ method: String, params: [String: Any] = [:]) throws -> [String: Any] {
        reqCounter += 1
        let req: [String: Any] = ["id": "gui-\(reqCounter)", "method": method, "params": params]
        let body = try JSONSerialization.data(withJSONObject: req)
        let conn = try UnixSocketConnection(path: apiSocketPath)
        try conn.writeAll(body)
        try conn.writeAll(Data([0x0A]))

        guard let line = try conn.readLine(), !line.isEmpty else {
            throw MaestroError.badResponse("空响应")
        }
        guard let obj = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else {
            throw MaestroError.badResponse("响应不是 JSON 对象")
        }
        if let ok = obj["ok"] as? [String: Any] {
            if let result = ok["result"] as? [String: Any] { return result }
            if let result = ok["result"] { _ = result }
            return [:]
        }
        if let err = obj["err"] as? [String: Any],
           let error = err["error"] as? [String: Any] {
            throw MaestroError.rpc(code: (error["code"] as? Int) ?? -1,
                                   message: (error["message"] as? String) ?? "unknown")
        }
        throw MaestroError.badResponse(String(data: line.prefix(200), encoding: .utf8) ?? "?")
    }
}
