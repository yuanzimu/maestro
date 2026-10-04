// ScenariosA.swift — A 01-12 daemon 生命周期与客户端发现；B 13-26 任务创建与校验
import Foundation
import MaestroCore

/// 期望 RPC 错误（返回错误以供进一步断言）
func expectRPC(_ code: Int, contains: String? = nil, _ body: () throws -> [String: Any]) throws -> MaestroError {
    do {
        _ = try body()
        throw TestFailure(message: "应报 RPC 错误 \(code)，实际成功")
    } catch let e as MaestroError {
        guard case .rpc(let c, let msg) = e else { throw TestFailure(message: "错误类型不对: \(e)") }
        try expect(c == code, "错误码应 \(code) 实际 \(c)：\(msg)")
        if let contains { try expect(msg.contains(contains), "消息应含「\(contains)」实际「\(msg)」") }
        return e
    }
}

func registerA_Lifecycle() {
    let s = Suite.shared

    s.add(1, "A", "daemon 启动（echo 模式）· server_status 就绪") {
        let d = try spawnDaemon(dataDir: Env.freshDir("a01"))
        defer { stopDaemon(d) }
        let st = try d.status()
        try expect(!st.version.isEmpty, "version 为空")
        try expect(st.pid > 0, "pid 非法")
        try expect(st.uptimeSecs >= 0, "uptime 非法")
    }

    s.add(2, "A", "同 data-dir 二实例：第二个退出，第一个无损") {
        let dir = Env.freshDir("a02")
        let d1 = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d1) }
        // 裸起第二个（不等待就绪）
        let p = Process()
        p.executableURL = Env.binDir.appendingPathComponent("maestro-daemon")
        p.arguments = ["--data-dir", dir.path]
        p.environment = ProcessInfo.processInfo.environment
        let errFile = dir.appendingPathComponent("second.log")
        FileManager.default.createFile(atPath: errFile.path, contents: nil)
        let fh = try FileHandle(forWritingTo: errFile)
        p.standardOutput = fh; p.standardError = fh
        try p.run()
        // 第二实例应迅速退出（bind 冲突）
        try waitUntil(timeout: 6, "第二 daemon 退出") { !p.isRunning }
        try expect(p.terminationStatus != 0, "第二实例应以非零退出（实际 \(p.terminationStatus)）")
        // 第一实例不受影响（活 socket 未被误删）
        let st = try d1.status()
        try expect(st.pid == d1.pid, "第一实例应继续服务")
    }

    s.add(3, "A", "客户端探活：无 daemon → false") {
        let api = MaestroAPI(dataDir: Env.freshDir("a03"))
        try expect(api.isDaemonAlive == false, "无 daemon 时应不可连")
    }

    s.add(4, "A", "客户端探活：daemon 在 → true") {
        let d = try spawnDaemon(dataDir: Env.freshDir("a04"))
        defer { stopDaemon(d) }
        try expect(d.api.isDaemonAlive, "daemon 在时应可连")
    }

    s.add(5, "A", "server_shutdown 后不可连；残留 socket 重启被清理") {
        let dir = Env.freshDir("a05")
        let d = try spawnDaemon(dataDir: dir)
        _ = try d.api.call("server_shutdown")
        try waitUntil(timeout: 5, "daemon 进程退出") { !d.process.isRunning }
        try expect(d.api.isDaemonAlive == false, "关停后不应可连")
        // 重启：stale socket 文件被探活清理，正常起
        let d2 = try spawnDaemon(dataDir: dir)
        defer { stopDaemon(d2) }
        try expect(d2.api.isDaemonAlive, "重启后应可连（stale socket 已清理）")
    }

    s.add(6, "A", "daemon kill -9 崩溃重启：任务状态持久化") {
        let dir = Env.freshDir("a06")
        var d = try spawnDaemon(dataDir: dir)
        let w = Env.freshDir("a06-work")   // echo 任务需独立干净 workdir（3 振判定）
        let r = try d.createTask(title: "持久化验证", workdir: w.path)
        guard let tid = (r["task"] as? [String: Any])?["id"] as? String else { throw TestFailure(message: "create 无 task.id") }
        _ = try d.waitTask(tid, state: "blocked", timeout: 25)   // echo worker 3 振
        stopDaemon(d, killHard: true)
        d = try spawnDaemon(dataDir: dir)                          // 崩溃恢复
        defer { stopDaemon(d) }
        let tasks = try d.taskList()
        try expect(tasks.contains { $0.id == tid && $0.state == "blocked" },
                    "重启后任务 \(tid) 应仍为 blocked，实际 \(tasks.map { "\($0.id):\($0.state)" })")
    }

    s.add(7, "A", "MAESTRO_DATA_DIR 环境变量覆盖数据目录发现") {
        let runner = URL(fileURLWithPath: CommandLine.arguments[0])
        let r = runCmd(runner.path, ["--probe-default-dir"],
                       env: ["MAESTRO_DATA_DIR": "/tmp/maestro-scenarios/env-override-x"])
        try expect(r.code == 0, "探针退出码 \(r.code)")
        try expect(r.out.trimmingCharacters(in: .whitespacesAndNewlines) == "/tmp/maestro-scenarios/env-override-x",
                    "应取 env 覆盖值，实际: \(r.out)")
    }

    s.add(8, "A", "无环境变量时默认 = $TMPDIR/maestro") {
        let runner = URL(fileURLWithPath: CommandLine.arguments[0])
        var env = ProcessInfo.processInfo.environment
        env.removeValue(forKey: "MAESTRO_DATA_DIR")
        let p = Process()
        p.executableURL = runner
        p.arguments = ["--probe-default-dir"]
        p.environment = env
        let pipe = Pipe()
        p.standardOutput = pipe
        try p.run(); p.waitUntilExit()
        let out = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let tmp = env["TMPDIR"] ?? "/tmp"
        let expected = URL(fileURLWithPath: tmp).appendingPathComponent("maestro").path
        try expect(out.trimmingCharacters(in: .whitespacesAndNewlines) == expected,
                    "默认目录应 \(expected)，实际 \(out)")
    }

    s.add(9, "A", "DaemonManager：PATH 含 bin+mock claude → 全部定位") {
        let mock = MockCLI.install(into: Env.work.appendingPathComponent("a09-bin"), script: MockCLI.ok)
        let runner = URL(fileURLWithPath: CommandLine.arguments[0])
        let path = "\(Env.binDir.path):\(mock.deletingLastPathComponent().path):/usr/bin:/bin"
        let r = runCmd(runner.path, ["--probe-locate"], env: ["PATH": path])
        try expect(r.out.contains("maestro-daemon"), "daemon 未定位: \(r.out) \(r.err)")
        try expect(r.out.contains("maestro-rounder"), "rounder 未定位")
        try expect(r.out.contains("claude"), "mock CLI 未检出")
        try expect(r.out.contains("\"dialect\":\"claude\""), "方言应为 claude")
    }

    s.add(10, "A", "DaemonManager：无 AI CLI → detectedCLI=nil") {
        let runner = URL(fileURLWithPath: CommandLine.arguments[0])
        let r = runCmd(runner.path, ["--probe-locate"], env: ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin"])
        try expect(r.out.contains("\"cli\":\"\""), "不应检出 CLI，实际: \(r.out)")
    }

    s.add(11, "A", "launchDaemon 无 CLI 且非演示模式 → 明确报错") {
        let dir = Env.freshDir("a11")
        let dm = DaemonManager(dataDir: dir)
        dm.locateBinaries()
        guard dm.daemonPath != nil else { throw TestFailure(message: "前置失败：daemonPath 未定位（PATH 应含 target/release）") }
        do {
            try dm.launchDaemon(force: false)
            throw TestFailure(message: "无 CLI 时 launchDaemon(force:false) 应报错")
        } catch let e as NSError where e.code == 2 {
            // 期望路径
        }
        try expect(dm.isOurProcessRunning == false, "不应有残留进程")
    }

    s.add(12, "A", "launchDaemon 演示模式（force）→ daemon 拉起") {
        let dir = Env.freshDir("a12")
        let dm = DaemonManager(dataDir: dir)
        try dm.launchDaemon(force: true)
        defer { _ = try? MaestroAPI(dataDir: dir).call("server_shutdown") }
        let api = MaestroAPI(dataDir: dir)
        try waitUntil(timeout: 8, "演示 daemon 就绪") { api.isDaemonAlive }
        let st = try? api.call("server_status")
        try expect(st?["pid"] != nil, "演示 daemon 应响应 status")
    }
}

func registerB_TaskCreate() {
    let s = Suite.shared

    s.add(13, "B", "task_create 基本字段（id/标题/workdir/状态）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b13"))
        defer { stopDaemon(d) }
        let w = Env.freshDir("b13-work")
        let r = try d.createTask(title: "修登录页", workdir: w.path)
        guard let t = r["task"] as? [String: Any], let id = t["id"] as? String else {
            throw TestFailure(message: "返回缺 task.id: \(r)")
        }
        try expect(id.hasPrefix("t-"), "id 应 t-N 形: \(id)")
        try expect((t["title"] as? String) == "修登录页", "title 不符")
        try expect((t["workdir"] as? String) == w.path, "workdir 不符")
        // create 结果的 Task 不含 state（状态经 task_get 取）。echo 演示 worker
        // 瞬时假完成（~200ms 内 3 振出局）—— task_get 往返间隙状态可能已到
        // blocked：合法集合含 blocked（字段断言才是本场景的核心价值）
        let state = try d.task(id)?.state ?? ""
        try expect(["queued", "planning", "working", "blocked"].contains(state),
                    "state 应 queued/planning/working/blocked，实际 \(state)")
    }

    s.add(14, "B", "task_list 字段齐全（round/failures/narrative）") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b14"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "列表字段", workdir: Env.freshDir("b14-work").path)
        let tasks = try d.taskList()
        try expect(tasks.count == 1, "应 1 个任务")
        let t = tasks[0]
        try expect(t.round >= 0, "round 字段缺失")
        try expect(t.acceptanceFailures >= 0, "acceptance_failures 缺失")
    }

    s.add(15, "B", "task_get 单任务与列表一致") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b15"))
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "详情", workdir: Env.freshDir("b15-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        let t = try d.task(id)
        try expect(t?.id == id && t?.title == "详情", "task_get 不一致")
    }

    s.add(16, "B", "中文+emoji 标题原样往返") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b16"))
        defer { stopDaemon(d) }
        let title = "修复 🐛 登录按钮圆角（P0）"
        let r = try d.createTask(title: title, workdir: Env.freshDir("b16-work").path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        try expect(try d.task(id)?.title == title, "标题被改写")
    }

    s.add(17, "B", "标题长度边界：200 过 / 201 拒") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b17"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: String(repeating: "字", count: 200), workdir: Env.freshDir("b17-work").path)
        _ = try expectRPC(-400, contains: "超长") {
            try d.createTask(title: String(repeating: "字", count: 201), workdir: Env.freshDir("b17-work2").path)
        }
    }

    s.add(18, "B", "空标题 → -400") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b18"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-400, contains: "title") {
            try d.createTask(title: "", workdir: Env.freshDir("b18-work").path)
        }
    }

    s.add(19, "B", "prompt 边界：100k 过 / 100001 拒") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b19"))
        defer { stopDaemon(d) }
        _ = try d.createTask(title: "长prompt", prompt: String(repeating: "a", count: 100_000),
                             workdir: Env.freshDir("b19-work").path)
        _ = try expectRPC(-400, contains: "超长") {
            try d.createTask(title: "超长prompt", prompt: String(repeating: "a", count: 100_001),
                             workdir: Env.freshDir("b19-work2").path)
        }
    }

    s.add(20, "B", "workdir 缺省 = daemon cwd") {
        let dir = Env.freshDir("b20")
        let d = try spawnDaemon(dataDir: dir)   // daemon cwd = Env.work（spawnDaemon 显式设定）
        defer { stopDaemon(d) }
        let r = try d.createTask(title: "缺省workdir")
        let wd = (r["task"] as? [String: Any])?["workdir"] as? String ?? ""
        try expect(samePath(wd, Env.work.path),
                    "缺省 workdir 应 \(Env.work.path)（物理等价即可），实际 \(wd)")
    }

    s.add(21, "B", "workdir 不存在 → -403") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b21"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-403, contains: "不存在") {
            try d.createTask(title: "坏workdir", workdir: "/nonexistent/maestro-b21")
        }
    }

    s.add(22, "B", "workdir 是文件 → -403") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b22"))
        defer { stopDaemon(d) }
        let f = Env.work.appendingPathComponent("b22-file")
        try "x".write(to: f, atomically: true, encoding: .utf8)
        _ = try expectRPC(-403) { try d.createTask(title: "文件workdir", workdir: f.path) }
    }

    s.add(23, "B", "workdir 含 .. → -403") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b23"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-403) { try d.createTask(title: "穿越", workdir: "/tmp/../etc") }
    }

    s.add(24, "B", "敏感目录 /etc → -403") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b24"))
        defer { stopDaemon(d) }
        _ = try expectRPC(-403, contains: "敏感") { try d.createTask(title: "敏感", workdir: "/etc") }
    }

    s.add(25, "B", "凭据子路径（.ssh）→ -403") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b25"))
        defer { stopDaemon(d) }
        let ssh = Env.freshDir("b25-work/proj/.ssh")
        _ = try expectRPC(-403, contains: "凭据") { try d.createTask(title: "ssh", workdir: ssh.path) }
    }

    s.add(26, "B", "workdir 带空格与中文 → OK") {
        let d = try spawnDaemon(dataDir: Env.freshDir("b26"))
        defer { stopDaemon(d) }
        let w = Env.freshDir("b26-work/我的 项目 dir")
        let r = try d.createTask(title: "空格路径", workdir: w.path)
        let id = (r["task"] as? [String: Any])?["id"] as! String
        try expect(try d.task(id)?.state != nil, "任务应存在")
    }
}
