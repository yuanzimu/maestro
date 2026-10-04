# R3 多轮驱动验证协议

> 版本 v1.1 · 2026-10-04（v1.0 · 2026-10-01）
> 对应 [DEV_PLAN.md](./DEV_PLAN.md) §9.2 R3——**P0 唯一技术风险项，0.15/0.16/U4/T1 的共同前提**
> 待验证命题：`claude -p` 单轮执行 + `--resume` 续接的**多轮驱动模式**（设计 §2.4/§6.1），能否替代「一次长跑」作为 Maestro 的 Worker 形态
>
> **状态：机制层已全部落地（§七），mock CLI 全场景验证通过；真实 claude CLI 端到端待 API key**

## 一、为什么这是最大风险

多轮驱动是四个特性的地基：U4 轻推（轮间注入指示）、T1 升级路径（换档重跑一轮）、U10 suspended 恢复（断点续跑）、U3 叙事（轮边界采样）。**若 resume 不能保持上下文或成本失控，这四项的地基全部要换**——所以它必须在 0.15 开工前有结论。

## 二、环境与夹具

**fixture 仓库**（`r3-fixture/`，git init 过）：

| 事实 | 植入位置 |
|---|---|
| FACT_1 | README.md 正文 |
| FACT_2 | `src/a.rs` 注释 |
| FACT_3 | `src/b.rs` 函数 doc |
| FACT_4 | 任意文件的 commit message |
| FACT_5 | 隐藏文件 `.project-notes` |

**驱动脚本**（`r3_driver.sh` 或 rust bin）：每轮执行 `claude -p "<prompt>" --output-format stream-json --verbose --max-turns 1`（flag 以本机 CLI 版本为准，**顺带记录版本与差异，喂给 R2**），落一行 JSONL：`{round, session_id, prompt, usage_in, usage_out, cache_read?, latency_ms, answer}`。

**稳定性**：每个场景跑 3 次，报告取多数结果（LLM 随机性）。

## 三、验证步骤

```
S0 基线轮   claude -p "通读 README 与 src，不要修改任何文件" → 捕获 session_id + 基线 usage
S1 浅召回   --resume <sid> -p "FACT_2 在哪个文件？只答文件名"
S2 深召回   隔 2 轮干扰任务（改写无关代码）后问 FACT_1
S3 轻推注入 --resume <sid> -p "补充指示：从现在起每句输出加前缀 [S]" → 下一轮验证前缀生效
S4 轻推持久 再下一轮（不再注入）验证前缀是否仍生效（记录衰减行为）
S5 长会话   10 轮混合任务后问第 1 轮的事实（Goal 3 轮 blocked 的上下文基础）
S6 成本曲线 全程逐轮记录 usage_in——判断 resume 是否重放全量历史
S7 异常路径 resume 不存在的 sid / 并发两次 resume 同 sid / kill -9 CLI 后 resume
```

## 四、场景矩阵与判定

| 场景 | 验证命题 | 通过标准 | 级别 | 失败出口 |
|---|---|---|---|---|
| S1 | resume 基本可用 | 3/3 召回正确 | **Block** | 0.15 不成立 → 回退「单轮长跑 + Goal blocked」形态，U4 v1 改道 P2 内置 Executor |
| S2 | 上下文跨轮保持 | 3/3 召回正确 | **Block** | 同 S1 |
| S3 | 轮间可注入指示（U4 前提） | 3/3 前缀生效 | **Block** | U4 v1 降级：轻推改为「攒到任务级重启时注入」 |
| S4 | 注入的持续性 | 记录行为（持续/衰减/丢失） | 记录 | 不 block；若衰减，0.16 投递策略改为「重要指示每轮重注」 |
| S5 | 长会话早期事实召回 | ≥2/3 | 条件 | 不过 → T5 上下文治理加「轮间摘要 steering」补偿 |
| S6 | 成本模型 | usage_in 增长曲线分类 | 条件 | 线性暴涨=重放全量历史 → 对策：a) 依赖 provider prompt cache（记录 cache_read 命中率）b) 轮间注入「上轮结论摘要」压缩历史；平台型=缓存生效，接受 |
| S7a | 坏 sid 容错 | 明确报错不崩溃 | Block | — |
| S7b | 并发 resume 同 sid | 观察行为（分叉/串行/报错） | 记录 | 若分叉 → 0.15 加**单写者锁**：同 session 同时只允许一个在途轮 |
| S7c | 崩溃后 resume | kill -9 后上一完成轮可续 | Block | 不行则 suspended 恢复降级为「checkpoint + 任务级重试」 |

## 五、报告模板（填完回 DEV_PLAN §9.2 R3 行）

```
R3 验证报告 · <日期> · claude CLI <版本>
S1 [✓/✗ 3/3]  S2 [✓/✗ 3/3]  S3 [✓/✗ 3/3]
S4 [持续/衰减/丢失]  S5 [n/3]  S6 [平台型/线性型，cache_read 率 x%]
S7a [✓]  S7b [分叉/串行/报错]  S7c [✓/✗]
结论：0.15 [go / go-with-调整项… / no-go→回退方案]
```

## 六、执行顺序建议

S0→S1→S3→S7c 先跑（Block 项里最快证伪的组合，约 20 分钟）——任何一个 ✗ 就提前止损，不用跑完 S5/S6。

## 七、实施状态（R23~R32 落地实况，2026-10-04）

多轮驱动已从「待验证命题」变为**已落地机制**。真实 CLI 的 S 场景验证仍待 API key，但机制层以 mock CLI 全量覆盖（e2e 51 项中的 15 项专测本协议）。

### 7.1 机制层落地物

| 落地物 | 位置 | 说明 |
|---|---|---|
| **maestro-rounder 二进制** | `crates/maestro-daemon/src/bin/rounder.rs` | 轮循环：每轮调底层 CLI 单轮 + `--resume` 续接；session 持久化于 `<cwd>/.maestro/session`；轮账 `.maestro/rounds.jsonl`；完成信号 `MAESTRO_DONE`；`MAESTRO_MAX_ROUNDS`（默认 20）防失控；`MAESTRO_ROUND_GAP_MS`（默认 1000）防秒回型 CLI 热循环 |
| **adapter 三件套** | `crates/maestro-daemon/src/adapter.rs` | `parse_stream_json` → RoundOutcome（session_id/usage 三桶/工具/结构化错误）；`classify_exit` → ExitClass（**Disconnect≠failed**，认证/配额 Fatal 优先，未知默认 Fatal）；`Dialect` trait（claude 默认，`MAESTRO_CLI_DIALECT` 可插拔，B7 Codex/Gemini 预留） |
| **轮间投递 API** | `crates/maestro-protocol/src/api.rs` | `TaskSteerPoll`（活 worker 轮边界拉取，仅当前 worker）· `TaskSteerAck`（消费确认，R32）· `TaskRoundReport`（轮账上报：usage 三桶/model/工具/摘要）· `TaskLedger`（查询） |
| **steering at-least-once** | `crates/maestro-daemon/src/steering.rs` | poll 取走进 inflight（未确认重投）、ack 消费确认、inflight 持久化跨重启、respawn 时 `take_all` 前置拼入 prompt、终态 `SteeringDropped` 不静默 |
| **计价闭环** | `crates/maestro-daemon/src/llm.rs` | 三桶互斥计价（input/cache_read/cache_creation）；Anthropic 写 cache 1.25x、OpenAI = 输入价；counterfactual = 同内容冷跑全价（U8 省钱口径） |

### 7.2 场景矩阵 → e2e 映射

| 场景 | e2e 测试（`crates/maestro-e2e/tests/`） | 状态 |
|---|---|---|
| S1 浅召回 | [r3_protocol.rs](../crates/maestro-e2e/tests/r3_protocol.rs) `s1_shallow_recall_via_resume` | ✅ mock |
| S2 深召回 | `s2_deep_recall_after_interference` | ✅ mock |
| S3/S4 轻推注入+持久 | `s3_s4_prefix_injection_persists` + multiround.rs `steering_injected_mid_task_changes_next_round` | ✅ mock（S4 记录到衰减行为→按预案不 block） |
| S5 长会话 | 机制层由 MAX_ROUNDS 循环覆盖；早期事实召回依赖真实 LLM | ⏳ 待真实验证 |
| S6 成本曲线 | `s6_usage_ledger_complete` + multiround.rs 断言逐轮 usage/cache_read/cache_creation 计价入账 | ✅ mock（三桶数据齐全；真实 cache 命中率待真机） |
| S7a 坏 sid | `s7a_bad_sid_errors_clearly` | ✅ mock |
| S7b 并发 resume | 未单列测试——**单写者由 daemon 结构保证**（每任务同时只有一个活 worker，stale worker 调 poll/report 返回 -403，multiround.rs `stale_worker_cannot_poll_or_report`） | ✅ 结构性解决 |
| S7c 崩溃后续接 | `s7c_crash_then_resume_last_completed_round` + multiround.rs `crash_recovery_resumes_session` | ✅ mock |

### 7.3 混沌补强（超出原协议的 e2e）

multiround.rs 另有：断连自动恢复（MockClock 推退避）、双任务并发、假完成三振出局（acceptance）、结构化过载（api_error_status 529）→ Suspended、at-least-once 重投/过期 ack -403/清空收敛、终态轻推 Dropped 不静默。

### 7.4 待真实 CLI 验证清单（拿到 API key 后）

1. S5 早期事实召回（≥2/3）
2. S6 真实 cache_read 命中率（决定「重要指示每轮重注」的性价比）
3. `--resume`/`--output-format stream-json`/`--max-turns` flag 与实际 CLI 版本的差异（喂 R2）
4. stream-json result 事件的 errors[]/api_error_status 字段实测（adapter 结构化错误路径）
