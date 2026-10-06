// ScenariosC.swift — F 55-62 急停；G 63-72 事件流；H 73-76 检查点
import Foundation
import MaestroCore

func registerF_Emergency() {
    let s = Suite.shared
    let fastEnv = ["MAESTRO_ROUND_GAP_MS": "100"]

    s.add(55, "F", "空载急停：freeze_ms<100 + 事件 + resume_all 幂等") {
        let d = try spawnDaemon(dataDir: Env.freshDir("f55"))
        defer { stopDaemon(d) }
        let sub = try liveSub(d)
        let r = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        try expect((r["sessions_preserved"] as? Bool) == true, "现场应保全")
        try expect((r["freeze_ms"] as? Int ?? 999) < 100, "freeze 应 <100ms，实际 \(r["freeze_ms"] ?? "?")")
        let evs = try collect(sub, deadline: 8) { $0.type == "emergency_stopped" }
        try expect(!evs.isEmpty, "未见 emergency_stopped 事件")
        let resume = try d.api.call("server_resume_all", params: ["steering": "flush"])
        try expect((resume["resumed"] as? [Any])?.isEmpty == true, "空载 resume 应空列表")
        // 即使空载（无 resumed 事件），也必须收到 emergency_resumed 权威解除标记
        let resumedEv = try collect(sub, deadline: 6) { $0.type == "emergency_resumed" }
        try expect(!resumedEv.isEmpty, "应收到 emergency_resumed 事件")
        try expect((resumedEv.first?.payload["resumed"] as? [Any])?.isEmpty == true,
                  "空载 emergency_resumed.resumed 应为空")
        sub.stop()
    }

    s.add(56, "F", "working 任务急停：冻结 + suspended(emergency_stop) + 进程 T") {
        let cli = try mockCLI("slow56", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("f56"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "急停对象", workdir: Env.freshDir("f56-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        let wpid = try d.liveWorkerPid()
        let stop = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        try expect((stop["suspended_tasks"] as? [String])?.contains(id) == true, "任务应在冻结名单: \(stop)")
        try expect((stop["freeze_ms"] as? Int ?? 999) < 100, "freeze 应 <100ms")
        let t = try d.waitTask(id, state: "suspended", timeout: 10)
        try expect(t.suspendReason == "emergency_stop", "reason 应 emergency_stop，实际 \(t.suspendReason ?? "?")")
        if let pid = wpid {
            try waitUntil(timeout: 5, "worker T 状态") { procState(pid)?.hasPrefix("T") == true }
        }
        _ = try d.api.call("task_cancel", params: ["task": id])   // 清理（frozen 下 cancel 允许）
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
    }

    s.add(57, "F", "急停期新任务：只入队（emergency_frozen）不启动") {
        let cli = try mockCLI("slow57", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("f57"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        let r = try d.createTask(title: "冻结期任务", workdir: Env.freshDir("f57-work").path)
        try expect((r["queued"] as? Bool) == true, "急停期任务应入队，实际 \(r)")
        try expect((r["reason"] as? String) == "emergency_frozen", "reason 应 emergency_frozen")
        let id = (r["task"] as? [String: Any])?["id"] as! String
        try expect(try d.task(id)?.state == "queued", "应停在 queued")
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
        _ = try d.waitTask(id, state: "working", timeout: 10)   // 恢复后被调度
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(58, "F", "resume_all(flush)：解冻续跑") {
        let cli = try mockCLI("slow58", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("f58"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let sub = try liveSub(d)
        let r = try d.createTask(title: "解冻对象", workdir: Env.freshDir("f58-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        let resume = try d.api.call("server_resume_all", params: ["steering": "flush"])
        try expect((resume["resumed"] as? [String])?.contains(id) == true, "应在 resumed 名单: \(resume)")
        // 非空载：emergency_resumed.resumed 应含该任务
        let resumedEv = try collect(sub, deadline: 6) { $0.type == "emergency_resumed" }
        try expect(!resumedEv.isEmpty, "应收到 emergency_resumed 事件")
        try expect((resumedEv.first?.payload["resumed"] as? [String])?.contains(id) == true,
                  "emergency_resumed.resumed 应含 \(id)")
        sub.stop()
        _ = try d.waitTask(id, state: "working", timeout: 10)
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(59, "F", "resume_all(hold)：积压轻推显式丢弃（steering_dropped）") {
        let cli = try mockCLI("slow59", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("f59"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let sub = try liveSub(d)
        let r = try d.createTask(title: "丢弃验证", workdir: Env.freshDir("f59-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_steer", params: ["task": id, "message": "冻结期指令"])
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        _ = try d.api.call("server_resume_all", params: ["steering": "hold"])
        let dropped = try collect(sub, deadline: 8) { $0.type == "steering_dropped" && $0.taskID == id }
        try expect(!dropped.isEmpty, "hold 应发 steering_dropped 事件")
        sub.stop()
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(60, "F", "急停中单任务 resume → -409 引导 resume_all") {
        let cli = try mockCLI("slow60", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("f60"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "单独恢复", workdir: Env.freshDir("f60-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        _ = try expectRPC(-409, contains: "resume_all") {
            try d.api.call("task_resume", params: ["task": id])
        }
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(61, "F", "急停后 daemon 崩溃重启：挂起保持，可恢复") {
        let cli = try mockCLI("slow61", MockCLI.slow)
        let dir = Env.freshDir("f61")
        var d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        let r = try d.createTask(title: "急停存活", workdir: Env.freshDir("f61-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        stopDaemon(d, killHard: true)
        d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let sub = try liveSub(d)
        let t = try d.task(id)
        try expect(t?.state == "suspended", "重启后应保持挂起，实际 \(t?.state ?? "?")")
        // 急停状态已持久化：重启后仍冻结，单任务 resume 被 -409 引导走 resume_all
        _ = try expectRPC(-409, contains: "resume_all") {
            try d.api.call("task_resume", params: ["task": id])
        }
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
        // 原 worker 已随重启死亡 → 走 dead_suspended respawn；id 出现在
        // EmergencyResumed.resumed，任务重新变 working
        let resumedEv = try collect(sub, deadline: 8) { $0.type == "emergency_resumed" }
        try expect(!resumedEv.isEmpty, "应收到 emergency_resumed 事件")
        try expect((resumedEv.first?.payload["resumed"] as? [String])?.contains(id) == true,
                  "emergency_resumed.resumed 应含 \(id)")
        sub.stop()
        _ = try d.waitTask(id, state: "working", timeout: 12)
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(62, "F", "急停/恢复的幂等性：重复调用不炸") {
        let d = try spawnDaemon(dataDir: Env.freshDir("f62"))
        defer { stopDaemon(d) }
        _ = try d.api.call("server_emergency_stop", params: ["reason": "a"])
        let second = try d.api.call("server_emergency_stop", params: ["reason": "b"])
        try expect((second["sessions_preserved"] as? Bool) == true, "二次急停应优雅（空结果）: \(second)")
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
        let again = try d.api.call("server_resume_all", params: ["steering": "flush"])
        try expect((again["resumed"] as? [Any])?.isEmpty == true, "未冻结时 resume_all 应空列表")
        _ = try d.status()   // daemon 仍健康
    }
}

func registerG_Events() {
    let s = Suite.shared

    s.add(63, "G", "from_seq=1 重放全史：无缺无漏（count == event_seq）") {
        let cli = try mockCLI("ok63", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("g63"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil),
                                 env: ["MAESTRO_ROUND_GAP_MS": "100"])
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "历史", workdir: Env.freshDir("g63-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)
        let total = try d.status().eventSeq
        let replayed = try readEvents(socketPath: d.api.eventsSocketPath, from: 1, live: false, maxWait: 8)
        try expect(UInt64(replayed.count) == total, "重放 \(replayed.count) 条 ≠ 总数 \(total)")
        try expect(replayed.first?.seq == 1, "首条 seq 应 1")
        try expect(replayed.contains { $0.type == "task_created" && $0.taskID == id }, "重放缺 task_created")
    }

    s.add(64, "G", "from_seq 中段游标：含语义后缀（seq ≥ from）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g64"))
        defer { stopDaemon(d) }
        for i in 1...3 {
            _ = try d.createTask(title: "任务\(i)", workdir: Env.freshDir("g64-w\(i)").path)
            _ = try waitUntil(timeout: 15, "任务\(i) blocked") {
                (try? self_task(d, (try? d.taskList())?.first { $0.title == "任务\(i)" }?.id ?? ""))??.state == "blocked"
            }
        }
        let all = try readEvents(socketPath: d.api.eventsSocketPath, from: 1, live: false, maxWait: 8)
        let n = all.count
        try expect(n >= 10, "事件太少: \(n)")
        let from = UInt64(n - 3)
        let suffix = try readEvents(socketPath: d.api.eventsSocketPath, from: from, live: false, maxWait: 8)
        try expect(suffix.count == 4, "seq>=from 含语义：应 4 条，实际 \(suffix.count)")
        try expect(suffix.first?.seq == from, "首条应 seq==\(from)，实际 \(suffix.first?.seq ?? 0)")
    }

    s.add(65, "G", "from_seq=当前+1：只收新事件，不重放") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g65"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "旧事件", workdir: Env.freshDir("g65-w0").path)
        try waitUntil(timeout: 15, "有历史") { (try? d.status())?.eventSeq ?? 0 > 2 }
        let cur = try d.status().eventSeq
        let sub = try AsyncSubscriber(fromSeq: cur + 1, socketPath: d.api.eventsSocketPath)
        _ = try d.createTask(title: "新事件", workdir: Env.freshDir("g65-w1").path)
        let evs = try collect(sub, deadline: 10) { $0.type == "task_created" && $0.taskTitle == "新事件" }
        try expect(!evs.isEmpty, "应收新任务创建事件")
        try expect(sub.all.allSatisfy { $0.seq > cur }, "不得重放旧事件: seqs \(sub.all.map { $0.seq })")
        sub.stop()
    }

    s.add(66, "G", "live:false 非跟随：重放完服务端即关连接") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g66"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "非跟随", workdir: Env.freshDir("g66-w").path)
        try waitUntil(timeout: 10, "有事件") { (try? d.status())?.eventSeq ?? 0 >= 1 }
        let conn = try UnixSocketConnection(path: d.api.eventsSocketPath)
        try conn.writeAll(try JSONSerialization.data(withJSONObject: ["from_seq": 1, "live": false]))
        try conn.writeAll(Data([0x0A]))
        var count = 0
        let start = Date()
        while Date().timeIntervalSince(start) < 6 {
            guard let line = try conn.readLine() else { break }   // EOF = 服务端关闭
            if !line.isEmpty { count += 1 }
        }
        conn.close()
        try expect(count >= 1, "应先收到事件")
        try expect(Date().timeIntervalSince(start) < 5.5, "服务端应在重放完关闭（EOF）")
    }

    s.add(67, "G", "多订阅者：各自独立收全") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g67"))
        defer { stopDaemon(d) }
        let s1 = try liveSub(d)
        let s2 = try liveSub(d)
        let r = try d.createTask(title: "广播", workdir: Env.freshDir("g67-w").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try collect(s1, deadline: 10) { $0.type == "task_created" && $0.taskID == id }
        _ = try collect(s2, deadline: 10) { $0.type == "task_created" && $0.taskID == id }
        s1.stop(); s2.stop()
    }

    s.add(68, "G", "断线续订（lastSeq+1）：无缝、无重复") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g68"))
        defer { stopDaemon(d) }
        let sub1 = try liveSub(d)
        _ = try d.createTask(title: "第一段", workdir: Env.freshDir("g68-w1").path)
        _ = try collect(sub1, deadline: 10) { $0.type == "task_created" }
        let last = sub1.all.map { $0.seq }.max() ?? 0
        sub1.stop()
        // 断线期间产生新事件
        _ = try d.createTask(title: "断线中", workdir: Env.freshDir("g68-w2").path)
        try waitUntil(timeout: 10, "断线事件已入史") {
            (try? d.status())?.eventSeq ?? 0 > last
        }
        let sub2 = try AsyncSubscriber(fromSeq: last + 1, socketPath: d.api.eventsSocketPath)
        try waitUntil(timeout: 10, "续订收到事件") { !sub2.all.isEmpty }
        let evs = sub2.all
        try expect((evs.first?.seq ?? 0) == last + 1, "续订首条应 seq==\(last + 1)，实际 \(evs.first?.seq ?? 0)")
        try expect(evs.allSatisfy { $0.seq > last }, "续订不得重放 ≤\(last) 的事件")
        sub2.stop()
    }

    s.add(69, "G", "信封完整性：seq 严格递增 + ts + priority") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g69"))
        defer { stopDaemon(d) }
        let sub = try liveSub(d)
        for i in 1...3 {
            _ = try d.createTask(title: "信封\(i)", workdir: Env.freshDir("g69-w\(i)").path)
        }
        _ = try collect(sub, deadline: 15) { $0.type == "task_created" && $0.taskTitle == "信封3" }
        let evs = sub.all
        try expect(evs.count >= 3, "事件不足: \(evs.count)")
        try expect(evs.map { $0.seq } == evs.map { $0.seq }.sorted(), "seq 应递增到达")
        try expect(evs.allSatisfy { $0.ts > 0 }, "ts 应非零")
        try expect(evs.allSatisfy { ["info", "warning", "critical"].contains($0.priority) },
                    "priority 非法: \(Set(evs.map { $0.priority }))")
        sub.stop()
    }

    s.add(70, "G", "EventStream.stop()：阻塞读被打断，线程 ≤2.5s 退出") {
        let d = try spawnDaemon(dataDir: Env.freshDir("g70"))
        defer { stopDaemon(d) }
        let stream = EventStream(socketPath: d.api.eventsSocketPath)
        var got = 0
        stream.onEvent = { _ in got += 1 }
        stream.start()
        try Thread.sleep(forTimeInterval: 1.0)   // 进入阻塞 read（无事件）
        let start = Date()
        stream.stop()
        try waitUntil(timeout: 2.5, "EventStream 线程退出") { stream.hasFinished }
        let elapsed = Date().timeIntervalSince(start)
        try expect(elapsed < 2.4, "退出耗时 \(elapsed)s —— stop 未打断阻塞读")
    }

    s.add(71, "G", "daemon 死亡重启：EventStream 自动重连且不重不漏") {
        let cli = try mockCLI("ok71", MockCLI.ok)
        let dir = Env.freshDir("g71")
        var d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil),
                                env: ["MAESTRO_ROUND_GAP_MS": "100"])
        let stream = EventStream(socketPath: d.api.eventsSocketPath)
        let box = EventBox()
        stream.onEvent = { box.append($0) }
        stream.start()
        let r = try d.createTask(title: "断连前", workdir: Env.freshDir("g71-w1").path)
        let id1 = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id1, state: "done", timeout: 30)
        try waitUntil(timeout: 8, "断连前事件已收") { box.contains { $0.type == "task_completed" && $0.taskID == id1 } }
        let lastBefore = box.all.map { $0.seq }.max() ?? 0
        stopDaemon(d, killHard: true)
        try Thread.sleep(forTimeInterval: 3.5)   // 经历 ≥2 次重连失败
        d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil),
                            env: ["MAESTRO_ROUND_GAP_MS": "100"])
        defer { stopDaemon(d) }
        let r2 = try d.createTask(title: "重连后", workdir: Env.freshDir("g71-w2").path)
        let id2 = (r2["task"] as? [String: Any])?["id"] as! String
        try waitUntil(timeout: 15, "重连后收到新事件") {
            box.contains { $0.type == "task_created" && $0.taskID == id2 }
        }
        // 不重复：重连后收到的事件 seq 均 > 断连前最大 seq
        let afterReconnect = box.all.filter { $0.seq > lastBefore }
        try expect(!afterReconnect.isEmpty, "应有新事件")
        let dupCount = box.all.count - Set(box.all.map { $0.seq }).count
        try expect(dupCount == 0, "不得重复投递（重复 \(dupCount) 条）")
        stream.stop()
    }

    s.add(72, "G", "AppState 集成：连接/裁剪500/急停标志（客户端状态机）") {
        // AppState 用进程 env MAESTRO_DATA_DIR（harness 启动时注入 appstate 目录）
        let dir = URL(fileURLWithPath: ProcessInfo.processInfo.environment["MAESTRO_DATA_DIR"]
                        ?? "/tmp/maestro-scenarios/appstate")
        try? FileManager.default.removeItem(at: dir)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let cli = try mockCLI("slow72", MockCLI.slow)
        let d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil))
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "状态机", workdir: Env.freshDir("g72-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)

        let state = AppState()
        state.start()
        defer { state.eventStreamStopForTest() }
        // 600 条轻推 → 600+ 事件，客户端应裁到 500
        DispatchQueue.global().async {
            for i in 0..<600 {
                _ = try? d.api.call("task_steer", params: ["task": id, "message": "灌事件 \(i)"])
            }
        }
        let ok = runloopWait(20) { state.events.count >= 500 && state.connected }
        try expect(ok, "AppState 应连上并裁到 500（connected=\(state.connected) events=\(state.events.count)）")
        try expect(state.events.count == 500, "事件应恰好 500，实际 \(state.events.count)")
        let seqs = state.events.prefix(20).map { $0.seq }
        try expect(seqs == seqs.sorted(by: >), "应新事件在前（降序）: \(seqs)")
        try expect(state.tasks.contains { $0.id == id }, "任务应已入列")
        // 急停标志：事件驱动翻转
        _ = try d.api.call("server_emergency_stop", params: ["reason": "test"])
        try expect(runloopWait(8) { state.isEmergencyStopped }, "isEmergencyStopped 应翻 true")
        _ = try d.api.call("server_resume_all", params: ["steering": "flush"])
        try expect(runloopWait(8) { !state.isEmergencyStopped }, "isEmergencyStopped 应翻 false")
        _ = try d.api.call("task_cancel", params: ["task": id])
    }
}

/// 线程安全事件盒（EventStream 回调收集）
final class EventBox {
    private var items: [MaestroEvent] = []
    private let lock = NSLock()
    func append(_ e: MaestroEvent) {
        lock.lock(); items.append(e); lock.unlock()
    }
    var all: [MaestroEvent] {
        lock.lock(); defer { lock.unlock() }
        return items
    }
    func contains(_ pred: (MaestroEvent) -> Bool) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return items.contains(where: pred)
    }
}

func registerH_Checkpoints() {
    let s = Suite.shared

    /// 建 git workdir（检查点依赖 git）
    func gitWork(_ name: String) throws -> URL {
        let w = Env.freshDir(name)
        try "readme".write(to: w.appendingPathComponent("README.md"), atomically: true, encoding: .utf8)
        let git = { (_ args: [String]) in runCmd("/usr/bin/git", args, cwd: w, timeout: 10) }
        _ = git(["init"])
        _ = git(["-c", "user.email=t@t.t", "-c", "user.name=t", "add", "-A"])
        _ = git(["-c", "user.email=t@t.t", "-c", "user.name=t", "commit", "-m", "init"])
        return w
    }

    s.add(73, "H", "baseline 检查点：task_create 即锚点，checkpoint_list 可见") {
        let d = try spawnDaemon(dataDir: Env.freshDir("h73"))
        defer { stopDaemon(d) }
        let w = try gitWork("h73-work")
        let r = try d.createTask(title: "检查点", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let cps = try d.api.call("checkpoint_list", params: ["task": id])
        let list = cps["checkpoints"] as? [[String: Any]] ?? []
        try expect(!list.isEmpty, "baseline 检查点缺失: \(cps)")
        try expect(list.contains { ($0["reason"] as? String) == "baseline" }, "应含 baseline: \(list)")
        try expect(list.allSatisfy { ($0["ref"] as? String ?? "").hasPrefix("refs/maestro/cp/") },
                    "ref 应在 maestro 命名空间: \(list)")
    }

    s.add(74, "H", "checkpoint_rollback：文件还原 + pre_rollback 安全垫") {
        let cli = try mockCLI("ok74", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("h74"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil),
                                env: ["MAESTRO_ROUND_GAP_MS": "100"])
        defer { stopDaemon(d) }
        let w = try gitWork("h74-work")
        let r = try d.createTask(title: "回滚对象", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)   // mock 产物 mock-result.md 已写入
        try expect(w.appendingPathComponent("mock-result.md").isFileExists, "前置：产物应在")
        let before = try d.api.call("checkpoint_list", params: ["task": id])
        let baseline = ((before["checkpoints"] as? [[String: Any]])?.first { ($0["reason"] as? String) == "baseline" })?["ref"] as? String
        guard let baselineRef = baseline else { throw TestFailure(message: "无 baseline ref") }
        // Swift 模型解析（B10 时光机用的就是 CheckpointInfo）
        let rawModels = before["checkpoints"] as? [[String: Any]] ?? []
        let models = rawModels.compactMap(CheckpointInfo.from)
        try expect(models.count == rawModels.count, "全部检查点应可解析为 CheckpointInfo")
        let baselineModel = models.first { $0.reason == "baseline" }
        try expect(baselineModel != nil, "应解析出 baseline 模型")
        try expect(baselineModel?.isPinned == true, "baseline 应 pinned")
        try expect(baselineModel?.round != nil, "baseline 应带 round（新 additive 字段）")
        try expect((baselineModel?.ts ?? 0) > 0, "baseline 应带 ts（>0）")

        let rb = try d.api.call("checkpoint_rollback", params: ["task": id, "to": baselineRef])
        let pre = rb["pre_rollback"] as? String ?? ""
        try expect(pre.hasPrefix("refs/maestro/cp/"), "pre_rollback 安全垫应存在: \(rb)")
        try expect(w.appendingPathComponent("mock-result.md").isFileExists == false, "回滚后产物应被还原")
        let after = try d.api.call("checkpoint_list", params: ["task": id])
        let refs = ((after["checkpoints"] as? [[String: Any]])?.compactMap { $0["ref"] as? String }) ?? []
        try expect(refs.contains(pre), "pre_rollback 应入检查点清单")

        // 撤销回滚：再回滚到 pre_rollback 安全垫 → 产物恢复（对应时光机「撤销回滚」）
        _ = try d.api.call("checkpoint_rollback", params: ["task": id, "to": pre])
        try expect(w.appendingPathComponent("mock-result.md").isFileExists,
                  "撤销回滚后产物应恢复（回到安全垫）")
    }

    s.add(75, "H", "rollback 到不存在的 ref → 报错不崩") {
        let d = try spawnDaemon(dataDir: Env.freshDir("h75"))
        defer { stopDaemon(d) }
        let w = try gitWork("h75-work")
        let r = try d.createTask(title: "坏ref", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let e = try expectRPC(-500) {
            try d.api.call("checkpoint_rollback", params: ["task": id, "to": "refs/maestro/cp/\(id)/99-baseline"])
        }
        try expect("\(e)".count > 0, "应有错误信息")
        _ = try d.status()   // daemon 仍健康
    }

    s.add(76, "H", "rollback 到非 maestro ref（refs/heads/main）→ -403 拦截") {
        let d = try spawnDaemon(dataDir: Env.freshDir("h76"))
        defer { stopDaemon(d) }
        let w = try gitWork("h76-work")
        let r = try d.createTask(title: "越界ref", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try expectRPC(-403, contains: "命名空间") {
            try d.api.call("checkpoint_rollback", params: ["task": id, "to": "refs/heads/main"])
        }
        // git 仓库无恙：main ref 未被动过
        let git = runCmd("/usr/bin/git", ["rev-parse", "--verify", "refs/heads/main"], cwd: w, timeout: 10)
        try expect(git.code == 0, "refs/heads/main 应原样存在")
    }
}
