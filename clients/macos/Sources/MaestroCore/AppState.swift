// AppState.swift — 全局状态：轮询刷新 + 事件流 + 动作（纯 Foundation，无 UI 依赖）
import Foundation
import AppKit

public final class AppState {
    // ---- 视图状态（只在主线程读写）----
    public var connected = false
    public var status: ServerStatus?
    public var tasks: [TaskSummary] = []
    public var inbox: [InboxItem] = []
    public var events: [MaestroEvent] = []       // 新的在前
    public var lastError: String?
    public var isEmergencyStopped = false
    public var isBusy = false

    /// 每次 refresh 完成后（主线程）回调 —— UI 刷新入口
    public var onUpdate: (() -> Void)?

    public let api: MaestroAPI
    public let daemon: DaemonManager
    public let engineSettingsStore: EngineSettingsStore
    public private(set) var engineSettings: EngineSettings

    private var pollTimer: Timer?
    private var eventStream: EventStream?
    private let queue = DispatchQueue(label: "maestro.api", qos: .userInitiated)

    public init() {
        let dir = MaestroAPI.defaultDataDir()
        api = MaestroAPI(dataDir: dir)
        daemon = DaemonManager(dataDir: dir)
        engineSettingsStore = EngineSettingsStore(dataDir: dir)
        engineSettings = engineSettingsStore.load()
    }

    /// 把引擎设置应用到 daemon 拉起环境（每次 startDaemon 前调用）
    public func applyEngineEnv() {
        daemon.engineEnv = engineSettingsStore.daemonEnv(for: engineSettings)
    }

    /// 保存引擎设置（设置窗口用）；返回错误描述
    @discardableResult
    public func saveEngineSettings(_ s: EngineSettings) -> String? {
        do {
            try engineSettingsStore.save(s)
            engineSettings = s
            applyEngineEnv()
            return nil
        } catch {
            return "引擎设置保存失败: \(error)"
        }
    }

    public func start() {
        daemon.locateBinaries()
        applyEngineEnv()
        refresh()
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
            self?.refresh()
        }
        startEventStream()
    }

    // MARK: - 刷新

    public func refresh() {
        queue.async { [weak self] in
            guard let self else { return }
            var connected = false
            var status: ServerStatus?
            var tasks: [TaskSummary] = []
            var inbox: [InboxItem] = []
            var err: String?

            if let s = try? self.api.call("server_status"),
               let st = ServerStatus.from(s) {
                connected = true
                status = st
                if let tl = try? self.api.call("task_list"),
                   let arr = tl["tasks"] as? [[String: Any]] {
                    tasks = arr.compactMap { TaskSummary.from($0) }
                }
                if let il = try? self.api.call("inbox_list"),
                   let arr = il["items"] as? [[String: Any]] {
                    inbox = arr.compactMap { InboxItem.from($0) }
                }
            } else {
                err = "无法连接 daemon（数据目录：\(self.api.dataDir.path)）"
            }

            DispatchQueue.main.async {
                self.connected = connected
                self.status = status
                self.tasks = Self.sorted(tasks)
                self.inbox = inbox
                if !connected { self.isEmergencyStopped = false }
                if let err { self.lastError = err }
                self.onUpdate?()
            }
        }
    }

    /// 非终态在前（blocked/suspended 优先），终态垫底
    public static func sorted(_ tasks: [TaskSummary]) -> [TaskSummary] {
        func rank(_ s: String) -> Int {
            switch s {
            case "blocked": return 0
            case "suspended": return 1
            case "working": return 2
            case "planning": return 3
            case "queued": return 4
            case "failed": return 5
            case "cancelled": return 6
            case "done": return 7
            default: return 8
            }
        }
        return tasks.sorted { a, b in
            let (ra, rb) = (rank(a.state), rank(b.state))
            return ra == rb ? a.title < b.title : ra < rb
        }
    }

    // MARK: - 事件流

    private func startEventStream() {
        let stream = EventStream(socketPath: api.eventsSocketPath)
        stream.onEvent = { [weak self] event in
            DispatchQueue.main.async {
                guard let self else { return }
                self.events.insert(event, at: 0)
                if self.events.count > 500 { self.events.removeLast(self.events.count - 500) }
                if event.type == "emergency_stopped" {
                    self.isEmergencyStopped = true
                    AppLog.emergency("收到 emergency_stopped 事件 → isEmergencyStopped=true")
                }
                if event.type == "resumed",
                   (event.payload["via"] as? String) == "resume_all" {
                    self.isEmergencyStopped = false
                    AppLog.emergency("收到 resumed(via=resume_all) 事件 → isEmergencyStopped=false")
                }
                // emergency_resumed 是 resume_all 完成的权威标记（空冻结时不会有 resumed
                // 事件，只有它能解除状态）
                if event.type == "emergency_resumed" {
                    let n = (event.payload["resumed"] as? [Any])?.count ?? 0
                    self.isEmergencyStopped = false
                    AppLog.emergency("收到 emergency_resumed（恢复 \(n) 个任务）→ isEmergencyStopped=false")
                }
            }
        }
        stream.start()
        eventStream = stream
    }

    // MARK: - 动作（fire-and-forget，完成后 refresh）

    private func act(_ label: String, _ body: @escaping (MaestroAPI) throws -> Void) {
        isBusy = true
        queue.async { [weak self] in
            guard let self else { return }
            var err: String?
            do { try body(self.api) } catch { err = "\(label)失败: \(error)" }
            DispatchQueue.main.async {
                self.isBusy = false
                if let err {
                    self.lastError = err
                    NSSound.beep()
                }
                self.refresh()
            }
        }
    }

    public func createTask(title: String, prompt: String, workdir: String) {
        var params: [String: Any] = ["title": title, "prompt": prompt]
        if !workdir.isEmpty { params["workdir"] = workdir }
        act("创建任务") { api in _ = try api.call("task_create", params: params) }
    }

    public func steer(task: String, message: String) {
        act("轻推") { api in _ = try api.call("task_steer", params: ["task": task, "message": message]) }
    }

    public func pause(task: String)   { act("暂停") { api in _ = try api.call("task_pause", params: ["task": task]) } }
    public func resume(task: String)  { act("恢复") { api in _ = try api.call("task_resume", params: ["task": task]) } }
    public func cancel(task: String)  { act("取消") { api in _ = try api.call("task_cancel", params: ["task": task]) } }

    public func emergencyStop() {
        act("急停") { api in _ = try api.call("server_emergency_stop", params: ["reason": "user_gui"]) }
    }

    public func resumeAll() {
        act("恢复全部") { api in _ = try api.call("server_resume_all", params: ["steering": "flush"]) }
    }

    // MARK: - Daemon 生命周期

    public func startDaemon(force: Bool = false) {
        do {
            applyEngineEnv()
            try daemon.launchDaemon(force: force)
            queue.asyncAfter(deadline: .now() + 0.6) { [weak self] in
                _ = self?.api.isDaemonAlive
                DispatchQueue.main.async { self?.refresh() }
            }
        } catch {
            lastError = "\(error)"
        }
    }

    public func shutdownDaemon() {
        act("关停 daemon") { api in _ = try api.call("server_shutdown") }
    }

    public func openDataDir() {
        NSWorkspace.shared.open(api.dataDir)
    }

    public func openDaemonLog() {
        NSWorkspace.shared.open(api.dataDir.appendingPathComponent("daemon.log"))
    }

    /// 测试/退出清理：停事件流（轮询 Timer 由 RunLoop 自理）
    public func eventStreamStopForTest() {
        eventStream?.stop()
    }
}
