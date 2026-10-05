// AppLog.swift — 轻量日志器：统一前缀 + 时间戳，stdout 与文件双写
//
// 用于排查 B3 核心分支（活动栏切换 / ⌘K 命令面板 / 急停）。
// 日志文件落 dataDir/logs/ui.log（append），便于用户回传排障。
import Foundation

public enum AppLog {
    /// 组件分类
    public enum Tag: String {
        case ui = "UI"
        case activity = "ACTIVITY"
        case emergency = "EMERGENCY"
    }

    /// 日志文件句柄（首次调用惰性初始化）
    private static var fileHandle: FileHandle? = {
        let dir = MaestroAPI.defaultDataDir().appendingPathComponent("logs")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("ui.log")
        FileManager.default.createFile(atPath: url.path, contents: nil)
        let h = try? FileHandle(forWritingTo: url)
        _ = try? h?.seekToEnd()
        return h
    }()

    public static func write(_ tag: Tag, _ message: String) {
        let stamp = AppLog.timestamp()
        let line = "[\(stamp)] [\(tag.rawValue)] \(message)"
        print(line)
        if let data = (line + "\n").data(using: .utf8) {
            fileHandle?.write(data)
        }
    }

    /// UI / 命令面板日志
    public static func ui(_ m: String) { write(.ui, m) }
    /// 活动栏切换日志
    public static func activity(_ m: String) { write(.activity, m) }
    /// 急停 / 恢复日志
    public static func emergency(_ m: String) { write(.emergency, m) }

    private static func timestamp() -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss.SSS"
        return f.string(from: Date())
    }
}
