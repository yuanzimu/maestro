// ScenariosD.swift — I 77-84 协议健壮性；J 85-90 并发与负载；K 91-100 GUI 进程与 CLI 集成
import Foundation
import MaestroCore

func registerI_Protocol() {
    let s = Suite.shared

    s.add(77, "I", "未知方法 → -400（协议双轨制不炸）") {
        // 静态断言：协议侧全部已知事件类型在 mac 客户端都有人话标签（无 unknown 噪声）
        let knownProtocolEvents = [
            "task_created", "task_started", "task_completed", "task_failed",
            "task_cancelled", "task_requeued", "worker_spawned", "worker_died",
            "goal_progress", "acceptance_gate_passed", "acceptance_gate_failed",
            "suspended", "resumed", "resume_attempt", "auto_recovery_exhausted",
            "emergency_stopped", "emergency_resumed", "emergency_snapshotted",
            "steering_dropped", "steering_queued", "steering_delivered",
            "checkpoint_created", "checkpoint_rolled_back", "narrative_snapshot",
            "feedback_recorded", "ledger_entry", "round_progress", "cost_drift",
            "context_compacted", "rounds_exhausted", "provider_switched",
            "batch_marked_suspended", "batch_failed",
        ]
        let missing = knownProtocolEvents.filter { MaestroEvent.typeNames[$0] == nil }
        try expect(missing.isEmpty, "以下事件缺人话标签: \(missing)")

        let d = try spawnDaemon(dataDir: Env.freshDir("i77"))
        defer { stopDaemon(d) }
        let resp = try rawRPC(socketPath: d.api.apiSocketPath,
                              line: #"{"id":"p1","method":"no_such_method","params":{}}"#)
        let err = resp["err"] as? [String: Any]
        try expect(err != nil, "应报错，实际 \(resp)")
        try expect(((err?["error"] as? [String: Any])?["code"] as? Int) == -400, "错误码应 -400")
    }

    s.add(78, "I", "畸形 JSON：-400 且连接存活（下一请求正常）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i78"))
        defer { stopDaemon(d) }
        let conn = try UnixSocketConnection(path: d.api.apiSocketPath)
        defer { conn.close() }
        try conn.writeAll(Data("这不是 JSON".utf8)); try conn.writeAll(Data([0x0A]))
        let bad = try parseLine(conn)
        try expect(((bad["err"] as? [String: Any])?["error"] as? [String: Any])?["code"] as? Int == -400,
                    "畸形应 -400: \(bad)")
        try conn.writeAll(Data(#"{"id":"p2","method":"server_status","params":{}}"#.utf8)); try conn.writeAll(Data([0x0A]))
        let good = try parseLine(conn)
        try expect(good["ok"] != nil, "连接应存活: \(good)")
    }

    s.add(79, "I", "空行被忽略（不误当请求）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i79"))
        defer { stopDaemon(d) }
        let conn = try UnixSocketConnection(path: d.api.apiSocketPath)
        defer { conn.close() }
        try conn.writeAll(Data([0x0A]))   // 空行
        try conn.writeAll(Data(#"{"id":"p3","method":"server_status","params":{}}"#.utf8)); try conn.writeAll(Data([0x0A]))
        let resp = try parseLine(conn)
        try expect(((resp["ok"] as? [String: Any])?["id"] as? String) == "p3", "应只有一条响应: \(resp)")
    }

    s.add(80, "I", "参数类型错误（task 非字符串）→ -400") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i80"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-400) {
            try d.api.call("task_pause", params: ["task": 12345])
        }
    }

    s.add(81, "I", "请求 id 原样回显") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i81"))
        defer { stopDaemon(d) }
        let resp = try rawRPC(socketPath: d.api.apiSocketPath,
                              line: #"{"id":"echo-me-42","method":"server_status","params":{}}"#)
        try expect(((resp["ok"] as? [String: Any])?["id"] as? String) == "echo-me-42", "id 应回显: \(resp)")
    }

    s.add(82, "I", "单连接流水线 3 连发：3 响应有序") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i82"))
        defer { stopDaemon(d) }
        let conn = try UnixSocketConnection(path: d.api.apiSocketPath)
        defer { conn.close() }
        let reqs = (1...3).map { #"{"id":"pipe-\#($0)","method":"server_status","params":{}}"# }
        try conn.writeAll(Data(reqs.joined(separator: "\n").utf8)); try conn.writeAll(Data([0x0A]))
        let r1 = try parseLine(conn), r2 = try parseLine(conn), r3 = try parseLine(conn)
        for (i, r) in [r1, r2, r3].enumerated() {
            try expect(((r["ok"] as? [String: Any])?["id"] as? String) == "pipe-\(i + 1)",
                        "第 \(i + 1) 响应 id 错: \(r)")
        }
    }

    s.add(83, "I", "5MB 超限行：-400 不断连不炸") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i83"))
        defer { stopDaemon(d) }
        let conn = try UnixSocketConnection(path: d.api.apiSocketPath)
        defer { conn.close() }
        // 5MB 的 prompt（超 100k 限制）—— 构造合法 JSON 但业务超限
        var line = Data(#"{"id":"big","method":"task_create","params":{"title":"大","prompt":""#.utf8)
        line.append(Data(String(repeating: "x", count: 5_000_000).utf8))
        line.append(Data(#"","workdir":"/tmp"}}"#.utf8))
        try conn.writeAll(line); try conn.writeAll(Data([0x0A]))
        let resp = try parseLine(conn)
        let code = (((resp["err"] as? [String: Any])?["error"] as? [String: Any])?["code"] as? Int) ?? 0
        try expect(code == -400, "应 -400 超长，实际 code=\(code)")
        // 连接仍可用
        try conn.writeAll(Data(#"{"id":"after","method":"server_status","params":{}}"#.utf8)); try conn.writeAll(Data([0x0A]))
        let after = try parseLine(conn)
        try expect(after["ok"] != nil, "大请求后连接应存活: \(after)")
    }

    s.add(84, "I", "客户端半行断开：daemon 不崩，后续正常") {
        let d = try spawnDaemon(dataDir: Env.freshDir("i84"))
        defer { stopDaemon(d) }
        let conn = try UnixSocketConnection(path: d.api.apiSocketPath)
        try conn.writeAll(Data(#"{"id":"half","method":"ser"#.utf8))   // 半行
        conn.close()                                                    // 立即断开
        try Thread.sleep(forTimeInterval: 0.5)
        let st = try d.status()   // daemon 仍健康
        try expect(st.pid == d.pid, "daemon 应存活")
    }
}

private func parseLine(_ conn: UnixSocketConnection) throws -> [String: Any] {
    guard let line = try conn.readLine(), !line.isEmpty else {
        throw TestFailure(message: "无响应行")
    }
    guard let obj = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else {
        throw TestFailure(message: "响应非 JSON: \(String(data: line.prefix(120), encoding: .utf8) ?? "?")")
    }
    return obj
}

func registerJ_Load() {
    let s = Suite.shared

    s.add(85, "J", "20 并发客户端：status 全部成功") {
        let d = try spawnDaemon(dataDir: Env.freshDir("j85"))
        defer { stopDaemon(d) }
        let group = DispatchGroup()
        var failures = 0
        let lock = NSLock()
        for _ in 0..<20 {
            group.enter()
            DispatchQueue.global().async {
                let api = MaestroAPI(dataDir: d.dataDir)
                for _ in 0..<10 {
                    do { _ = try api.call("server_status") }
                    catch { lock.lock(); failures += 1; lock.unlock() }
                }
                group.leave()
            }
        }
        group.wait()
        try expect(failures == 0, "并发失败 \(failures) 次")
    }

    s.add(86, "J", "200 次串行快打：0 失败") {
        let d = try spawnDaemon(dataDir: Env.freshDir("j86"))
        defer { stopDaemon(d) }
        for _ in 0..<200 {
            _ = try d.api.call("server_status")
        }
        try expect(true, "")
    }

    s.add(87, "J", "30 任务连发：id 唯一 + 全部入列（深度限制内）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("j87"))
        defer { stopDaemon(d, killHard: true) }
        var ids = Set<String>()
        for i in 1...30 {
            let r = try d.createTask(title: "压测\(i)", workdir: Env.freshDir("j87-w\(i)").path)
            ids.insert((r["task"] as? [String: Any])?["id"] as! String)
        }
        try expect(ids.count == 30, "id 应唯一，实际 \(ids.count)/30")
        let tasks = try d.taskList()
        try expect(tasks.count >= 30, "列表应 ≥30，实际 \(tasks.count)")
    }

    s.add(88, "J", "并发混合操作（pause/resume/steer/status）：不崩、状态一致") {
        let cli = try mockCLI("slow88", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("j88"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil))
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "混战对象", workdir: Env.freshDir("j88-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        let group = DispatchGroup()
        let api = MaestroAPI(dataDir: d.dataDir)
        for i in 0..<8 {
            group.enter()
            DispatchQueue.global().async {
                defer { group.leave() }
                for j in 0..<10 {
                    switch (i + j) % 4 {
                    case 0: _ = try? api.call("task_pause", params: ["task": id])
                    case 1: _ = try? api.call("task_resume", params: ["task": id])
                    case 2: _ = try? api.call("task_steer", params: ["task": id, "message": "并发推 \(i)-\(j)"])
                    default: _ = try? api.call("server_status")
                    }
                }
            }
        }
        group.wait()
        _ = try d.status()   // daemon 存活
        let t = try d.task(id)
        try expect(t != nil && ["working", "suspended", "blocked"].contains(t?.state ?? "?"),
                    "终态应合法，实际 \(t?.state ?? "?")")
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(89, "J", "30 任务大列表：客户端模型完整解析") {
        let d = try spawnDaemon(dataDir: Env.freshDir("j89"))
        defer { stopDaemon(d) }
        for i in 1...30 {
            _ = try d.createTask(title: "解析\(i)", workdir: Env.freshDir("j89-w\(i)").path)
        }
        let tasks = try d.taskList()
        try expect(tasks.count == 30, "应 30 个，实际 \(tasks.count)")
        try expect(tasks.allSatisfy { !$0.id.isEmpty && !$0.title.isEmpty }, "字段应完整")
        try expect(Set(tasks.map { $0.title }).count == 30, "标题应各异")
    }

    s.add(90, "J", "负载下事件流：seq 严格有序不丢") {
        let d = try spawnDaemon(dataDir: Env.freshDir("j90"))
        defer { stopDaemon(d) }
        let sub = try liveSub(d)
        for i in 1...30 {
            _ = try d.createTask(title: "事件压测\(i)", workdir: Env.freshDir("j90-w\(i)").path)
        }
        _ = try collect(sub, deadline: 20) {
            $0.type == "task_created" && $0.taskTitle == "事件压测30"
        }
        let seqs = sub.all.map { $0.seq }
        try expect(seqs == seqs.sorted(), "seq 应有序到达")
        try expect(Set(seqs).count == seqs.count, "不得重复")
        try expect(seqs.count >= 30, "事件不得丢（\(seqs.count)/30+）")
        sub.stop()
    }
}

func registerK_GUI() {
    let s = Suite.shared
    let cliBin = Env.binDir.appendingPathComponent("maestro")

    s.add(91, "K", "GUI 冷启动（无 daemon 无 CLI）：进程稳定不崩") {
        let dir = Env.freshDir("k91")
        let gui = try spawnGUI(dataDir: dir, pathEnv: "/usr/bin:/bin:/usr/sbin:/sbin", tag: "k91")
        defer { gui.terminate() }
        try Thread.sleep(forTimeInterval: 4)
        try expect(gui.isRunning, "GUI 4s 后应仍存活")
        try expect(!dir.appendingPathComponent("maestro.api.sock").isFileExists,
                  "无 CLI 不应自动拉 daemon")
        let err = gui.stderrText
        try expect(!err.lowercased().contains("crash"), "stderr 疑似崩溃: \(err.prefix(300))")
        screenshot("k91-cold")
    }

    s.add(92, "K", "GUI 连已有 daemon：API 轮询 + 事件订阅建立") {
        let dir = Env.freshDir("k92")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        let base = eventsConnectionCount(daemonPid: d.pid)
        let gui = try spawnGUI(dataDir: dir, tag: "k92")
        defer { gui.terminate() }
        try waitUntil(timeout: 8, "GUI 建立事件订阅") {
            eventsConnectionCount(daemonPid: d.pid) >= base + 1
        }
        try expect(gui.isRunning, "GUI 应存活")
        screenshot("k92-connected")
    }

    s.add(93, "K", "GUI 自动拉起 daemon（检测到 CLI）→ 全链路可用") {
        let dir = Env.freshDir("k93")
        let mock = MockCLI.install(into: Env.freshDir("k93-bin"), script: MockCLI.ok)
        let path = "\(Env.binDir.path):\(mock.deletingLastPathComponent().path):/usr/bin:/bin"
        let gui = try spawnGUI(dataDir: dir, pathEnv: path, tag: "k93")
        defer { gui.terminate() }
        // GUI 启动逻辑：daemon 不在 + 有 daemon 二进制 + 有 CLI → 自动拉起
        let api = MaestroAPI(dataDir: dir)
        try waitUntil(timeout: 10, "GUI 自动拉起 daemon") { api.isDaemonAlive }
        try Thread.sleep(forTimeInterval: 1.5)
        screenshot("k93-autolaunch")
        // 全链路：经 GUI 拉起的 daemon 创建任务 → mock CLI 两轮完成
        let w = Env.freshDir("k93-work")
        let r = try api.call("task_create", params: ["title": "GUI 拉起链路", "prompt": "跑通", "workdir": w.path])
        let id = (r["task"] as? [String: Any])?["id"] as! String
        try waitUntil(timeout: 30, "任务完成") {
            guard let tl = try? api.call("task_list"),
                  let arr = tl["tasks"] as? [[String: Any]],
                  let t = arr.first(where: { ($0["id"] as? String) == id }) else { return false }
            return (t["state"] as? String) == "done"
        }
        _ = try api.call("server_shutdown")
    }

    s.add(94, "K", "GUI 有 daemon 二进制但无 CLI：不自动拉起（留引导）") {
        let dir = Env.freshDir("k94")
        let path = "\(Env.binDir.path):/usr/bin:/bin"   // 有 daemon，无 claude
        let gui = try spawnGUI(dataDir: dir, pathEnv: path, tag: "k94")
        defer { gui.terminate() }
        try Thread.sleep(forTimeInterval: 3.5)
        try expect(!dir.appendingPathComponent("maestro.api.sock").isFileExists,
                    "无 CLI 不应拉 daemon")
        try expect(gui.isRunning, "GUI 应存活")
    }

    s.add(95, "K", "GUI 退出：daemon 独立存活（任务不受影响）") {
        let dir = Env.freshDir("k95")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        let gui = try spawnGUI(dataDir: dir, tag: "k95")
        try Thread.sleep(forTimeInterval: 3)
        gui.terminate()
        try Thread.sleep(forTimeInterval: 1)
        try expect(d.isRunning, "daemon 不应随 GUI 退出")
        _ = try d.status()
    }

    s.add(96, "K", "daemon 中途死亡重启：GUI 自动重连不崩") {
        let dir = Env.freshDir("k96")
        let d = try spawnDaemon(dataDir: dir)
        let gui = try spawnGUI(dataDir: dir, tag: "k96")
        defer { gui.terminate() }
        try Thread.sleep(forTimeInterval: 3)
        stopDaemon(d, killHard: true)   // daemon 猝死（kill -9）
        try Thread.sleep(forTimeInterval: 1.5)   // 让 GUI 体验一段失联
        let d2 = try spawnDaemon(dataDir: dir)   // 新 daemon 接管同 socket
        defer { stopDaemon(d2) }
        try waitUntil(timeout: 10, "GUI 重连（新 daemon 事件订阅）") {
            eventsConnectionCount(daemonPid: d2.pid) >= 2
        }
        try expect(gui.isRunning, "GUI 应存活")
        let err = gui.stderrText
        try expect(!err.lowercased().contains("crash"), "stderr 疑似崩溃: \(err.prefix(300))")
        screenshot("k96-reconnected")
    }

    s.add(97, "K", "CLI 全家桶：status/task list/inbox/doctor") {
        let dir = Env.freshDir("k97")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "CLI可见", workdir: Env.freshDir("k97-work").path)
        _ = try waitUntil(timeout: 15, "任务 blocked") {
            (try? d.taskList())?.first { $0.title == "CLI可见" }?.state == "blocked"
        }
        let env = ["MAESTRO_DATA_DIR": dir.path]
        var r = runCmd(cliBin.path, ["status"], env: env, timeout: 10)
        try expect(r.code == 0 && r.out.contains("daemon v"), "status 异常: \(r.out) \(r.err)")
        r = runCmd(cliBin.path, ["task", "list"], env: env, timeout: 10)
        try expect(r.code == 0 && r.out.contains("CLI可见"), "task list 异常: \(r.out)")
        r = runCmd(cliBin.path, ["inbox"], env: env, timeout: 10)
        try expect(r.code == 0 && r.out.contains("CLI可见"), "inbox 异常: \(r.out)")
        r = runCmd(cliBin.path, ["doctor"], env: env, timeout: 15)
        try expect(r.code == 0 && r.out.contains("全部通过"), "doctor 异常: \(r.out)")
    }

    s.add(98, "K", "CLI stop/resume：JSON 输出可解析") {
        let dir = Env.freshDir("k98")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        let env = ["MAESTRO_DATA_DIR": dir.path]
        var r = runCmd(cliBin.path, ["stop"], env: env, timeout: 10)
        try expect(r.code == 0, "stop 退出码 \(r.code)")
        try expect(r.out.contains("\"sessions_preserved\""), "stop 应输出 JSON: \(r.out)")
        r = runCmd(cliBin.path, ["resume"], env: env, timeout: 10)
        try expect(r.code == 0 && r.out.contains("\"resumed\""), "resume 应输出 JSON: \(r.out)")
    }

    s.add(99, "K", "CLI events：非跟随即出；--follow 实时滚动") {
        let dir = Env.freshDir("k99")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "事件CLI", workdir: Env.freshDir("k99-work").path)
        try waitUntil(timeout: 10, "有历史事件") { (try? d.status())?.eventSeq ?? 0 >= 2 }
        let env = ["MAESTRO_DATA_DIR": dir.path]
        // 非跟随：打印历史即退出（R58 修复项）
        var r = runCmd(cliBin.path, ["events"], env: env, timeout: 10)
        try expect(r.code == 0, "非跟随 events 退出码 \(r.code)：\(r.out.prefix(200))")
        try expect(!r.out.isEmpty, "应打印历史事件")
        // --follow：实时滚动，杀掉即退
        let p = Process()
        p.executableURL = cliBin
        p.arguments = ["events", "--follow", "--human"]
        var e2 = ProcessInfo.processInfo.environment
        e2["MAESTRO_DATA_DIR"] = dir.path
        p.environment = e2
        let pipe = Pipe()
        p.standardOutput = pipe
        try p.run()
        let box = FollowOutput(pipe: pipe)
        _ = try d.createTask(title: "滚动中的新任务", workdir: Env.freshDir("k99-w2").path)
        try waitUntil(timeout: 10, "follow 输出新事件") { box.text.contains("滚动中的新任务") }
        p.terminate()
        p.waitUntilExit()
    }

    s.add(100, "K", "maestro ui：Web 指挥台起服务（HTTP 200 HTML）") {
        let dir = Env.freshDir("k100")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        let p = Process()
        p.executableURL = cliBin
        p.arguments = ["ui", "--port", "17331"]
        var e = ProcessInfo.processInfo.environment
        e["MAESTRO_DATA_DIR"] = dir.path
        p.environment = e
        try p.run()
        defer {
            p.terminate()
            if p.isRunning { kill(p.processIdentifier, SIGKILL) }
        }
        var ok200 = false
        var body = ""
        try waitUntil(timeout: 12, "UI HTTP 200") {
            let r = runCmd("/usr/bin/curl", ["-s", "--noproxy", "*", "-o", "/dev/null", "-w", "%{http_code}",
                                             "http://127.0.0.1:17331/"], timeout: 5)
            ok200 = r.out.trimmingCharacters(in: .whitespacesAndNewlines) == "200"
            return ok200
        }
        let page = runCmd("/usr/bin/curl", ["-s", "--noproxy", "*", "http://127.0.0.1:17331/"], timeout: 5)
        body = page.out
        try expect(ok200, "HTTP 非 200")
        try expect(body.lowercased().contains("<html"), "应返回 HTML 页面: \(body.prefix(120))")
    }
}

/// follow 模式 stdout 收集器
final class FollowOutput {
    private(set) var text = ""
    init(pipe: Pipe) {
        pipe.fileHandleForReading.readabilityHandler = { [weak self] h in
            let d = h.availableData
            if d.isEmpty { h.readabilityHandler = nil; return }
            self?.text += String(data: d, encoding: .utf8) ?? ""
        }
    }
}
