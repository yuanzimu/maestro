// CheckpointTimelineController.swift — B10 Checkpoint 时光机窗口
//
// 独立窗口（非 run-modal），生命周期与命令面板一致：AppDelegate 强持有，
// Esc/失焦/关闭按钮触发 onClose 释放。
//
// 交互：
//   时间线（时间升序，最新在下）→ 选点 →「恢复到此点」
//   → 确认 sheet（pre-rollback 会自动留撤销点）
//   → 恢复后显示「撤销回滚」，点击回到 pre_rollback 安全垫
import AppKit
import MaestroCore

final class CheckpointTimelineController: NSObject, NSWindowDelegate {
    private var panel: NSWindow!
    private var tableView: NSTableView!
    private var titleLabel: NSTextField!
    private var btnRestore: NSButton!
    private var btnUndo: NSButton!
    private var hintLabel: NSTextField!

    private let state: AppState
    private let taskID: String
    private let taskTitle: String
    private let onClose: () -> Void

    private var checkpoints: [CheckpointInfo] = []
    private var didClose = false
    /// 最近一次回滚的安全垫（非 nil = 可撤销）
    private var lastPreRollback: String?

    init(state: AppState, taskID: String, taskTitle: String, onClose: @escaping () -> Void) {
        self.state = state
        self.taskID = taskID
        self.taskTitle = taskTitle
        self.onClose = onClose
        super.init()
    }

    func show() {
        panel = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 480, height: 520),
                         styleMask: [.titled, .fullSizeContentView],
                         backing: .buffered, defer: false)
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.isMovableByWindowBackground = true
        panel.delegate = self
        panel.isReleasedWhenClosed = false
        panel.backgroundColor = .windowBackgroundColor

        let content = NSView()
        panel.contentView = content

        titleLabel = NSTextField(labelWithString: "时光机 · \(taskTitle)")
        titleLabel.font = .systemFont(ofSize: 14, weight: .semibold)
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(titleLabel)

        let closeBtn = NSButton(title: "关闭", target: self, action: #selector(closeClicked))
        closeBtn.bezelStyle = .rounded
        closeBtn.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(closeBtn)

        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.borderType = .noBorder
        content.addSubview(scroll)

        tableView = NSTableView()
        tableView.headerView = nil
        tableView.style = .plain
        tableView.rowHeight = 52
        tableView.dataSource = self
        tableView.delegate = self
        tableView.target = self
        tableView.doubleAction = #selector(rowDoubleClicked)
        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("cp"))
        col.resizingMask = .autoresizingMask
        tableView.addTableColumn(col)
        scroll.documentView = tableView

        hintLabel = NSTextField(labelWithString: "选择一个时间点，恢复后此之后的改动会被回滚（可撤销）")
        hintLabel.font = .systemFont(ofSize: 10)
        hintLabel.textColor = .secondaryLabelColor
        hintLabel.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(hintLabel)

        let actionRow = NSStackView()
        actionRow.orientation = .horizontal
        actionRow.spacing = 8
        actionRow.translatesAutoresizingMaskIntoConstraints = false

        btnRestore = NSButton(title: "恢复到此点", target: self, action: #selector(restoreClicked))
        btnRestore.bezelStyle = .rounded
        btnRestore.keyEquivalent = "\r"
        btnUndo = NSButton(title: "撤销回滚", target: self, action: #selector(undoClicked))
        btnUndo.bezelStyle = .rounded
        btnUndo.isHidden = true

        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        actionRow.addArrangedSubview(btnUndo)
        actionRow.addArrangedSubview(gap)
        actionRow.addArrangedSubview(btnRestore)
        content.addSubview(actionRow)

        NSLayoutConstraint.activate([
            titleLabel.topAnchor.constraint(equalTo: content.topAnchor, constant: 14),
            titleLabel.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            closeBtn.centerYAnchor.constraint(equalTo: titleLabel.centerYAnchor),
            closeBtn.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
            scroll.topAnchor.constraint(equalTo: titleLabel.bottomAnchor, constant: 12),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: hintLabel.topAnchor, constant: -8),
            hintLabel.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            hintLabel.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            actionRow.topAnchor.constraint(equalTo: hintLabel.bottomAnchor, constant: 8),
            actionRow.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 16),
            actionRow.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            actionRow.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -14),
            actionRow.heightAnchor.constraint(equalToConstant: 30),
        ])

        centerWindow()
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        reload()
    }

    /// 聚焦已打开的时光机窗口
    func makeKey() {
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    // MARK: 数据

    private func reload(scrollToBottom: Bool = true) {
        state.loadCheckpoints(task: taskID) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let cps):
                self.checkpoints = cps
                self.tableView.reloadData()
                if scrollToBottom && !cps.isEmpty {
                    let last = cps.count - 1
                    self.tableView.selectRowIndexes(IndexSet(integer: last), byExtendingSelection: false)
                    self.tableView.scrollRowToVisible(last)
                }
            case .failure(let e):
                self.hintLabel.stringValue = "加载失败：\(e.message)"
            }
        }
    }

    // MARK: 恢复 / 撤销

    private var selectedCheckpoint: CheckpointInfo? {
        let r = tableView.selectedRow
        guard r >= 0 && r < checkpoints.count else { return nil }
        return checkpoints[r]
    }

    @objc private func restoreClicked() {
        guard let cp = selectedCheckpoint else {
            AppLog.ui("时光机 未选点，忽略恢复")
            return
        }
        confirmRollback(to: cp)
    }

    @objc private func rowDoubleClicked() {
        guard tableView.clickedRow >= 0 else { return }
        confirmRollback(to: checkpoints[tableView.clickedRow])
    }

    /// pre-rollback 确认 sheet
    private func confirmRollback(to cp: CheckpointInfo) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "恢复到「\(cp.reasonText)」？"
        alert.informativeText = "此时间点之后的改动会被回滚。系统会自动保留一个撤销点，可随时「撤销回滚」。"
        alert.addButton(withTitle: "恢复")
        alert.addButton(withTitle: "取消")
        alert.beginSheetModal(for: panel) { [weak self] resp in
            guard resp == .alertFirstButtonReturn else { return }
            self?.performRollback(to: cp)
        }
    }

    private func performRollback(to cp: CheckpointInfo) {
        state.rollbackCheckpoint(task: taskID, to: cp.ref) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let pre):
                AppLog.ui("时光机 已恢复到 \(cp.ref)，可撤销点 \(pre)")
                self.lastPreRollback = pre
                self.btnUndo.isHidden = pre.isEmpty
                self.hintLabel.stringValue = "已恢复到「\(cp.reasonText)」，可点「撤销回滚」还原"
                self.reload()
            case .failure(let e):
                AppLog.ui("时光机 恢复失败：\(e.message)")
                self.presentErrorInfo(e.message)
            }
        }
    }

    @objc private func undoClicked() {
        guard let pre = lastPreRollback, !pre.isEmpty else { return }
        state.rollbackCheckpoint(task: taskID, to: pre) { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                AppLog.ui("时光机 已撤销回滚，恢复原状")
                self.lastPreRollback = nil
                self.btnUndo.isHidden = true
                self.hintLabel.stringValue = "已撤销回滚，任务回到恢复前状态"
                self.reload()
            case .failure(let e):
                self.presentErrorInfo(e.message)
            }
        }
    }

    private func presentErrorInfo(_ m: String) {
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "操作失败"
        alert.informativeText = m
        alert.addButton(withTitle: "好")
        alert.beginSheetModal(for: panel)
    }

    // MARK: 关闭

    @objc private func closeClicked() { close() }

    private func close() {
        guard !didClose else { return }
        didClose = true
        let keepAlive = self
        panel.close()
        onClose()
        withExtendedLifetime(keepAlive) {}
    }

    func windowDidResignKey(_ notification: Notification) {
        // 有 sheet（确认框）时忽略失焦，避免确认流程误关窗口
        if panel.attachedSheet != nil { return }
        close()
    }

    func windowWillClose(_ notification: Notification) {
        close()
    }

    private func centerWindow() {
        if let scr = NSScreen.main {
            let vf = scr.visibleFrame
            panel.center()
            panel.setFrameOrigin(NSPoint(x: vf.midX - panel.frame.width / 2,
                                        y: vf.midY + vf.height * 0.10))
        }
    }
}

extension CheckpointTimelineController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { checkpoints.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cp = checkpoints[row]
        let cell = NSTableCellView()

        let icon = NSTextField(labelWithString: cp.reasonIcon)
        icon.font = .systemFont(ofSize: 16)
        icon.translatesAutoresizingMaskIntoConstraints = false

        let title = NSTextField(labelWithString: cp.reasonText)
        title.font = .systemFont(ofSize: 12, weight: .medium)
        title.lineBreakMode = .byTruncatingTail
        title.translatesAutoresizingMaskIntoConstraints = false

        var metaParts: [String] = []
        if let t = cp.timeString { metaParts.append(t) }
        if !cp.commit.isEmpty { metaParts.append(String(cp.commit.prefix(7))) }
        let sub = NSTextField(labelWithString: metaParts.joined(separator: " · "))
        sub.font = .monospacedSystemFont(ofSize: 9, weight: .regular)
        sub.textColor = .secondaryLabelColor
        sub.translatesAutoresizingMaskIntoConstraints = false

        cell.addSubview(icon)
        cell.addSubview(title)
        cell.addSubview(sub)
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 12),
            icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 24),
            title.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 6),
            title.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -10),
            title.topAnchor.constraint(equalTo: cell.topAnchor, constant: 8),
            sub.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            sub.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            sub.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 2),
        ])
        return cell
    }
}
