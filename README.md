<div align="center">

# Maestro

**自动省 token 且效果不打折的 AI 任务指挥台**

持久多 Agent 运行时 × 规划执行闭环 —— 派活走人，回来收结果

[![CI](https://github.com/yuanzimu/maestro/actions/workflows/ci.yml/badge.svg)](https://github.com/yuanzimu/maestro/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](https://www.rust-lang.org/)

</div>

---

## 它是什么

Maestro 把"用 AI 干活"从**守着终端监工**变成**派活走人**：

- 你在命令行（未来是桌面 App）里输入一句话需求
- Maestro 的守护进程接管：规划 → 派发 → 驱动多个 AI Worker（Claude Code / Codex / Gemini / Amp / OpenCode）并行执行 → 验收 → 汇总
- 中途 AI 遇到需要你决策的事，进入统一的**收件箱**，像处理消息一样批量处理
- 你可以随时合上电脑、断网、甚至 `kill -9` 守护进程 —— **任务现场完整保留**，回来继续

一句话：**Maestro = AI 任务的"nginx"**。 nginx 托管 HTTP 请求的生命周期，Maestro 托管 AI 任务的整个生命周期。

## 为什么做这个

现有工具各占一端，中间地带没人补：

| | 代表 | 强处 | 短板 |
|---|---|---|---|
| 终端多路复用 | herdr | 多 Agent 持久并行、状态感知 | 纯终端、无任务规划、屏幕检测不可靠 |
| IDE 内闭环 | Trae SOLO / Cursor | 规划→执行→验证闭环、图形化 | 单线程、子代理一次性、绑死在编辑器里 |
| LLM 网关 | LiteLLM / Portkey | 转发与计费 | 不管任务生命周期，只是 API 代理 |

**Maestro = herdr 的运行时 + Trae 的规划闭环 + nginx 的网关形态**，且全部开源（Apache-2.0）。

## 核心特性

### 🎛️ 任务编排（已实现）
- **多任务队列**：并发槽位调度、workdir 互斥防冲突、FIFO 补位、队列深度限流防风暴
- **多轮驱动 Worker**：Maestro 持有任务循环（CLI 单轮 + `--resume` 续接），而非一次性甩给 CLI —— 这是「中途轻推」「模型升级重试」的机制地基
- **五方言适配器矩阵**：`claude` / `amp` / `codex` / `gemini` / `opencode` 的 stream-json 事件流 → 统一状态机（planning / working / blocked / suspended / done / failed），结构化数据 100% 准确，无需屏幕正则猜测

### 🛡️ 可靠性（已实现，188 项测试守护）
- **suspended ≠ failed**：断连/合盖睡眠/Provider 故障自动进入挂起态，退避恢复（30s→5m，10 次后才升级人工）—— 错误的标签会引发用户删掉重跑、白烧一遍 token
- **kill -9 无损恢复**：SQLite WAL 事件溯源 + argv 重放（`claude --resume <id>`），守护进程随时被杀、重启后现场完整
- **全局急停三阶段**：`FREEZE`（SIGSTOP 进程组，<100ms 瞬时静止）→ `SNAPSHOT`（逐 worktree git 快照）→ `DECIDE`（恢复/放弃/回滚由你决定）—— 出事的 8 分钟里你需要的是刹车和后悔药，不是误杀
- **Checkpoint 时光机**：任务级 git ref 快照（`write-tree`/`commit-tree` 原语，零打扰 <200ms，不动 HEAD 不污染分支），含 pre-rollback 安全垫
- **孤儿进程零残留**：daemon 重启绝不收养旧 Worker（一律杀 + 标 suspended），PID 文件带 start_time 双因子防 PID 复用误杀
- **steering 队列 at-least-once**：轻推消息持久化 + 消费确认 + 跨重启重投 —— 「说了没人听」比不做更伤信任

### 💰 Token 经济（框架已落地，P1 全量）
- **模型分级路由 L0~L3**：确定性操作不走 LLM，简单任务用小模型（1x 成本），复杂任务才上旗舰（20x）；验收不过自动带上下文升级重试 —— 质量由验收门兜底
- **Token 计量账本**：每任务每 Worker 的用量入事件流，cache 感知计价（cache_creation 单独计价），与 CLI 自报成本对账（漂移 >25% 自动告警）
- **目标**：同一基准任务集相比「旗舰直连」总成本降 ≥50%，验收通过率 ≥98%（P1 建 20 任务基准集，可证伪）

### 🧭 体验层
- **Blocked 收件箱**：所有 Worker 的权限请求/方案选择/冲突裁决聚合成一个待办列表，带行动建议
- **人话叙事**：`task get` 输出「第 N 轮：{top3 工具}×{次数}，累计 $X，耗时 T｜最近：{摘要}」；`events --human` 12 类事件可读渲染
- **Web 指挥台**：`maestro ui` 零依赖起 HTTP + 内嵌单页（任务卡进度叙事 + 轻推/恢复/放弃就地操作）
- **doctor 自检**：`maestro doctor` 一条命令定位 daemon/socket/CLI/环境问题

## 架构

```
┌─────────────────────────────────────────────┐
│ 客户端层（同一协议，可插拔）                    │
│  maestro CLI · maestro ui (Web) · Tauri GUI(P1)│
└──────────────────┬──────────────────────────┘
                   │ JSON over IPC（Unix socket / Windows TCP 回环）
┌──────────────────┴──────────────────────────┐
│ maestro-daemon（Rust，常驻后台）              │
│ · 单线程权威 Core：独占全部状态，无锁竞争        │
│ · 每连接线程 + mpsc 汇聚 + 有界订阅队列         │
│ · 持久层：SQLite WAL 事件溯源（可回放可审计）    │
│ · 内置 LLM client（分类/叙事/成本预估共用）      │
├─────────────────────────────────────────────┤
│ Worker 运行时                                │
│ · headless 子进程（stream-json 权威状态流）     │
│ · 环境三元组注入（MAESTRO_ENV/PANE_ID/SOCKET）│
│ · 五方言适配器：claude/amp/codex/gemini/opencode│
│ · 多轮驱动循环 + steering 轮间投递 + 轮数预算    │
└─────────────────────────────────────────────┘
```

**关键设计决策**（继承 herdr 验证过的模式 + 修正其坑）：

1. **headless 优先于 PTY**：Claude Code/Codex 等的 `--output-format stream-json` 给出权威状态，完全绕开 herdr「VT 屏幕解析 + 300ms 轮询 + 230 行正则」的检测引擎（其官方承认会误判）
2. **单线程权威状态**：守护进程主循环独占状态，写者走 mpsc —— 无锁竞争
3. **Worker stdout/stderr 落文件不接管道**：防 SIGSTOP 后管道满导致 wait() 死锁（同类项目实测坑）
4. **不引入 tokio/async**：std 线程足够，符合轻量硬约束（daemon release 4.2MB）
5. **checkpoint 用 git plumbing**：直接调 `git` CLI，不引 libgit2 —— 依赖只有系统 git

## 快速开始

### 前置依赖

| 依赖 | 用途 |
|---|---|
| Rust stable | 构建（见 `rust-toolchain.toml`） |
| git ≥ 2.30 | checkpoint 原语（调 plumbing 命令） |
| cc / pkg-config | rusqlite bundled 编译 |
| claude CLI（或其它支持的 Agent CLI） | 真实 Worker 执行 |

### 构建

```bash
git clone https://github.com/yuanzimu/maestro.git
cd maestro
cargo build --workspace
```

### 跑起来

```bash
# 1. 自检环境
cargo run -p maestro-cli -- doctor

# 2. 启动 daemon（自动 setsid 脱离后台）
cargo run -p maestro-daemon

# 3. 创建任务
cargo run -p maestro-cli -- task create \
  --title "修复登录页样式" \
  --prompt "把登录按钮改成圆角并加 loading 态" \
  --workdir /path/to/your/project

# 4. 看状态
cargo run -p maestro-cli -- status          # daemon 概览
cargo run -p maestro-cli -- task list       # 任务列表
cargo run -p maestro-cli -- inbox           # 待办收件箱
cargo run -p maestro-cli -- events --human --follow  # 人话事件流

# 5. Web 指挥台（可选）
cargo run -p maestro-cli -- ui              # 浏览器打开 http://localhost:7331

# 6. 急停 / 恢复
cargo run -p maestro-cli -- stop            # 全局急停（保现场）
cargo run -p maestro-cli -- resume          # 恢复
```

### 测试

```bash
cargo test --workspace        # 188 项测试（含 e2e：kill -9 恢复/急停/混沌/负载）
cargo clippy --all-targets -- -D warnings
```

## 项目结构

```
maestro/
├── crates/
│   ├── maestro-protocol/   # 纯数据契约：API schema + 事件类型 + UX 埋点
│   ├── maestro-daemon/     # 守护进程：Core/server/worker/adapter/persist/
│   │                       #   steering/suspend/emergency/checkpoints/ledger/llm
│   ├── maestro-client/     # 客户端库（CLI/GUI 共用，跨平台 IPC 抽象）
│   ├── maestro-cli/        # CLI：status/task/worker/inbox/stop/resume/events/doctor/ui
│   ├── maestro-testkit/    # 测试基建：MockClock/FakeAdapter/脚本 Worker/pgid 沙箱
│   └── maestro-e2e/        # 端到端：挂起矩阵/急停矩阵/混沌/负载/多轮/持久化/竞态
├── docs/                   # 设计与分析文档（见下）
└── .github/workflows/ci.yml  # 三平台 CI（ubuntu/windows/macos）
```

## 文档

| 文档 | 内容 |
|---|---|
| [docs/PROJECT_PLAN.md](docs/PROJECT_PLAN.md) | 产品战略：定位/差异化/功能蓝图/架构决策 |
| [docs/DEV_PLAN.md](docs/DEV_PLAN.md) | 开发计划：Token 经济层 + Server 版 + 修订路线图（当前执行入口） |
| [docs/P0_SETUP.md](docs/P0_SETUP.md) | P0 搭建指南：目录结构/依赖清单/测试基建/CI |
| [docs/U10_T6_DESIGN.md](docs/U10_T6_DESIGN.md) | 急停/checkpoint/错峰执行的详细设计 |
| [docs/HERDR_ANALYSIS.md](docs/HERDR_ANALYSIS.md) | herdr 源码分析报告（架构灵感来源） |
| [docs/TRAE_SOLO_ANALYSIS.md](docs/TRAE_SOLO_ANALYSIS.md) | Trae SOLO 架构分析（规划闭环灵感来源） |
| [docs/UX_DEEP_DIVE.md](docs/UX_DEEP_DIVE.md) | 信任旅程体验设计（U 组 14 项） |

## 路线图

| 阶段 | 状态 | 目标 |
|---|---|---|
| **P0 核心引擎** | ✅ 基本完成（57 轮迭代，188 测试全绿，三平台 CI） | 守护进程 + 多轮驱动 + 急停/checkpoint + 账本 + 五方言 + CLI |
| **P1 Desktop MVP** | 🚧 下一步 | Tauri GUI + Planner + Token 经济全量 + 信任体验 U 组（3 个 Sprint） |
| **P2 Server「AI nginx」** | 📋 已设计 | 认证/多租户/配额/审计 + install.sh 一条命令部署 |
| **P3 生态** | 📋 规划中 | Worker 市场 / 插件 / Web 客户端 / MCP / 跨项目批量 |

> 真实的 claude CLI 端到端验证待 API key；P0 已用 MockClock + 脚本 Worker 在 CI 全绿。

## 技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 语言 | Rust | 单二进制、性能好、与 herdr 同赛道验证 |
| 异步 | 无（std 线程 + mpsc） | 单线程权威循环不需要 tokio，符合轻量 |
| 存储 | SQLite（rusqlite bundled，WAL） | 事件溯源、可回放、可审计 |
| IPC | Unix socket / Windows TCP 回环 | 零新依赖，JSONL 线协议 |
| LLM | ureq 阻塞式 | daemon 内部调用都是小 JSON 非流式 |
| 未来 GUI | Tauri 2 | 比 Electron 小一个量级，复用 Rust 核心 |

**体积实测**（release）：daemon 4.2MB · CLI 1.4MB · rounder 628KB —— 距 <30MB 约束有 7 倍余量。

## 贡献

项目处于 P0 收尾、P1 启动阶段。最有价值的贡献方向：

1. **Worker 适配器**：B7 适配器矩阵（Codex/Gemini/Amp/OpenCode 方言已落地，真实 CLI 验证需要各平台实测）
2. **效果基准集**：P1 要建 20 任务基准，欢迎贡献真实任务样本
3. **文档与翻译**：协议文档、适配器开发指南

提 Issue / PR 前请先读 [docs/DEV_PLAN.md](docs/DEV_PLAN.md) 了解当前阶段与决策记录。

## License

Apache-2.0 —— 对商业友好，利于社区采纳。

---

<div align="center">
<sub>灵感来源：<a href="https://herdr.dev">herdr</a>（运行时）× <a href="https://www.trae.ai">Trae</a>（规划闭环）× nginx（网关形态）</sub>
</div>
