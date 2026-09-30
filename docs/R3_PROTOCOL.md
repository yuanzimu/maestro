# R3 多轮驱动验证协议

> 版本 v1.0 · 2026-10-01
> 对应 [DEV_PLAN.md](./DEV_PLAN.md) §9.2 R3——**P0 唯一技术风险项，0.15/0.16/U4/T1 的共同前提**
> 待验证命题：`claude -p` 单轮执行 + `--resume` 续接的**多轮驱动模式**（设计 §2.4/§6.1），能否替代「一次长跑」作为 Maestro 的 Worker 形态

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
