// main.swift — AppKit 引导：窗口构建 + 定时刷新（无需 Xcode 工程）
import AppKit
import MaestroCore

final class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow?
    let state = AppState()

    // 顶栏
    private var statusDot: CircleView!
    private var statusLabel: NSTextField!
    private var statusSub: NSTextField!
    private var activityBar: ActivityBar!
    private var btnEmergency: NSButton!
    private var gearMenu: NSPopUpButton!
    private var bottomInfo: NSTextField!

    // 内容
    private var tabContainers: [NSView] = []
    private var tasksTable: NSTableView!
    private var tasksController: TasksTableController!
    private var eventsTable: NSTableView!
    private var eventsController: EventsTableController!
    private var inboxTable: NSTableView!
    private var inboxController: InboxTableController!
    private var inboxActionLabel: NSTextField!
    private var btnInboxResume: NSButton!
    private var btnInboxCancel: NSButton!

    // 详情
    private var detailText: NSTextView!
    private var steerField: NSTextField!
    private var steerSendBtn: NSButton!
    private var btnPause: NSButton!
    private var btnResume: NSButton!
    private var btnCancel: NSButton!

    private var selectedTaskID: String?
    private var selectedInboxID: String?
    private var uiTimer: Timer?

    // 菜单栏常驻（B3-3）
    private var statusItem: NSStatusItem?
    private var miNewTask: NSMenuItem!
    private var miEmergency: NSMenuItem!
    private var miInbox: NSMenuItem!
    private var miShowWindow: NSMenuItem!

    // 命令面板：非 run-modal 独立窗口，必须由 AppDelegate 强持有，
    // 否则局部变量释放后 tableView 的 target（unsafe_unretained）变野指针 → 点击崩溃
    private var palette: CommandPaletteController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildWindow()
        buildMenu()
        buildStatusItem()
        state.onUpdate = { [weak self] in self?.reloadUI() }
        state.start()

        // 开箱即用：启动时 daemon 不在且检测到 AI CLI → 自动拉起（P1-A1）
        if !state.api.isDaemonAlive {
            state.daemon.locateBinaries()
            if state.daemon.daemonPath != nil && state.daemon.detectedCLI != nil {
                state.startDaemon()
            }
        }

        uiTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
            self?.reloadUI()
        }
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false // 关窗后经菜单栏 status item 常驻、可重开；daemon 也独立后台常驻
    }

    // MARK: - 窗口与布局

    private func buildWindow() {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 640),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable],
                          backing: .buffered, defer: false)
        window?.title = "Maestro — AI 任务指挥台"
        window?.minSize = NSSize(width: 860, height: 520)
        window?.isReleasedWhenClosed = false // 关窗保留对象，经 status item 重开（防野指针）
        guard let content = window?.contentView else { return }

        let topBar = NSStackView()
        topBar.orientation = .horizontal
        topBar.spacing = 12
        topBar.edgeInsets = NSEdgeInsets(top: 0, left: 12, bottom: 0, right: 12)
        topBar.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(topBar)

        let bottomBar = NSStackView()
        bottomBar.orientation = .horizontal
        bottomBar.spacing = 8
        bottomBar.edgeInsets = NSEdgeInsets(top: 4, left: 12, bottom: 4, right: 12)
        bottomBar.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(bottomBar)

        // 左侧活动栏（B3-1，IA 对齐 B0-2）
        let activityBar = ActivityBar()
        activityBar.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(activityBar)
        self.activityBar = activityBar

        let contentBox = NSView()
        contentBox.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(contentBox)

        NSLayoutConstraint.activate([
            topBar.topAnchor.constraint(equalTo: content.topAnchor, constant: 8),
            topBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            topBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            topBar.heightAnchor.constraint(equalToConstant: 46),
            // 活动栏：顶栏下、底栏上，贴左，固定宽
            activityBar.topAnchor.constraint(equalTo: topBar.bottomAnchor, constant: 4),
            activityBar.bottomAnchor.constraint(equalTo: bottomBar.topAnchor),
            activityBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            activityBar.widthAnchor.constraint(equalToConstant: 60),
            // 内容盒：活动栏右侧
            contentBox.topAnchor.constraint(equalTo: topBar.bottomAnchor, constant: 4),
            contentBox.bottomAnchor.constraint(equalTo: bottomBar.topAnchor),
            contentBox.leadingAnchor.constraint(equalTo: activityBar.trailingAnchor),
            contentBox.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            bottomBar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            bottomBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            bottomBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            bottomBar.heightAnchor.constraint(equalToConstant: 24),
        ])

        // ---- 顶栏左：状态 ----
        statusDot = CircleView()
        statusDot.translatesAutoresizingMaskIntoConstraints = false
        topBar.addArrangedSubview(statusDot)

        let statusStack = NSStackView()
        statusStack.orientation = .vertical
        statusStack.alignment = .leading
        statusStack.spacing = 0
        statusLabel = NSTextField(labelWithString: "Daemon 未连接")
        statusLabel.font = .systemFont(ofSize: 12, weight: .medium)
        statusSub = NSTextField(labelWithString: "")
        statusSub.font = .systemFont(ofSize: 10)
        statusSub.textColor = .secondaryLabelColor
        statusStack.addArrangedSubview(statusLabel)
        statusStack.addArrangedSubview(statusSub)
        topBar.addArrangedSubview(statusStack)
        NSLayoutConstraint.activate([
            statusDot.widthAnchor.constraint(equalToConstant: 11),
            statusDot.heightAnchor.constraint(equalToConstant: 11),
        ])

        let spacer1 = NSView()
        spacer1.setContentHuggingPriority(.init(1), for: .horizontal)
        topBar.addArrangedSubview(spacer1)

        // ---- 顶栏右：急停 / 新任务 / 引擎操作（导航已移交活动栏）----
        btnEmergency = NSButton(title: "🛑 急停", target: self, action: #selector(emergencyClicked))
        btnEmergency.bezelStyle = .rounded
        topBar.addArrangedSubview(btnEmergency)

        let btnNew = NSButton(title: "＋ 新任务", target: self, action: #selector(newTaskClicked))
        btnNew.bezelStyle = .rounded
        btnNew.keyEquivalent = "n"
        topBar.addArrangedSubview(btnNew)

        gearMenu = NSPopUpButton()
        gearMenu.addItems(withTitles: ["⚙", "引擎设置…", "启动 Daemon", "演示模式启动（无 AI CLI）", "关停 Daemon",
                                       "打开数据目录", "打开 daemon 日志"])
        gearMenu.selectItem(at: 0)
        gearMenu.item(at: 0)?.isEnabled = false
        gearMenu.target = self
        gearMenu.action = #selector(gearSelected)
        topBar.addArrangedSubview(gearMenu)

        // ---- 底栏 ----
        bottomInfo = NSTextField(labelWithString: "")
        bottomInfo.font = .systemFont(ofSize: 10)
        bottomInfo.textColor = .secondaryLabelColor
        bottomBar.addArrangedSubview(bottomInfo)

        // ---- 四个视图容器（任务/收件箱/事件/设置）----
        let tasksView = buildTasksView()
        let inboxView = buildInboxView()
        let eventsView = buildEventsView()
        let settingsView = buildSettingsView()
        tabContainers = [tasksView, inboxView, eventsView, settingsView]
        for v in tabContainers {
            v.translatesAutoresizingMaskIntoConstraints = false
            contentBox.addSubview(v)
            NSLayoutConstraint.activate([
                v.topAnchor.constraint(equalTo: contentBox.topAnchor),
                v.bottomAnchor.constraint(equalTo: contentBox.bottomAnchor),
                v.leadingAnchor.constraint(equalTo: contentBox.leadingAnchor),
                v.trailingAnchor.constraint(equalTo: contentBox.trailingAnchor),
            ])
        }
        tabContainers[1].isHidden = true
        tabContainers[2].isHidden = true
        tabContainers[3].isHidden = true

        // 活动栏导航回调
        activityBar.onSelect = { [weak self] idx in
            self?.switchTo(idx)
        }
        activityBar.select(0)
    }

    private func makeTable(_ columns: [(id: String, title: String, width: CGFloat)]) -> (NSTableView, NSScrollView) {
        let tv = NSTableView()
        tv.style = .fullWidth
        tv.rowHeight = 40
        tv.usesAlternatingRowBackgroundColors = true
        for c in columns {
            let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(c.id))
            col.title = c.title
            col.width = c.width
            col.resizingMask = .userResizingMask
            col.sortDescriptorPrototype = nil
            tv.addTableColumn(col)
        }
        let scroll = NSScrollView()
        scroll.documentView = tv
        scroll.hasVerticalScroller = true
        return (tv, scroll)
    }

    private func buildTasksView() -> NSView {
        tasksController = TasksTableController()
        tasksController.state = state
        tasksController.onSelect = { [weak self] id in
            self?.selectedTaskID = id
            self?.reloadUI()
        }
        let (tv, scroll) = makeTable([
            ("state", "状态", 110),
            ("main", "任务", 380),
            ("round", "轮次", 60),
        ])
        tasksTable = tv
        tasksTable.dataSource = tasksController
        tasksTable.delegate = tasksController
        tasksTable.rowHeight = 44

        // 详情面板
        let detail = NSView()
        detail.translatesAutoresizingMaskIntoConstraints = false

        let detailScroll = NSScrollView()
        detailScroll.translatesAutoresizingMaskIntoConstraints = false
        detailScroll.hasVerticalScroller = true
        detailText = NSTextView()
        detailText.isEditable = false
        detailText.isSelectable = true
        detailText.drawsBackground = false
        detailText.textContainer?.lineFragmentPadding = 4
        detailScroll.documentView = detailText
        detail.addSubview(detailScroll)

        let steerRow = NSStackView()
        steerRow.orientation = .horizontal
        steerRow.spacing = 8
        steerRow.translatesAutoresizingMaskIntoConstraints = false
        let steerLabel = NSTextField(labelWithString: "轻推：")
        steerLabel.font = .systemFont(ofSize: 12, weight: .medium)
        steerField = NSTextField()
        steerField.placeholderString = "补充指示，下一轮生效"
        steerField.translatesAutoresizingMaskIntoConstraints = false
        steerSendBtn = NSButton(title: "发送", target: self, action: #selector(steerSendClicked))
        steerRow.addArrangedSubview(steerLabel)
        steerRow.addArrangedSubview(steerField)
        steerRow.addArrangedSubview(steerSendBtn)
        detail.addSubview(steerRow)

        let actionRow = NSStackView()
        actionRow.orientation = .horizontal
        actionRow.spacing = 8
        actionRow.translatesAutoresizingMaskIntoConstraints = false
        btnPause = NSButton(title: "暂停", target: self, action: #selector(pauseClicked))
        btnResume = NSButton(title: "恢复", target: self, action: #selector(resumeClicked))
        btnCancel = NSButton(title: "取消任务", target: self, action: #selector(cancelClicked))
        actionRow.addArrangedSubview(btnPause)
        actionRow.addArrangedSubview(btnResume)
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        actionRow.addArrangedSubview(gap)
        actionRow.addArrangedSubview(btnCancel)
        detail.addSubview(actionRow)

        NSLayoutConstraint.activate([
            detailScroll.topAnchor.constraint(equalTo: detail.topAnchor, constant: 10),
            detailScroll.leadingAnchor.constraint(equalTo: detail.leadingAnchor, constant: 10),
            detailScroll.trailingAnchor.constraint(equalTo: detail.trailingAnchor, constant: -10),
            detailScroll.bottomAnchor.constraint(equalTo: steerRow.topAnchor, constant: -8),
            steerRow.leadingAnchor.constraint(equalTo: detail.leadingAnchor, constant: 12),
            steerRow.trailingAnchor.constraint(equalTo: detail.trailingAnchor, constant: -12),
            steerRow.heightAnchor.constraint(equalToConstant: 28),
            actionRow.topAnchor.constraint(equalTo: steerRow.bottomAnchor, constant: 8),
            actionRow.leadingAnchor.constraint(equalTo: detail.leadingAnchor, constant: 12),
            actionRow.trailingAnchor.constraint(equalTo: detail.trailingAnchor, constant: -12),
            actionRow.bottomAnchor.constraint(equalTo: detail.bottomAnchor, constant: -10),
            steerField.widthAnchor.constraint(greaterThanOrEqualToConstant: 160),
        ])

        let split = NSSplitView()
        split.translatesAutoresizingMaskIntoConstraints = false
        split.dividerStyle = .thin
        split.isVertical = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        detail.translatesAutoresizingMaskIntoConstraints = false
        split.addSubview(scroll)
        split.addSubview(detail)
        DispatchQueue.main.async { [weak split] in
            split?.setPosition(580, ofDividerAt: 0)
        }
        return split
    }

    private func buildInboxView() -> NSView {
        let box = NSView()
        inboxController = InboxTableController()
        inboxController.state = state
        inboxController.onSelect = { [weak self] id in
            self?.selectedInboxID = id
            self?.reloadUI()
        }
        let (itv, scroll) = makeTable([
            ("title", "任务", 420),
            ("kind", "等待", 200),
        ])
        inboxTable = itv
        inboxTable.dataSource = inboxController
        inboxTable.delegate = inboxController
        inboxTable.rowHeight = 36

        let actionRow = NSStackView()
        actionRow.orientation = .horizontal
        actionRow.spacing = 10
        actionRow.translatesAutoresizingMaskIntoConstraints = false
        inboxActionLabel = NSTextField(labelWithString: "AI 遇到需要你决策的事会出现在这里")
        inboxActionLabel.font = .systemFont(ofSize: 11)
        inboxActionLabel.textColor = .secondaryLabelColor
        btnInboxResume = NSButton(title: "继续这个任务", target: self, action: #selector(inboxResumeClicked))
        btnInboxCancel = NSButton(title: "取消这个任务", target: self, action: #selector(inboxCancelClicked))
        actionRow.addArrangedSubview(inboxActionLabel)
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        actionRow.addArrangedSubview(gap)
        actionRow.addArrangedSubview(btnInboxResume)
        actionRow.addArrangedSubview(btnInboxCancel)

        box.addSubview(scroll)
        box.addSubview(actionRow)
        scroll.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: box.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: box.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: box.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: actionRow.topAnchor),
            actionRow.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 12),
            actionRow.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -12),
            actionRow.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -8),
            actionRow.heightAnchor.constraint(equalToConstant: 30),
        ])
        return box
    }

    private func buildEventsView() -> NSView {
        eventsController = EventsTableController()
        eventsController.state = state
        let (etv, scroll) = makeTable([
            ("time", "时间", 72),
            ("prio", "●", 24),
            ("type", "事件", 120),
            ("task", "任务", 110),
            ("detail", "详情", 400),
        ])
        eventsTable = etv
        eventsTable.dataSource = eventsController
        eventsTable.delegate = eventsController
        eventsTable.rowHeight = 28
        return scroll
    }

    /// 设置主视图（活动栏第四图标；B0 IA）
    private func buildSettingsView() -> NSView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        let doc = NSView()
        scroll.documentView = doc
        doc.translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 24, right: 24)
        stack.translatesAutoresizingMaskIntoConstraints = false
        doc.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: doc.topAnchor),
            stack.leadingAnchor.constraint(equalTo: doc.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
        ])
        // 文档宽度跟随滚动视口（在加入父视图后由约束系统生效）
        stack.widthAnchor.constraint(equalTo: scroll.widthAnchor, multiplier: 1).isActive = true

        func title(_ t: String) {
            let l = NSTextField(labelWithString: t)
            l.font = .systemFont(ofSize: 15, weight: .semibold)
            stack.addArrangedSubview(l)
        }
        func row(_ items: [NSButton]) {
            let r = NSStackView(views: items)
            r.orientation = .horizontal
            r.spacing = 8
            stack.addArrangedSubview(r)
        }
        func infoLine(_ l: NSTextField) {
            l.font = .systemFont(ofSize: 12)
            l.textColor = .secondaryLabelColor
            stack.addArrangedSubview(l)
        }

        title("引擎")
        let engineState = NSTextField(labelWithString: "—")
        engineState.identifier = NSUserInterfaceItemIdentifier("settings-engine-state")
        infoLine(engineState)

        row([
            NSButton(title: "启动", target: self, action: #selector(settingsStartDaemon)),
            NSButton(title: "演示模式启动", target: self, action: #selector(settingsStartDemo)),
            NSButton(title: "关停", target: self, action: #selector(settingsStopDaemon)),
        ])

        title("引擎设置（网关 / 硬预算）")
        let summary = NSTextField(labelWithString: "—")
        summary.identifier = NSUserInterfaceItemIdentifier("settings-engine-summary")
        infoLine(summary)
        row([
            NSButton(title: "打开引擎设置…", target: self, action: #selector(settingsOpenEngine)),
        ])

        title("数据与日志")
        row([
            NSButton(title: "打开数据目录", target: self, action: #selector(settingsOpenData)),
            NSButton(title: "打开 daemon 日志", target: self, action: #selector(settingsOpenLog)),
        ])

        title("关于")
        infoLine(NSTextField(labelWithString: "Maestro —— AI 任务指挥台 · Apache-2.0"))
        infoLine(NSTextField(labelWithString: "关窗口不杀引擎，任务照跑；菜单栏图标可随时回来。"))

        return scroll
    }

    @objc private func settingsStartDaemon() { state.startDaemon() }
    @objc private func settingsStartDemo() { state.startDaemon(force: true) }
    @objc private func settingsStopDaemon() { state.shutdownDaemon() }
    @objc private func settingsOpenEngine() {
        EngineSettingsPanelController(state: state).show()
    }
    @objc private func settingsOpenData() { state.openDataDir() }
    @objc private func settingsOpenLog() { state.openDaemonLog() }

    private func buildMenu() {
        let mainMenu = NSMenu()

        // App 菜单
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "关于 Maestro",
                        action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "退出 Maestro (⌘Q)",
                        action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        // 文件菜单（⌘N 新任务 / ⌘K 命令面板）
        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: "文件")
        let miNew = NSMenuItem(title: "新任务…", action: #selector(newTaskClicked), keyEquivalent: "n")
        fileMenu.addItem(miNew)
        let miPalette = NSMenuItem(title: "命令面板", action: #selector(commandPaletteClicked), keyEquivalent: "k")
        fileMenu.addItem(miPalette)
        fileMenu.addItem(.separator())
        fileMenu.addItem(withTitle: "关闭窗口", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        fileItem.submenu = fileMenu
        mainMenu.addItem(fileItem)

        // 导航菜单（活动栏四视图）
        let navItem = NSMenuItem()
        let navMenu = NSMenu(title: "导航")
        navMenu.addItem(withTitle: "任务", action: #selector(goTasks), keyEquivalent: "1")
        navMenu.addItem(withTitle: "收件箱", action: #selector(goInbox), keyEquivalent: "2")
        navMenu.addItem(withTitle: "事件", action: #selector(goEvents), keyEquivalent: "3")
        navMenu.addItem(withTitle: "设置", action: #selector(goSettings), keyEquivalent: "4")
        navItem.submenu = navMenu
        mainMenu.addItem(navItem)

        // 引擎菜单（急停/恢复 + 引擎设置）
        let engineItem = NSMenuItem()
        let engineMenu = NSMenu(title: "引擎")
        // ⌘. 是 macOS 系统保留（取消/Esc 等价键）→ 急停改用 ⌘⇧E
        let miStop = NSMenuItem(title: "急停 / 恢复全部", action: #selector(emergencyClicked), keyEquivalent: "e")
        miStop.keyEquivalentModifierMask = [.command, .shift]
        engineMenu.addItem(miStop)
        engineMenu.addItem(withTitle: "引擎设置…", action: #selector(goEngineSettings), keyEquivalent: ",")
        engineItem.submenu = engineMenu
        mainMenu.addItem(engineItem)

        // 自定义 action 项显式绑到 AppDelegate（否则沿 responder chain 找不到而置灰）
        for m in fileMenu.items + navMenu.items + engineMenu.items {
            m.target = self
        }
        // performClose 走窗口本身（target 恢复 nil）
        fileMenu.item(withTitle: "关闭窗口")?.target = nil

        NSApp.mainMenu = mainMenu
    }

    @objc private func goTasks() { switchTo(0) }
    @objc private func goInbox() { switchTo(1) }
    @objc private func goEvents() { switchTo(2) }
    @objc private func goSettings() { switchTo(3) }
    @objc private func goEngineSettings() {
        EngineSettingsPanelController(state: state).show()
    }

    // MARK: - 菜单栏 status item

    private func buildStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.title = "M"
        let menu = NSMenu()

        miShowWindow = NSMenuItem(title: "显示 Maestro 窗口", action: #selector(showMainWindow), keyEquivalent: "")
        menu.addItem(miShowWindow)
        menu.addItem(.separator())
        miNewTask = NSMenuItem(title: "新任务…", action: #selector(newTaskClicked), keyEquivalent: "n")
        menu.addItem(miNewTask)
        miInbox = NSMenuItem(title: "收件箱", action: #selector(openInboxFromStatus), keyEquivalent: "")
        menu.addItem(miInbox)
        miEmergency = NSMenuItem(title: "急停", action: #selector(emergencyClicked), keyEquivalent: "")
        menu.addItem(miEmergency)
        menu.addItem(.separator())
        let miSettings = NSMenuItem(title: "引擎设置…", action: #selector(openEngineSettingsFromStatus), keyEquivalent: "")
        menu.addItem(miSettings)
        let miQuit = NSMenuItem(title: "退出 Maestro", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        menu.addItem(miQuit)

        item.menu = menu
        statusItem = item
    }

    @objc private func showMainWindow() {
        if window == nil { buildWindow() }
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func openInboxFromStatus() {
        showMainWindow()
        switchTo(1)
    }

    @objc private func openEngineSettingsFromStatus() {
        EngineSettingsPanelController(state: state).show()
    }

    private func refreshStatusItem() {
        // 未读（待处理收件箱）角标显示在按钮标题
        let n = state.inbox.count
        statusItem?.button?.title = n > 0 ? "M \(n)" : "M"
        miInbox.title = n > 0 ? "收件箱（\(n) 待处理）" : "收件箱"
        if state.isEmergencyStopped {
            miEmergency.title = "▶ 恢复全部"
        } else {
            miEmergency.title = "🛑 急停"
        }
        miEmergency.isEnabled = state.connected
        miNewTask.isEnabled = state.connected
        miInbox.isEnabled = state.connected
    }

    // MARK: - 刷新

    private func reloadUI() {
        refreshStatusItem()
        // 状态区
        statusDot.color = state.connected ? .systemGreen : .systemRed
        statusDot.needsDisplay = true
        statusLabel.stringValue = state.connected ? "Daemon 已连接" : "Daemon 未连接"
        if let s = state.status {
            statusSub.stringValue = "v\(s.version) · 运行 \(fmtUptime(s.uptimeSecs)) · 事件 #\(s.eventSeq)"
        } else {
            statusSub.stringValue = ""
        }
        // 活动栏角标：收件箱待处理数
        activityBar.badge(index: 1, count: state.inbox.count)

        // 设置视图：引擎状态 / 引擎摘要
        if let content = window?.contentView {
        if let es = content.viewWith(identifier: "settings-engine-state") as? NSTextField {
            if state.connected, let s = state.status {
                es.stringValue = "运行中 · v\(s.version) · pid \(s.pid) · 已运行 \(fmtUptime(s.uptimeSecs))"
            } else {
                es.stringValue = "未运行"
            }
        }
        if let es = content.viewWith(identifier: "settings-engine-summary") as? NSTextField {
            let e = state.engineSettings
            let gw = e.gatewayEnabled ? "网关 \(e.gatewayURL)" : "网关关闭（直连）"
            let cost = e.maxCostCents > 0 ? "$\(Double(e.maxCostCents)/100.0)" : "花费不限"
            let wall = e.maxWallMinutes > 0 ? "\(e.maxWallMinutes) 分钟" : "时长不限"
            es.stringValue = "\(gw) ｜ 预算：\(cost) / \(wall)"
        }
        }

        // 急停按钮
        if state.isEmergencyStopped {
            btnEmergency.title = "▶ 恢复全部"
            btnEmergency.contentTintColor = .systemGreen
            btnEmergency.toolTip = "恢复所有挂起的任务（投递积压轻推）"
        } else {
            btnEmergency.title = "🛑 急停"
            btnEmergency.contentTintColor = .systemRed
            btnEmergency.toolTip = "全局急停：冻结所有 Worker 并拍快照，现场保留"
        }

        // 齿轮菜单可用性
        if let start = gearMenu.item(withTitle: "启动 Daemon") { start.isEnabled = !state.connected }
        if let demo = gearMenu.item(withTitle: "演示模式启动（无 AI CLI）") { demo.isEnabled = !state.connected }
        if let stop = gearMenu.item(withTitle: "关停 Daemon") { stop.isEnabled = state.connected }

        // 表格
        tasksTable.reloadData()
        inboxTable.reloadData()
        eventsTable.reloadData()

        // 选中任务的详情
        let task = state.tasks.first { $0.id == selectedTaskID }
        detailText.textStorage?.setAttributedString(
            detailAttributedString(for: task, events: state.events))
        let steerable = task != nil && !(task?.isTerminal ?? true)
        steerField.isEnabled = steerable
        steerSendBtn.isEnabled = steerable
        btnPause.isHidden = !(task?.state == "working" || task?.state == "planning" || task?.state == "queued")
        btnResume.isHidden = !(task?.state == "suspended" || task?.state == "blocked")
        btnCancel.isHidden = !steerable

        // 收件箱操作
        let hasSel = selectedInboxID != nil
        btnInboxResume.isEnabled = hasSel
        btnInboxCancel.isEnabled = hasSel
        inboxActionLabel.stringValue = state.inbox.isEmpty
            ? "AI 遇到需要你决策的事会出现在这里"
            : "选中一条后可继续或取消（\(state.inbox.count) 项待处理）"

        // 底栏
        if let err = state.lastError {
            bottomInfo.textColor = .systemOrange
            bottomInfo.stringValue = err
        } else {
            bottomInfo.textColor = .secondaryLabelColor
            bottomInfo.stringValue = "数据目录：\(state.api.dataDir.path) · \(state.daemon.launchSummary)"
        }
    }

    private func fmtUptime(_ secs: Int) -> String {
        if secs < 60 { return "\(secs)s" }
        if secs < 3600 { return "\(secs / 60)m" }
        return "\(secs / 3600)h\(secs % 3600 / 60)m"
    }

    // MARK: - 动作

    @objc private func switchTo(_ index: Int) {
        guard index >= 0 && index < tabContainers.count else {
            AppLog.activity("切换失败：非法索引 \(index)（共 \(tabContainers.count) 视图）")
            return
        }
        let names = ["任务", "收件箱", "事件", "设置"]
        AppLog.activity("切换视图 → \(names[index])（index=\(index)）")
        for (i, v) in tabContainers.enumerated() {
            v.isHidden = i != index
        }
        activityBar.select(index)
    }

    @objc private func emergencyClicked() {
        if state.isEmergencyStopped {
            AppLog.emergency("用户点「恢复全部」→ resume_all(flush)")
            state.resumeAll()
        } else {
            AppLog.emergency("用户点「全局急停」→ emergency_stop")
            state.emergencyStop()
        }
    }

    @objc private func newTaskClicked() {
        let panel = NewTaskPanelController(state: state)
        panel.onCreate = { [weak self] title, prompt, dir in
            self?.state.createTask(title: title, prompt: prompt, workdir: dir)
        }
        panel.show()
    }

    /// 命令面板（⌘K，B3-5）
    @objc private func commandPaletteClicked() {
        // 已打开则直接聚焦，不重复建窗
        if let existing = palette {
            AppLog.ui("⌘K 面板已打开，聚焦现有窗口")
            existing.focus()
            return
        }
        let p = CommandPaletteController(
            onClose: { [weak self] in
                // 面板关闭（Esc/失焦/执行命令）→ 释放强引用
                AppLog.ui("⌘K 面板关闭，释放持有")
                self?.palette = nil
            },
            provider: { [weak self] query -> [CommandItem] in
            guard let self else { return [] }
            var items: [CommandItem] = [
                CommandItem(title: "新任务", subtitle: "创建并派发给 AI") { [weak self] in self?.newTaskClicked() },
                CommandItem(title: self.state.isEmergencyStopped ? "恢复全部" : "全局急停",
                            subtitle: "冻结所有 worker / 恢复") { [weak self] in self?.emergencyClicked() },
                CommandItem(title: "收件箱", subtitle: "需要你决策的任务") { [weak self] in self?.switchTo(1) },
                CommandItem(title: "事件流", subtitle: "实时动态") { [weak self] in self?.switchTo(2) },
                CommandItem(title: "引擎设置", subtitle: "CCR 网关 + 硬预算") { [weak self] in
                    guard let self else { return }
                    EngineSettingsPanelController(state: self.state).show()
                },
                CommandItem(title: "设置", subtitle: "引擎 / 数据 / 日志") { [weak self] in self?.switchTo(3) },
                CommandItem(title: "启动 Daemon", subtitle: "拉起后台引擎") { [weak self] in self?.state.startDaemon() },
                CommandItem(title: "关停 Daemon", subtitle: "停止后台引擎") { [weak self] in self?.state.shutdownDaemon() },
            ]
            // 任务：标题/id 含 query 即列出（限制 40 条）
            for t in state.tasks.prefix(40) {
                items.append(CommandItem(title: t.title, subtitle: "\(t.id) · \(t.state)") { [weak self] in
                    self?.selectedTaskID = t.id
                    self?.switchTo(0)
                })
            }
            return CommandFilter.filter(items, query: query)
        })
        palette = p
        AppLog.ui("打开 ⌘K 命令面板")
        p.show()
    }

    @objc private func steerSendClicked() {
        guard let id = selectedTaskID else { return }
        let msg = steerField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !msg.isEmpty else { return }
        state.steer(task: id, message: msg)
        steerField.stringValue = ""
    }

    @objc private func pauseClicked()  { if let id = selectedTaskID { state.pause(task: id) } }
    @objc private func resumeClicked() { if let id = selectedTaskID { state.resume(task: id) } }
    @objc private func cancelClicked() { if let id = selectedTaskID { state.cancel(task: id) } }

    @objc private func inboxResumeClicked() { if let id = selectedInboxID { state.resume(task: id) } }
    @objc private func inboxCancelClicked() { if let id = selectedInboxID { state.cancel(task: id) } }

    @objc private func gearSelected() {
        guard let title = gearMenu.titleOfSelectedItem else { return }
        switch title {
        case "引擎设置…": EngineSettingsPanelController(state: state).show()
        case "启动 Daemon": state.startDaemon()
        case "演示模式启动（无 AI CLI）": state.startDaemon(force: true)
        case "关停 Daemon": state.shutdownDaemon()
        case "打开数据目录": state.openDataDir()
        case "打开 daemon 日志": state.openDaemonLog()
        default: break
        }
        DispatchQueue.main.async { [weak self] in
            self?.gearMenu.selectItem(at: 0)
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
