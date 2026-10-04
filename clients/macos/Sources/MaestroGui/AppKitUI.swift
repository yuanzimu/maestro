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
