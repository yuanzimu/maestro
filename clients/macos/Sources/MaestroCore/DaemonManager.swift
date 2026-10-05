// DaemonManager.swift — 定位/拉起内嵌 daemon，检测本机可用的 AI CLI
import Foundation

public enum AICli: String, CaseIterable {
    case claude, codex, gemini, amp, opencode

    public var displayName: String {
        switch self {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .gemini: return "Gemini CLI"
        case .amp: return "Amp"
        case .opencode: return "OpenCode"
        }
    }
    /// daemon/rounder 的方言名（MAESTRO_CLI_DIALECT，与 adapter.rs 对齐）
    public var dialect: String { rawValue }
}

public final class DaemonManager {
    public let dataDir: URL
    private(set) public var daemonPath: URL?
    private(set) public var rounderPath: URL?
    private(set) public var cliPath: URL?
    private(set) public var detectedCLI: AICli?
    private var process: Process?
    /// 引擎设置环境覆盖（网关/预算；由 AppState 从 EngineSettingsStore 注入）
    public var engineEnv: [String: String] = [:]

    public init(dataDir: URL) {
        self.dataDir = dataDir
    }

    // MARK: - 二进制定位（bundle 内优先；开发模式回退 PATH 搜索）

    public func locateBinaries() {
        let bundleBin = Bundle.main.resourceURL?.appendingPathComponent("bin")
        daemonPath = findBinary("maestro-daemon", bundleHint: bundleBin?.appendingPathComponent("maestro-daemon"))
        rounderPath = findBinary("maestro-rounder", bundleHint: bundleBin?.appendingPathComponent("maestro-rounder"))
        detectInnerCLI()
    }

    private func findBinary(_ name: String, bundleHint: URL?) -> URL? {
        if let hint = bundleHint, isExecutable(hint) { return hint }
        // PATH 搜索（开发模式：cargo target/release 或已安装到系统）
        let env = ProcessInfo.processInfo.environment
        var dirs = (env["PATH"] ?? "").split(separator: ":").map { URL(fileURLWithPath: String($0)) }
        dirs.append(URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".cargo/bin"))
        for d in dirs {
            let candidate = d.appendingPathComponent(name)
            if isExecutable(candidate) { return candidate }
        }
        return nil
    }

    private func isExecutable(_ url: URL) -> Bool {
        (try? url.checkResourceIsReachable()) ?? false
    }

    /// 检测本机第一个可用的 AI CLI（claude/codex/gemini/amp/opencode）
    private func detectInnerCLI() {
        let env = ProcessInfo.processInfo.environment
        var dirs = (env["PATH"] ?? "").split(separator: ":").map { URL(fileURLWithPath: String($0)) }
        dirs.append(URL(fileURLWithPath: "/usr/local/bin"))
        dirs.append(URL(fileURLWithPath: "/opt/homebrew/bin"))
        dirs.append(URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".local/bin"))
        for cli in AICli.allCases {
            for d in dirs {
                let candidate = d.appendingPathComponent(cli.rawValue)
                if isExecutable(candidate) {
                    detectedCLI = cli
                    cliPath = candidate
                    return
                }
            }
        }
        detectedCLI = nil
        cliPath = nil
    }

    // MARK: - 启动 / 关停

    /// 拉起 daemon。检测到 AI CLI 时接 rounder（多轮驱动 + 轻推 + 计量）；
    /// 检测不到时仍可启动（daemon 自带的演示用 echo worker），由 force 参数兜底。
    @discardableResult
    public func launchDaemon(force: Bool = false) throws {
        locateBinaries()
        if process?.isRunning == true { return }
        guard let daemon = daemonPath else {
            throw NSError(domain: "maestro", code: 1,
                          userInfo: [NSLocalizedDescriptionKey:
                            "未找到 maestro-daemon 二进制（bundle 损坏或 PATH 中不存在）"])
        }

        var args = ["--data-dir", dataDir.path]
        if let rounder = rounderPath, let cli = cliPath {
            args += ["--worker", rounder.path, "--", cliPath!.path]
        } else if !force {
            throw NSError(domain: "maestro", code: 2,
                          userInfo: [NSLocalizedDescriptionKey:
                            "未检测到受支持的 AI CLI（claude / codex / gemini / amp / opencode）。安装其一后再启动，或选择「演示模式」"])
        }

        let p = Process()
        p.executableURL = daemon
        p.arguments = args
        var env = ProcessInfo.processInfo.environment
        if let cli = detectedCLI { env["MAESTRO_CLI_DIALECT"] = cli.dialect }
        // 引擎设置覆盖（网关/预算；显式覆盖，不继承父进程同名变量）
        for (k, v) in engineEnv { env[k] = v }
        p.environment = env

        // stdout/stderr 落文件（不接管道，防管道满死锁 —— 与 daemon 自身设计同款）
        try? FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let logFile = dataDir.appendingPathComponent("daemon.log")
        if !FileManager.default.fileExists(atPath: logFile.path) {
            FileManager.default.createFile(atPath: logFile.path, contents: nil)
        }
        if let fh = try? FileHandle(forWritingTo: logFile) {
            _ = try? fh.seekToEnd()
            p.standardOutput = fh
            p.standardError = fh
        }
        try p.run()
        process = p
    }

    /// daemon 是否是本 GUI 拉起的
    public var isOurProcessRunning: Bool { process?.isRunning == true }

    public var launchSummary: String {
        if let cli = detectedCLI {
            return "Worker 引擎：maestro-rounder + \(cli.displayName)"
        }
        return "未检测到 AI CLI（claude/codex/gemini/amp/opencode）"
    }
}
