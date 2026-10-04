// ScenariosB.swift — C 27-38 真实 worker 任务流转；D 39-46 暂停/恢复/取消；E 47-54 轻推
import Foundation
import MaestroCore

/// 建 mock CLI（独立目录，避免同名冲突）并返回其路径
func mockCLI(_ name: String, _ script: String) throws -> URL {
    MockCLI.install(into: Env.freshDir("cli-\(name)"), script: script)
}

func registerC_WorkerFlows() {
    let s = Suite.shared
    let fastEnv = ["MAESTRO_ROUND_GAP_MS": "100"]   // daemon 白名单透传，加速轮循环

    s.add(27, "C", "mock CLI 两轮任务 → done（事件全生命周期）") {
        let cli = try mockCLI("ok", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("c27"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let w = Env.freshDir("c27-work")
        // 事件订阅先就位（from_seq=0：live-only）
        let sub = try liveSub(d)
        let r = try d.createTask(title: "两轮完成", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.waitTask(id, state: "done", timeout: 30)
        try expect(t.round == 2, "应 2 轮完成，实际 \(t.round)")
        let types = try collect(sub, deadline: 12) { $0.taskID == id }
        let names = Set(types.map { $0.type })
        try expect(names.contains("task_created"), "缺 task_created: \(names)")
        try expect(names.contains("worker_spawned"), "缺 worker_spawned")
        try expect(names.contains("ledger_entry"), "缺 ledger_entry（轮账未入账）")
        try expect(names.contains("task_completed"), "缺 task_completed")
        // 轮账：2 条 ledger_entry，input_tokens 合计 900+1500=2400（usage 对象内）
        let ledgers = types.filter { $0.type == "ledger_entry" }
        try expect(ledgers.count == 2, "应 2 条入账，实际 \(ledgers.count)")
        let inSum = ledgers.compactMap {
            (($0.payload["usage"] as? [String: Any])?["input_tokens"] as? Int) ?? 0
        }.reduce(0, +)
        try expect(inSum == 2400, "input_tokens 应合计 2400，实际 \(inSum)")
        try expect(types.contains { $0.type == "round_progress" && ($0.payload["round"] as? Int) == 2 },
                  "缺第 2 轮 round_progress")
    }

    s.add(28, "C", "done 任务详情：round/终态/产物落盘") {
        let cli = try mockCLI("ok2", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("c28"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let w = Env.freshDir("c28-work")
        let r = try d.createTask(title: "详情验证", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.waitTask(id, state: "done", timeout: 30)
        try expect(t.isTerminal, "done 应为终态")
        try expect(w.appendingPathComponent("mock-result.md").isFileExists, "mock 产物未落盘")
        // 会话/轮账持久化在 workdir/.maestro/<task>/
        let stateDir = w.appendingPathComponent(".maestro").appendingPathComponent(id)
        try expect(stateDir.appendingPathComponent("session").isFileExists, "session 凭据未持久化")
        try expect(stateDir.appendingPathComponent("rounds.jsonl").isFileExists, "轮账未持久化")
    }

    s.add(29, "C", "task_ledger：轮数与 token 汇总正确") {
        let cli = try mockCLI("ok3", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("c29"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let w = Env.freshDir("c29-work")
        let r = try d.createTask(title: "账本", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)
        let ledger = try d.api.call("task_ledger", params: ["task": id])
        try expect((ledger["rounds"] as? Int) == 2, "rounds 应 2，实际 \(ledger["rounds"] ?? "?")")
        // usage：900+1500 = 2400；cache_read 600；creation 200
        try expect((ledger["input_tokens"] as? Int) == 2400, "input_tokens 应 2400，实际 \(ledger["input_tokens"] ?? "?")")
        try expect((ledger["cache_read_tokens"] as? Int) == 600, "cache_read 应 600")
        try expect((ledger["cache_creation_tokens"] as? Int) == 200, "cache_creation 应 200")
        try expect((ledger["ledger_entries"] as? Int) == 2, "entries 应 2")
    }

    s.add(30, "C", "worker_list：活 worker 带 pid") {
        let cli = try mockCLI("slow30", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("c30"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        _ = try d.createTask(title: "长任务", workdir: Env.freshDir("c30-work").path)
        try waitUntil(timeout: 10, "worker 出现") {
            (try? d.workers())?.isEmpty == false
        }
        let ws = try d.workers()
        try expect(ws.contains { ($0["pid"] as? Int ?? 0) > 0 }, "worker 应带 pid: \(ws)")
    }

    s.add(31, "C", "echo 假完成 3 振 → blocked(acceptance_failed)") {
        let d = try spawnDaemon(dataDir: Env.freshDir("c31"))
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "假完成", workdir: Env.freshDir("c31-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.waitTask(id, state: "blocked", timeout: 30)
        try expect(t.blockedKind == "acceptance_failed", "blocked_kind 应 acceptance_failed，实际 \(t.blockedKind ?? "?")")
        try expect(t.acceptanceFailures >= 3, "3 振计数应 ≥3，实际 \(t.acceptanceFailures)")
    }

    s.add(32, "C", "blocked 任务进收件箱（kind/title 对应）") {
        let dir = Env.freshDir("c32")
        let d = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "收件箱项", workdir: Env.freshDir("c32-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "blocked", timeout: 30)
        let inbox = try d.inbox()
        try expect(inbox.contains { $0.task == id && $0.kind == "acceptance_failed" && $0.title == "收件箱项" },
                  "收件箱应含 \(id)，实际 \(inbox.map { "\($0.task):\($0.kind)" })")
        // 客户端渲染文案
        try expect(inbox.first?.kindText == "验收 3 次失败", "kindText 不符: \(inbox.first?.kindText ?? "?")")
    }

    s.add(33, "C", "收件箱 resume → 重新入队（验收清零）→ 再 blocked") {
        let d = try spawnDaemon(dataDir: Env.freshDir("c33"))
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "再战", workdir: Env.freshDir("c33-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "blocked", timeout: 30)
        let resume = try d.api.call("task_resume", params: ["task": id])
        try expect((resume["requeued"] as? Bool) == true, "resume 应 requeued，实际 \(resume)")
        let t2 = try d.waitTask(id, state: "blocked", timeout: 30)   // echo worker 必然再次 3 振
        try expect(t2.acceptanceFailures >= 3, "重跑后应再次 3 振")
    }

    s.add(34, "C", "CLI 非零退出 → failed") {
        let cli = try mockCLI("fail34", MockCLI.fail)
        let d = try spawnDaemon(dataDir: Env.freshDir("c34"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "失败任务", workdir: Env.freshDir("c34-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.waitTask(id, state: "failed", timeout: 20)
        try expect(t.isTerminal, "failed 应终态")
    }

    s.add(35, "C", "API 429 → 断连挂起（自动恢复通道）") {
        let cli = try mockCLI("apierr35", MockCLI.apiErr)
        let d = try spawnDaemon(dataDir: Env.freshDir("c35"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "限流任务", workdir: Env.freshDir("c35-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        try waitUntil(timeout: 20, "进入 suspended（断连自动恢复）") {
            guard let t = try? self_task(d, id) else { return false }
            return t.state == "suspended"
        }
        let t = try d.task(id)!
        try expect(["network_lost", "provider_outage"].contains(t.suspendReason ?? ""),
                    "suspend_reason 应 network_lost/provider_outage，实际 \(t.suspendReason ?? "?")")
    }

    s.add(36, "C", "永不完成 + MAX_ROUNDS=3 → blocked(rounds_exhausted)") {
        let cli = try mockCLI("nd36", MockCLI.neverDone)
        let d = try spawnDaemon(dataDir: Env.freshDir("c36"), worker: .rounder(cli: cli, maxRounds: 3, roundGapMs: nil),
                                 env: ["MAESTRO_MAX_ROUNDS": "3", "MAESTRO_ROUND_GAP_MS": "100"])
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "轮数耗尽", workdir: Env.freshDir("c36-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.waitTask(id, state: "blocked", timeout: 30)
        try expect(t.blockedKind == "rounds_exhausted", "kind 应 rounds_exhausted，实际 \(t.blockedKind ?? "?")")
        try expect(t.round >= 3, "应至少跑满 3 轮，实际 \(t.round)")
    }

    s.add(37, "C", "同 workdir 互斥：第二个 queued(workdir_busy)，随后自动启动") {
        let cli = try mockCLI("ok37", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("c37"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let w = Env.freshDir("c37-work")   // 两个任务同 workdir
        _ = try d.createTask(title: "先来", workdir: w.path)
        let r2 = try d.createTask(title: "后到", workdir: w.path)
        try expect((r2["queued"] as? Bool) == true, "第二个任务应入队，实际 \(r2)")
        try expect((r2["reason"] as? String) == "workdir_busy", "reason 应 workdir_busy，实际 \(r2["reason"] ?? "?")")
        let id2 = (r2["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id2, state: "done", timeout: 40)   // 第一个完成后自动启动
    }

    s.add(38, "C", "并发槽位 MAX_WORKERS=1：第二个 queued(slots_full) → 顺序完成") {
        let cli = try mockCLI("ok38", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("c38"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil),
                                 env: ["MAESTRO_MAX_WORKERS": "1", "MAESTRO_ROUND_GAP_MS": "100"])
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "占槽", workdir: Env.freshDir("c38-w1").path)
        let r2 = try d.createTask(title: "等槽", workdir: Env.freshDir("c38-w2").path)
        try expect((r2["queued"] as? Bool) == true && (r2["reason"] as? String) == "slots_full",
                    "应 slots_full，实际 \(r2)")
        let id2 = (r2["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id2, state: "done", timeout: 40)
    }
}

/// 后台事件订阅器（live-only）：readEvents 的异步版，先订阅再触发动作
final class AsyncSubscriber {
    private(set) var events: [MaestroEvent] = []
    private let conn: UnixSocketConnection
    private let lock = NSLock()
    private var thread: Thread!

    init(fromSeq: UInt64 = 0, socketPath: String) throws {
        conn = try UnixSocketConnection(path: socketPath)
        try conn.writeAll(try JSONSerialization.data(withJSONObject: ["from_seq": fromSeq, "live": true]))
        try conn.writeAll(Data([0x0A]))
        thread = Thread { [weak self] in
            while let line = (try? self?.conn.readLine()) ?? nil {
                guard !line.isEmpty else { continue }
                guard let obj = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
                      let seq = obj["seq"] as? UInt64 else { continue }
                let ev = obj["event"] as? [String: Any] ?? [:]
                self?.lock.lock(); defer { self?.lock.unlock() }
                self?.events.append(MaestroEvent(seq: seq, ts: obj["ts"] as? UInt64 ?? 0,
                                                 priority: obj["priority"] as? String ?? "info",
                                                 type: ev["type"] as? String ?? "?", payload: ev))
            }
        }
        thread.start()
    }

    /// 收集到满足条件的事件（带截止时间）
    func collect(deadline: TimeInterval, _ pred: (MaestroEvent) -> Bool) throws -> [MaestroEvent] {
        try waitUntil(timeout: deadline, "事件到达") {
            self.lock.lock(); defer { self.lock.unlock() }
            return self.events.contains(where: pred)
        }
        lock.lock(); defer { lock.unlock() }
        return events.filter(pred)
    }

    var all: [MaestroEvent] {
        lock.lock(); defer { lock.unlock() }
        return events
    }

    func stop() { conn.close() }
}

func readEventsAsync(socketPath: String) throws -> AsyncSubscriber {
    try AsyncSubscriber(socketPath: socketPath)
}

/// collect(sub, deadline:) { pred } —— 与 AsyncSubscriber.collect 签名对齐的糖
func collect(_ sub: AsyncSubscriber, deadline: TimeInterval, _ pred: @escaping (MaestroEvent) -> Bool) throws -> [MaestroEvent] {
    try sub.collect(deadline: deadline, pred)
}

/// TaskSummary 便捷取（waitUntil 闭包里用）
func self_task(_ d: DaemonHandle, _ id: String) throws -> TaskSummary? {
    try? d.task(id)
}

func registerD_PauseCancel() {
    let s = Suite.shared
    let fastEnv = ["MAESTRO_ROUND_GAP_MS": "100"]

    s.add(39, "D", "pause：working → suspended(user_pause)，进程组冻结 T") {
        let cli = try mockCLI("slow39", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("d39"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "暂停对象", workdir: Env.freshDir("d39-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        try Thread.sleep(forTimeInterval: 0.5)   // 确保进入 sleep 中段
        let paused = try d.api.call("task_pause", params: ["task": id])
        try expect((paused["paused"] as? Bool) == true, "pause 应成功: \(paused)")
        let t = try d.waitTask(id, state: "suspended", timeout: 10)
        try expect(t.suspendReason == "user_pause", "reason 应 user_pause，实际 \(t.suspendReason ?? "?")")
        if let wpid = try d.liveWorkerPid() {
            try waitUntil(timeout: 5, "worker 进程 T 状态") { procState(wpid)?.hasPrefix("T") == true }
        } else {
            throw TestFailure(message: "无活 worker pid")
        }
        _ = try d.api.call("task_cancel", params: ["task": id])   // 清理
    }

    s.add(40, "D", "resume：suspended → working（解冻续跑）") {
        let cli = try mockCLI("slow40", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("d40"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "恢复对象", workdir: Env.freshDir("d40-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_pause", params: ["task": id])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        let resumed = try d.api.call("task_resume", params: ["task": id])
        try expect((resumed["resumed"] as? Bool) == true, "resume 应成功: \(resumed)")
        _ = try d.waitTask(id, state: "working", timeout: 10)
        _ = try d.api.call("task_cancel", params: ["task": id])   // 清理
    }

    s.add(41, "D", "pause 非 working（已 done）→ -409") {
        let cli = try mockCLI("ok41", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("d41"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "已完成", workdir: Env.freshDir("d41-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)
        _ = try expectRPC(-409, contains: "not running") {
            try d.api.call("task_pause", params: ["task": id])
        }
    }

    s.add(42, "D", "resume 非 suspended（working 中）→ -409") {
        let cli = try mockCLI("slow42", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("d42"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "运行中", workdir: Env.freshDir("d42-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try expectRPC(-409, contains: "not suspended") {
            try d.api.call("task_resume", params: ["task": id])
        }
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(43, "D", "cancel：working → cancelled，worker 进程组回收") {
        let cli = try mockCLI("slow43", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("d43"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "取消对象", workdir: Env.freshDir("d43-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        let wpid = try d.liveWorkerPid() ?? 0
        let c = try d.api.call("task_cancel", params: ["task": id])
        try expect((c["cancelled"] as? Bool) == true, "cancel 应成功: \(c)")
        _ = try d.waitTask(id, state: "cancelled", timeout: 10)
        try expect(wpid == 0 || !isAlive(wpid), "worker 进程应被回收")
    }

    s.add(44, "D", "cancel 终态任务 → 优雅报错（不误伤）") {
        let cli = try mockCLI("ok44", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("d44"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "完成了的", workdir: Env.freshDir("d44-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)
        _ = try expectRPC(-404, contains: "not cancellable") {
            try d.api.call("task_cancel", params: ["task": id])
        }
        // 状态未被破坏
        try expect(try d.task(id)?.state == "done", "done 状态不应被改")
    }

    s.add(45, "D", "suspended 状态下 cancel → cancelled") {
        let cli = try mockCLI("slow45", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("d45"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "挂起中取消", workdir: Env.freshDir("d45-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_pause", params: ["task": id])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        _ = try d.api.call("task_cancel", params: ["task": id])
        _ = try d.waitTask(id, state: "cancelled", timeout: 10)
    }

    s.add(46, "D", "pause 后 daemon 崩溃重启 → 挂起保持 → resume 续接") {
        let cli = try mockCLI("slow46", MockCLI.slow)
        let dir = Env.freshDir("d46")
        var d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        let r = try d.createTask(title: "崩溃存活", workdir: Env.freshDir("d46-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_pause", params: ["task": id])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        stopDaemon(d, killHard: true)
        d = try spawnDaemon(dataDir: dir, worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let t = try d.task(id)
        try expect(t?.state == "suspended", "重启后应保持 suspended，实际 \(t?.state ?? "?")")
        let resumed = try d.api.call("task_resume", params: ["task": id])
        try expect((resumed["resumed"] as? Bool) == true, "resume 应成功: \(resumed)")
        _ = try d.waitTask(id, state: "working", timeout: 10)
        _ = try d.api.call("task_cancel", params: ["task": id])
    }
}

func registerE_Steering() {
    let s = Suite.shared
    let fastEnv = ["MAESTRO_ROUND_GAP_MS": "100"]

    s.add(47, "E", "轻推 working 任务：入队 + steering_queued 事件") {
        let cli = try mockCLI("steer47", MockCLI.makeSteerCLI(round1Sleep: 4))
        let d = try spawnDaemon(dataDir: Env.freshDir("e47"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let sub = try liveSub(d)
        let r = try d.createTask(title: "被轻推的", workdir: Env.freshDir("e47-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        let steer = try d.api.call("task_steer", params: ["task": id, "message": "优先处理登录按钮"])
        try expect((steer["queued"] as? Bool) == true, "应 queued: \(steer)")
        try expect((steer["seq"] as? Int) ?? 0 > 0, "应带 seq")
        let evs = try collect(sub, deadline: 8) { $0.type == "steering_queued" && $0.taskID == id }
        try expect(!evs.isEmpty, "未见 steering_queued 事件")
        sub.stop()
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(48, "E", "轻推在轮边界投递：delivered 事件 + 注入下一轮（rounds.jsonl）") {
        let cli = try mockCLI("steer48", MockCLI.makeSteerCLI(round1Sleep: 3))
        let d = try spawnDaemon(dataDir: Env.freshDir("e48"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let w = Env.freshDir("e48-work")
        let sub = try liveSub(d)
        let r = try d.createTask(title: "投递验证", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_steer", params: ["task": id, "message": "记得写测试"])
        _ = try d.waitTask(id, state: "done", timeout: 30)
        let delivered = try collect(sub, deadline: 10) { $0.type == "steering_delivered" && $0.taskID == id }
        try expect(!delivered.isEmpty, "未见 steering_delivered")
        try expect((delivered.first?.payload["message"] as? String)?.contains("写测试") == true,
                    "投递内容不符: \(delivered.first?.payload["message"] ?? "?")")
        sub.stop()
        // rounds.jsonl：注入轮 steering_injected=true
        let roundsFile = w.appendingPathComponent(".maestro").appendingPathComponent(id).appendingPathComponent("rounds.jsonl")
        try waitUntil(timeout: 5, "rounds.jsonl 落盘") { roundsFile.isFileExists }
        let text = try String(contentsOf: roundsFile, encoding: .utf8)
        try expect(text.contains("\"steering_injected\":true"), "轮账应标记注入: \(text.prefix(300))")
    }

    s.add(49, "E", "轻推终态任务 → -409") {
        let cli = try mockCLI("ok49", MockCLI.ok)
        let d = try spawnDaemon(dataDir: Env.freshDir("e49"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "完事后", workdir: Env.freshDir("e49-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "done", timeout: 30)
        _ = try expectRPC(-409, contains: "finished") {
            try d.api.call("task_steer", params: ["task": id, "message": "late"])
        }
    }

    s.add(50, "E", "轻推未知任务 → -404") {
        let d = try spawnDaemon(dataDir: Env.freshDir("e50"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-404, contains: "not found") {
            try d.api.call("task_steer", params: ["task": "t-404", "message": "ghost"])
        }
    }

    s.add(51, "E", "轻推空消息 → -400") {
        let d = try spawnDaemon(dataDir: Env.freshDir("e51"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "占位", workdir: Env.freshDir("e51-work").path)
        _ = try expectRPC(-400, contains: "不能为空") {
            try d.api.call("task_steer", params: ["task": "t-1", "message": ""])
        }
    }

    s.add(52, "E", "轻推长度边界：10000 过 / 10001 拒") {
        let cli = try mockCLI("slow52", MockCLI.slow)
        let d = try spawnDaemon(dataDir: Env.freshDir("e52"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let r = try d.createTask(title: "长推", workdir: Env.freshDir("e52-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_steer", params: ["task": id, "message": String(repeating: "推", count: 10_000)])
        _ = try expectRPC(-400, contains: "超长") {
            try d.api.call("task_steer", params: ["task": id, "message": String(repeating: "推", count: 10_001)])
        }
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(53, "E", "挂起期间轻推 → resume 后投递（积压不丢）") {
        let cli = try mockCLI("steer53", MockCLI.makeSteerCLI(round1Sleep: 3))
        let d = try spawnDaemon(dataDir: Env.freshDir("e53"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let w = Env.freshDir("e53-work")
        let sub = try liveSub(d)
        let r = try d.createTask(title: "挂起积压", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        _ = try d.api.call("task_pause", params: ["task": id])
        _ = try d.waitTask(id, state: "suspended", timeout: 10)
        _ = try d.api.call("task_steer", params: ["task": id, "message": "醒来先做这个"])
        _ = try d.api.call("task_resume", params: ["task": id])
        let delivered = try collect(sub, deadline: 20) { $0.type == "steering_delivered" && $0.taskID == id }
        try expect(!delivered.isEmpty, "恢复后应投递积压轻推")
        sub.stop()
        _ = try d.api.call("task_cancel", params: ["task": id])
    }

    s.add(54, "E", "50 条连发轻推：全序投递（seq 单调）") {
        let cli = try mockCLI("steer54", MockCLI.makeSteerCLI(round1Sleep: 7))
        let d = try spawnDaemon(dataDir: Env.freshDir("e54"), worker: .rounder(cli: cli, maxRounds: nil, roundGapMs: nil), env: fastEnv)
        defer { stopDaemon(d, killHard: true) }
        let sub = try liveSub(d)
        let r = try d.createTask(title: "连发", workdir: Env.freshDir("e54-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        _ = try d.waitTask(id, state: "working", timeout: 15)
        for i in 1...50 {
            _ = try d.api.call("task_steer", params: ["task": id, "message": "指令 \(i)"])
        }
        // 轮边界（~7s）一次性投递 50 条 —— 等齐 50 条 delivered
        let deadline = Date().addingTimeInterval(25)
        var delivered: [MaestroEvent] = []
        while Date() < deadline {
            delivered = sub.all.filter { $0.type == "steering_delivered" && $0.taskID == id }
            if delivered.count >= 50 { break }
            Thread.sleep(forTimeInterval: 0.2)
        }
        try expect(delivered.count == 50, "应投递 50 条，实际 \(delivered.count)")
        sub.stop()
        _ = try d.api.call("task_cancel", params: ["task": id])
    }
}
