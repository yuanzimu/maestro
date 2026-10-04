// AppState.swift — 全局状态：轮询刷新 + 事件流 + 动作（纯 Foundation，无 UI 依赖）
import Foundation
import AppKit

final class AppState {
    // ---- 视图状态（只在主线程读写）----
    var connected = false
    var status: ServerStatus?
    var tasks: [TaskSummary] = []
    var inbox: [InboxItem] = []
    var events: [MaestroEvent] = []       // 新的在前
    var lastError: String?
    var isEmergencyStopped = false
    var isBusy = false

    /// 每次 refresh 完成后（主线程）回调 —— UI 刷新入口
    var onUpdate: (() -> Void)?

    let api: MaestroAPI
    let daemon: DaemonManager

    private var pollTimer: Timer?
    private var eventStream: EventStream?
    private let queue = DispatchQueue(label: "maestro.api", qos: .userInitiated)

    init() {
        let dir = MaestroAPI.defaultDataDir()
        api = MaestroAPI(dataDir: dir)
        daemon = DaemonManager(dataDir: dir)
    }

    func start() {
        daemon.locateBinaries()
        refresh()
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
            self?.refresh()
        }
        startEventStream()
    }

    // MARK: - 刷新

    func refresh() {
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
    static func sorted(_ tasks: [TaskSummary]) -> [TaskSummary] {
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
                if event.type == "emergency_stopped" { self.isEmergencyStopped = true }
                if event.type == "resumed",
                   (event.payload["via"] as? String) == "resume_all" { self.isEmergencyStopped = false }
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

    func createTask(title: String, prompt: String, workdir: String) {
        var params: [String: Any] = ["title": title, "prompt": prompt]
        if !workdir.isEmpty { params["workdir"] = workdir }
        act("创建任务") { api in _ = try api.call("task_create", params: params) }
    }

    func steer(task: String, message: String) {
        act("轻推") { api in _ = try api.call("task_steer", params: ["task": task, "message": message]) }
    }

    func pause(task: String)   { act("暂停") { api in _ = try api.call("task_pause", params: ["task": task]) } }
    func resume(task: String)  { act("恢复") { api in _ = try api.call("task_resume", params: ["task": task]) } }
    func cancel(task: String)  { act("取消") { api in _ = try api.call("task_cancel", params: ["task": task]) } }

    func emergencyStop() {
        act("急停") { api in _ = try api.call("server_emergency_stop", params: ["reason": "user_gui"]) }
    }

    func resumeAll() {
        act("恢复全部") { api in _ = try api.call("server_resume_all", params: ["steering": "flush"]) }
    }

    // MARK: - Daemon 生命周期

    func startDaemon(force: Bool = false) {
        do {
            try daemon.launchDaemon(force: force)
            queue.asyncAfter(deadline: .now() + 0.6) { [weak self] in
                _ = self?.api.isDaemonAlive
                DispatchQueue.main.async { self?.refresh() }
            }
        } catch {
            lastError = "\(error)"
        }
    }

    func shutdownDaemon() {
        act("关停 daemon") { api in _ = try api.call("server_shutdown") }
    }

    func openDataDir() {
        NSWorkspace.shared.open(api.dataDir)
    }

    func openDaemonLog() {
        NSWorkspace.shared.open(api.dataDir.appendingPathComponent("daemon.log"))
    }
}
