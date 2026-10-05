// AppKitUI.swift — 纯 AppKit 界面（不依赖 SwiftUI 宏 —— Command Line Tools 可直接构建）
import AppKit
import MaestroCore

// MARK: - 状态徽章

func stateMeta(_ s: String) -> (text: String, color: NSColor) {
    switch s {
    case "queued":    return ("⏳ 排队中", .systemGray)
    case "planning":  return ("🧭 规划中", .systemPurple)
    case "working":   return ("🔨 干活中", .systemBlue)
    case "blocked":   return ("❗ 等你决定", .systemOrange)
    case "suspended": return ("💤 挂起", .systemYellow)
    case "done":      return ("✅ 完成", .systemGreen)
    case "failed":    return ("❌ 失败", .systemRed)
    case "cancelled": return ("🚫 已取消", .systemGray)
    default:          return ("❓ \(s)", .systemGray)
    }
}

func suspendReasonText(_ r: String) -> String {
    switch r {
    case "user_pause": return "手动暂停"
    case "emergency_stop": return "全局急停"
    case "network_lost": return "断网（自动恢复）"
    case "provider_outage": return "供应商故障（自动恢复）"
    case "budget_exceeded": return "预算耗尽"
    case "system_sleep": return "系统睡眠（自动恢复）"
    case "daemon_crash": return "daemon 崩溃"
    default: return r
    }
}

func blockedKindText(_ k: String) -> String {
    switch k {
    case "permission": return "等权限批准"
    case "plan_approval": return "方案待审"
    case "goal_stalled": return "无进展 3 轮"
    case "acceptance_failed": return "验收 3 次失败"
    case "infra": return "基建故障"
    case "rounds_exhausted": return "轮数耗尽"
    default: return k
    }
}

// MARK: - 小控件

final class CircleView: NSView {
    var color: NSColor = .systemRed
    override func draw(_ dirtyRect: NSRect) {
        color.setFill()
        NSBezierPath(ovalIn: bounds.insetBy(dx: 1.5, dy: 1.5)).fill()
    }
}

final class TextFieldCell: NSTableCellView {
    let label = NSTextField(labelWithString: "")
    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        label.translatesAutoresizingMaskIntoConstraints = false
        label.lineBreakMode = .byTruncatingTail
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -4),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }
}

// MARK: - 表格控制器

final class TasksTableController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    weak var state: AppState?
    var onSelect: ((String?) -> Void)?

    func numberOfRows(in tableView: NSTableView) -> Int { state?.tasks.count ?? 0 }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let task = state?.tasks[safe: row] else { return nil }
        let id = tableColumn?.identifier.rawValue ?? ""
        let cell = (tableView.makeView(withIdentifier: NSUserInterfaceItemIdentifier(id), owner: nil) as? TextFieldCell)
            ?? TextFieldCell()
        cell.identifier = NSUserInterfaceItemIdentifier(id)

        switch id {
        case "state":
            let meta = stateMeta(task.state)
            cell.label.attributedStringValue = NSAttributedString(string: meta.text, attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .semibold),
                .foregroundColor: meta.color,
            ])
        case "main":
            let title = NSAttributedString(string: task.title, attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: .medium),
                .foregroundColor: NSColor.labelColor,
            ])
            var parts = [title]
            if let n = task.narrative, !n.isEmpty {
                parts.append(NSAttributedString(string: "\n" + n, attributes: [
                    .font: NSFont.systemFont(ofSize: 11),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]))
            }
            let text = NSMutableAttributedString()
            for p in parts { text.append(p) }
            cell.label.attributedStringValue = text
            cell.label.maximumNumberOfLines = 2
        case "round":
            cell.label.attributedStringValue = NSAttributedString(string: "R\(task.round)", attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .medium),
                .foregroundColor: NSColor.secondaryLabelColor,
            ])
        default:
            break
        }
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard let tv = notification.object as? NSTableView else { return }
        let idx = tv.selectedRow
        onSelect?(idx >= 0 ? state?.tasks[safe: idx]?.id : nil)
    }
}

final class InboxTableController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    weak var state: AppState?
    var onSelect: ((String?) -> Void)?

    func numberOfRows(in tableView: NSTableView) -> Int { state?.inbox.count ?? 0 }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let item = state?.inbox[safe: row] else { return nil }
        let id = tableColumn?.identifier.rawValue ?? ""
        let cell = (tableView.makeView(withIdentifier: NSUserInterfaceItemIdentifier(id), owner: nil) as? TextFieldCell)
            ?? TextFieldCell()
        cell.identifier = NSUserInterfaceItemIdentifier(id)
        switch id {
        case "title":
            cell.label.attributedStringValue = NSAttributedString(string: item.title, attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: .medium),
            ])
        case "kind":
            cell.label.attributedStringValue = NSAttributedString(string: blockedKindText(item.kind ?? ""), attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .medium),
                .foregroundColor: NSColor.systemOrange,
            ])
        default: break
        }
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard let tv = notification.object as? NSTableView else { return }
        let idx = tv.selectedRow
        onSelect?(idx >= 0 ? state?.inbox[safe: idx]?.task : nil)
    }
}

final class EventsTableController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    weak var state: AppState?
    private static let fmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f
    }()

    func numberOfRows(in tableView: NSTableView) -> Int { state?.events.count ?? 0 }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let ev = state?.events[safe: row] else { return nil }
        let id = tableColumn?.identifier.rawValue ?? ""
        let cell = (tableView.makeView(withIdentifier: NSUserInterfaceItemIdentifier(id), owner: nil) as? TextFieldCell)
            ?? TextFieldCell()
        cell.identifier = NSUserInterfaceItemIdentifier(id)
        switch id {
        case "time":
            let date = Date(timeIntervalSince1970: Double(ev.ts) / 1000)
            cell.label.attributedStringValue = NSAttributedString(string: Self.fmt.string(from: date), attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 10, weight: .regular),
                .foregroundColor: NSColor.secondaryLabelColor,
            ])
        case "prio":
            let color: NSColor = ev.priority == "critical" ? .systemRed : (ev.priority == "warning" ? .systemOrange : .systemGray)
            cell.label.attributedStringValue = NSAttributedString(string: "●", attributes: [
                .font: NSFont.systemFont(ofSize: 10),
                .foregroundColor: color,
            ])
        case "type":
            cell.label.attributedStringValue = NSAttributedString(string: ev.typeText, attributes: [
                .font: NSFont.systemFont(ofSize: 11, weight: .medium),
            ])
        case "task":
            if let t = ev.taskID {
                cell.label.attributedStringValue = NSAttributedString(string: String(t.prefix(14)), attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 9, weight: .regular),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ])
            }
        case "detail":
            cell.label.attributedStringValue = NSAttributedString(string: ev.detailText, attributes: [
                .font: NSFont.systemFont(ofSize: 11),
                .foregroundColor: NSColor.secondaryLabelColor,
            ])
        default: break
        }
        return cell
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

extension NSView {
    /// 递归按 identifier 查找子视图
    func viewWith(identifier id: String) -> NSView? {
        if identifier?.rawValue == id { return self }
        for sub in subviews {
            if let v = sub.viewWith(identifier: id) { return v }
        }
        return nil
    }
}

// MARK: - 活动栏（B3-1，IA 对齐 B0-2）

/// 活动栏单个按钮：SF Symbol + 选中底色 + 角标
final class ActivityBarButton: NSButton {
    var isSelected = false {
        didSet { needsDisplay = true }
    }
    var badgeCount = 0 {
        didSet { needsDisplay = true }
    }

    init(symbol: String, tooltip: String, tag: Int) {
        super.init(frame: NSRect(x: 0, y: 0, width: 44, height: 44))
        self.tag = tag
        self.toolTip = tooltip
        isBordered = false
        image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip)
        image?.isTemplate = true
        contentTintColor = .labelColor
        symbolConfig = .init(pointSize: 17, weight: .regular)
        title = ""
        target = nil // 由 ActivityBar 的 stack/手势处理；这里用 sendAction
    }
    var symbolConfig: NSImage.SymbolConfiguration! {
        didSet {
            if let c = symbolConfig, let im = image { image = im.withSymbolConfiguration(c) }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        // 选中圆角底
        if isSelected {
            let r = bounds.insetBy(dx: 5, dy: 5)
            NSColor.selectedContentBackgroundColor.withAlphaComponent(0.18).setFill()
            NSBezierPath(roundedRect: r, xRadius: 8, yRadius: 8).fill()
        }
        super.draw(dirtyRect)
        // 角标（红点 + 数字）
        if badgeCount > 0 {
            let txt = badgeCount > 99 ? "99+" : String(badgeCount)
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 9, weight: .bold),
                .foregroundColor: NSColor.white,
            ]
            let pad: CGFloat = 4
            let w = txt.size(withAttributes: attrs).width + pad * 2
            let dot = NSRect(x: bounds.midX + 5, y: bounds.midY - 18, width: w, height: 15)
            NSColor.systemRed.setFill()
            NSBezierPath(roundedRect: dot, xRadius: 7.5, yRadius: 7.5).fill()
            let s = NSMutableParagraphStyle()
            s.alignment = .center
            var a = attrs
            a[.paragraphStyle] = s
            txt.draw(in: dot.offsetBy(dx: 0, dy: 2), withAttributes: a)
        }
    }
}

/// 左侧活动栏：任务 / 收件箱 / 事件 / 设置
final class ActivityBar: NSVisualEffectView {
    var onSelect: ((Int) -> Void)?
    private var buttons: [ActivityBarButton] = []

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        blendingMode = .withinWindow
        material = .sidebar
        state = .active

        let items: [(symbol: String, name: String)] = [
            ("list.bullet", "任务"),
            ("tray.full", "收件箱"),
            ("waveform", "事件"),
            ("gearshape", "设置"),
        ]
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 4
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
        ])

        for (i, it) in items.enumerated() {
            let b = ActivityBarButton(symbol: it.symbol, tooltip: it.name, tag: i)
            b.target = self
            b.action = #selector(clicked(_:))
            buttons.append(b)
            stack.addArrangedSubview(b)
            b.widthAnchor.constraint(equalToConstant: 44).isActive = true
            b.heightAnchor.constraint(equalToConstant: 44).isActive = true
        }
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .vertical)
        stack.addArrangedSubview(gap)
    }

    required init?(coder: NSCoder) { fatalError() }

    @objc private func clicked(_ sender: ActivityBarButton) {
        onSelect?(sender.tag)
    }

    /// 选中第几个（只更新视觉，不触发 onSelect）
    func select(_ index: Int) {
        for (i, b) in buttons.enumerated() {
            b.isSelected = i == index
        }
    }

    /// 设置某图标的角标数
    func badge(index: Int, count: Int) {
        guard index < buttons.count else { return }
        buttons[index].badgeCount = count
    }
}

// MARK: - 命令面板（B3-5，对齐 B1-2）

/// 命令面板条目
struct CommandItem: CommandMatchable {
    let title: String
    let subtitle: String
    let action: () -> Void
}

final class CommandPaletteController: NSObject, NSWindowDelegate, NSSearchFieldDelegate {
    private var panel: NSWindow!
    private var searchField: NSSearchField!
    private var tableView: NSTableView!
    private var items: [CommandItem] = []
    /// 命令提供者：按 query 返回候选（固定命令 + 匹配任务）
    private let provider: (String) -> [CommandItem]
    private var selectedIndex = 0
    /// 关闭回调（AppDelegate 凭它释放强引用；只触发一次）
    private let onClose: () -> Void
    private var didClose = false

    init(onClose: @escaping () -> Void,
         provider: @escaping (String) -> [CommandItem]) {
        self.onClose = onClose
        self.provider = provider
        super.init()
    }

    func show() {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 580, height: 380),
                         styleMask: [.titled, .fullSizeContentView],
                         backing: .buffered, defer: false)
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.isMovableByWindowBackground = true
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.delegate = self
        panel.isReleasedWhenClosed = false
        panel.backgroundColor = .windowBackgroundColor

        let content = NSView()
        panel.contentView = content

        searchField = NSSearchField()
        searchField.placeholderString = "输入命令或搜索任务…"
        searchField.translatesAutoresizingMaskIntoConstraints = false
        searchField.delegate = self
        searchField.bezelStyle = .roundedBezel
        content.addSubview(searchField)

        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.borderType = .noBorder
        content.addSubview(scroll)

        tableView = NSTableView()
        tableView.headerView = nil
        tableView.style = .plain
        tableView.rowHeight = 40
        tableView.dataSource = self
        tableView.delegate = self
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("c"))
        col.resizingMask = .autoresizingMask
        tableView.addTableColumn(col)
        tableView.target = self
        tableView.doubleAction = #selector(tableDoubleClicked)
        tableView.action = #selector(tableSingleClicked)
        scroll.documentView = tableView

        NSLayoutConstraint.activate([
            searchField.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            searchField.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            searchField.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
            scroll.topAnchor.constraint(equalTo: searchField.bottomAnchor, constant: 10),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])

        updateItems(keepSelection: false)

        // 居中偏上
        if let scr = NSScreen.main {
            let vf = scr.visibleFrame
            panel.center()
            panel.setFrameOrigin(NSPoint(x: vf.midX - panel.frame.width / 2,
                                        y: vf.midY + vf.height * 0.12))
        }
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func updateItems(keepSelection: Bool) {
        let q = searchField.stringValue.trimmingCharacters(in: .whitespaces)
        items = provider(q)
        AppLog.ui("⌘K 搜索「\(q)」→ \(items.count) 条候选")
        selectedIndex = keepSelection ? min(selectedIndex, max(0, items.count - 1)) : 0
        tableView.reloadData()
        if !items.isEmpty {
            tableView.selectRowIndexes(IndexSet(integer: selectedIndex), byExtendingSelection: false)
            tableView.scrollRowToVisible(selectedIndex)
        }
    }

    private func runSelected() {
        guard selectedIndex >= 0 && selectedIndex < items.count else {
            AppLog.ui("⌘K 回车但无有效选中（index=\(selectedIndex)）")
            return
        }
        let item = items[selectedIndex]
        AppLog.ui("⌘K 执行命令「\(item.title)」")
        close()
        item.action()
    }

    /// 重新聚焦已打开的面板（光标放回搜索框）
    func focus() {
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        panel.makeFirstResponder(searchField)
    }

    private func close() {
        panel.close()
        guard !didClose else { return }
        didClose = true
        onClose()
    }

    // MARK: 文本变化

    func controlTextDidChange(_ obj: Notification) {
        updateItems(keepSelection: false)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        switch commandSelector {
        case #selector(NSResponder.moveUp(_:)):
            selectedIndex = CommandFilter.move(index: selectedIndex, direction: .up, count: items.count)
            AppLog.ui("⌘K ↑ 选中第 \(selectedIndex) 条")
            tableView.selectRowIndexes(IndexSet(integer: selectedIndex), byExtendingSelection: false)
            tableView.scrollRowToVisible(selectedIndex)
            return true
        case #selector(NSResponder.moveDown(_:)):
            selectedIndex = CommandFilter.move(index: selectedIndex, direction: .down, count: items.count)
            AppLog.ui("⌘K ↓ 选中第 \(selectedIndex) 条")
            tableView.selectRowIndexes(IndexSet(integer: selectedIndex), byExtendingSelection: false)
            tableView.scrollRowToVisible(selectedIndex)
            return true
        case #selector(NSResponder.insertNewline(_:)):
            runSelected()
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            AppLog.ui("⌘K Esc 关闭")
            close()
            return true
        default:
            return false
        }
    }

    func windowDidResignKey(_ notification: Notification) {
        AppLog.ui("⌘K 面板失焦，自动关闭")
        close()
    }
}

extension CommandPaletteController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let item = items[row]
        let cell = NSTableCellView()
        let title = NSTextField(labelWithString: item.title)
        title.font = .systemFont(ofSize: 13, weight: .medium)
        title.lineBreakMode = .byTruncatingTail
        title.translatesAutoresizingMaskIntoConstraints = false
        let sub = NSTextField(labelWithString: item.subtitle)
        sub.font = .systemFont(ofSize: 10)
        sub.textColor = .secondaryLabelColor
        sub.lineBreakMode = .byTruncatingTail
        sub.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(title)
        cell.addSubview(sub)
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
            title.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -10),
            title.topAnchor.constraint(equalTo: cell.topAnchor, constant: 6),
            sub.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            sub.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            sub.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 1),
        ])
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        selectedIndex = row
        return true
    }

    @objc private func tableDoubleClicked() {
        if tableView.clickedRow >= 0 {
            selectedIndex = tableView.clickedRow
            runSelected()
        }
    }

    @objc private func tableSingleClicked() {
        if tableView.clickedRow >= 0 { selectedIndex = tableView.clickedRow }
    }
}

// MARK: - 详情文本

func detailAttributedString(for task: TaskSummary?, events: [MaestroEvent]) -> NSAttributedString {
    let text = NSMutableAttributedString()
    func append(_ s: String, _ attrs: [NSAttributedString.Key: Any] = [:]) {
        text.append(NSAttributedString(string: s, attributes: attrs))
    }
    let body: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.labelColor]
    let secondary: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.secondaryLabelColor]
    let mono: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular), .foregroundColor: NSColor.secondaryLabelColor]

    guard let task else {
        append("选择左侧任务查看详情", secondary)
        return text
    }
    append(task.title + "\n", [.font: NSFont.systemFont(ofSize: 15, weight: .semibold)])
    let meta = stateMeta(task.state)
    append(meta.text, [.font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: meta.color])
    append("  ·  第 \(task.round) 轮\n\n", secondary)
    if let n = task.narrative, !n.isEmpty {
        append(n + "\n\n", [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.labelColor])
    }
    append("任务 ID  ", secondary)
    append(task.id + "\n", mono)
    if let r = task.suspendReason {
        append("挂起原因  ", secondary)
        append(suspendReasonText(r) + "\n", body)
    }
    if let k = task.blockedKind {
        append("等待项  ", secondary)
        append(blockedKindText(k) + "\n", [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.systemOrange])
    }
    if task.acceptanceFailures > 0 {
        append("验收失败  ", secondary)
        append("\(task.acceptanceFailures) 次\n", body)
    }

    let taskEvents = events.filter { $0.taskID == task.id }.prefix(25)
    if !taskEvents.isEmpty {
        append("\n最近事件\n", [.font: NSFont.systemFont(ofSize: 12, weight: .semibold)])
        let fmt = EventsTableController.fmtStatic
        for ev in taskEvents {
            let date = Date(timeIntervalSince1970: Double(ev.ts) / 1000)
            append(fmt.string(from: date) + "  ", mono)
            append(ev.typeText + (ev.detailText.isEmpty ? "" : "  " + ev.detailText) + "\n",
                   [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.secondaryLabelColor])
        }
    }
    return text
}

// MARK: - 新任务面板

final class NewTaskPanelController: NSObject, NSWindowDelegate {
    var onCreate: ((String, String, String) -> Void)?

    private var panel: NSWindow!
    private var titleField: NSTextField!
    private var promptView: NSTextView!
    private var workdirField: NSTextField!
    private let state: AppState

    init(state: AppState) {
        self.state = state
    }

    func show() {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 460),
                         styleMask: [.titled, .closable],
                         backing: .buffered, defer: false)
        panel.title = "派个新任务"
        panel.delegate = self
        panel.isReleasedWhenClosed = false

        let content = NSView()
        panel.contentView = content

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 20),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: content.bottomAnchor, constant: -16),
        ])

        func caption(_ s: String) -> NSTextField {
            let l = NSTextField(labelWithString: s)
            l.font = NSFont.systemFont(ofSize: 11)
            l.textColor = .secondaryLabelColor
            return l
        }

        stack.addArrangedSubview(caption("标题（如：修复登录页样式）"))
        titleField = NSTextField()
        titleField.placeholderString = "给任务起个名字"
        stack.addArrangedSubview(titleField)
        NSLayoutConstraint.activate([
            titleField.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])

        stack.addArrangedSubview(caption("提示词（想让 AI 干什么）"))
        promptView = NSTextView()
        promptView.font = NSFont.systemFont(ofSize: 12)
        promptView.isRichText = false
        let scroll = NSScrollView()
        scroll.documentView = promptView
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 150),
        ])

        stack.addArrangedSubview(caption("工作目录（项目路径）"))
        let dirRow = NSStackView()
        dirRow.orientation = .horizontal
        dirRow.spacing = 8
        workdirField = NSTextField()
        workdirField.placeholderString = "/path/to/project"
        workdirField.stringValue = UserDefaults.standard.string(forKey: "maestro.lastWorkdir") ?? NSHomeDirectory()
        workdirField.translatesAutoresizingMaskIntoConstraints = false
        dirRow.addArrangedSubview(workdirField)
        let browseBtn = NSButton(title: "浏览…", target: self, action: #selector(browse))
        dirRow.addArrangedSubview(browseBtn)
        stack.addArrangedSubview(dirRow)
        NSLayoutConstraint.activate([
            dirRow.widthAnchor.constraint(equalTo: stack.widthAnchor),
            workdirField.widthAnchor.constraint(equalTo: dirRow.widthAnchor, constant: -80),
        ])

        if let cli = state.daemon.detectedCLI {
            let hint = NSTextField(labelWithString: "将由 \(cli.displayName) 执行")
            hint.font = NSFont.systemFont(ofSize: 11)
            hint.textColor = .secondaryLabelColor
            stack.addArrangedSubview(hint)
        }

        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 10
        let cancel = NSButton(title: "取消", target: self, action: #selector(cancel))
        cancel.keyEquivalent = "\u{1b}"
        buttons.addArrangedSubview(cancel)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.init(1), for: .horizontal)
        buttons.addArrangedSubview(spacer)
        let create = NSButton(title: "创建并派发", target: self, action: #selector(create))
        create.keyEquivalent = "\r"
        create.bezelStyle = .rounded
        create.controlSize = .large
        buttons.addArrangedSubview(create)
        buttons.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(buttons)
        NSLayoutConstraint.activate([
            buttons.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])

        panel.center()
        NSApp.runModal(for: panel)
    }

    @objc private func browse() {
        let open = NSOpenPanel()
        open.canChooseDirectories = true
        open.canChooseFiles = false
        open.canCreateDirectories = true
        if open.runModal() == .OK, let url = open.url {
            workdirField.stringValue = url.path
        }
    }

    @objc private func cancel() {
        panel.close()
        NSApp.stopModal()
    }

    @objc private func create() {
        let title = titleField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let prompt = promptView.string.trimmingCharacters(in: .whitespacesAndNewlines)
        let dir = workdirField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, !prompt.isEmpty else { NSSound.beep(); return }
        UserDefaults.standard.set(dir, forKey: "maestro.lastWorkdir")
        panel.close()
        NSApp.stopModal()
        onCreate?(title, prompt, dir)
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.stopModal()
    }
}

extension EventsTableController {
    static let fmtStatic: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f
    }()
}

// MARK: - 引擎设置窗口

final class EngineSettingsPanelController: NSObject, NSWindowDelegate {
    private let appState: AppState
    private var panel: NSWindow!
    private var gatewaySwitch: NSButton!
    private var urlField: NSTextField!
    private var tokenField: NSTextField!
    private var costField: NSTextField!
    private var wallField: NSTextField!
    private var hintLabel: NSTextField!

    init(state: AppState) { self.appState = state }

    func show() {
        let s = appState.engineSettings
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 480, height: 430),
                         styleMask: [.titled, .closable],
                         backing: .buffered, defer: false)
        panel.title = "引擎设置"
        panel.delegate = self
        panel.isReleasedWhenClosed = false

        let content = NSView()
        panel.contentView = content
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 9
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor, constant: 18),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -20),
        ])

        func caption(_ t: String) -> NSTextField {
            let l = NSTextField(labelWithString: t)
            l.font = .systemFont(ofSize: 11)
            l.textColor = .secondaryLabelColor
            return l
        }
        func sectionTitle(_ t: String) {
            let l = NSTextField(labelWithString: t)
            l.font = .systemFont(ofSize: 12, weight: .semibold)
            stack.addArrangedSubview(l)
        }

        // ---- 网关 ----
        sectionTitle("模型网关（CCR）")
        gatewaySwitch = NSButton(checkboxWithTitle: "经 CCR 路由（分级路由 / key 池轮换 / provider fallback）",
                                 target: self, action: #selector(gatewayToggled))
        gatewaySwitch.state = s.gatewayEnabled ? .on : .off
        stack.addArrangedSubview(gatewaySwitch)

        stack.addArrangedSubview(caption("网关地址"))
        urlField = NSTextField(string: s.gatewayURL)
        urlField.placeholderString = "http://127.0.0.1:3456"
        stack.addArrangedSubview(urlField)
        urlField.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        stack.addArrangedSubview(caption("鉴权令牌（本地网关用）"))
        tokenField = NSTextField(string: s.gatewayToken)
        stack.addArrangedSubview(tokenField)
        tokenField.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        // ---- 硬预算 ----
        sectionTitle("硬预算（超限立即冻结 worker、挂起等你决定）")
        let costRow = NSStackView()
        costRow.orientation = .horizontal
        costRow.spacing = 6
        costField = NSTextField(string: String(s.maxCostCents))
        costField.alignment = .right
        costField.widthAnchor.constraint(equalToConstant: 90).isActive = true
        costRow.addArrangedSubview(costField)
        costRow.addArrangedSubview(NSTextField(labelWithString: "美分（0 = 不限花费）"))
        let g1 = NSView(); g1.setContentHuggingPriority(.init(1), for: .horizontal)
        costRow.addArrangedSubview(g1)
        stack.addArrangedSubview(costRow)
        costRow.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        let wallRow = NSStackView()
        wallRow.orientation = .horizontal
        wallRow.spacing = 6
        wallField = NSTextField(string: String(s.maxWallMinutes))
        wallField.alignment = .right
        wallField.widthAnchor.constraint(equalToConstant: 90).isActive = true
        wallRow.addArrangedSubview(wallField)
        wallRow.addArrangedSubview(NSTextField(labelWithString: "分钟（0 = 不限时长）"))
        let g2 = NSView(); g2.setContentHuggingPriority(.init(1), for: .horizontal)
        wallRow.addArrangedSubview(g2)
        stack.addArrangedSubview(wallRow)
        wallRow.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        hintLabel = NSTextField(labelWithString: "重启引擎后新设置生效（急停/关停 → 重新启动）。")
        hintLabel.font = .systemFont(ofSize: 10)
        hintLabel.textColor = .tertiaryLabelColor
        stack.addArrangedSubview(hintLabel)

        // ---- 按钮 ----
        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 10
        let cancel = NSButton(title: "取消", target: self, action: #selector(cancel))
        buttons.addArrangedSubview(cancel)
        let gap = NSView(); gap.setContentHuggingPriority(.init(1), for: .horizontal)
        buttons.addArrangedSubview(gap)
        let save = NSButton(title: "保存", target: self, action: #selector(save))
        save.bezelStyle = .rounded
        save.keyEquivalent = "\r"
        buttons.addArrangedSubview(save)
        buttons.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(buttons)
        buttons.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        gatewayToggled()
        panel.center()
        NSApp.runModal(for: panel)
    }

    @objc private func gatewayToggled() {
        let on = gatewaySwitch.state == .on
        urlField.isEnabled = on
        tokenField.isEnabled = on
    }

    @objc private func cancel() {
        panel.close(); NSApp.stopModal()
    }

    @objc private func save() {
        var next = appState.engineSettings
        next.gatewayEnabled = gatewaySwitch.state == .on
        next.gatewayURL = urlField.stringValue.trimmingCharacters(in: .whitespaces)
        next.gatewayToken = tokenField.stringValue
        next.maxCostCents = Int(costField.stringValue) ?? 0
        next.maxWallMinutes = Int(wallField.stringValue) ?? 0
        // 校验：启用网关时 URL 不得空
        if next.gatewayEnabled && next.gatewayURL.isEmpty {
            hintLabel.textColor = .systemRed
            hintLabel.stringValue = "网关已启用，地址不能空。"
            return
        }
        if let err = appState.saveEngineSettings(next) {
            hintLabel.textColor = .systemRed
            hintLabel.stringValue = err
            return
        }
        panel.close(); NSApp.stopModal()
    }

    func windowWillClose(_ notification: Notification) { NSApp.stopModal() }
}
