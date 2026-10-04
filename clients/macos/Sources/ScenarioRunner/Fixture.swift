// Fixture.swift — 场景测试基座：目录/二进制定位、mock AI CLI、daemon 编排、等待与原语
import Foundation
import MaestroCore

// MARK: - 错误与断言

struct TestFailure: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}

func fail(_ msg: String) throws -> Never {
    throw TestFailure(message: msg)
}

func expect(_ cond: Bool, _ msg: String) throws {
    if !cond { throw TestFailure(message: msg) }
}

// MARK: - 环境

enum Env {
    /// 仓库根（本文件位于 clients/macos/Sources/ScenarioRunner/）
    static let repoRoot = URL(fileURLWithPath: #file)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    /// Rust release 二进制（daemon / rounder / cli）
    static let binDir = repoRoot.appendingPathComponent("target/release")
    /// 场景工作区（每次全量运行前清空）
    static let work = URL(fileURLWithPath: "/tmp/maestro-scenarios")
    /// GUI 可执行文件：env 覆盖 > .app 产物（用户实际运行的）> SwiftPM 构建物
    static var guiBinary: URL {
        if let p = ProcessInfo.processInfo.environment["MAESTRO_GUI_BIN"] {
            return URL(fileURLWithPath: p)
        }
        let candidates = [
            work.deletingLastPathComponent().appendingPathComponent("maestro/clients/macos/dist/stage/Maestro.app/Contents/MacOS/Maestro"),
            repoRoot.appendingPathComponent("clients/macos/.build/release/MaestroGui"),
            repoRoot.appendingPathComponent("clients/macos/.build/debug/MaestroGui"),
        ]
        return candidates.first { (try? $0.checkResourceIsReachable()) ?? false } ?? candidates[0]
    }

    /// 场景过滤：MAESTRO_SCENARIOS="1-12,27"（缺省 = 全部）
    static var scenarioFilter: Set<Int>? {
        guard let raw = ProcessInfo.processInfo.environment["MAESTRO_SCENARIOS"], !raw.isEmpty else { return nil }
        var nums = Set<Int>()
        for part in raw.split(separator: ",") {
            let s = part.trimmingCharacters(in: .whitespaces)
            if let r = s.split(separator: "-").first, let l = s.split(separator: "-").last,
               let a = Int(r), let b = Int(l), s.contains("-") {
                nums.formUnion(a...b)
            } else if let n = Int(s) {
                nums.insert(n)
            }
        }
        return nums
    }

    static func resetWorkspace() {
        // ⚠️ 只清内容不清目录本身 —— harness 进程的 cwd 就是这里，自删会变幽灵目录
        try? FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        if let kids = try? FileManager.default.contentsOfDirectory(at: work, includingPropertiesForKeys: nil) {
            for k in kids { try? FileManager.default.removeItem(at: k) }
        }
        try? FileManager.default.createDirectory(at: work.appendingPathComponent("shots"), withIntermediateDirectories: true)
    }

    static func freshDir(_ name: String) -> URL {
        let d = work.appendingPathComponent(name)
        try? FileManager.default.removeItem(at: d)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
}

extension URL {
    var isFileExists: Bool {
        (try? checkResourceIsReachable()) ?? false
    }
}

/// 路径等价判定：macOS 的 /tmp 是符号链接（物理路径 /private/tmp），而新版
/// Foundation 的 resolvingSymlinksInPath 不再解析它 —— daemon current_dir()
/// 给物理路径，客户端 URL 给逻辑路径，显式归一后比较
func samePath(_ a: String, _ b: String) -> Bool {
    func normalize(_ p: String) -> String {
        if p.hasPrefix("/private/tmp/") { return String(p.dropFirst("/private".count)) }
        return p
    }
    return normalize(a) == normalize(b)
}

extension MaestroEvent {
    /// task_created 等事件携带完整 task 对象时的标题（扁平 payload 无 title）
    var taskTitle: String? {
        (payload["task"] as? [String: Any])?["title"] as? String
    }
}

// MARK: - 等待原语

/// 轮询等待条件成立（后台线程 sleep —— 不驱动主 RunLoop）
func waitUntil(timeout: TimeInterval = 15, _ what: String, _ cond: () throws -> Bool) throws {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if try cond() { return }
        Thread.sleep(forTimeInterval: 0.05)
    }
    if try cond() { return }
    throw TestFailure(message: "等待超时（\(Int(timeout))s）：\(what)")
}

/// 主 RunLoop 版（AppState 场景用 —— Timer 与 main queue 回调需要跑 loop）
func runloopWait(_ timeout: TimeInterval, _ cond: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return true }
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
    }
    return cond()
}

// MARK: - 进程工具

@discardableResult
func runCmd(_ path: String, _ args: [String], env: [String: String]? = nil,
            cwd: URL? = nil, timeout: TimeInterval = 30) -> (code: Int32, out: String, err: String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: path)
    p.arguments = args
    if let cwd { p.currentDirectoryURL = cwd }
    var e = ProcessInfo.processInfo.environment
    if let env { for (k, v) in env { e[k] = v } }
    p.environment = e
    let outPipe = Pipe(), errPipe = Pipe()
    p.standardOutput = outPipe
    p.standardError = errPipe
    do {
        try p.run()
    } catch {
        return (-127, "", "spawn 失败: \(error)")
    }
    let deadline = Date().addingTimeInterval(timeout)
    while p.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
    if p.isRunning {
        p.terminate()
        p.waitUntilExit()
        return (-124, readAll(outPipe), readAll(errPipe))
    }
    return (p.terminationStatus, readAll(outPipe), readAll(errPipe))
}

private func readAll(_ pipe: Pipe) -> String {
    pipe.fileHandleForReading.readabilityHandler = nil
    let d = pipe.fileHandleForReading.readDataToEndOfFile()
    return String(data: d, encoding: .utf8) ?? ""
}

/// 进程是否存活
func isAlive(_ pid: Int32) -> Bool { kill(pid, 0) == 0 }

/// ps 读进程状态字母（R/S/T/U…）
func procState(_ pid: Int32) -> String? {
    let r = runCmd("/bin/ps", ["-o", "stat=", "-p", String(pid)], timeout: 5)
    let s = r.out.trimmingCharacters(in: .whitespacesAndNewlines)
    return s.isEmpty ? nil : s
}

// MARK: - mock AI CLI（claude 方言：stream-json；rounder 的多轮驱动用它跑通全链）

enum MockCLI {
    /// 两轮完成：首轮「进行中」，次轮写唯一产物 + MAESTRO_DONE
    static let ok = """
    #!/bin/bash
    SID="mock-sess-001"
    STATE="$PWD/.maestro-mock-state"
    echo '{"type":"system","subtype":"init","session_id":"'$SID'","model":"mock-1"}'
    if [ -f "$STATE" ]; then
      echo "# 任务完成报告 ($RANDOM)" > "$PWD/mock-result.md"
      echo '{"type":"result","subtype":"success","result":"任务完成，已处理全部事项 MAESTRO_DONE","session_id":"'$SID'","usage":{"input_tokens":1500,"output_tokens":120,"cache_read_input_tokens":600,"cache_creation_input_tokens":200},"total_cost_usd":0.013,"model":"mock-1"}'
      rm -f "$STATE"
    else
      touch "$STATE"
      echo '{"type":"result","subtype":"success","result":"正在处理第一步……","session_id":"'$SID'","usage":{"input_tokens":900,"output_tokens":50},"total_cost_usd":0.004,"model":"mock-1"}'
    fi
    """
    /// 永不完成（轮数耗尽用）
    static let neverDone = """
    #!/bin/bash
    echo '{"type":"system","subtype":"init","session_id":"mock-2","model":"mock-1"}'
    echo '{"type":"result","subtype":"success","result":"仍在处理中……","session_id":"mock-2","usage":{"input_tokens":500,"output_tokens":30},"total_cost_usd":0.001,"model":"mock-1"}'
    """
    /// 每轮慢速（长时间 working —— pause/急停/cancel 场景用）
    static let slow = """
    #!/bin/bash
    echo '{"type":"system","subtype":"init","session_id":"mock-3","model":"mock-1"}'
    sleep 50
    echo '{"type":"result","subtype":"success","result":"慢轮结束","session_id":"mock-3","usage":{"input_tokens":100,"output_tokens":10},"total_cost_usd":0.0005,"model":"mock-1"}'
    """
    /// 非零退出（Failed 场景用）
    static let fail = """
    #!/bin/bash
    echo "boom: 内部错误" >&2
    exit 1
    """
    /// API 429 断连（自动恢复挂起场景用）—— 真实 claude CLI 的失败形态：
    /// 错误文本进 stderr + 非零退出。rounder 对非零退出透传 CLI 的 stderr，
    /// daemon 分类 "API Error: 429" → Disconnect → Suspended(NetworkLost)
    static let apiErr = """
    #!/bin/bash
    echo "API Error: 429 {\\"type\\":\\"error\\",\\"error\\":{\\"type\\":\\"rate_limit_error\\"}}" >&2
    exit 1
    """
    /// 轻推验证：首轮睡 N 秒（给 steer 留窗口），第 3 轮写产物完成
    ///（bash 变量 $1 不可用 —— daemon 传的是 claude 方言参数；用两种烘焙变体控制首窗长度）
    static func makeSteerCLI(round1Sleep: Int) -> String {
        """
        #!/bin/bash
        SID="mock-steer"
        C="$PWD/.maestro-mock-count"
        n=$(cat "$C" 2>/dev/null || echo 0)
        n=$((n+1)); echo $n > "$C"
        echo '{"type":"system","subtype":"init","session_id":"'$SID'","model":"mock-1"}'
        if [ $n -eq 1 ]; then sleep \(round1Sleep); fi
        if [ $n -ge 3 ]; then
          echo "# 完成 ($RANDOM)" > "$PWD/mock-result.md"
          echo '{"type":"result","subtype":"success","result":"任务完成 MAESTRO_DONE","session_id":"'$SID'","usage":{"input_tokens":800,"output_tokens":60},"total_cost_usd":0.005,"model":"mock-1"}'
        else
          echo '{"type":"result","subtype":"success","result":"第 '$n' 轮进行中","session_id":"'$SID'","usage":{"input_tokens":800,"output_tokens":60},"total_cost_usd":0.005,"model":"mock-1"}'
        fi
        """
    }

    /// 写一组 mock CLI 到目录并 chmod +x；返回 (目录, [名:路径])
    @discardableResult
    static func install(into dir: URL, name: String = "claude", script: String) -> URL {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let f = dir.appendingPathComponent(name)
        try? script.write(to: f, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: f.path)
        return f
    }
}

// MARK: - daemon 编排

/// worker 形态
enum WorkerSpec {
    case echo
    case rounder(cli: URL, maxRounds: Int?, roundGapMs: Int?)
}

final class DaemonHandle {
    let process: Process
    let dataDir: URL
    let api: MaestroAPI
    let logFile: URL
    private(set) var workerPids: [Int32] = []

    init(process: Process, dataDir: URL, api: MaestroAPI, logFile: URL) {
        self.process = process
        self.dataDir = dataDir
        self.api = api
        self.logFile = logFile
    }

    var pid: Int32 { process.processIdentifier }
    var isRunning: Bool { process.isRunning }

    /// daemon 日志文本（排障用）
    var logText: String {
        (try? String(contentsOf: logFile, encoding: .utf8)) ?? ""
    }

    // ---- RPC 便捷层 ----

    func status() throws -> ServerStatus {
        guard let s = try? api.call("server_status"), let st = ServerStatus.from(s) else {
            throw TestFailure(message: "server_status 不可用或解析失败")
        }
        return st
    }

    func taskList() throws -> [TaskSummary] {
        let r = try api.call("task_list")
        guard let arr = r["tasks"] as? [[String: Any]] else { throw TestFailure(message: "task_list 无 tasks 数组") }
        return arr.compactMap { TaskSummary.from($0) }
    }

    func task(_ id: String) throws -> TaskSummary? {
        let r = try api.call("task_get", params: ["task": id])
        // task_get 返回扁平结构（e2e 事实契约）；兼容嵌套形态
        if let t = r["task"] as? [String: Any] { return TaskSummary.from(t) }
        if r["id"] != nil { return TaskSummary.from(r) }
        return nil
    }

    @discardableResult
    func createTask(title: String, prompt: String = "把登录按钮改圆", workdir: String? = nil) throws -> [String: Any] {
        var p: [String: Any] = ["title": title, "prompt": prompt]
        if let workdir { p["workdir"] = workdir }
        return try api.call("task_create", params: p)
    }

    func inbox() throws -> [InboxItem] {
        let r = try api.call("inbox_list")
        guard let arr = r["items"] as? [[String: Any]] else { throw TestFailure(message: "inbox_list 无 items 数组") }
        return arr.compactMap { InboxItem.from($0) }
    }

    /// 等任务到达指定状态（state 前缀匹配，如 "blocked"）
    func waitTask(_ id: String, state: String, timeout: TimeInterval = 20) throws -> TaskSummary {
        try waitUntil(timeout: timeout, "任务 \(id) 到达 \(state)") {
            guard let t = try? self.task(id) else { return false }
            return t.state == state
        }
        guard let t = try task(id) else { throw TestFailure(message: "任务消失: \(id)") }
        return t
    }

    /// worker 进程列表（worker_list）
    func workers() throws -> [[String: Any]] {
        let r = try api.call("worker_list")
        return r["workers"] as? [[String: Any]] ?? []
    }

    /// 当前活 worker pid（ Rounder 链路才有真实长驻进程）
    func liveWorkerPid() throws -> Int32? {
        let ws = try workers()
        return ws.compactMap { ($0["pid"] as? Int).map(Int32.init) }.first
    }
}

/// 拉起一个真实 daemon
/// - Parameters:
///   - dataDir: 数据目录（调用方先建好/清好）
///   - worker: .echo（占位演示）/ .rounder(cli:…)（多轮驱动）
///   - env: 额外环境（MAESTRO_MAX_WORKERS 等）
func spawnDaemon(dataDir: URL, worker: WorkerSpec = .echo,
                 env extra: [String: String] = [:]) throws -> DaemonHandle {
    let daemonBin = Env.binDir.appendingPathComponent("maestro-daemon")
    try expect((try? daemonBin.checkResourceIsReachable()) ?? false, "找不到 \(daemonBin.path) —— 先 cargo build --release")
    var args = ["--data-dir", dataDir.path]
    switch worker {
    case .echo:
        break
    case .rounder(let cli, let maxRounds, let gap):
        let rounder = Env.binDir.appendingPathComponent("maestro-rounder")
        args += ["--worker", rounder.path, "--", cli.path]
        _ = maxRounds; _ = gap
    }
    let p = Process()
    p.executableURL = daemonBin
    p.arguments = args
    p.currentDirectoryURL = Env.work   // 稳定 cwd（防 harness cwd 变幽灵目录）
    var e = ProcessInfo.processInfo.environment
    e["MAESTRO_CLI_DIALECT"] = "claude"
    for (k, v) in extra { e[k] = v }
    p.environment = e
    let logFile = dataDir.appendingPathComponent("daemon.log")
    FileManager.default.createFile(atPath: logFile.path, contents: nil)
    let fh = try FileHandle(forWritingTo: logFile)
    try fh.seekToEnd()
    p.standardOutput = fh
    p.standardError = fh
    try p.run()
    let handle = DaemonHandle(process: p, dataDir: dataDir,
                              api: MaestroAPI(dataDir: dataDir), logFile: logFile)
    // 等 socket 就绪
    try waitUntil(timeout: 10, "daemon 就绪（\(dataDir.lastPathComponent)）") { handle.api.isDaemonAlive }
    return handle
}

/// 优雅关停（RPC → 等 3s → SIGKILL 兜底）；killHard: 直接 -9（崩溃恢复场景）
func stopDaemon(_ d: DaemonHandle, killHard: Bool = false) {
    if killHard {
        kill(d.pid, SIGKILL)
    } else {
        _ = try? d.api.call("server_shutdown")
    }
    let deadline = Date().addingTimeInterval(3)
    while d.process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
    if d.process.isRunning {
        kill(d.pid, SIGKILL)
        while d.process.isRunning { Thread.sleep(forTimeInterval: 0.05) }
    }
    // 关停后清残留 worker 进程组（崩溃/强杀时可能留下孤儿；正常 shutdown 自理）
    if killHard { reapWorkers(of: d.pid) }
}

/// 清理可能残留的 rounder/mock 进程（按命令行特征）
/// 无竞态 live 订阅：from_seq = 当前 event_seq+1 ——「connect 完成」≠「订阅注册完成」，
/// 窗口内发布的事件靠订阅注册时的 store 重放补齐（服务端 seq>=from 含语义）
func liveSub(_ d: DaemonHandle) throws -> AsyncSubscriber {
    let cur = try d.status().eventSeq
    return try AsyncSubscriber(fromSeq: cur + 1, socketPath: d.api.eventsSocketPath)
}

private func reapWorkers(of daemonPid: Int32) {
    let r = runCmd("/bin/ps", ["-eo", "pid=,command="], timeout: 5)
    for line in r.out.split(separator: "\n") {
        let s = String(line)
        guard s.contains("maestro-rounder") || s.contains("maestro-mock") || s.contains(".build-arm") else { continue }
        guard let pid = Int32(s.trimmingCharacters(in: .whitespaces).split(separator: " ").first ?? "") else { continue }
        if pid == daemonPid { continue }
        kill(pid, SIGKILL)
    }
}

// MARK: - 原始协议（线协议级测试用）

/// 发一行原始请求，读一行响应
func rawRPC(socketPath: String, line: String, connectFirst: Bool = true) throws -> [String: Any] {
    let conn = try UnixSocketConnection(path: socketPath)
    defer { conn.close() }
    try conn.writeAll(Data(line.utf8))
    try conn.writeAll(Data([0x0A]))
    guard let resp = try conn.readLine(), !resp.isEmpty else {
        throw TestFailure(message: "无响应")
    }
    guard let obj = try? JSONSerialization.jsonObject(with: resp) as? [String: Any] else {
        throw TestFailure(message: "响应非 JSON: \(String(data: resp.prefix(120), encoding: .utf8) ?? "?")")
    }
    return obj
}

/// 事件 socket 原始读取（live:false 读到 EOF；live:true 限时收）
func readEvents(socketPath: String, from seq: UInt64, live: Bool,
                maxWait: TimeInterval = 10, minCount: Int = 1) throws -> [MaestroEvent] {
    final class Box { var events: [MaestroEvent] = [] }
    let box = Box()
    let conn = try UnixSocketConnection(path: socketPath)
    let hello: [String: Any] = ["from_seq": seq, "live": live]
    try conn.writeAll(try JSONSerialization.data(withJSONObject: hello))
    try conn.writeAll(Data([0x0A]))
    let t = Thread {
        while let line = (try? conn.readLine()) ?? nil {
            guard !line.isEmpty else { continue }
            guard let obj = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
                  let s = obj["seq"] as? UInt64 else { continue }
            let ev = obj["event"] as? [String: Any] ?? [:]
            box.events.append(MaestroEvent(seq: s, ts: obj["ts"] as? UInt64 ?? 0,
                                           priority: obj["priority"] as? String ?? "info",
                                           type: ev["type"] as? String ?? "?", payload: ev))
        }
    }
    t.start()
    if live {
        try waitUntil(timeout: maxWait, "事件到达（≥\(minCount) 条）") { box.events.count >= minCount }
        Thread.sleep(forTimeInterval: 0.5)   // 再薅半秒新事件
    } else {
        let deadline = Date().addingTimeInterval(maxWait)
        while Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
    }
    conn.close()
    return box.events
}

// MARK: - GUI 进程

final class GuiHandle {
    let process: Process
    let stderrFile: URL
    var pid: Int32 { process.processIdentifier }
    var isRunning: Bool { process.isRunning }
    init(process: Process, stderrFile: URL) {
        self.process = process
        self.stderrFile = stderrFile
    }
    var stderrText: String {
        (try? String(contentsOf: stderrFile, encoding: .utf8)) ?? ""
    }
    func terminate() {
        if process.isRunning { process.terminate() }
        let deadline = Date().addingTimeInterval(3)
        while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.05) }
        if process.isRunning { kill(pid, SIGKILL) }
    }
}

/// 拉起 GUI（真实窗口）；pathEnv 覆盖 PATH（自动拉起/检测场景用）
func spawnGUI(dataDir: URL, pathEnv: String? = nil, tag: String) throws -> GuiHandle {
    let bin = Env.guiBinary
    try expect((try? bin.checkResourceIsReachable()) ?? false, "GUI 二进制不存在: \(bin.path)")
    let p = Process()
    p.executableURL = bin
    var e = ProcessInfo.processInfo.environment
    e["MAESTRO_DATA_DIR"] = dataDir.path
    if let pathEnv { e["PATH"] = pathEnv }
    p.environment = e
    let errFile = Env.work.appendingPathComponent("gui-\(tag).stderr")
    FileManager.default.createFile(atPath: errFile.path, contents: nil)
    let fh = try FileHandle(forWritingTo: errFile)
    p.standardOutput = fh
    p.standardError = fh
    try p.run()
    return GuiHandle(process: p, stderrFile: errFile)
}

/// daemon 事件 socket 上的订阅连接数（lsof：listener 1 条 + 每订阅者 1 条）
func eventsConnectionCount(daemonPid: Int32) -> Int {
    let r = runCmd("/usr/sbin/lsof", ["-U", "-a", "-p", String(daemonPid)], timeout: 10)
    return r.out.split(separator: "\n").filter { $0.contains("maestro.events.sock") }.count
}

/// 截图留证（用户可翻看）
func screenshot(_ name: String) {
    _ = runCmd("/usr/sbin/screencapture", ["-x", Env.work.appendingPathComponent("shots/\(name).png").path], timeout: 15)
}
