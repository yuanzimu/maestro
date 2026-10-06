// Checkpoint.swift — Checkpoint 时光机数据模型（对齐 daemon checkpoint.list 返回）
//
// wire（core.rs）：checkpoint_list → result.checkpoints: [{seq, reason, ref, commit}]
// reason 为 CpReason 的 snake_case：
//   baseline / round_start / emergency / manual / pre_rollback /
//   acceptance_passed / pre_merge
import Foundation

/// 轻量文本错误（异步 completion 的失败类型）
public struct SimpleError: Error, LocalizedError {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
}

public struct CheckpointInfo: Equatable {
    public let seq: Int
    /// 原始 reason 字符串（snake_case；daemon 未识别时可能为 nil）
    public let reason: String?
    /// 完整 git 引用 refs/maestro/cp/<task>/<seq>-<label>
    public let ref: String
    public let commit: String
    /// 所属轮次（来自 meta；旧 daemon/解析失败为 nil）
    public let round: Int?
    /// Unix 毫秒时间戳（来自 meta；可能为 nil）
    public let ts: UInt64?

    public init(seq: Int, reason: String?, ref: String, commit: String,
                round: Int?, ts: UInt64?) {
        self.seq = seq
        self.reason = reason
        self.ref = ref
        self.commit = commit
        self.round = round
        self.ts = ts
    }

    public static func from(_ d: [String: Any]) -> CheckpointInfo? {
        guard let ref = d["ref"] as? String,
              let seq = d["seq"] as? Int else { return nil }
        return CheckpointInfo(seq: seq,
                              reason: d["reason"] as? String,
                              ref: ref,
                              commit: d["commit"] as? String ?? "",
                              round: d["round"] as? Int,
                              ts: d["ts"] as? UInt64)
    }

    /// 是否永久保留（baseline/验收/合并前/回滚安全垫）
    public var isPinned: Bool {
        switch reason {
        case "baseline", "acceptance_passed", "pre_merge", "pre_rollback": return true
        default: return false
        }
    }

    /// reason 的人话标签
    public var reasonText: String {
        switch reason {
        case "baseline": return "起点"
        case "round_start":
            if let r = round { return "第 \(r) 轮开始" }
            return "轮起点"
        case "emergency": return "急停"
        case "manual": return "手动"
        case "pre_rollback": return "回滚安全垫"
        case "acceptance_passed":
            if let r = round { return "第 \(r) 轮验收通过" }
            return "验收通过"
        case "pre_merge": return "合并前"
        default: return reason ?? "?"
        }
    }

    /// 时间字符串（HH:mm:ss；无 ts 为 nil）
    public var timeString: String? {
        guard let ts else { return nil }
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f.string(from: Date(timeIntervalSince1970: Double(ts) / 1000))
    }

    /// 时间线显示的简短图标标记
    public var reasonIcon: String {
        switch reason {
        case "baseline": return "🚩"
        case "round_start": return "⏱"
        case "emergency": return "🛑"
        case "manual": return "📌"
        case "pre_rollback": return "↩️"
        case "acceptance_passed": return "✅"
        case "pre_merge": return "🔀"
        default: return "•"
        }
    }
}
