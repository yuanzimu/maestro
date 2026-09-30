# herdr v0.9.1 源码分析报告

> 基于对 /maestro/reference/herdr（2026-09-25 克隆，v0.9.1，约 25 万行 Rust）的源码分析。
> 分析方法：三个并行探索代理分别深入检测引擎、Server/API/持久化、PTY/Pane/Worktree 三个子系统。

## 0. 项目概况

| 维度 | 数据 |
|---|---|
| 版本 | 0.9.1（Apache-2.0） |
| 代码量 | 379 个 Rust 文件，~25 万行 |
| 最大模块 | client 6.0万行 / app 3.6万 / server 2.8万 / platform 1.3万 / pane 1.2万 |
| 关键依赖 | tokio、portable-pty(0.9, patched)、ratatui、crossterm、interprocess、bincode、schemars |
| Vendor | libghostty-vt（Zig 终端仿真器，7 个 patch）、portable-pty（3 个 Windows patch） |

模块行数分布说明工作量重心：TUI client 占比最大，server + api + protocol 合计约 4.5 万行——**"瘦客户端 + 守护进程"架构本身约占全项目 1/5 的代码量**，这是 Maestro 需要预算的。

---

## 1. 状态检测引擎（src/detect/，5300 行）

### 双层证据模型

```
第 1 层：身份识别（跑的是谁）—— 进程层
  轮询 pane 的前台进程组 → 读 /proc 的 argv0
  → normalized_process_name() 解包 runtime 包装
    （node/bun/python -c、node_modules 路径特征、版本化二进制名）

第 2 层：状态识别（在干什么）—— 屏幕层
  终端底部快照（detection_text()）+ OSC title/progress
  → 声明式规则引擎（per-agent TOML manifest）
  → 4 态状态机：Idle / Working / Blocked / Unknown
```

### Manifest 规则结构（TOML，编译期内嵌 + 三级热更新）

```toml
[[rules]]
id = "live_blocked_form"
state = "blocked"
priority = 980                      # 多规则命中取最高优先级，非短路
region = "after_last_horizontal_rule"
visible_blocker = true              # 强信号：屏上真实可见的阻断 UI
contains = ["esc to cancel"]        # 大小写不敏感子串
any = [ { contains = ["enter to confirm"] } ]   # 嵌套 OR
not = [ { contains = ["waiting for permission"] } ]
```

- 三种 matcher：`contains` / `regex`（整体多行）/ `line_regex`（逐行）
- gate 求值：`contains全中 && regex全中 && all全真 && (any空 || 任一真) && not全假`
- 复杂度上限防护：128 规则 / 8 层 gate / 512 gates / 1024 matchers
- 三级热更新：bundled → 远程 catalog（herdr.dev/agent-detection/index.toml）→ 本地覆盖 `~/.config/herdr/agent-detection/<agent>.toml`

### 检测调度（轮询而非流式）

- PTY 读回调只递增 AtomicU64 序列号，**不做匹配**（极轻）
- 独立 tokio 任务每 **300ms** 轮询；内容 seq 无变化且 idle 时**跳过读屏**
- 防抖：Working→Idle 需连续 3 次×100ms 确认（上限 700ms）；`visible_*` 硬证据直接放行
- 宽限窗：agent 切换后 3s 启动宽限 + 清 OSC 证据；新 agent 识别有 8s acquisition 窗口

### Unknown 兜底策略

- 进程识别不出 → agent=None → Unknown
- 已识别但无规则命中 → 默认 Idle（Codex 特判为 Unknown，因其 UI 无法区分）
- 规则可声明 `skip_state_update`：识别到 transcript 查看器等界面时**冻结状态而非误判**

### 局限与教训

1. 屏幕文案匹配本质脆弱——claude.toml 有 230 行规则就是持续追版本的代价
2. herdr 自己的推荐：**对有 hook 的 agent（pi/opencode）走 lifecycle hook 拿权威状态**，屏幕正则只是无 hook agent 的降级方案
3. 只有 4 态，无 done/error 细分

---

## 2. Server / API / 持久化架构（~4.5 万行）

### 进程模型

```
用户终端                        后台守护进程（setsid 脱离终端）
┌─────────────┐  herdr.sock (JSON API, 0o600)  ┌─────────────────────┐
│ thin client │────────────────────────────────▶│ 每连接1线程 → mpsc    │
│             │  herdr-client.sock (bincode)    │ HeadlessServer      │
│             │────────────────────────────────▶│  单线程权威事件循环    │
└─────────────┘                                 │  ├ App(权威状态+PTY) │
      ▲ ServerMessage(渲染流/控制)                │  ├ EventHub(订阅)    │
      └─────────────────────────────────────────│  └ session_writer   │
                                                └─────────────────────┘
```

- 自动启动：无 server 则 setsid spawn 守护进程 → 轮询 socket 就绪（15s 超时）→ 附着
- 主循环每轮：drain 内部事件 → API 请求 → 新客户端 → 客户端事件 → 定时任务 → 渲染推流
- **写者双车道**：可靠 control 车道 + 容量 1 可丢弃的 render 车道（慢客户端不积压）

### 协议双轨制（最值得抄的决策）

| 轨道 | 用途 | 策略 |
|---|---|---|
| 私有二进制协议（bincode，帧=u32len+payload） | 同版本 client | PROTOCOL_VERSION=22 **精确匹配**，不兼容即拒绝 |
| 稳定 endpoint 契约（JSON，EndpointControl） | 跨版本/SSH/Cloud | generation 永久冻结；新增字段必须带 serde default；新枚举值必须有 Unknown 兜底；codec 能力协商 |

API 层：单行 JSON-RPC 风格（`{id, method, params}` 扁平 tag 枚举），~130 个方法按 `server./agent./pane./tab./workspace./plugin./events.` 命名空间，全部 derive `schemars::JsonSchema` 生成 schema。

### 持久化（防崩溃三件套）

- **写入时机**：脏标记 + 5s 去抖 + 后台保存线程；pane 退出前 checkpoint；关停时落盘；logind delay lock 保证主机重启前完成最后一次写入
- **原子写**：tmp + rename（手工解 symlink）
- **三层备份**：session-backups/ 保留 3 份（覆盖前先复制）+ session-snapshots/ 保留 48 份（15min 间隔，layout 指纹去重）+ 单调递增时间戳防时钟回拨
- **恢复**：版本号大于当前即忽略 → 按 cwd 重建 shell → 回放历史 ANSI → `resume_agents_on_restore` 续接 agent 会话
- **live handoff**（版本升级零中断）：老 server 通过 Unix socket + SCM_RIGHTS 把 PTY master fd 直接传给新 server 进程

### 踩过的坑（代码中显式修补）

- 时钟回拨导致备份序错乱 → `max(prev+1)` 单调时间戳
- Windows Job 对象杀子进程 → 改走 WMI 启动
- listener 未集成进 select 导致空闲 CPU 自旋 → 250ms 轮询兜底
- 慢客户端背压 → render 车道容量 1 可丢弃
- 老 server + 新 client 能力不匹配 → 附着前用状态接口显式校验并给出升级指引

---

## 3. PTY / Pane / Worktree（~1.8 万行）

### PTY Actor 模型（每 pane 一个专职 OS 线程）

- 单线程状态机：`drain_commands → apply_controls → flush_writes → poll(PTY + wake_pipe)`
- **输入提交状态机**（向 TUI agent 注入 prompt 的可靠协议）：

```
WritingText → WaitingUntil(now+300ms) → WritingEnter
（先写文本、flush、等 300ms、再写 Enter——保证文本已被 agent 读走后才按回车）
```

- 僵尸处理：spawn 后立刻 `spawn_blocking(child.wait())`，wait 结果分类后阻塞式发 `PaneDied` 事件
- 关闭升级：对**会话内全部进程**（不只 shell pid）HUP(250ms)→TERM(250ms)→KILL(250ms)
- portable-pty 的 3 个 patch 全部是 Windows 专属（DLL 劫持防护、cmd 引号语义、REG_MULTI_SZ NUL）；Unix 侧原样使用

### 环境变量即集成协议（无需修改 agent 的最小侵入方案）

```bash
# 第一层：终端身份
TERM=<herdr专用>  TERM_PROGRAM=herdr
# 并清除 ITERM_SESSION_ID/TMUX/ZELLIJ/KITTY_WINDOW_ID 等宿主终端变量

# 第二层：herdr 通道
HERDR_ENV=1                    # 兼作阻止嵌套 herdr
HERDR_SOCKET_PATH=...          # agent 侧 hook 经此回报状态
HERDR_WORKSPACE_ID/HERDR_TAB_ID/HERDR_PANE_ID=...
# 并清除嵌套 agent 变量（CLAUDECODE、CODEX_THREAD_ID 等）
```

### Agent 会话恢复 = argv 重放

- 支持 18 种 agent 的 resume argv 映射：`claude --resume <id>`、`codex resume <id>`、`pi --session <path>`……
- Session ID 三个来源：① agent hook 回报（如 Claude SessionStart hook 检查 HERDR_ENV 后经 socket 调 `pane.report_agent_session`）② 用户手输 resume 命令被解析 ③ 恢复快照
- 恢复时按 `dedupe_key` 去重防止同一会话双开；**session ID 永远当 argv 数据不当 shell 文本**（有专门测试验证 `"abc; rm -rf /"` 作为单个参数传递）

### Worktree 集成

- 纯 git CLI 封装（LC_ALL=C 保证错误分类稳定）
- `worktree/brave-river-0000` 风格分支名 + `<worktree_root>/<repo>/<branch-slug>` 路径
- 脏工作区识别 + 残留目录校验清理 + dubious ownership 处理
- **显式触发创建，不自动为每个 agent 创建**

### 为什么 vendor ghostty

Worker 需要的不是「终端」而是「**可查询的屏幕状态机**」：PTY 字节喂给 Zig 实现的 VT 解析器，维护屏幕+回滚，`detection_text()` 取底部快照供检测引擎用。vendor 治理纪律极强：每个 patch 记录 issue/上游讨论/验证命令/移除条件，`just check` 自动校验 patch 与索引一致。

---

## 4. 对 Maestro 的直接结论

### 必须抄的决策

| # | 决策 | 来源 |
|---|---|---|
| 1 | 瘦客户端 + 后台守护进程，GUI 只是客户端之一 | server 架构 |
| 2 | 双 socket：机器可读 JSON API / 高频二进制事件流分开 | api/protocol |
| 3 | 协议双轨制：私有协议严格同版本 + 稳定 endpoint generation 分层 | protocol |
| 4 | 单线程权威状态 + 每连接线程 + mpsc 汇聚（无锁竞争） | headless server |
| 5 | PTY actor 单线程 + wake pipe；Text→delay→Enter 三段提交 | pty actor |
| 6 | 环境变量三元组（ENV/PANE_ID/SOCKET_PATH）作为 agent 集成协议 | pane env |
| 7 | 持久化：去抖+checkpoint+关停三时机、原子写、多层备份、单调时间戳 | persist |
| 8 | 检测优先走 agent hook（权威状态），屏幕正则只是降级方案 | detect |
| 9 | Session 恢复 = argv 重放 + ID 当数据不当 shell 文本 | agent_resume |
| 10 | spawn 即 wait 防僵尸；关闭信号三级升级 | pane 生命周期 |

### 必须避的坑

- 屏幕正则检测是维护无底洞（claude.toml 230 行规则）→ Maestro 的 Worker 优先用 **headless 模式 CLI**（如 `claude -p --output-format stream-json`）拿结构化事件，根本不需要屏幕检测
- 时钟回拨 / symlink / 慢客户端背压 / stale socket 探活——照抄 herdr 的修补
- TUI client 占 6 万行——Maestro 用 Tauri GUI，省掉这部分，但 server+api+protocol 约 4.5 万行的量级要有心理预期

### Maestro 相对 herdr 的架构简化机会

1. **Worker 用 headless 模式而非交互 TUI**：Claude Code/Codex 都有 print/stream-json 模式，输出是结构化 JSON 事件流（工具调用、权限请求、完成信号），**不需要 VT 屏幕解析和 300ms 轮询**，状态直接来自事件流——这是 herdr（为了保留人类可用的终端）没有的选择自由，也是 Maestro 最大的简化红利
2. **不需要 vendor ghostty**：不渲染屏幕就不用终端仿真器
3. **GUI 用 Tauri 而非 ratatui**：跨平台桌面体验 + Web 技术栈生态
4. 保留屏幕检测仅作为「用户手动把交互式 agent 接入」时的兼容层（P2+ 可选）

---

## 附录：关键源码位置索引

| 主题 | 位置 |
|---|---|
| 状态机定义 | src/detect/mod.rs:10-20 |
| 进程识别 | src/detect/mod.rs:245-282, 369-408 |
| 规则引擎 | src/detect/manifest.rs:153-199, 1231-1278, 368-373 |
| 检测任务/防抖 | src/pane.rs:763-1075; src/pane/agent_detection.rs |
| Server 主循环 | src/server/headless.rs:384-494 |
| 自动启动守护进程 | src/server/autodetect.rs:194, 295 |
| API schema | src/api/schema.rs:35-52 |
| 协议帧/版本 | src/protocol/wire.rs:20, 1662-1755 |
| 稳定 endpoint 层 | src/protocol/endpoint.rs:1-33 |
| 快照结构 | src/persist/snapshot.rs:12, 100-112 |
| 原子写+备份 | src/persist/io.rs:44-61; writer.rs:78-186 |
| 恢复流程 | src/persist/restore.rs:65, 804-833 |
| PTY actor | src/pty/actor/unix.rs:426, 480-484 |
| 输入三段提交 | src/pty/actor/unix.rs:480-484 |
| 环境注入 | src/pane.rs:92-118, 166-202 |
| Agent resume | src/agent_resume.rs:136-256, 289-298 |
| 关闭信号升级 | src/pane.rs:1575-1623 |
| Hook 通道示例 | src/integration/assets/claude/herdr-agent-state.sh |
| Worktree | src/worktree.rs:21, 194, 309, 346 |
