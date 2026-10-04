// main.swift — ScenarioRunner 入口：注册 100 场景并顺序执行
import Foundation
import MaestroCore

// 子进程探针模式：在受控环境变量下检查客户端发现逻辑（A07-A10 场景用）
if CommandLine.arguments.contains("--probe-default-dir") {
    print(MaestroAPI.defaultDataDir().path)
    exit(0)
}
if CommandLine.arguments.contains("--probe-locate") {
    let dir = MaestroAPI.defaultDataDir()
    let dm = DaemonManager(dataDir: dir)
    dm.locateBinaries()
    func p(_ u: URL?) -> String { u?.path ?? "" }
    print("{\"daemon\":\"\(p(dm.daemonPath))\",\"rounder\":\"\(p(dm.rounderPath))\",\"cli\":\"\(p(dm.cliPath))\",\"dialect\":\"\(dm.detectedCLI?.dialect ?? "")\"}")
    exit(0)
}

struct Scenario {
    let num: Int
    let name: String
    let group: String
    let run: () throws -> Void
}

final class Suite {
    static let shared = Suite()
    private var scenarios: [Scenario] = []
    private var registered = Set<Int>()

    func add(_ num: Int, _ group: String, _ name: String, _ run: @escaping () throws -> Void) {
        assert(!registered.contains(num), "场景编号重复: \(num)")
        registered.insert(num)
        scenarios.append(Scenario(num: num, name: name, group: group, run: run))
    }

    func execute() {
        scenarios.sort { $0.num < $1.num }
        if let filter = Env.scenarioFilter {
            scenarios = scenarios.filter { filter.contains($0.num) }
            print("（场景过滤：\(scenarios.count) 个）")
        }
        print("Maestro Mac 客户端 100 场景验收 —— \(Date())")
        print("GUI 二进制: \(Env.guiBinary.path)")
        print("Rust bin:   \(Env.binDir.path)")
        print("工作区:     \(Env.work.path)")
        print("──────────────────────────────────────────────────────")
        var passed = 0
        var failed: [(Scenario, String)] = []
        let totalStart = Date()
        for s in scenarios {
            let start = Date()
            let ms: (() -> String) = { String(format: "%5.0fms", start.timeIntervalSinceNow * -1000) }
            do {
                try s.run()
                passed += 1
                print("[\(String(format: "%03d", s.num))/100] ✅ \(s.group) \(s.name) (\(ms()))")
            } catch {
                let msg = (error as? TestFailure)?.message ?? "\(error)"
                failed.append((s, msg))
                print("[\(String(format: "%03d", s.num))/100] ❌ \(s.group) \(s.name) —— \(msg) (\(ms()))")
            }
            fflush(stdout)
        }
        print("──────────────────────────────────────────────────────")
        print("总计 \(scenarios.count) 场景：✅ \(passed) 通过，❌ \(failed.count) 失败，耗时 \(Int(Date().timeIntervalSince(totalStart)))s")
        if !failed.isEmpty {
            print("\n失败清单：")
            for (s, msg) in failed {
                print("  #\(String(format: "%03d", s.num)) [\(s.group)] \(s.name)\n      └─ \(msg)")
            }
        }
        // 报告落盘
        var report = "# Maestro Mac 客户端 100 场景验收报告\n\n- 时间：\(Date())\n- 结果：**\(passed)/100 通过**（失败 \(failed.count)）\n\n"
        report += "| # | 组 | 场景 | 结果 | 耗时 |\n|---:|---|---|---|---|\n"
        for s in scenarios {
            let isFail = failed.contains { $0.0.num == s.num }
            report += "| \(s.num) | \(s.group) | \(s.name) | \(isFail ? "❌" : "✅") | — |\n"
        }
        if !failed.isEmpty {
            report += "\n## 失败详情\n\n"
            for (s, msg) in failed {
                report += "- **#\(s.num) \(s.name)**：\(msg)\n"
            }
        }
        try? report.write(to: Env.work.appendingPathComponent("report.md"), atomically: true, encoding: .utf8)
        exit(failed.isEmpty ? 0 : 1)
    }
}

// 注册全部场景
registerA_Lifecycle()      // A 01-12  daemon 生命周期与发现
registerB_TaskCreate()     // B 13-26  任务创建与校验
registerC_WorkerFlows()    // C 27-38  真实 worker 任务流转
registerD_PauseCancel()    // D 39-46  暂停/恢复/取消
registerE_Steering()       // E 47-54  轻推
registerF_Emergency()      // F 55-62  急停
registerG_Events()         // G 63-72  事件流
registerH_Checkpoints()    // H 73-76  检查点
registerI_Protocol()       // I 77-84  协议健壮性
registerJ_Load()           // J 85-90  并发与负载
registerK_GUI()            // K 91-100 GUI 进程与 CLI 集成

Env.resetWorkspace()
Suite.shared.execute()
