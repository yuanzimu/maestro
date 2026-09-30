# Maestro — AI 任务指挥台

> 项目策划文档 · v0.4 · 2026-09-25（v0.3 增补开发计划；v0.4 用户视角重定位：双产品矩阵 + Token 经济 + 轻量硬约束）
>
> 配套文档：
> - **[DEV_PLAN.md](./DEV_PLAN.md)** — 开发计划 v2.0：Token 经济层 + Server 版「AI nginx」+ 修订路线图 ⭐ 执行入口
> - [HERDR_ANALYSIS.md](./HERDR_ANALYSIS.md) — herdr v0.9.1 源码分析报告（约 25 万行，含关键源码位置索引）
> - [TRAE_SOLO_ANALYSIS.md](./TRAE_SOLO_ANALYSIS.md) — Trae SOLO 模式五层架构分析

## 一、产品定位

**一句话**：自动省 token 且效果不打折的 AI 任务指挥台 —— herdr 的持久多 Agent 运行时 × Trae 的规划执行闭环，交付为跨平台桌面客户端 + Linux 服务端「AI nginx」。

### 产品矩阵

| 产品 | 形态 | 受众 | 核心承诺 |
|---|---|---|---|
| **Maestro Desktop** | Windows / macOS / Linux 客户端（Tauri，内嵌 daemon） | 个人开发者 | 零配置、轻量（<20MB）、派活走人、自动省 token |
| **Maestro Server** | Linux 部署版（headless daemon + 网关层） | 团队 / 服务器 / CI | 「AI 化的 nginx」：认证 / 多租户 / 配额 / 审计，同一引擎 |

| 维度 | 定义 |
|---|---|
| 差异化 1 | **Token 经济**：模型分级路由（L0~L3 + 升级路径）+ Code Mode + 缓存三件套，目标省 ≥50% 且验收通过率 ≥98%（基准集可证伪） |
| 差异化 2 | herdr 是终端运行时不管规划；Trae 是单线程闭环没有持久并行；本产品补齐中间地带并开源 |
| License | Apache-2.0（对商业友好，利于社区采纳） |

## 二、灵感来源分析

### herdr 的优势（运行时层）
- 状态感知：实时识别每个 pane 里 agent 的 working/blocked/idle/done 状态
- 会话持久化：关盖/断网/重启，agent 不死（后台 server 架构）
- Agent 互操作：Socket API + CLI，一个 agent 能指挥另一个 agent（如 Architect 派 Codex review 代码）
- worktree 隔离、模型无关、Rust 单二进制

### herdr 的不足
- 纯终端操作，对不熟 tmux 的用户有门槛
- 只是运行时，不做任务规划
- 状态检测对无 lifecycle hook 的 agent 不可靠（官方承认会误判为 idle）

### Trae Auto 模式的优势（规划层）
- Plan → 自动执行 → 验证 的完整闭环（规划即执行）
- 图形化体验：inline diff、可视化任务列表（TodoWrite）
- 子代理/技能/MCP 生态、权限自动化（auto 模式白名单放行）

### Trae Auto 的不足
- 子代理是一次性的，没有可观察的持久后台并行 agent
- 绑定在 IDE 里，无法脱离编辑器使用

### 融合机会
herdr 管「多 agent 并行运行时」强但没规划、界面是终端；Trae auto 管「单线程规划执行」强但没有多 agent 持久并行。两者互补。

## 三、核心理念：符合日常使用习惯的三个洞察

1. **人们习惯"派活"，不习惯"监工"** —— 主控 Agent 接需求自动拆解派发，用户只需在收到通知时回来决策
2. **人们习惯收件箱，不习惯终端滚动** —— 所有 Agent 的提问/权限请求聚合成一个待办收件箱，批量处理
3. **人们习惯关掉窗口，不习惯保持会话** —— 守护进程架构，应用退出任务照跑，回来时状态还在

## 四、功能蓝图

### 从 herdr 继承（运行时层）

| 能力 | 说明 |
|---|---|
| Worker 状态机 | planning / working / blocked / done / failed，实时检测每个执行 Agent 状态 |
| 会话持久化 | 后台守护进程 + SQLite 存储，重启后恢复任务现场 |
| Agent 互操作 API | Worker 之间可互相派活、等待、读取结果（coder 完成后自动请 reviewer 审查） |
| Git worktree 隔离 | 每个并行 Worker 在独立 worktree 干活，从机制上消灭并行冲突 |
| 外部 Agent 支持 | Claude Code / Codex / Gemini CLI 等通过适配器接入，模型无关 |

### 从 Trae auto 继承（规划层）

| 能力 | 说明 |
|---|---|
| Plan → Execute 闭环 | 主控 Agent 先出计划，用户确认（或 auto 模式直接跑），逐步执行并打勾 |
| 可视化任务树 | 计划拆解成任务依赖图（DAG），取代 herdr 的纯文本侧边栏 |
| Diff 审查体验 | Worker 的代码改动以 IDE 级 inline diff 呈现，一键采纳/拒绝 |
| 权限自动化 | auto 模式下白名单自动放行，敏感操作进收件箱人工确认 |

### 自主创新（体验层）

| 能力 | 说明 |
|---|---|
| **Blocked 收件箱** | 全局聚合所有 Worker 的等待项：权限请求、方案选择、冲突待裁决，像处理消息一样逐条消灭 |
| **双模式切换** | 「专注模式」单 Agent 深入干活（Trae 式）；「并行模式」多 Worker 分头推进（herdr 式），主控自动判断或用户指定 |
| **结果验收门** | 每个 Worker 完成后自动触发外部验证（跑测试/lint），防止多 Agent 互相"点头合谋" |
| **Worker 市场** | 社区贡献执行器配方（如"前端专家"的 prompt + 工具配置 + 首选 CLI） |

## 五、架构设计

```
┌────────────────────────────────────────────┐
│ Tauri 桌面壳（React + Tailwind）             │
│ 指挥台 UI：任务图 / Worker 状态栏 / Diff 审查 /  │
│ Blocked 收件箱 / 终端回放（只读）              │
└──────────────┬─────────────────────────────┘
               │ WebSocket（实时状态流）
┌──────────────┴─────────────────────────────┐
│ 核心守护进程（Rust，常驻后台）                 │
│ · 任务编排器：DAG 调度 / 依赖解析 / 重试       │
│ · Worker 运行时：PTY 管理 + 状态检测引擎       │
│ · 持久层：SQLite（任务/消息/事件流水）         │
│ · 本地 API：HTTP + Unix Socket（供插件和CLI）  │
├────────────────────────────────────────────┤
│ 主控 Agent（Planner，自研）                   │
│ · 模型无关：OpenAI-compatible + 各厂商 SDK    │
│ · 职责：理解需求→生成计划→派发→汇总→验收       │
│ · 权限策略引擎（auto / ask 分级）              │
├────────────────────────────────────────────┤
│ Worker 层（可插拔执行器，混合架构）             │
│ · 内置 Executor：自研轻量 agent（工具调用+循环）│
│ · CLI 适配器：Claude Code / Codex / Gemini…  │
│ · 每人一个 worktree + 沙箱预算（token/时长）   │
└────────────────────────────────────────────┘
```

### 关键设计决策

1. **守护进程与 GUI 分离**（继承 herdr 验证过的 server 架构）：GUI 只是其中一个客户端，CLI 和 Web 端未来都能接入
2. **Worker 用 headless 模式而非交互 TUI**（⭐ 最大的架构红利，来自源码分析）：Claude Code / Codex 均有 print/stream-json 模式，输出是结构化 JSON 事件流——**直接拿权威状态，完全不需要 herdr 那套 VT 屏幕解析 + 300ms 轮询 + 230 行正则规则的检测引擎**。这是 herdr 为了保留人类可用终端而无法做的选择。屏幕检测仅作为 P2+ 的兼容层（用户手动接入交互式 agent 时）
3. **协议双轨制**（抄 herdr）：私有二进制协议严格同版本匹配（GUI-守护进程同发布）；对外稳定 JSON API 按 endpoint generation 冻结，新增字段必须带默认值、新枚举值必须有 Unknown 兜底
4. **单线程权威状态 + 每连接线程 + mpsc 汇聚**（抄 herdr）：守护进程主循环无锁竞争；写者双车道（可靠控制车道 + 可丢弃事件车道）防慢客户端背压
5. **环境变量即集成协议**（抄 herdr）：`MAESTRO_ENV + PANE_ID + SOCKET_PATH` 三元组注入 Worker 进程，agent 侧 hook 经 socket 回报 session ID
6. **持久化三时机 + 三层备份**（抄 herdr）：脏标记+5s 去抖、pane 退出前 checkpoint、关停时落盘；tmp+rename 原子写；备份保留 3 份 + 快照 48 份；单调递增时间戳防时钟回拨
7. **Session 恢复 = argv 重放**（抄 herdr）：持久化 session ref → 校验（ID 当数据不当 shell 文本）→ dedupe_key 去重 → 按 `claude --resume <id>` 类 argv 拉起
8. **事件溯源存储**：SQLite append-only 事件流，任务状态可回放、可审计

## 六、技术选型

| 层 | 选型 | 理由 |
|---|---|---|
| 桌面壳 | Tauri 2 | 比 Electron 体积小一个量级，后端直接复用 Rust 核心 |
| 核心 | Rust | 与 herdr 同赛道验证过的选择：单二进制、PTY 生态成熟（portable-pty，Unix 侧无需 patch）、性能好 |
| Worker 承载 | **headless 子进程 + PTY**（双模式） | 首选 headless（`claude -p --output-format stream-json` 等结构化事件流）；PTY 仅用于 P2+ 的交互式 agent 兼容层 |
| 前端 | React + TypeScript | 生态最大，方便社区贡献 UI |
| LLM 接入 | OpenAI-compatible 统一层 + Anthropic/Gemini 原生 SDK | 模型无关是开源项目的基本盘 |
| 存储 | SQLite + 事件溯源 | 任务状态可回放、可审计 |
| IPC | Unix socket / 命名管道（interprocess 库，herdr 同款） | 跨平台双 socket：JSON API + 事件流 |

## 七、MVP 路线图

> 完整版（59 项特性清单 + 每阶段任务分解 + 验收标准 + 取舍对照）见 **[DEV_PLAN.md](./DEV_PLAN.md)**，本节为摘要。

| 阶段 | 周期 | 目标 | 关键交付 | 特性数 |
|---|---|---|---|---|
| **P0 验证** | 2~3 周 | 跑通运行时核心 | 守护进程 + headless Worker（claude stream-json）+ Goal 3轮blocked + allowlist + SQLite + argv 重放恢复 + CLI | 17 |
| **P1 MVP** | 4~6 周 | 拿得出手的开源首版 | Tauri GUI + Planner（状态注入+Plan审批门）+ 多 Worker 并行 + Blocked 收件箱 + inline diff + worktree 隔离 + 三级记忆 | 24 |
| **P2 协作** | +1 个月 | 多 Agent 闭环 | Worker 互操作 + Spec 三工件 + 验收门 + 内置 Executor + Code Mode 沙箱 + PTY 兼容层 + 完整 DAG | 15 |
| **P3 生态** | 持续 | 社区增长 | Worker 市场 + 插件 + SSH 远程 + Web 客户端 + MCP + live handoff | 11 |

**P0 详细任务分解**（基于 herdr 分析校准）：

1. **守护进程骨架**：setsid 脱离 + 单线程权威事件循环 + 每连接线程 + mpsc 汇聚 + 双 socket（JSON API / 事件流）。参考 herdr src/server/headless.rs、autodetect.rs
2. **Worker 运行时 v0**：headless 子进程管理（spawn 即 spawn_blocking(wait) 防僵尸 + PaneDied 事件）、`claude -p --output-format stream-json` 事件流解析 → planning/working/blocked(permission_request)/done/failed 状态机、环境变量三元组注入
3. **持久化 v0**：SQLite（任务/Worker/session ref）+ 脏标记去抖写 + tmp+rename 原子写 + 关停落盘；恢复 = argv 重放（`claude --resume <id>`，ID 当数据不当 shell 文本）
4. **验收指标**：
   - 连续跑真实任务 30 分钟，kill 守护进程重启后状态无损恢复
   - headless 事件流的状态判定 100% 准确（结构化数据，无启发式）
   - Blocked（权限请求）从发生到进收件箱 < 1s

## 八、开源运营策略

- **冷启动**：在 HN / Reddit r/LocalLLaMA / V2EX 发「herdr + GUI + planner」定位帖；herdr 本人在 HN 有热度，蹭对位讨论
- **文档即护城河**：Worker 适配器协议文档写清楚，让社区帮忙适配更多 CLI agent
- **不做 SaaS**：坚持本地优先，数据（含代码上下文）不出机器，这是与商业产品竞争的道德高地

## 九、风险与对策

| 风险 | 对策 |
|---|---|
| ~~外部 Agent 状态检测不可靠~~ | **已通过架构规避**（v0.2）：headless stream-json 事件流给出权威状态，无需 herdr 式屏幕检测；交互式 agent 接入（P2+）才需处理，届时抄 herdr 的「hook 优先 + 屏幕正则降级 + unknown 冻结」策略 |
| 各 Agent CLI 的 stream-json 格式不一且会改版 | 适配器层版本化 + herdr 式三级热更新（bundled → 远程 catalog → 本地覆盖）；格式解析容错（未知字段忽略、未知枚举→Unknown） |
| 多 Agent 并行改同一仓库冲突 | worktree 强制隔离（抄 herdr 的 git CLI 封装 + 脏区识别 + 残留清理）+ 主控负责合并顺序 + 冲突进收件箱人工裁决 |
| Agent 互评"合谋"（互相点赞放水） | 验收门强制外部验证：测试/lint 通过才算 done |
| 自研 Executor 工作量失控 | P2 才启动，定位为"外部 CLI 都不可用时的保底"，不做大而全 |
| 主控规划质量决定上限 | 计划必须先给用户过目（学 Trae plan mode），auto 模式可配置为仅低风险任务直跑 |
| 守护进程复杂度超预期（herdr server+api+protocol 约 4.5 万行） | P0 只做最小子集：单 socket + 10 个左右 API 方法；协议双轨制等高级机制按需引入 |
| 已知的坑直接照抄 herdr 修补 | 时钟回拨（单调时间戳）/ 慢客户端背压（双车道）/ stale socket 探活（dev/ino 身份校验）/ symlink 处理 |

## 十、下一步行动

1. **开工 P0**：Rust 守护进程 + headless Worker（Claude Code stream-json）+ SQLite 持久化 + 断线恢复（不写一行 GUI 代码，先验证最难的底座）
2. 搭项目骨架：cargo workspace（daemon / cli / protocol 三个 crate，参考 herdr 的模块划分但大幅精简）
3. 第一个适配器：Claude Code headless 事件流 → 状态机（对照 herdr src/agent_resume.rs 的 18 种 resume argv 映射，先支持 claude 一种）
4. 开发时随手查 herdr 源码：reference/herdr/ 已就位，关键位置索引见 [HERDR_ANALYSIS.md](./HERDR_ANALYSIS.md) 附录

---

## 参考资料

### 配套分析文档（本地）
- **[HERDR_ANALYSIS.md](./HERDR_ANALYSIS.md)** — herdr v0.9.1 源码分析：三大子系统 + file:line 级源码索引
- **[TRAE_SOLO_ANALYSIS.md](./TRAE_SOLO_ANALYSIS.md)** — Trae SOLO 模式架构分析：五层架构拆解 + 对 Maestro 的借鉴要点

### 本地源码（已克隆，供开发时查阅）
- `maestro/reference/herdr/` — herdr v0.9.1 完整源码（浅克隆）

### 在线资料
- [herdr 官网与文档](https://herdr.dev/docs/preview/cli-reference/)
- [herdr: Terminal Runtime for AI Coding Agents（innFactory 评测）](https://innfactory.ai/en/ai-harness/herdr/)
- [Herdr 智能体多路复用——智能体基建系列（中文解析）](https://www.cnblogs.com/weiluliaokeji/p/23104389)
- [herdr SKILL.md 解析与 CLI 实战指南](https://blog.gitcode.com/9a6a6d5c6db35abfd1aa746d9c772fe5.html)
