# Trae SOLO 模式（Auto 模式）架构分析

> 调研日期：2026-09-25
> 方法：一手运行时观察（agent 本人即运行在 Trae SOLO 运行时中）+ 本机安装目录逆向 + 官方文档
> 结论先行：auto 模式官方名称为 **SOLO 模式**（本机 MCP 目录名即为 `solo_agent`），定位是"AI 主导、确认计划后自动推进"的规划-执行闭环。本文档是 PROJECT_PLAN.md 中"Trae Auto 模式的优势"一节的技术依据，也是 Maestro 规划层/运行时层设计的直接参考。

## 一、模式定位

| 维度 | 说明 |
|---|---|
| 产品形态 | TraeCode IDE 左上角模式切换：Code 模式（人主导）/ SOLO 模式（AI 主导） |
| 官方定义 | AI 自主规划并执行：需求理解 → 代码生成 → 测试 → 成果预览/部署 |
| 关键机制 | "确认计划后自动推进"——审批门之前必须人审，审批门之后全自动 |
| 本机证据 | `~/.trae-cn/mcps/s_all-<hash>/solo_agent/`、settings 中 `AI.toolcall.v2.solo.command.mode: alwaysRun` |

## 二、五层架构拆解

auto 模式不只是一个 agent，而是五层机制的组合。Maestro 的差异化机会在于对每一层做"开源可插拔"改造。

### 1. 会话引导层 —— 每轮对话前的状态注入

每条用户消息前，运行时注入大量系统级上下文：

- **环境信息**：工作目录、操作系统、时区/日期、当前模型
- **终端状态**：终端池上限（20 个）、各终端 idle 标记（防止误杀运行中命令）
- **Todo 状态**：当前任务清单及其状态机（in_progress 仅允许一项）
- **记忆上下文**：用户画像 + 项目记忆 + 最近会话话题摘要
- **模式与规范提醒**：Plan/Spec 模式激活状态、语言与输出规范

设计意图：用确定性状态注入代替模型猜测，让 agent 每轮决策都有完整现场。

### 2. 规划层 —— 两种递进的规划范式 + Goal 预算

内置为两个技能（`TRAE-plan-mode` / `TRAE-spec-mode`，本机 `builtin/trae/doutops/skills/`）：

**Plan 模式**（中小型任务，单计划单审批门）：

```
调研代码 → 写计划文档 .trae/documents/{NAME}_plan.md
→ NotifyUser 请求批准（审批门）→ 执行 + 验证 → 汇报
```

- 计划可手动编辑或自然语言指令修订
- 审批前禁止修改任何文件/系统状态

**Spec 模式**（系统级任务，三工件 + 独立 Review 门）：

| 工件 | 用途 | 写入时机 |
|---|---|---|
| `spec.md` | 需求大纲与验收标准 | Specify 阶段 |
| `tasks.md` | 任务队列、状态、证据 | Plan/Implement 阶段 |
| `review.md` | 独立检查点与 Review 历史 | Review 阶段 |

- 存储于 `.trae/specs/{任务名}/`，可版本控制，作为项目知识资产
- 首次创建时暂停等待确认；执行时任务状态自动更新
- 支持跨会话恢复（中断后续跑）
- **独立 Review 门**：验收由独立上下文执行，防止"自己写自己过"

**Goal 系统**（运行时第三层保险）：

- `create_goal` 可设 token 预算
- 同一阻塞条件连续 3 轮（含自动续跑）无进展 → 自动标记 blocked
- 防止 agent 无限空转烧 token

### 3. 执行层 —— 两种工具执行范式并存（最关键发现）

**范式 A：直接工具调用**

LLM 逐个调用 Read/Shell/Edit/Grep 等工具，每步一个 LLM 往返。灵活但 token 开销大、过程不确定。

**范式 B：Code Mode（Exec）—— 沙箱编排**

LLM 写一段 JavaScript，在**隔离 V8 沙箱**中一次编排多个工具：

- 沙箱内**无**文件系统、网络、fetch、require、process、setTimeout
- 唯一出站通道：`await tools.<name>(args)`；`ALL_TOOLS` 供运行时自省工具 schema
- `text()` 输出结果、`exit()` 提前终止

配套编排决策（`TRAE-code-mode-orchestrator` 技能）：满足以下至少两条才用 Code Mode——

1. **Fan-out**：同一工具并行打 N 个目标（上限 20 并发）
2. **数据依赖**：A 步输出经 JS 变换喂给 B 步
3. **条件分支**：下一步取决于上一步结果
4. **循环/迭代**：同逻辑处理列表（有界：重试≤5、轮询≤3 轮）
5. **聚合/去重/评分**：合并多次工具调用结果
6. **验证门**：产出后必须校验（读回文件、查退出码、验字段）

模式库含：工具发现（强制先跑）→ 并行扇出+错误隔离 → 顺序管道 → 条件分支 → 有界重试 → 去重评分 → 产物生成+验证 → 长任务轮询 → 反模式清单。

**本质：把"多步 agent 循环"压缩成"一次确定性脚本执行"。** 收益：LLM 往返次数从 N 次降为 1 次（省 token）、逻辑确定性（防跑偏）、可强制验证再报成功（防谎报）。

### 4. 自主与安全层 —— "auto" 的前提

| 机制 | 说明 | 本机证据 |
|---|---|---|
| 权限 auto-run | 命令/MCP 工具自动放行 | settings: `AI.toolcall.v2.solo.command.mode: alwaysRun`、`ide.mcp.autoRun` |
| 路径白名单 | 写/删操作硬约束在 allowlist 内（工作区、/tmp、~/.cache、~/.local/bin 等），越界直接拒绝 | hooks 环境目录 `ModularData/ai-agent/hooks_env` |
| 后台任务系统 | 长命令转后台（job ID + 输出文件），完成时事件通知 agent，避免轮询与上下文污染 | `/tmp/trae-agent-toolhost-*/jobs/` |
| 技能完整性校验 | agent→model→skill 三级 manifest，从对象存储拉 zip 包 + sha256 校验 | `~/.trae-cn/builtin/manifests/*.json` |
| 浏览器/计算机使用 | 受控浏览器自动化与桌面操作技能 | `builtin/global/skills/TRAE-browseruse*`、`TRAE-computer-use` |

### 5. 上下文管理层 —— 让闭环能持续运转

- **记忆三级结构**：
  - `user_profile.md`（跨项目用户画像）
  - `project_memory.md`（项目级硬约束/惯例/教训）
  - `session_memory_*.jsonl + topics.md`（按日归档的会话级记忆）
  - 新会话先 Grep 记忆再做任务
- **上下文自动压缩**：接近窗口上限时系统自动折叠历史消息，会话长度不受窗口限制
- **子代理隔离**：主 agent 派生 Explore/Plan/general-purpose/code-auditor 等子代理，独立上下文、只回传结论，保护主上下文
- **执行环境一致**：git worktree 预留（`~/.trae-cn/worktrees/`）+ bash profile 快照（`toolhost/native-runcommand-snapshots/`）

## 三、关键文件索引（本机证据）

| 路径 | 内容 |
|---|---|
| `~/.config/Trae CN/User/settings.json` | auto-run 权限配置 |
| `~/.trae-cn/builtin/trae/doutops/skills/TRAE-plan-mode/` | Plan 模式完整工作流 |
| `~/.trae-cn/builtin/trae/doutops/skills/TRAE-spec-mode/` | Spec 模式完整工作流 |
| `~/.trae-cn/builtin/global/skills/TRAE-code-mode-orchestrator/` | Code Mode 编排决策 + 模式库 |
| `~/.trae-cn/mcps/s_all-<hash>/solo_agent/integrated_code_mode/tools/Exec.json` | Exec 沙箱工具定义 |
| `~/.trae-cn/builtin/manifests/*.json` | 技能分发清单（sha256 校验） |
| `~/.trae-cn/memory/` | 三级记忆系统 |
| `~/.trae-cn/worktrees/`、`~/.trae-cn/toolhost/` | 隔离执行环境 |

## 四、对 Maestro 的借鉴要点

对应 PROJECT_PLAN.md 的分层（运行时层 ← herdr，规划层 ← Trae auto）：

1. **P0 直接可抄：blocked 判定** —— "同一阻塞条件连续 3 轮无进展自动停"，这是 Trae 防空转的最省力机制，Maestro 的 Blocked 收件箱可精确复用此语义。
2. **P0 直接可抄：Code Mode 双范式** —— Worker 执行器提供"逐工具对话式"与"沙箱脚本编排"两种模式；后者一次往返完成多步确定性操作，是省 token、防跑偏、强制验证的关键。Maestro 的 Rust daemon 可用 Deno/quickjs/rusty_v8 提供等价沙箱。
3. **P1 可借鉴：审批门 + 工件化规划** —— Plan/Spec 的"文档即规划"（.trae/documents、.trae/specs）配合 SQLite 事件溯源天然契合：plan/spec 本身就是事件流中的工件事件。
4. **P1 可借鉴：独立 Review 门** —— 对应 Maestro"结果验收门（防 Agent 合谋）"，Trae 已验证独立上下文审查的必要性。
5. **差异化机会：五层皆闭源耦合** —— Trae 的状态注入、权限白名单、技能分发均绑定官方客户端。Maestro 全部开源化 + Worker 可插拔（P2），并补 Trae 缺失的"多 Agent 并行编排"（Trae 只有主从子代理，无对等协作）。
6. **安全基线** —— 路径白名单 allowlist 是 auto 权限的必要配套，Maestro daemon 必须第一天就有，不能后补。

## 五、参考来源

- 火山引擎官方文档：SOLO Agent（https://www.volcengine.com/docs/86677/2210092）
- TraeCode Documentation：SOLO mode overview（https://docs.trae.ai/ide/solo-mode）
- 本机运行时文件（2026-09-25 逆向）
