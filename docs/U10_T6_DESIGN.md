# U10 危机安全网 & T6 错峰执行 —— 详细设计

> 版本 v1.1 · 2026-10-04（v1.0 · 2026-10-01）
> 隶属：[DEV_PLAN.md](./DEV_PLAN.md) v2.4 的实现依据；需求源自 [UX_DEEP_DIVE.md](./UX_DEEP_DIVE.md) 第二部分 U10/U11/U12
> 范围：suspended 状态机 / emergency_stop API / Checkpoint 时光机 / T6 调度逻辑与 provider 接入

---

# 第〇部分 实施状态对照（v1.1 新增，P0 实况 R1~R32）

> U10 第一部分（§1~§4）已随 0.17/0.18/0.19 全部落地；T6 第二部分（§5~§9）P1 开工。本部分记录实现与本设计的**语义修正**，冲突时以本部分 + 代码为准。

## §0.1 落地清单

| 设计节 | 里程碑 | 实现位置 | 测试 |
|---|---|---|---|
| §2 suspended 状态机 | 0.17 ✅ | `daemon/src/suspend.rs` + `core.rs` | a_suspended.rs / recovery_gap.rs |
| §2.5 孤儿清理 | 0.17 ✅ | daemon 重启扫描 pidfile，绝不收养 | `recover_reaps_and_marks_daemon_crash` |
| §2.4 断连检测 | 0.15/0.18 ✅ | `daemon/src/adapter.rs`（见 §0.4 修正） | a_suspended.rs / multiround.rs |
| §3 emergency_stop 三阶段 | 0.18 ✅ | `daemon/src/emergency.rs` | b_emergency.rs（B1/B5/B7/B8/B12/B13）+ race_window.rs 竞态 |
| §4 checkpoint 时光机 | 0.19 ✅ | `daemon/src/checkpoint.rs`（git plumbing） | b8 回滚 / security.rs ref 命名空间 |
| §5~§9 T6 错峰/batch | P1 ⏳ | 未开工 | — |

## §0.2 语义修正一：steering 投递三路口径（R23，取代 §3.1 的 flush 描述）

设计 §3.1 中「resume_all steering=flush → 按序投递」在多轮驱动落地后精确化为**三个投递路口**（`core.rs` steering_prefix / api_steer_poll）：

| 路口 | 时机 | 机制 |
|---|---|---|
| **活 worker 轮边界** | 每轮完成后 | worker 调 `TaskSteerPoll` 拉取，注入下一轮 prompt |
| **respawn / 恢复重拉** | worker 死亡重拉时 | daemon `take_all` 把积压+未确认消息**前置拼入 prompt**（daemon 确认的投递） |
| **终态** | Done/Failed/Blocked/Cancel | `SteeringDropped` 事件逐条发出——**不静默** |

因此 resume_all 的 `flush` 实现为「**不 drain，留队**」等轮边界 poll 取走（而非直接推送）；`hold` = drop_all + 每条 SteeringDropped。

## §0.3 语义修正二：at-least-once 投递（R32，调研 opencode-queue 落地）

设计未覆盖投递可靠性。实现引入**确认语义**（`steering.rs`）：

- poll 取走的消息进入 `inflight`（**持久化**，kill -9 不丢）；再次 poll 会重投未确认的
- worker 消费后（下一轮跑完 / CLI 失败轮已跑过该 prompt / 结构化错误轮）调 `TaskSteerAck` 确认；仅当前 worker 有效（stale worker -403）
- rounder 在 CLI 启动前被杀 → 未确认 → daemon 重投；**已跑过的失败轮也 ack**（防网络故障指令死循环重投）

## §0.4 语义修正三：断连检测升级为「结构化错误优先」（R28 调研落地）

设计 §2.4 仅提 stderr 模式匹配。实现（`adapter.rs` classify_exit）分层：

1. **结构化错误优先**：stream-json result 事件的 `errors[]` / `api_error_status`（Claude Code 官方口径）——429/5xx → retryable → Suspended 自动恢复
2. stderr 模式匹配兜底：认证/配额/billing → Fatal（不重试）；connection reset/timeout/fetch failed/EPIPE → Disconnect（Suspended，非 failed）
3. 未知 stderr 默认 Fatal（安全默认）；`error_max_turns` 不算错误

## §0.5 用例组覆盖实况

- **A 组**：A2（disconnect_auto_resumes，MockClock 推退避）· A6 · A8 · A10 已测；A1/A3/A4/A7 属 P1（A3 随 A7 circuit breaker，A4 随宿主睡眠检测，A7 随 E3 预算）——A9 由 persistence.rs events_replay + multiround.rs crash_recovery 覆盖
- **B 组**：B1+B5（合并测）· B7（hold 侧）· B8 · B12 · B13 已测；B2/B3/B4/B6/B9/B10/B11 属 P1 补强；B14 随 T6
- **混沌补强**（multiround.rs，超出原设计）：stale worker 双防护 -403、断连自动恢复、结构化过载 529→Suspended、at-least-once 全链、双任务并发

---

# 第一部分 U10 危机安全网

## 1. 设计目标与不变量

| # | 不变量（违反即 bug） |
|---|---|
| I1 | 急停后 **≤100ms** 内所有 Worker 进程组停止产生新的文件写入/网络请求 |
| I2 | 急停不销毁任何可恢复现场：事件流完整、session ref 完整、worktree 文件保留、有急停时刻的 checkpoint |
| I3 | 挂起（suspended）的任务在 daemon 崩溃/重启后仍可恢复，无孤儿进程残留 |
| I4 | 任何回滚操作本身可撤销（回滚前自动快照 pre-rollback checkpoint） |
| I5 | 断连/供应商故障导致的挂起**自动恢复**；人为挂起**只手动恢复**——恢复策略由挂起原因决定，不可配置混淆 |

## 2. suspended 状态机

### 2.1 状态与转移

```
                 ┌──────────┐
                 │  queued  │
                 └────┬─────┘
                      ▼
                 ┌──────────┐   block(权限/提问/Goal 3轮)
                 │ planning │──────────────┐
                 └────┬─────┘              ▼
                      ▼                ┌────────┐
                 ┌──────────┐ suspend  │blocked │── suspend ──┐
      ┌─────────▶│ working  │────────▶ └────┬───┘             │
      │          └──┬───┬───┘               │ unblock         │
      │ resume       │   │ done(验收门过)    ▼                 ▼
      │             ▼   ▼               ┌──────────┐
      │        ┌─────┐ ┌─────┐          │ suspended │◀─────────┘
      └────────┤done │ │failed│          └────┬─────┘
               └─────┘ └─────┘               │ resume(手动/自动)
                                          ┌──┴──────────┐
                                          ▼             ▼
                                      cancelled      working(续跑)
```

- `blocked` = 等待**用户决策**（权限/方案选择/验收三次失败）；`suspended` = **执行暂停**（基础设施/人为）。两者都进收件箱，但图标、语义、恢复路径不同
- `failed` 仅保留给**不可恢复**错误：重试预算耗尽、配置错误、验收门结构性失败。断连/断供**永远不是 failed**

### 2.2 挂起原因枚举与自动恢复矩阵

```rust
enum SuspendReason {
    UserPause,        // 用户手动暂停任务
    EmergencyStop,    // 全局急停波及
    NetworkLost,      // 网络不可达
    ProviderOutage,   // 供应商 429/5xx 风暴（circuit breaker 打开）
    BudgetExceeded,   // Worker 预算耗尽（E3）
    SystemSleep,      // 宿主机休眠/唤醒检测
    DaemonCrash,      // daemon 异常退出时的孤儿清理路径
}
```

| 原因 | 自动恢复 | 恢复条件 | 策略 |
|---|---|---|---|
| NetworkLost | ✅ | 连通性探测通过 | 退避 30s→1m→2m→5m(封顶)，10 次失败(≈30min)后升级为 blocked(infra) 进收件箱 |
| ProviderOutage | ✅ | circuit breaker 半开探测通过 | 恢复时优先原 provider；若配置了 failover（A7）则切换后直接续跑 |
| SystemSleep | ✅ | OS 唤醒事件 | 唤醒即续，注入 steering 注记「宿主刚从睡眠恢复」 |
| UserPause | ❌ 仅手动 | `task.resume` | — |
| EmergencyStop | ❌ 仅手动 | `server.resume_all` / `task.resume` | 显式恢复，UI 明示「现场完整保留」 |
| BudgetExceeded | ❌ 仅手动 | 用户提额或批准继续 | 进收件箱带花费明细 |
| DaemonCrash | ❌ 仅手动 | 重启后呈 suspended 等待确认 | 恢复=argv 重放（0.12 已有） |

### 2.3 持久化与事件

挂起/恢复是**事件**，状态由事件流重放派生（与 herdr 一致，不存终态）：

```rust
// 事件负载（进 0.2 的 schema）
SuspendEvent { task_id, worker_id, reason, session_ref,     // CLI 会话引用（--resume 用）
               checkpoint_ref, round_no, ts }
ResumeEvent  { task_id, worker_id, from_reason, via,        // auto|user|resume_all
               new_session_ref?, ts }                        // 恢复后 session ref 可能变化
```

### 2.4 断连检测（headless 适配器层）

- **进程侧**：CLI 退出码 + stderr 模式匹配（`connection reset` / `timeout` / `ECONNREFUSED` / `fetch failed`）→ 适配器映射为 `NetworkLost` 而非 `failed`
- **daemon 侧预防**：daemon 内置 LLM client（0.14）的连通性探测器发现 provider 不可达时，在**轮次边界**（0.15 多轮驱动）预防性挂起——比让 CLI 跑进崩溃便宜（省一次半途废轮的 token）
- 恢复路径：`claude --resume <session_ref>` 在同一 worktree 续跑；若 CLI 不支持 resume 的 provider，则退化为「上一轮 checkpoint + 任务级重试」

### 2.5 孤儿进程处理（daemon 崩溃恢复）

每个 Worker 启动时写 PID 文件 `data_dir/workers/<worker_id>.json`：`{pid, pgid, task_id, round_no, started_at, stopped: bool}`。daemon 启动时扫描：

```
for pidfile in workers/:
  if 进程不存在 → 清理 pidfile，任务保持事件流里的状态（重放即真相）
  if 进程存在且 pgid 属于我们且状态为 T(stopped) → SIGKILL + 任务标记 suspended(DaemonCrash)
  if 进程存在且 pgid 属于我们且在运行     → SIGKILL + 任务标记 suspended(DaemonCrash)（session 可恢复，杀比收养简单）
  if pgid 不属于我们（PID 复用）          → 清理 pidfile
```

**原则**：daemon 重启后 Worker 一律不收养、直接杀——因为 session ref + checkpoint 已保证可恢复性，收养的复杂度（重建 PTY/流）不值。

## 3. emergency_stop API

### 3.1 三阶段语义：FREEZE → SNAPSHOT → DECIDE

**为什么 SIGSTOP 而不是直接杀**：杀进程 = 丢失进行中轮次的对话与半成品状态，且「现场」是残缺的；SIGSTOP 让进程组**瞬时静止**（不产生新写入），文件系统静止后 checkpoint 才是干净快照。杀不杀、何时杀，交给用户在 DECIDE 阶段决定。

```
阶段 1 FREEZE（目标 < 100ms）
  1. orchestrator.freeze()          // 停止派发新轮次；steering 队列冻结不投递
  2. kill(-pgid, SIGSTOP) 逐 Worker // 进程组整体静止（含 CLI 子进程）
  3. 发 EmergencyStopped 事件        // {frozen_workers[], ts, reason}

阶段 2 SNAPSHOT（目标 < 2s/Worker，自动执行）
  4. 逐 worktree 执行 checkpoint（§4.3 capture 原语；进程已停 → FS 静止）
  5. 记录各任务 session_ref + round_no + checkpoint_ref
  6. 发 EmergencySnapshotted 事件   // 现场固化完成的信号，UI 据此解锁「恢复/回滚」

阶段 3 DECIDE（用户驱动，无超时；但 daemon 退出兜底见下）
  - server.resume_all / task.resume → SIGCONT + unfreeze + steering 按 param 决定 flush|hold
  - task.cancel                     → SIGCONT + TERM(250ms)→KILL 升级（复用 0.5）+ cancelled
  - rollback + resume               → §4.4 restore + 注入 steering「用户已回滚到 cp-X」+ resume
```

**daemon 退出兜底**：若用户迟迟不决定而 daemon 被关掉，优雅关停钩子将 SIGSTOPped 的 Worker SIGKILL 并标记 `suspended(EmergencyStop)`；粗暴 kill -9 场景由 §2.5 孤儿清理兜底。**任何路径下 Worker 都不会以「运行中的孤儿」形态存活**（I3）。

### 3.2 协议定义（进 0.2 schema）

```json
// 方法：server.emergency_stop
{ "id": "req-1", "method": "server.emergency_stop",
  "params": { "reason": "user_panic" } }          // reason 可选，仅审计用
// 结果（SNAPSHOT 完成后返回，或 FREEZE 后立即返回 + 事件流补快照进度）
{ "result": { "frozen_workers": ["w1","w2"],
              "suspended_tasks": ["t1","t2"],
              "checkpoints":      ["cp:t1:r7","cp:t2:r3"],
              "sessions_preserved": true, "freeze_ms": 47 } }

// 方法：server.resume_all（急停的逆操作）
{ "params": { "steering": "flush" } }             // flush=投递冻结期间积压的轻推 | hold=丢弃

// 方法：task.pause / task.resume / task.cancel（任务粒度，非急停场景）
{ "params": { "task_id": "t1" } }

// 事件
EmergencyStopped  { workers[], ts, reason }
EmergencySnapshotted { per_task: [{task_id, checkpoint_ref, session_ref, round_no}] }
```

CLI 对应：`maestro stop` / `maestro resume` / `maestro task pause|resume|cancel <id>`（进 0.13）。

### 3.3 不变量测试（P0 验收用例）

| 用例 | 断言 |
|---|---|
| 急停时 Worker 正在写文件 | FREEZE 后 mtime 不再变化；checkpoint 提交成功（半成品写入按原样入快照，如实记录） |
| 急停 → kill -9 daemon → 重启 | 无孤儿进程（ps 验证 pgid）；任务呈 suspended(EmergencyStop)；resume_all 后续跑 |
| 急停 → 回滚 → resume | worktree 精确恢复到 cp 内容；对话注入了回滚注记；断言 I4（pre-rollback cp 存在） |
| resume steering=hold | 冻结期间的轻推消息被丢弃且事件流有记录（不是静默丢） |
| 急停期间新任务提交 | 排队不执行，unfreeze 后按序启动 |

## 4. Checkpoint 时光机

### 4.1 设计原则

- **零打扰**：不移动 HEAD、不污染用户分支、不产生 stash 条目——只用底层 plumbing 命令建 commit 对象 + 挂 ref
- **worktree 即边界**：每个并行 Worker 的 worktree 独立快照；主仓合并点另有 pre-merge 快照（P2 B12）
- **可解释**：每个 checkpoint 带结构化 message（任务/轮次/原因/时间），UI 时间线与 U3 叙事事件对齐

### 4.2 ref 命名空间与保留策略

```
refs/maestro/cp/<task_id>/<seq>-<label>        # seq 单调递增
message = JSON { task, round, reason: baseline|round_start|emergency|manual|pre_rollback|pre_merge,
                 parent_cp, ts }
保留策略（每任务）：
  · 永久保留：baseline（任务起点）、acceptance_passed（验收通过的点）、pre_merge、pre_rollback
  · 滚动保留：最近 50 个 + 最近 24h 内全部
  · 任务 done 后 GC：保留 baseline + final + 用户星标，其余删除（git reflog expire + gc --prune=now 于低负载时）
存储成本：commit 对象按内容去重（git 天然），每轮增量通常 KB 级，50 个 checkpoint ≈ 一个普通 commit 量级
```

### 4.3 capture 原语（<200ms 典型仓库）

```rust
fn capture(worktree: &Path, task: &TaskId, round: u32, reason: CpReason) -> Result<Ref> {
    // 1. git add -A            —— 暂存全部（含 untracked；.gitignore 的构建产物天然排除）
    // 2. TREE=$(git write-tree)
    // 3. CP=$(git commit-tree $TREE -p $PARENT -m $JSON_MSG)   // 不动 HEAD
    // 4. git update-ref refs/maestro/cp/<task>/<seq> $CP
    // 5. git reset             —— 还原 index，工作区零扰动
}
```

触发点（全部是多轮驱动的天然钩子，无额外调度）：任务启动（baseline）/ **每轮开始前**（round_start，成本 ~100ms/轮可忽略）/ emergency_stop 阶段 2 / 手动 / pre-merge（P2）/ pre-rollback（自动安全垫）。

### 4.4 restore 原语（回滚）

```rust
fn restore(worktree: &Path, cp: &Ref) -> Result<()> {
    capture(reason: PreRollback)?;                  // I4：回滚本身可撤销
    run!("git reset --hard {}", cp)?;
    run!("git clean -fd")?;                          // 清 untracked；保留 .gitignore'd（构建产物）
}
// 恢复后的 resume：--resume 的对话指向的是回滚前的文件状态，
// 必须注入 steering："用户已将工作区回滚到 checkpoint <ref>，请基于当前文件现状继续"
```

### 4.5 UI 时间线（P1 Sprint B）

```
任务「重构 auth 模块」
 ●──●──●──●──●──●──●──▶ 现在
 r1  r2  r3  r4  r5  r6  r7
 起点 读文件 定位bug 写修复 写测试 ←急停⚡ 被回滚↩
     ↑ 每个节点 = checkpoint × U3 叙事标签联合
     点击节点 → diff(vs 上一cp / vs 当前) · 恢复到此点 · 星标保留
```

---

# 第二部分 T6 错峰执行

## 5. 核心洞察：两条半价路径，适用对象不同

| 路径 | 本质 | 适合 | 不适合 |
|---|---|---|---|
| **Batch API**（OpenAI/Anthropic，-50%） | 一次性提交请求集合，24h 内返回 | **非交互、可并行**的步骤：批量文件分析、N 仓库同模板任务、分类扫描、项目名片预计算 | 交互式 agent 循环（工具结果依赖下一步，天然串行） |
| **off-peak 时段**（Gemini，-50%） | 特定时段计费半价，交互性不变 | **可延迟的交互任务**：睡前派的重构/文档 | 有 deadline 且等不到窗口的任务 |

**设计前提（实现期校准，调研任务 R4/R5，见 [DEV_PLAN.md](./DEV_PLAN.md) §9.2）**：Batch 的 24h 是窗口上限而非承诺（历史中位通常 <1h，T4 账本积累真实 ETA）；off-peak 窗口与触发机制（自动计费 vs 参数开关）以 provider 文档为准，做成**可配置元数据**而非硬编码。

## 6. 调度逻辑

### 6.1 eligibility：什么能被延迟

步骤级属性 `deferrable`，来源三处：
1. 用户任务标记：「可延后 / 今晚跑」
2. Planner 标注：计划中的非交互 fan-out 步骤（每文件一请求的批量分析）
3. U14 批量任务自动 deferrable（同模板 × N 仓库 = 完美 Batch 形态）

**硬约束**：deadline 前无用户交互依赖的步骤才可 deferrable（blocked-waiting 的不行——用户睡了没人按审批）；deadline 默认「明早 9 点」或用户指定。

### 6.2 决策函数（进审批门，与 U8 成本预估合并呈现）

```
step.ready ∧ deferrable:
  candidates = []
  for path in {batch(每个支持的 provider), offpeak(每个有窗口的 provider), interactive}:
      price = estimate(step, path.model) × path.discount
      eta   = path == batch    ? batch_eta_confidence()        // T4 历史分位
            : path == offpeak  ? window_start + est_duration
            : now + est_duration
      if eta ≤ deadline: candidates += {path, price, eta}
  choose = argmin(price)，eta 同价取早者
  审批门呈现三栏：「现在 $1.20 · 今晚半价 $0.60(00:00 UTC 起) · 批量 $0.60(明早 6 点前)」
```

### 6.3 执行路径

**off-peak 路径**：步骤进 deferred 队列（持久化）；daemon 定时器在窗口开启时唤醒调度。笔记本休眠错过窗口的处理：唤醒时重估——仍在窗口内则跑；错过则下一窗口或按 deadline 自动升级 interactive（账本记 `offpeak→interactive_promote`，成本报告如实呈现）。

**batch 路径**：见 §7。**deadline 风险对冲**：`deadline − now < 2h` 且 batch 未完成 → 自动升级 interactive，升级可能性在审批门三栏中预先声明（不搞惊喜计费）。

## 7. Batch Manager（daemon 组件）

### 7.1 生命周期

```
收集期（10 分钟聚合窗 / 满 200 项 / 8MB 提前触发）
  → 构造 JSONL（每行一个 step 的请求）
  → POST /v1/batches（OpenAI）或 /v1/messages/batches（Anthropic）
  → BatchSubmitted {batch_id, item_ids[]} 事件
轮询期（每 5 分钟）
  → GET batch 状态；U3 叙事：「批量分析 200 文件已提交，预计明早 6 点前返回」
完成期
  → 下载结果文件 → 逐 item 解析状态
     成功 → 结果写回 step（验收门照常跑——半价不豁免质量）
     失败 → 重试 ≤2 次 → 仍败则 interactive 兜底（ledger 记 fallback 原因）
```

### 7.2 约束与坑

- Batch 不支持流式/实时端点；**单请求内的工具调用可以用，但请求间不能依赖**——eligibility 检查必须验证「这一步的所有请求可独立构造」
- 批次大小/数量上限（OpenAI：50k 请求/200MB per batch）、速率限制独立计——超限自动分片
- 结果文件 30 天过期 → 完成即取即解析即归档摘要（账本只存计量数据，不存原文）
- Anthropic 与 OpenAI 的 batch 语义差异（input 格式/字段名）封装在 provider adapter 内，batch_manager 只见统一 trait

### 7.3 在途 batch 的急停处置（B14 细化：标记 + 轮询）

> 前提：R5 未证实 batch 可撤销前，处置按「不可取消」设计；若 R5 证实支持 cancel，急停时做一次 best-effort cancel，其余流程不变。

**状态机**（BatchInFlight，daemon 内部）：

```
Collecting → Submitted(batch_id) → Polling(active)
                   │                    │  emergency_stop
                   │                    ▼
                   │              Polling(frozen)   ← 观测模式：降频轮询只为状态呈现，结果不回写任务
                   │                    │  resume_all
                   │                    ▼
                   │              Polling(active) + 立即 poll 一次
                   ▼                    ▼
              Completed(buffered?) → 回写步骤（验收门照常）
              SLExpired(24h)      → §最终失败流程
              Abandoned           → 用户放弃 / 回滚致 stale / 升级后晚到
```

**急停时刻的标记动作**：
1. 不杀（SIGSTOP 管不到 provider 侧）、默认不 cancel
2. 发 `BatchMarkedSuspended {batch_id, task_id}` 事件
3. 关联任务 → `suspended(EmergencyStop, external_inflight=[batch_id])`
4. 轮询切换 frozen 节奏（见下表）

**resume_all 语义（默认：等结果，不重提）**——batch 已付费，重提=双花：
- resume 后立即 poll 一次 + 恢复正常节奏；结果到达照常回写
- **例外——回滚致 stale**：若用户在 DECIDE 阶段回滚了该任务 worktree，batch 结果基于回滚前状态 → resume 时注入 steering「batch 结果基于 pre-rollback 状态，先校验再采信」；用户也可选放弃（Abandoned，费用照记账本，不冲销）

**轮询间隔与超时阈值**（默认值，S6 TOML 可配）：

| 参数 | 默认值 | 说明 |
|---|---|---|
| poll_normal | 5 min | 正常节奏（§7.1） |
| poll_frozen | 15 min | 急停观测模式（只呈现不回写） |
| poll_backoff | ×2 封顶 30 min | 轮询遇 429/5xx 时退避 |
| poll_warn | 连续 3 次失败 | 发 WarningEvent（网络问题 ≠ batch 失败，别误杀） |
| poll_dead | 累计 2h 无一次成功 poll | 任务 → blocked(infra)，人工查网络 |
| deadline_watch | 每次 poll 检查 | `deadline − now < 2h && 未完成` → 触发升级（下条） |
| provider_sl | submit + 24h | catalog 元数据；到期 → SLExpired |
| item_retry | ≤ 2 | 失败 item 重新入聚合窗 |

**deadline 升级与晚到结果的去重**：升级线触发时，未完成 items 立即转 interactive；此后 batch 结果若晚到，**已升级的 item 以 interactive 结果为准，晚到的 batch 结果标 Abandoned**（不计双花双跑）；未升级的 item 照常采信 batch 结果。

**最终失败回滚流程**（SLExpired / item 重试 2 次仍败）：

```
1. 可下载输出文件 → item 级甄别：成功项保留（钱已花，结果照用）；失败项列清单
2. 失败项重试 ≤2（重新入聚合窗）
3. 重试仍败 → interactive 兜底（ledger path=batch_fallback，该段折扣=0）
4. 兜底也过不了验收门 → 走 U6 失败路径：suspended/failed + 断点续跑只重跑该步骤
5. worktree 安全：batch 是分析类输出（文本），不直接写盘；落盘发生在后续 apply 步骤，
   apply 步骤自带 round_start checkpoint——「回滚」= restore 到该 checkpoint + steering 注记
   （复用 0.18/0.19 原语，B14 不新增回滚机制）
6. 账本收口：BatchFailed{batch_id, failed_items, reason} + U9 通知(priority=异常)
   + 收件箱三选一：[interactive 重跑] [放弃该步骤] [查看检讨]
```

## 8. 账本与配置（T4/S6 集成）

```toml
# T4 账本新增字段
StepLedger { path: interactive|offpeak|batch|batch_fallback|offpeak_promote,
             discount: 0.5, counterfactual_cost, actual_cost }
# 周报新增一行：「错峰+批量本周省 $3.80（其中 2 次因 deadline 自动升级，多花 $0.4，已如实计入）」

# S6 配置
[execution.offpeak]
enabled = true
default_deadline = "09:00"          # 本地时区
[[execution.offpeak.provider]]
id = "gemini"
window_utc = ["07:00", "23:00"]     # 实现期按官方文档校准
[execution.batch]
enabled = true
aggregation_minutes = 10
poll_minutes = 5
```

## 9. 风险与对策

| 风险 | 对策 |
|---|---|
| provider 折扣政策变动（窗口/比例/机制） | 折扣元数据外置为可更新 catalog（P2 检测清单热更新同机制），不硬编码 |
| Batch 实际 ETA 逼近 deadline | T4 积累分位数 ETA + 2h 提前升级线 + 审批门预先声明 |
| 笔记本休眠错过窗口 | 唤醒重估逻辑；Server 版用 systemd timer 天然免疫 |
| 半价路径被滥用导致 deadline 全违约 | eligibility 硬约束（交互依赖检测）+ 默认 deadline 保守 |
| 「省 token 报告」夸大 | counterfactual 只对比「同模型同路径的牌价」，不拿降档省的算进错峰功劳 |

---

# 第三部分 U10 单元测试用例（P0 实现依据）

> 与 §3.3 的关系：§3.3 是验收场景级描述；本部分细化为可直接落地的用例组 A（suspended 状态机）与用例组 B（emergency_stop 三阶段），合并覆盖不变量 I1~I5。

## §10 用例组 A：suspended 状态机自动恢复矩阵

**被测单元**：`state_machine`（reason→策略映射与转移合法性）、`recovery_scheduler`（退避与升级）、`orphan_reaper`（孤儿清理）。
**测试基建**：MockClock（退避不睡真实时间）、FakeAdapter（注入 stderr 模式/网络失败/卡任意状态）、带已知 pgid 的脚本 Worker。

| # | 用例 | 操作 | 断言 | 覆盖 |
|---|---|---|---|---|
| A1 | reason→恢复策略全表 | 对 7 种 SuspendReason 逐一触发，读恢复策略 | NetworkLost/ProviderOutage/SystemSleep→auto；UserPause/EmergencyStop/BudgetExceeded/DaemonCrash→manual；**未知 reason 兜底 manual**（安全默认） | I5 |
| A2 | 断连退避表与升级 | MockClock 推进 30s→1m→2m→5m→5m…探测持续失败 | 尝试次数与退避表一致；第 10 次（≈30min）后变 blocked(infra) 且进收件箱；此后再走 10min **无新尝试**（自动尝试封口） | I5 |
| A3 | 断供熔断→半开→恢复 | breaker 打开→suspend；半开探测成功/失败各测一次 | 成功→原 provider 续跑；失败→重新打开**不振荡** | I5 |
| A4 | 睡眠唤醒注记 | 触发 SystemSleep→唤醒事件 | 任务恢复 + steering 注记「宿主刚从睡眠恢复」；ResumeEvent.via=SystemSleep | I5 |
| A5 | 恢复走 --resume | suspend→resume | spawn argv 含 `--resume <session_ref>`；CLI 上报新 ref 后 ResumeEvent 记录 new_session_ref | I2 前置 |
| A6 | 人为挂起拒绝自动恢复 | 对 UserPause/EmergencyStop/BudgetExceeded 分别触发定时器/网络恢复事件 | **零 spawn**；仅 `task.resume` API 可恢复 | I5 |
| A7 | 预算耗尽恢复需批准 | BudgetExceeded→未提额直接 resume→error；提额后 resume→成功 | 拒绝时收件箱项带花费明细 | I5 |
| A8 | 孤儿清理四象限 | 构造 pidfile：①进程已死 ②活着且 stopped(T) ③活着且运行中 ④PID 被外来进程复用 | ①清文件、状态以事件重放为准；②③SIGKILL+标记 suspended(DaemonCrash)；④只清文件**不碰外来进程**；扫描后本 pgid 无存活进程 | I3 |
| A9 | 状态由事件重放派生 | 写入含多次 suspend/resume 的事件流→重放 | 派生状态与在线状态一致；kill -9 后重放结果不变 | I3 |
| A10 | 断连≠failed（回归钉） | 适配器注入 `connection reset` stderr | 状态=Suspended(NetworkLost)，**永不** Failed | I5 |

```rust
// A2 骨架（伪代码）：MockClock 驱动退避，全程不睡真实时间
#[test]
fn networklost_backoff_then_escalate() {
    let clock = MockClock::new(); let d = Daemon::with(clock.clone());
    let t = d.spawn_task(cfg()); d.fail_next_round(stderr("fetch failed"));
    assert_eq!(t.state(), Suspended(NetworkLost));
    for (i, wait) in [30, 60, 120, 300, 300].iter().enumerate() {
        clock.advance_secs(*wait); d.probe_network(false);
        assert_eq!(t.resume_attempts(), i + 1);
    }
    clock.advance_to(TOTAL_30MIN); d.probe_network(false);
    assert_eq!(t.state(), Blocked(Infra)); assert!(d.inbox_has(t, "网络持续不可达"));
    clock.advance_secs(600);
    assert_eq!(t.resume_attempts(), 10); // 升级 blocked 后不再自动尝试
}
```

## §11 用例组 B：emergency_stop 三阶段协议

**被测单元**：`freeze`（FREEZE）、`snapshots`（SNAPSHOT）、`decide`（resume_all / cancel / rollback+resume）及其与 checkpoint 原语的联动。

| # | 用例 | 操作 | 断言 | 覆盖 |
|---|---|---|---|---|
| B1 | FREEZE 延迟 | 假 Worker 每 5ms 写一次文件；调 emergency_stop | 返回值 freeze_ms ≤ 100；EmergencyStopped 事件时间戳后 200ms 内**无新写入**；进程状态=T | I1 |
| B2 | 冻结后无新网络请求 | Worker 循环打本地 mock HTTP；stop | EmergencyStopped 后 mock server 收包数为 0（允许 250ms 在途宽限） | I1 |
| B3 | 事件流完整 | stop 时事件流高频写入中 | 冻结点前事件序号连续无空洞；EmergencyStopped/EmergencySnapshotted 均在流中 | I2 |
| B4 | session ref 保全 | stop→逐任务断言 | SuspendEvent.session_ref 与适配器当前一致；resume_all 后 spawn argv 携带该 ref | I2 |
| B5 | 急停时刻 checkpoint | SNAPSHOT 完成后检查 refs | 每个挂起任务都有 reason=emergency 的 cp；每 Worker 快照耗时 <2s（事件时间戳差） | I2 |
| B6 | 先冻后照的顺序 | 高频快照与 stop 并发执行 | 所有 cp 创建时间戳 ≥ 全部 Worker 进入 T 态的时间戳（**禁止照到移动中的 FS**） | I1+I2 |
| B7 | steering flush/hold | stop 前注入 2 条轻推；分别以 flush/hold 恢复 | flush：两条按序投递至下一轮；hold：两条丢弃且各有一条 SteeringDropped 事件（**不静默**） | I2 |
| B8 | 回滚+恢复（I4 主径） | stop→rollback 到 cp-2→resume | ①先自动生成 pre-rollback cp ②worktree 与 cp-2 树 diff 为空 ③resume 的 steering 含「已回滚到 cp-2」注记 | I4 |
| B9 | 回滚的回滚 | B8 后再回滚到更早点，再前滚到 B8 的 pre-rollback cp | 两个 pre-rollback cp 都在；前滚成功（回滚本身可撤销） | I4 |
| B10 | cancel 路径 | stop→task.cancel | TERM 250ms 内退出则不发 KILL（信号序列断言）；状态=cancelled；worktree 保留不清 | I2 |
| B11 | DECIDE 中 daemon 崩溃 | stop→kill -9 daemon→重启 | 无本 pgid 存活进程；任务呈 suspended(EmergencyStop)；resume_all 仍可用；cp 完整 | I3 |
| B12 | 冻结期新任务 | stop 后 create task | 状态=queued 零 spawn；resume_all 后按序启动（调度器冻结而非死亡） | I1 |
| B13 | 急停幂等 | 连续两次 emergency_stop | 第二次返回当前冻结状态；无重复 SIGSTOP 报错、无重复快照 | I1 |
| B14 | 在途 batch 与急停（处置细则见设计 §7.3） | T6 batch 已提交未返回时 stop | batch 不受 SIGSTOP 影响：标记 suspended-external + 轮询切 frozen 节奏（15min 观测模式）；resume_all **不重复提交**且立即 poll 一次；回滚过的任务 resume 时带 stale 校验 steering；事件流记录 batch_id 与处置全程 | I2 |

```rust
// B1 骨架（伪代码）：写文件 Worker + 冻结延迟
#[test]
fn freeze_halts_writes_within_100ms() {
    let wt = Worktree::scratch();
    let w = FakeCli::writing_every(wt.file("out.log"), millis(5)).spawn();
    let res = daemon.emergency_stop("user_panic").unwrap();
    assert!(res.freeze_ms <= 100, "FREEZE 超时: {}", res.freeze_ms);
    assert!(!w.saw_write_within(ms(200)), "SIGSTOP 后仍有写入");
    assert_eq!(w.proc_state(), Stopped);
}
```

### 不变量覆盖矩阵

| 不变量 | A 组 | B 组 |
|---|---|---|
| I1 ≤100ms 冻结 | — | B1 B2 B6 B12 B13 |
| I2 现场不销毁 | A5 | B3 B4 B5 B7 B10 B14 |
| I3 崩溃可恢复无孤儿 | A8 A9 | B11 |
| I4 回滚可撤销 | — | B8 B9 |
| I5 恢复策略由原因决定 | A1~A4 A6 A7 A10 | — |

## §12 测试基建需求（随 P0.1 一并搭建）

- **MockClock**：退避/窗口/超时全部走虚拟时钟，CI 不睡真实时间
- **FakeAdapter**：可注入 stderr 模式、按脚本吐 stream-json、可卡在任意状态
- **脚本 Worker**：`loop_write.sh` / `loop_http.sh`（带已知 pgid，供 I1 与孤儿清理用）
- **pgid 沙箱**：测试进程组隔离工具，保证孤儿清理只碰自己的 pgid

---

# 第四部分 T6 审批门三栏比价界面原型

## §13 ASCII 原型与操作路径

### 13.1 主界面（三路径皆可用）

```
┌─ 计划审批 ─ 「重构 auth 模块并补回归测试」 ──────────────────────────┐
│                                                                      │
│  计划 5 步 · 涉及 12 文件 · 路由 L2（中档）· 预估 84k tokens           │
│                                                                      │
│  ⏱ 截止：明早 09:00  〔d 更改〕◄── ④ 改时间后，三栏价格/ETA/可用性重算 │
│                                                                      │
│  这笔任务怎么跑？（←→ 选择 · Enter 确认）                              │
│                                                                      │
│   ┌─ ① 立即执行 ───┐   ┌─ ② 今晚半价 ✓荐 ─┐   ┌─ ③ 批量提交 ────┐   │
│   │                │   │                  │   │                  │   │
│   │   $1.20        │   │   $0.60  省 50%  │   │   $0.60  省 50%  │   │
│   │   ~25 分钟出结果│   │   00:00 窗口起跑  │   │   聚合 10 分钟后  │   │
│   │                │   │   +25 分钟出结果  │   │   提交，明早 6 点 │   │
│   │                │   │                  │   │   前返回          │   │
│   └────────────────┘   └──────────────────┘   └──────────────────┘   │
│                                                                      │
│  条款：③ 若距截止 <2h 仍未返回 → 自动转 ①，差价如实入账本              │
│                                                                      │
│      [ Enter 确认 ]     [ p 计划详情 ]     [ Esc 取消 ]               │
└──────────────────────────────────────────────────────────────────────┘
```

### 13.2 两个置灰变体

```
变体一：③ 不可用（步骤含工具调用链，依 R5 结论）
   ┌─ ③ 批量提交 ────┐
   │ ░░░ 不可用 ░░░  │   聚焦时提示：「第 3 步需要工具调用接力，
   │   $0.60（置灰） │   批量通道不支持」→ 用户只能在 ①② 中选
   └─────────────────┘

变体二：④ 截止改成「今晚 23:00」后的联动
   ② 置灰（窗口 00:00 才开，赶不上 → 提示「赶不上窗口」）
   ③ 置灰（明早 6 点 > 截止 23:00 → 提示「会超时」）
   → 仅剩 ① 可选并高亮；推荐标记 ✓ 消失
   规则：deadline < 路径 ETA 的路径全部置灰；全灰则回退 ①
```

### 13.3 用户操作路径

| 分支 | 用户操作 | 系统响应 | 之后的界面状态 |
|---|---|---|---|
| **① 立即执行** | 聚焦①按 Enter | 任务→working，interactive 路径 | 任务卡实时显示进度叙事+成本（B1/U3）；通知按 U9 分级 |
| **② 今晚半价** | 聚焦②按 Enter | 步骤→deferred 队列（持久化） | 任务卡显示「等待 00:00 窗口」；窗口开启**自动续跑**并通知；若截止先到→自动升①＋通知「已升级，差价入账」 |
| **③ 批量提交** | 聚焦③按 Enter | 步骤→聚合窗（10min/200 项） | 叙事显示「批量已提交，预计明早 6 点前返回」；距截止 <2h 风险线触发自动升① |
| **④ 改截止** | 按 d 选新时间 | 调 §6.2 决策函数重算三栏 | 价格/ETA/置灰态/推荐标记 ✓ 联动刷新（见 13.2 变体二） |
| 查看计划 | 按 p | 展开计划 5 步与验收命令 | Esc 返回，选择状态保留 |
| 反悔 | 按 Esc | 不派活，草稿进收件箱 | — |

**实现原则（单一事实源）**：三栏的数据、置灰判定、推荐标记全部由 §6.2 决策函数同一段代码驱动——UI 展示的与调度器执行的永远一致，禁止界面试算与调度逻辑两套代码。
