// PaletteMockCheck — ⌘K 命令面板 mock 验证
//
// 本机只有 Command Line Tools（无 XCTest），故用轻量断言程序驱动从面板抽出的
// 纯逻辑 CommandFilter，验证：
//   1. 模糊搜索（大小写不敏感、title/subtitle、中文子串、空 query、保序）
//   2. ↑↓ 快捷键索引移动（边界不越界、空列表）
//   3. 完整交互序列（打开 → 输入 → ↓↓↑ → 回车执行）
//
// 运行：cd clients/macos && swift run PaletteMockCheck
import Foundation
import MaestroCore

// MARK: 迷你断言框架

var passed = 0
var failed = 0

func check(_ cond: Bool, _ name: String, _ detail: String = "") {
    if cond {
        passed += 1
        print("  ✓ \(name)")
    } else {
        failed += 1
        print("  ✗ \(name)\(detail.isEmpty ? "" : "  — \(detail)")")
    }
}

func eq<T: Equatable>(_ actual: T, _ expected: T, _ name: String) {
    check(actual == expected, name, "期望 \(expected)，实际 \(actual)")
}

// MARK: mock 数据（结构与 main.swift 的 provider 一致：8 固定命令 + 任务）

func makeMockCommands() -> [FilterableCommand] {
    var items: [FilterableCommand] = [
        FilterableCommand(title: "新任务", subtitle: "创建并派发给 AI"),
        FilterableCommand(title: "全局急停", subtitle: "冻结所有 worker / 恢复"),
        FilterableCommand(title: "收件箱", subtitle: "需要你决策的任务"),
        FilterableCommand(title: "事件流", subtitle: "实时动态"),
        FilterableCommand(title: "引擎设置", subtitle: "CCR 网关 + 硬预算"),
        FilterableCommand(title: "设置", subtitle: "引擎 / 数据 / 日志"),
        FilterableCommand(title: "启动 Daemon", subtitle: "拉起后台引擎"),
        FilterableCommand(title: "关停 Daemon", subtitle: "停止后台引擎"),
    ]
    let tasks: [(String, String, String)] = [
        ("t-001", "整理本周周报", "running"),
        ("t-002", "Review PR #42", "waiting"),
        ("t-003", "抓取竞品定价数据", "running"),
        ("t-004", "Daily Standup Notes", "done"),
        ("t-005", "回复客户邮件", "blocked"),
    ]
    for (id, title, state) in tasks {
        items.append(FilterableCommand(title: title, subtitle: "\(id) · \(state)"))
    }
    return items
}

let all = makeMockCommands()

// MARK: 1. 模糊搜索

print("【模糊搜索】")
eq(CommandFilter.filter(all, query: "").count, all.count, "空 query 返回全部 13 条")
eq(CommandFilter.filter(all, query: "   ").count, all.count, "纯空白 query 视同空")

let daemon = CommandFilter.filter(all, query: "daemon")
eq(daemon.map(\.title), ["启动 Daemon", "关停 Daemon"], "英文小写子串命中 title")
eq(CommandFilter.filter(all, query: "DAEMON").map(\.title),
   daemon.map(\.title), "大小写不敏感")

eq(CommandFilter.filter(all, query: "t-002").map(\.title),
   ["Review PR #42"], "按 subtitle 中的任务 id 搜")
eq(CommandFilter.filter(all, query: "running").map(\.title),
   ["整理本周周报", "抓取竞品定价数据"], "按 subtitle 中的状态搜")
eq(CommandFilter.filter(all, query: "周报").map(\.title),
   ["整理本周周报"], "中文子串命中")

let taskHits = CommandFilter.filter(all, query: "任务").map(\.title)
check(taskHits.contains("新任务") && taskHits.contains("收件箱"),
      "「任务」同时命中 title(新任务) 与 subtitle(收件箱)")

check(CommandFilter.filter(all, query: "不存在的命令xyz").isEmpty, "无命中返回空数组")
eq(CommandFilter.filter(all, query: "设置").map(\.title),
   ["引擎设置", "设置"], "过滤后保持原始顺序")

// MARK: 2. ↑↓ 快捷键

print("【↑↓ 快捷键】")
eq(CommandFilter.move(index: 0, direction: .down, count: 5), 1, "↓ 正常下移")
eq(CommandFilter.move(index: 3, direction: .up, count: 5), 2, "↑ 正常上移")
eq(CommandFilter.move(index: 0, direction: .up, count: 5), 0, "到顶继续 ↑ 不越界")
eq(CommandFilter.move(index: 4, direction: .down, count: 5), 4, "到底继续 ↓ 不越界")
eq(CommandFilter.move(index: 0, direction: .down, count: 0), 0, "空列表 ↓ 恒为 0")
eq(CommandFilter.move(index: 0, direction: .up, count: 0), 0, "空列表 ↑ 恒为 0")
eq(CommandFilter.move(index: 0, direction: .down, count: 1), 0, "单条列表不越界")

// MARK: 3. 完整交互序列（模拟 CommandPaletteController 的真实状态机）

print("【完整交互序列】")

/// 模拟面板控制器中和搜索/选择相关的最小状态机（与 AppKitUI 中逻辑一致）
final class MockPalette {
    private let source: [FilterableCommand]
    private(set) var items: [FilterableCommand]
    private(set) var selectedIndex = 0
    /// 记录“回车执行”了哪条
    private(set) var executed: [String] = []

    init(source: [FilterableCommand]) {
        self.source = source
        self.items = CommandFilter.filter(source, query: "")
    }

    /// 输入/改变搜索词 → 与产品一致：重置选中到第 0 条
    func type(_ query: String) {
        items = CommandFilter.filter(source, query: query)
        selectedIndex = 0
    }

    func arrow(_ dir: MoveDirection) {
        selectedIndex = CommandFilter.move(index: selectedIndex, direction: dir, count: items.count)
    }

    @discardableResult
    func enter() -> String? {
        guard selectedIndex >= 0 && selectedIndex < items.count else { return nil }
        let title = items[selectedIndex].title
        executed.append(title)
        return title
    }
}

let palette = MockPalette(source: all)

// 打开 → 第 0 条是「新任务」
eq(palette.items.count, 13, "打开面板有 13 条候选")
eq(palette.items[palette.selectedIndex].title, "新任务", "默认选中第 0 条")

// 输入 daemon → 2 条，选中重置到「启动 Daemon」
palette.type("daemon")
eq(palette.items.count, 2, "输入后候选收缩为 2 条")
eq(palette.items[palette.selectedIndex].title, "启动 Daemon", "输入后选中重置到第 0 条")

// ↓ 到「关停 Daemon」，再 ↓ 到底不越界，再 ↑ 回来
palette.arrow(.down)
eq(palette.items[palette.selectedIndex].title, "关停 Daemon", "↓ 移到第 1 条")
palette.arrow(.down)
eq(palette.selectedIndex, 1, "到底后继续 ↓ 不越界")
palette.arrow(.up)
eq(palette.items[palette.selectedIndex].title, "启动 Daemon", "↑ 回到第 0 条")

// 回车 → 执行当前选中
let ran = palette.enter()
eq(ran, "启动 Daemon", "回车执行选中的「启动 Daemon」")
eq(palette.executed, ["启动 Daemon"], "仅执行一次")

// 无结果时回车不执行任何命令
palette.type("zzzz-no-match")
check(palette.items.isEmpty, "无结果候选为空")
let ranEmpty = palette.enter()
check(ranEmpty == nil, "空候选回车不执行（对应 runSelected 的 guard）")

// 急停命令检索：中文精确缩到唯一候选
palette.type("急停")
eq(palette.items.map(\.title), ["全局急停"], "「急停」唯一命中全局急停")
eq(palette.enter(), "全局急停", "回车执行全局急停")

// MARK: 结果汇总

print("")
if failed == 0 {
    print("PASS — 全部 \(passed) 项通过")
    exit(0)
} else {
    print("FAIL — \(failed) 项失败，\(passed) 项通过")
    exit(1)
}
