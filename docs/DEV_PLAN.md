# Maestro 开发计划

> 版本 v2.4 · 2026-10-01
> v2.0：双产品矩阵 + Token 经济第一等公民 + 轻量硬约束
> v2.1：新增 U 组体验层重大改进（8 项，源自 [UX_DEEP_DIVE.md](./UX_DEEP_DIVE.md) 第一部分）
> v2.2：以 U 组信任旅程为主线重排 P0/P1/P2——P0 三个 UX 地基（LLM client / 多轮驱动 / 事件埋点），P1 三个 Sprint（入口信心→过程掌控→回顾成长）
> v2.3：实际使用视角增补 U9~U14（第二部分）——P0 新增 suspended 语义、急停 API、doctor 命令；新增 T6 错峰执行
> v2.4：**U9~U14 细化为可执行任务**合并进各阶段；U10/T6 完成详细设计（见 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md)：三阶段急停 / git ref 快照原语 / T6 双路径调度与 Batch Manager）
>
> 依据：[HERDR_ANALYSIS.md](./HERDR_ANALYSIS.md) + [TRAE_SOLO_ANALYSIS.md](./TRAE_SOLO_ANALYSIS.md) + [UX_DEEP_DIVE.md](./UX_DEEP_DIVE.md) + [U10_T6_DESIGN.md](./U10_T6_DESIGN.md)
> 战略文档：[PROJECT_PLAN.md](./PROJECT_PLAN.md)

---

## 一、产品矩阵与受众

```
┌─────────────────────────────┐   ┌─────────────────────────────────┐
│ Maestro Desktop             │   │ Maestro Server（Linux 部署版）    │
│ Win / macOS / Linux 客户端   │   │ 「AI 化的 nginx」：Agent 编排网关 │
│                             │   │                                 │
│ 受众：个人开发者             │   │ 受众：团队 / 服务器 / CI 场景     │
│ · 装完即用，零配置           │   │ · 5 分钟部署（install.sh+systemd）│
│ · 关窗口任务照跑             │   │ · 多用户隔离 + 配额 + 审计       │
│ · 省 token 且效果不变        │   │ · 同样的省 token 引擎 + 安全层   │
└──────────────┬──────────────┘   └──────────────┬──────────────────┘
               │                                  │
               └──────────┬───────────────────────┘
                          ▼
        ┌──────────────────────────────────┐
        │  maestro-core（共享 Rust 引擎）     │
        │  任务编排 / Token 经济 / 安全 /    │
        │  Worker 运行时 / 持久化            │
        └──────────────────────────────────┘
```

**同一引擎，两种交付**：Desktop = 内嵌 daemon 的 GUI 壳；Server = headless daemon + 网关能力（认证/多租户/配额）。Desktop 未来可切换「引擎位置：本地 / 远程 Server」（同一 API 协议）。

**Server 版的 nginx 类比**：

| nginx 概念 | Maestro Server 对应 |
|---|---|
| 反向代理 | 模型分级路由（L0~L3 任务→贵贱模型） |
| upstream 池 | LLM API + 本地模型池 |
| 负载均衡 | Agent/Worker 池调度 |
| limit_req | 用户/项目 token 配额 |
| 访问控制 | 认证（API key / mTLS）+ 路径 allowlist |
| access log | 事件溯源全链路审计 |
| nginx.conf | 声明式 TOML 配置 |
| 单二进制 + systemd | 同样单二进制 + systemd unit |

**关键差异**：LiteLLM/Portkey 等 LLM Gateway 只做「请求转发」；Maestro Server 托管「整个任务生命周期」（接收任务→规划→执行→验收→交付），是 **Agent 网关**而非 API 代理。

## 二、核心价值主张与量化目标

### 用户承诺（按受众视角排序）

1. **自动省 token，效果不打折** —— 不需要用户理解任何优化细节，装完即享受
2. **轻量** —— 下载快、安装小、不卡机器
3. **易用** —— 派活就走人，回来收结果
4. **安全**（Server 版）—— 多用户各干各的，出了事可审计

### 量化验收指标（每次发版跑基准）

| 维度 | 指标 | 目标 |
|---|---|---|
| **Token 成本** | 同一基准任务集 vs「旗舰模型直连」基线的总成本 | **降 ≥ 50%** |
| **效果一致** | 基准任务验收通过率（测试/lint） vs 基线 | **≥ 基线的 98%**（统计持平） |
| **轻量 Desktop** | 安装包 / idle 内存 / 冷启动 | **< 20MB / < 150MB / < 2s** |
| **轻量 Server** | 单二进制 / 部署步骤 | **< 30MB / install.sh 一条命令** |
| **易用** | 首个任务跑通操作步骤 | **≤ 3 步**（安装→输入任务→确认计划） |
| **延迟** | blocked → 收件箱/通知 | < 1s |

### 效果基准集（benchmark-suite，P1 建立）

选 20 个真实任务（10 个 GitHub issue 修复 + 10 个重构/特性任务），每任务定义：输入、验收命令（测试/lint）、人工评分标准。**这是「效果一致」可度量的唯一方式**——没有基准，省 token 就是不可证伪的营销话术。

## 三、Token 经济层设计（v2.0 新增重点）

五个机制按节省量排序，全部对用户透明（「自动」）：

### T1. 模型分级路由（最大节省来源）

```
任务进入 → 分类器 → 分级执行 → 验收门 → (不过?) 自动升级重试
```

| 级别 | 任务类型 | 模型档位 | 相对成本 |
|---|---|---|---|
| L0 | 确定性操作（格式化、已知答案的缓存命中、纯脚本） | 无 LLM / 沙箱脚本 | ~0 |
| L1 | 简单任务（读文件、跑命令、机械重构、问答） | 小模型（Haiku/Flash/4o-mini 级） | 1x |
| L2 | 标准任务（常规编码、测试编写） | 中档模型 | ~5x |
| L3 | 复杂任务（架构设计、跨文件重构、疑难 bug） | 旗舰模型 | ~20x |

- **分类器**：规则优先（任务标签、涉及文件数、变更类型），模糊时才用小模型判断（分类本身只花几百 token）
- **升级路径**：L1 验收不过 → 自动带上下文升级 L2 → 再不过 L3 → 仍不过进收件箱人工介入。**这是「省 token 且效果一致」的核心保障**：便宜模型先试，质量由验收门兜底
- **降级学习**：同类任务连续 3 次在 L1 一次通过 → 该类任务默认 L1（项目级记忆）

### T2. Code Mode 沙箱编排（从 P2 提前到 P1）

Trae 已验证：把「N 次 LLM 往返」压缩成「1 次生成脚本 + 1 次确定性执行」。

- 沙箱选型：**rquickjs**（QuickJS 绑定，二进制增量 ~2MB）而非 rusty_v8（+30MB，违背轻量）；脚本仅编排工具调用，不需要 V8 性能
- 触发启发式（继承 Trae）：fan-out / 数据依赖 / 条件分支 / 有界循环 / 聚合 / 验证门，满足 ≥2 条才用
- 强制验证：脚本必须读回产物/查退出码才能报成功

### T3. 缓存三件套

| 缓存 | Key | 收益 |
|---|---|---|
| Prompt cache | 系统提示词 + 项目记忆前缀**稳定化**（记忆只追加不重写，保证前缀命中 Anthropic/OpenAI 的 prompt cache） | 命中时输入成本 ~0.1x |
| 结果缓存 | 内容 hash（文件分析、依赖图、符号索引、上轮 review 结论） | 重复任务 ~0 token |
| 会话缓存 | session ref（恢复不重放历史，直接 argv 续接） | 长任务省全量历史 |

### T4. Token 计量账本 + 价值呈现

- 每任务一本账：路由级别、各机制节省量、总成本 vs 基线估算
- **用户可见的报告**：任务完成通知里带「本次比直连旗舰省 63%」——省 token 是产品核心卖点，必须让用户看见

### T5. 上下文治理包（强化原 F 组）

- 状态注入代替历史重放（Trae 模式：环境/池状态/任务清单/记忆，每轮几百 token 而非全量历史）
- 子代理隔离：探索性搜索在独立上下文完成，只回传结论
- 自动压缩：接近窗口上限折叠历史

### 节省量估算（P1 基准集上校准）

| 机制 | 估计节省 | 说明 |
|---|---|---|
| T1 路由 | 30~50% | 取决于任务分布（现实多数任务是 L1/L2） |
| T2 Code Mode | 20~40% | 多步操作场景显著 |
| T3 缓存 | 10~30% | 重复/迭代场景 |
| T5 上下文治理 | 20~40% | 长会话场景 |
| **合计目标** | **≥ 50%** | 机制间有重叠，以基准实测为准 |

## 四、效果一致性保障（「省 token 不降质」的机制）

省 token 的每一个机制都配一个质量兜底，**效果一致性是设计出来的，不是碰运气**：

| 省 token 机制 | 质量兜底 |
|---|---|
| L1/L2 便宜模型 | 升级路径（验收不过自动升级重试） |
| Code Mode 压缩往返 | 强制验证门（读回产物 + 退出码，防谎报） |
| 结果缓存 | 内容 hash 作 key，未变即完全一致（确定性） |
| 上下文压缩 | 压缩只折叠过程性历史，关键决策/约束永久保留 |
| 任何机制 | **验收门是唯一 done 标准**：外部测试/lint 通过，Agent 自称完成不算数 |

人工审批门保留在：架构决策、破坏性操作、验收失败三次后——**关键节点质量 > token 节省**。

## 五、Server 版（AI nginx）安全设计

Desktop 版安全（allowlist、沙箱、预算）全部继承，Server 版追加：

| ID | 能力 | 说明 | 阶段 |
|---|---|---|---|
| S1 | 认证 | API key / mTLS；HTTP + Unix socket 双接入 | P2 |
| S2 | 多租户隔离 | 每用户/项目独立 worktree + 独立事件流；跨租户不可见 | P2 |
| S3 | 配额管理 | 用户/项目级 token 与并发配额，超额排队不拒绝 | P2 |
| S4 | 审计 | 事件溯源天然支持：谁在何时派了什么任务、动了哪些文件、花了多少 token，全链路可回放 | P0(账本)→P2(报表) |
| S5 | 部署 | install.sh + systemd unit + 单二进制；`maestro-server` 一条命令起服务 | P2 |
| S6 | 声明式配置 | 类 nginx.conf 的 TOML：路由策略/模型池/配额/allowlist | P2 |
| S7 | 出站脱敏（可选） | 代码上下文发往云端前的密钥扫描与红线脱敏（对比商业 SaaS 的道德高地） | P3 |
| S8 | 容器沙箱（可选） | 高安全场景：Worker 进程跑在 Docker/bwrap 内 | P3 |

## 六、修订后特性总清单

> 完整清单沿用 v1.0 的 A~H 编号体系 + 新增 T（Token 经济）、S（Server 安全）两组。此处只列**变化**，未列出者维持 v1.0 定义。

### 新增：T 组 Token 经济层（第一等公民）

| ID | 特性 | 阶段 |
|---|---|---|
| T1 | 模型分级路由 L0~L3 + 规则分类器 + 升级路径 + 降级学习 | P0 框架 / P1 落地 |
| T2 | Code Mode 沙箱（rquickjs，轻量选型） | P1（从 P2 提前） |
| T3 | 缓存三件套（prompt cache 前缀稳定化 / 结果缓存 / 会话缓存） | P1 |
| T4 | Token 计量账本 + 省 token 报告（用户可见） | P0 账本 / P1 报告 |
| T5 | 上下文治理包（状态注入 / 子代理隔离 / 自动压缩） | P1（强化原 F 组） |
| T6 | 错峰执行：可延后任务自动排入 provider 折扣时段 / Batch API（OpenAI Batch -50%、Gemini off-peak -50%）；审批门预估标注「半价时段省 X」 | P2 |

### 新增：S 组 Server 安全层

S1~S8 见上表，P2 为主。

### 新增：U 组 体验层重大改进（v2.1，源自 [UX_DEEP_DIVE.md](./UX_DEEP_DIVE.md)）

> 核心洞察：自主 Agent 产品本质是「信任管理」——派活前的信心 / 跑动中的掌控感 / 结果后的可验证与可恢复。U 组全部构建在已有底座（事件溯源/账本/记忆/审批门/session ref）之上。

| ID | 特性 | 阶段 | 一句话 |
|---|---|---|---|
| U1 | 全渠道任务入口：全局快捷输入 `Alt+M` + 剪贴板智能识别；浏览器扩展/GitHub webhook/邮件入口 | P1 快捷输入 / P2 扩展+webhook | 派活摩擦决定使用频率 |
| U2 | 零配置首跑：探测已有 CLI 配置一键导入 + 示例任务 + 项目名片（轻量索引） | P1 | 前 10 分钟决定留存 |
| U3 | 里程碑叙事：事件流经 L1 小模型压缩为人类可读进度 + ETA + 实时成本 | P1 | 「working...」对信任零贡献 |
| U4 | **飞行中转向**：轻推（步骤间生效）→ 暂停对话 → 逐步接管 三档干预 | P1 轻推 / P2 完整（依赖内置 Executor） | ⭐ 最大交互创新：随时可握方向盘 |
| U5 | 验收面：结果卡（摘要+验收状态+花费+节省）+ 变更叙事 + hunk 级部分接受（拒绝理由回转 Worker） | P1 结果卡+叙事 / P1.5 hunk 级 | 拒绝不是终点，是下一轮输入 |
| U6 | 失败体验：断点续跑（DAG 只重跑失败节点）+ 人话检讨 + 失败即记忆 | P1 断点续跑 / P2 检讨报告 | 用户怕的不是失败，是全部重来 |
| U7 | 教学闭环：👍/👎+理由落记忆 + 周学习简报 + 偏好可查可改 | P1 反馈闭环 / P2 简报 | 产品变好且用户看得见 |
| U8 | 成本护栏：审批门附带成本预估 + 预算三档（宽松/标准/严格）+ 月度软顶 | P1 | 解决「敢不敢放手」 |
| U9 | 多任务噪音治理：通知分级（仅 blocked/done/异常打扰）+ digest 批处理 + 任务级静音 + 深链直达 | P1 Sprint B；priority 埋点 P0 | 通知疲劳会让用户关通知，然后错过真正的 blocked |
| U10 | 危机安全网：全局急停（挂起保现场非杀死）+ Checkpoint 时光机（任务级 git 快照）+ 禁区（正在编辑的文件 hands-off） | 急停 API + suspended 语义 **P0**；UI/checkpoint P1 Sprint B；禁区 P1.5 | 出事的 8 分钟里需要的是刹车和后悔药 |
| U11 | 基础设施现实：suspended ≠ failed（断连自动续跑）+ provider 故障切换（可用性轴）+ key 生命周期（过期提醒/配额预警） | suspended 语义 **P0**；failover + key P1 | 合盖睡眠不该判死刑；Anthropic 挂了还有 OpenAI |
| U12 | 人的节律：隔夜/离开摘要 + 定时任务（cron）+ 错峰执行（T6） | P2 | 「昨晚干成了什么卡在哪花了多少」+ 半价时段省真金白银 |
| U13 | maestro doctor：一条命令自检（daemon/socket/CLI/key）+ 装机自检 + 错误 actionable 化 | doctor 核心 **P0 CLI**；完整版 P1 | 第一天的失败是留存问题 |
| U14 | 重复劳动规模化：任务模板（参数化）+ 任务即文件（task.yaml 进 git 可分享）+ 跨项目批量 | P2 模板+任务即文件（依赖 C7 协议冻结）；批量 P3 | 8 个仓库同一条命令不该做 8 遍 |

### U 组依赖关系与排期逻辑（v2.2 重排的依据）

**三个隐藏依赖被显式化**（原 v2.1 把 U 组当纯呈现层，低估了地基需求）：

1. **daemon 内置 LLM client**：U3 叙事压缩 / U8 成本预估 / U6 检讨 / T1 模糊分类，都需要 daemon 自己调 LLM（不经外部 CLI）→ **P0 必须有**
2. **多轮驱动 Worker 模式**：Maestro 持有任务循环（CLI 单轮执行 + `--resume` 续接），而非一次性甩给 CLI → U4 v1 轻推**不用等 P2 内置 Executor**，P1 即可；同时它是 T1 升级重试的天然载体（重试=我们的循环里换档位再跑一轮）→ **P0 定形态**
3. **UX 事件埋点**：叙事快照 / 反馈 / steering 消息 / 断点状态必须在 P0 就进事件 schema——P1 的所有体验呈现都要求「数据已在流」，否则 P1 无米下锅 → **P0 埋点**

```
依赖图（→ = 依赖；【P0】= v2.2 新增地基）

【P0】daemon 内置 LLM client ──┬─ T1 模糊分类器
                               ├─ U3 里程碑叙事（P1-B）
                               ├─ U8 成本预估（P1-A）
                               └─ U6 人话检讨（P2）/ U7 周简报（P2）
【P0】多轮驱动 Worker + steering 消息队列 ─┬─ U4 v1 轻推（P1-B）
                                          └─ T1 升级路径重试轮
【P0】UX 事件埋点（叙事/反馈/转向/断点）── U3/U5/U6/U7 的 P1 呈现

P1 Sprint A（入口与信心）：U1 v1 快捷输入（仅需 P0 API）· U2 首跑（仅需任务系统）
                          · U8 护栏（需 T1 路由 + Planner）
P1 Sprint B（过程掌控）：U3 叙事（需 LLM client）· U4 v1 轻推（需多轮驱动）
P1 Sprint C（回顾与成长）：U5 结果卡（需 G6 验收门状态 + T4 账本）
                          · U5 hunk 级（需 worktree〔Sprint B〕+ 任务回流）
                          · U6 v1 断点续跑（需子任务状态持久化〔P0 session ref〕）
                          · U7 v1 反馈（需 F1 记忆，Sprint C 内先行）
P2：U4 完整版（需内置 Executor D2）· U6 v2 检讨（需失败事件库）
    · U7 v2 简报（需数周数据累积）· U1 v2 扩展/webhook（需 Server S1 + 协议冻结 A7）
```

### 变化：原 v1.0 特性调整（v2.1 增补，v2.2 沿用）

| ID | 特性 | v1.0 | v2.0 | 理由 |
|---|---|---|---|---|
| D3/D4 | Code Mode + 编排启发式 | P2 | **P1** | 省 token 是核心卖点，必须进 MVP |
| D2 | 内置 Executor | P2（保底件） | **P2（体验关键件）** | U4 飞行中转向的完整实现载体（mid-run 干预需要拥有 agent 循环）；优先级从「保底」升级为「差异化关键」 |
| B8/B9/B10 | PTY 兼容层 + 屏幕检测引擎 + manifest 热更新 | P2 | **P3（可选）** | 「轻量」硬约束：砍掉 VT 检测复杂度；headless 已覆盖主流 agent；文档明确支持列表即可 |
| F3/F4/F5 | 压缩/子代理/环境快照 | P2 | **P1（并入 T5）** | 上下文治理=省 token 机制，进 MVP |
| B13 | 远程 SSH Worker | P3 | **P3（语义升级）** | 升级为「Desktop 连远程 Server 引擎」，同一 API 协议 |
| C6 | 任务 DAG | P2 保持 | P2 保持 | — |
| G6 | 验收门 | P2 | **P0 v0（退出码）/ P2 完整（测试+lint）/ 语义扩大：状态直接呈现在结果卡上（U5）** | 效果一致性的地基，提前；人机共用同一验证标准 |

### 桌面易用性原则（贯穿所有 GUI 工作）

- 零配置默认：默认模型策略已优化，一键可换
- 渐进披露：普通用户看不到 DAG/路由细节；高级面板可展开
- 通知驱动：派活→关窗口→blocked/done 系统通知
- 首跑 ≤ 3 步：安装 → 输入任务 → 确认计划

## 七、修订后路线图

### P0 核心引擎（2~3 周）—— CLI 验证 + UX 地基三项

目标：跑通底座 + Token 计量路由框架 + **v2.2 新增的三个 UX 地基**（没有它们，P1 的 U 组全部悬空）。

| # | 任务 | 特性 | 参考 |
|---|---|---|---|
| 0.1 | cargo workspace：maestro-{protocol,daemon,client,cli} | — | — |
| 0.2 | protocol：数据模型 + 10 个 API 方法 schema + 事件类型（**含 UX 埋点：叙事快照/反馈/steering 消息/断点状态/通知 priority 字段**；**含 `server.emergency_stop`/`server.resume_all`/`task.pause|resume|cancel` API 与 SuspendEvent/ResumeEvent/EmergencyStopped 事件**，定义见 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md) §2.3/§3.2） | A5 + 埋点 + 急停 | herdr api/schema.rs |
| 0.3 | 守护进程：setsid + 单线程事件循环 + 每连接线程 + mpsc | A1/A3 | herdr headless.rs |
| 0.4 | 双 socket + 双车道 + stale 探活 | A2/A4/A9 | herdr ipc.rs |
| 0.5 | Worker 运行时 v0：headless 子进程 + 信号三级升级 + 环境三元组 | B1/B3/B4 | herdr pane.rs |
| 0.6 | Claude headless 适配器：stream-json → 状态机（**含 suspended 态：断连/暂停 ≠ failed**；stderr 模式→NetworkLost 映射，设计 §2.4） | B2/B6/U11 | 新写 |
| 0.7 | 任务队列 v0 + Goal 3 轮 blocked | C5/C6 | Trae Goal |
| 0.8 | **模型路由框架**：分级 trait + 规则分类器 + 升级路径骨架 | T1 | nginx upstream 思路 |
| 0.9 | **Token 计量账本**：每任务每 Worker 的用量记录（event 流内） | T4 | — |
| 0.10 | **验收门 v0**：命令退出码 + 读回校验 | G6 | Trae 验证门 |
| 0.11 | 安全基线：allowlist + session ID 校验 | E2/E4 | Trae hooks_env |
| 0.12 | 持久化：事件溯源 + 去抖 + 原子写 + 备份链 + argv 重放恢复 | — | herdr persist |
| 0.13 | maestro-cli 最小集（status/task/worker/inbox + **doctor 核心命令**：daemon/socket/版本/CLI 探测自检） | U13 | — |
| 0.14 | **【v2.2】daemon 内置 LLM client**：OpenAI-compatible 统一层，daemon 直调小模型（T1 分类 / U3 叙事 / U8 预估共用） | T1/U3/U8 地基 | — |
| 0.15 | **【v2.2】多轮驱动 Worker 模式**：Maestro 持有任务循环（CLI 单轮 + `--resume` 续接），替代一次性长跑 | B2 扩展 / U4·T1 地基 | herdr agent_resume |
| 0.16 | **【v2.2】steering 消息队列**：轻推消息持久化 + 多轮驱动的步骤间投递点 | U4 地基 | — |
| 0.17 | **【v2.4】suspended 状态机**（设计 §2）：SuspendReason 枚举 + 自动恢复矩阵（NetworkLost/ProviderOutage/SystemSleep 自动，退避 30s→5m，10 次后升 blocked）+ 恢复走 `--resume` 续接 + **孤儿进程清理**（PID 文件 + daemon 重启扫描，绝不收养直接杀，设计 §2.5）**；测试用例组 A（A1~A10）见设计 §10** | U10/U11 | 设计 §2/§10 |
| 0.18 | **【v2.4】emergency_stop 三阶段**（设计 §3）：FREEZE（freeze 派发 + SIGSTOP 进程组，<100ms）→ SNAPSHOT（逐 worktree checkpoint，<2s/个）→ DECIDE（resume_all/cancel/rollback+resume 由用户）；daemon 退出兜底杀干净**；测试用例组 B（B1~B14）见设计 §11** | U10 | 设计 §3/§11 |
| 0.19 | **【v2.4】checkpoint 快照原语**（设计 §4.3/4.4）：capture=git add -A→write-tree→commit-tree→update-ref→reset（零打扰 <200ms）；restore=pre-rollback 自动快照→reset --hard→clean -fd；触发点=baseline/每轮开始/急停/手动；保留策略（永久 baseline+验收点，滚动 50 个/24h，任务完 GC） | U10 | 设计 §4 |

**P0 验收**：kill -9 恢复无损（**含 steering 队列不丢**）/ 状态判定 100% / blocked<1s / 账本能回答「这个任务花了多少 token」/ **多轮驱动下任务可被中途注入指示并影响下一轮** / **断连后任务呈 suspended 而非 failed，恢复自动续跑** / **emergency_stop：FREEZE<100ms、无孤儿进程残留（kill -9 daemon 后重启验证）、checkpoint 可恢复（含 pre-rollback 安全垫）——不变量 I1~I5 以设计 §3.3/§10-11 用例组 A/B 全绿为准** / **doctor 一条命令定位常见故障**。

#### P0 实施进度（2026-10-01，20 轮迭代 R1~R20）

| 状态 | 任务 | 说明 |
|---|---|---|
| ✅ | 0.1 / 0.2 / 0.3 / 0.4 / 0.5 | 六 crate workspace（+testkit/e2e）、protocol schema、单线程 Core、双 socket IPC、Worker 运行时（stdout 落文件防 SIGSTOP 死锁、PID+start_time 双因子、三级升级杀） |
| ✅ | 0.7 任务队列 + Goal 3 轮 | 并发槽位（默认 4）+ workdir 互斥 + FIFO 补位（queue_seq 消歧）+ MAX_QUEUE_DEPTH=100 防风暴（R12 调研落地：Buildkite/gitlab-runner 语义） |
| ✅ | 0.10 验收门 v0 | 读回校验（SipHash 内容指纹，修同长碰撞）+ 3 振出局 blocked(AcceptanceFailed) + TaskRequeued 重试路径 + **失败差异反思回喂**（aider 模式，steering 注入） |
| ✅ | 0.12 持久化 | SQLite WAL 事件溯源 + kill -9 重放恢复 + 恢复时自动重排退避调度（R11 审计补的缺口） |
| ✅ | 0.13 CLI | status/task/worker/inbox/stop/resume/events/doctor/shutdown + 表格化列表 + 收件箱行动建议（R19） |
| ✅ | 0.14 LLM client | OpenAI 兼容（ureq 阻塞式）+ 牌价表（含 cache 读价，R24 修 haiku 输出价 40→400）+ **cache 感知计价闭环**（rounder 轮账 → priced_usage_entry → actual/counterfactual cents）；T1/U3/U8 消费方在 P1 接入 |
| ✅ | 0.15 多轮驱动 | **R23 机制层完成**：maestro-rounder 二进制（轮循环 + `--resume` 续接 + session 持久化 + DONE 信号 + 轮间隔防热循环）+ TaskSteerPoll 轮间投递（仅当前 worker 可拉）+ **TaskRoundReport 轮账入账**（usage→LedgerEntry，token 计量闭环）+ 2 e2e（U4 轻推改下一轮输出 / kill -9 跨实例续接 session）；**真实 claude CLI 验证待 API key** |
| ✅ | 0.16 steering 队列 | jsonl 持久化 + flush/hold + kill -9 不丢 + 重试轮回喂投递；**R23 投递语义修正**：活 worker 留队轮边界 poll 取走、respawn 路径前置注入 prompt（真投递）、终态 SteeringDropped 不静默 |
| ✅ | 0.17 suspended 状态机 | 七值枚举 + 自动恢复矩阵（30s→5m 退避，10 次升 blocked）+ 孤儿清理（绝不收养） |
| ✅ | 0.18 emergency_stop | FREEZE→SNAPSHOT→DECIDE 三阶段 + 竞态修复（过期退出误杀/死 worker 假恢复，R14） |
| ✅ | 0.19 checkpoint | git plumbing 零打扰 + pre-rollback 安全垫 + pinned（baseline/验收点）+ 滚动 GC |
| ⬜ | 0.6 / 0.8 / 0.9 / 0.11 | Claude headless 适配器（待 R2/R3 真实 CLI）、模型路由框架、账本 UI 呈现、allowlist 安全基线 |

**测试**：118 项全绿（R1~R23，daemon 单测 52 + e2e 44 + protocol 12 + testkit 8 + CLI 3，clippy 零告警）。里程碑见 git log。**已知缺陷（v0 接受）**见 [acceptance.rs](../crates/maestro-daemon/src/acceptance.rs) 模块头：无产物型任务误判（0.15 结构化验收断言接管）、workdir 外产物不可见。

**R23 调试战果（dash vfork 之谜）**：sigstop/b1 测试 ~33% flake 的根因不是「高负载 D 态」而是 **dash 对单条外部命令用 vfork**——父进程阻塞在不可中断的 vfork-wait 直到子进程 exec；STOP 恰落在 vfork 窗口时父进程停在 D（子进程 T），SIGSTOP 已投递且回用户态即生效，但 /proc 主 pid 永不显示 T。测试断言改为 T|D 双态（= 组不再执行用户代码）。产品语义（I1 冻结 = kill 返回）不受影响。

### P1 Desktop MVP（4~6 周）—— 按信任旅程组织为三个 Sprint

目标：跨平台客户端（Win/macOS/Linux）+ Token 经济全量落地 + U 组信任体验。**v2.2 重组：不再按技术模块排列，而是按用户旅程分 Sprint**——每个 Sprint 出口是一个用户可感知的信任断言。

#### Sprint A 入口与信心（~2 周）—— 让用户敢派活

| # | 任务 | 特性 | 依赖 |
|---|---|---|---|
| A1 | Tauri 壳（三平台构建）+ 自动拉起 daemon + 事件流接入 | A8/A6 | P0 全部 |
| A2 | 零配置首跑：CLI 配置探测一键导入（`~/.claude` 等）+ 示例任务 + 项目名片（轻量索引） | U2 | 任务系统 |
| A3 | 全局快捷输入 `Alt+M` + 剪贴板智能识别（错误日志/URL/截图/代码片段） | U1 v1 | daemon API |
| A4 | Planner v1：状态注入 + Plan 审批门 + **成本预估**（档位 × 计划规模）+ Todo 状态机 | C1/C2/C7/U8 | T1 路由 + LLM client（0.14） |
| A5 | 权限引擎（auto/ask/deny）+ Worker 预算 + 预算三档（宽松/标准/严格）+ 月度软顶 | E1/E3/U8 | — |
| A6 | 模型路由落地：L1 小模型池 + 升级路径实测（多轮驱动重试轮）+ 降级学习 | T1 | 0.8/0.14/0.15 |
| A7 | Provider 可用性 failover：circuit breaker（连续 N 次 429/5xx 打开→半开探测）+ 自动切备用 provider 重试 + 切换事件进事件流（U3 叙事「已切到 OpenAI 继续」） | U11 | 0.17 suspended |
| A8 | key 生命周期：key vault（加密存储）+ 定期探测有效性 + 过期前 7 天通知 + 配额将尽 actionable 通知（深链直达设置页） | U11 | A1 设置页 |
| A9 | doctor 完整版：装机首启自动跑 + 检查项扩展（key/配额/worktree/git 可用性/磁盘）+ 错误 actionable 化（所有基础设施错误带动作按钮） | U13 | 0.13 doctor 核心 |

**Sprint A 出口**：新用户 3 步内（安装→输任务→确认带成本预估的计划）跑通首个任务。

#### Sprint B 过程掌控（~2 周）—— 让用户敢离开

| # | 任务 | 特性 | 依赖 |
|---|---|---|---|
| B1 | 里程碑叙事：事件流经 L1 压缩为人类可读进度 + ETA（历史时长统计）+ 实时成本 | U3 | 0.14 LLM client + 埋点 |
| B2 | **轻推 steering v1**：运行中任务接收补充指示，下一轮生效（UI 入口 + 事件回显） | U4 v1 | 0.15/0.16 |
| B3 | Blocked 收件箱 + 桌面通知 | G1/G5 | P0 inbox |
| B4 | **通知治理 v1（U9）**：分级——仅 blocked/done/failed/预算触顶 弹系统通知，其余进活动流；digest 批处理（低优先级 30 分钟聚合一条）；任务级静音（「做完叫我」）；深链直达（点通知直接落到该 blocked 的决策界面） | U9 | P0 priority 埋点 + B3 |
| B5 | 指挥台 UI：Worker 状态栏 + 任务列表 + 进度叙事呈现 + DAG 可视化 v1 | G4/U3 呈现 | B1 |
| B6 | Code Mode 沙箱（rquickjs）+ 编排启发式 | T2/D4 | — |
| B7 | Codex/Gemini 适配器 + worktree 隔离 | B6/B11 | 0.6 适配器框架 |
| B8 | 双模式切换：专注模式（单 Agent 深入）/ 并行模式（多 Worker 分头），Planner 自动判断或用户指定 | G2 | B7 worktree |
| B9 | **急停/恢复 UI（U10）**：全局急停按钮（触达 0.18 三阶段）+ 挂起态任务列表（现场完整明示）+ 逐任务 resume/cancel/rollback 操作流 + steering flush|hold 选择 | U10 | 0.18/0.19 |
| B10 | **Checkpoint 时光机 UI（U10）**：任务级时间线（checkpoint × U3 叙事标签联合）+ 节点 diff（vs 上一 cp / vs 当前）+ 恢复到此点（带 pre-rollback 确认）+ 星标保留 | U10 | 0.19 + B1 叙事 |

**Sprint B 出口**：派活后关窗口，凭叙事与通知掌握全程；发现跑偏当场轻推拉回。

#### Sprint C 回顾与成长（~2 周）—— 让用户敢派更重要的活

| # | 任务 | 特性 | 依赖 |
|---|---|---|---|
| C1 | 结果卡：一句话摘要 + 验收门状态（测试 12/12）+ 花费 + 节省 + 变更叙事 | U5 | G6 状态 + T4 账本 |
| C2 | inline diff + **hunk 级部分接受**：拒绝的 hunk 附理由自动转修正任务回 Worker | U5/G3 | B7 worktree |
| C3 | 断点续跑 v1：失败后只重跑失败子任务（已完成部分保留） | U6 | P0 session ref |
| C4 | 三级记忆 + 记忆优先 + 上下文治理（状态注入/子代理隔离/自动压缩） | F1/F2/T5 | — |
| C5 | 反馈闭环：结果卡 👍/👎 + 理由落项目记忆（失败原因亦入记忆） | U7 v1 | C4 |
| C6 | 缓存三件套（prompt 前缀稳定化/结果缓存/会话缓存）+ 省 token 报告（通知带节省数据 + 月度汇总） | T3/T4 | — |
| C7 | 效果基准集（20 任务）+ CI 每次发版跑 + 协议双轨制冻结（对外 API generation） | A7 | — |

**Sprint C 出口**：基准集证明 token 降 ≥50% 且验收通过率 ≥98%；结果卡 5 秒内完成扫视；GitHub 开源发布。

**P1 总验收**：三平台包各 <20MB / Sprint A·B·C 三个出口断言全部成立。

### P2 Server 版「AI nginx」+ 完整协作（+1 个月）

目标：同一引擎服务化（认证、多租户、配额、部署体验）+ U 组完整形态。

| # | 任务 | 特性 | 依赖 |
|---|---|---|---|
| 2.1 | 认证接入层（API key/mTLS）+ HTTP API（复用同一 schema） | S1 | — |
| 2.2 | 多租户隔离（每用户/项目独立 worktree + 事件流） | S2 | P1-B7 worktree |
| 2.3 | 配额管理（token/并发，超额排队） | S3 | 2.1 |
| 2.4 | 部署：install.sh + systemd + 单二进制 <30MB | S5 | — |
| 2.5 | 声明式配置（TOML：路由/模型池/配额/allowlist） | S6 | 2.3 |
| 2.6 | 审计报表（事件流 → 谁何时干了什么花了多少） | S4 | P0 账本 |
| 2.7 | Worker 互操作（coder→reviewer 链） | B6 扩展 | 2.8 DAG |
| 2.8 | 完整 DAG 调度 | C6 | P0 队列 v0 |
| 2.9 | Spec 模式三工件 + 独立 Review 门 | C3/C4 | — |
| 2.10 | 验收门完整版（测试+lint+安全扫描） | G6 | P0 验收门 v0 |
| 2.11 | **内置 Executor（体验关键件）** + **U4 完整飞行中转向**（暂停对话/逐步接管，需拥有 agent 循环） | D2/U4 | 2.8 |
| 2.12 | Worktree 自动合并编排 + 冲突进收件箱 | B12 | 2.2 |
| 2.13 | 浏览器扩展 + GitHub webhook 入口 + 邮件任务 | U1 v2 | 2.1 + P1-C7 协议冻结 |
| 2.14 | 失败人话检讨（LLM client 生成三候选出路）+ 周学习简报 | U6 v2/U7 v2 | 0.14 + 数周数据累积 |
| 2.15 | **离开/隔夜摘要（U12）**：检测用户离开 N 小时后归来 → 首屏摘要卡（完成清单+diff 就绪待审+卡点+花费+节省，由 0.14 LLM client 从事件流生成） | U12 | T4 账本 + B1 叙事 |
| 2.16 | **定时任务（U12）**：cron 语义（`每周一 9 点：更新依赖跑全量测试`）+ 持久化调度器 + 报告进收件箱 + Server 版 systemd timer 对齐 | U12 | — |
| 2.17 | **T6 错峰执行（设计 §5-9）**：eligibility（步骤级 deferrable 属性 + 硬约束「deadline 前无用户交互依赖」）+ 决策函数（三路径比价：interactive/offpeak/batch，审批门三栏呈现，**界面原型与操作路径见设计 §13**）+ off-peak 窗口管理（deferred 队列 + 窗口唤醒 + 笔记本休眠错窗重估） | U12/T6 | 设计 §6/§13 + A4 审批门 |
| 2.18 | **T6 Batch Manager（设计 §7）**：聚合窗（10min/200 项/8MB）→ 提交（OpenAI `/v1/batches` + Anthropic batches 统一 trait）→ 轮询（5min）→ 结果逐 item 回写（验收门照常）+ 失败重试≤2 → interactive 兜底 + deadline<2h 自动升级 + 账本记 path/discount/counterfactual（周报如实含升级多花的钱）+ **在途 batch 急停处置（标记+轮询状态机/间隔阈值/最终失败回滚）见设计 §7.3** | T6 | 设计 §7/§7.3 + 2.17 |
| 2.19 | **任务模板（U14）**：参数化 `{repo}`/`{package}` + 模板库 UI + 从历史任务一键存模板 | U14 | C7 协议冻结 |
| 2.20 | **任务即文件（U14）**：task.yaml 导入导出（schema 与 2.19 模板同构）+ 进 git 可审查可版本化 + 团队共享目录 | U14 | 2.19 |
| 2.21 | **禁区 hands-off（U10 补完）**：编辑器插件/文件监听把「正在编辑的文件」实时声明为动态 deny（E2 allowlist 扩展）+ Worker 触碰即暂停询问 + IDE 集成（VS Code/JetBrains 插件） | U10 禁区 | 0.11 allowlist + P1-B9 |

**P2 验收**：5 分钟从零部署到首个任务完成 / 租户间零泄漏（渗透测试）/ 配额硬顶生效 / **运行中任务可暂停对话并带修正继续** / **离开一夜归来首屏摘要准确（完成/卡点/花费三问可答）** / **T6 端到端：一个可延后任务经审批门三栏比价 → 半价路径执行 → 验收门照常通过 → 账本与周报如实呈现（含升级多花的钱）** / **编辑中的文件被 Worker 触碰时暂停询问** / Server 无 GUI 依赖纯 headless。

### P3 生态（持续）

Worker 市场（G7）/ 插件系统（H1）/ Web 客户端（H2，含移动端查看+审批，经 Server）/ MCP（H3）/ live handoff（A11）/ 多会话（A10）/ 出站脱敏（S7）/ 容器沙箱（S8）/ 浏览器技能（D6）/ **Desktop 连远程 Server 引擎**（B13 语义升级）/ PTY 兼容层（B8-B10，按社区需求决定是否做）/ 三视图评估（对话/看板/CLI 同源呈现）/ **跨项目批量任务**（U14，「在所有 service 仓库跑安全扫描」）。

## 八、决策记录（v1→v2，含 v2.2 重排与 v2.3 增补）

| # | 决策 | 理由 |
|---|---|---|
| 1 | Token 经济升格为独立层（T 组），T1/T4 提前进 P0 | 没有「账本+路由框架」就无法度量与优化；省 token 是用户核心诉求 |
| 2 | Code Mode 从 P2 提前到 P1，沙箱选 rquickjs 而非 rusty_v8 | 省 token 最确定的机制必须进 MVP；+30MB 的 V8 违背轻量（<20MB 包） |
| 3 | PTY 兼容层 + 屏幕检测从 P2 降级到 P3 可选 | 轻量优先：headless 已覆盖主流 agent；VT 检测是 herdr 最大的复杂度来源 |
| 4 | 新增 Server 版产品线（S 组），P2 交付 | 「AI nginx」与 daemon 架构同构，复用 maestro-core；打开团队市场 |
| 5 | 建立效果基准集（20 任务 + CI） | 「效果一致」必须可证伪，否则是营销话术 |
| 6 | 验收门从 P2 提前到 P0 v0 | 效果一致性的地基；升级路径依赖它 |
| 7 | Desktop/Server 同协议，未来 Desktop 可连远程引擎 | 一套 API 两种交付，架构故事完整 |
| 8 | **【v2.2】daemon 内置 LLM client 进 P0（0.14）** | U3/U8/U6/U7 与 T1 分类器的共同隐藏依赖，晚于 P0 会让 P1 Sprint A/B 悬空 |
| 9 | **【v2.2】多轮驱动 Worker 模式进 P0（0.15）** | U4 轻推因此不必等 P2 内置 Executor，P1 Sprint B 即可交付；且是 T1 升级重试的天然载体 |
| 10 | **【v2.2】UX 事件埋点进 P0 schema（0.2）** | P1 所有体验呈现要求「数据已在流」；P0 不埋点 = P1 无米下锅 |
| 11 | **【v2.2】P1 按信任旅程重组为 3 个 Sprint** | 每个 Sprint 出口是用户可感知的信任断言（敢派活→敢离开→敢派重要的活），而非技术模块完成度 |
| 12 | **【v2.2】steering 队列持久化纳入 P0 验收（kill -9 不丢）** | 轻推是承诺，消息丢了就是「说了没人听」，比不做更伤信任 |
| 13 | **【v2.3】P0 状态机新增 `suspended` 态（断连 ≠ failed）** | 错误的标签引发错误的用户行为（删任务重跑，再烧一遍 token）；状态语义是 schema 级决策，P0 定对 |
| 14 | **【v2.3】`server.emergency_stop` + `task.cancel` 进 P0 API schema** | API 缺席则 P1 的急停 UI 是空话，属 schema regret；急停=挂起保现场而非杀死 |
| 15 | **【v2.3】doctor 核心命令进 P0 CLI** | alpha 用户的自调试工具，第一天失败=流失；doctor 把支持成本降一个量级 |
| 16 | **【v2.3】新增 T6 错峰执行（P2）** | 半价时段/Batch API 是唯一零质量代价的省 token 机制——不降模型档位只挪时间，无需效果兜底 |
| 17 | **【v2.3】通知分级埋点（priority 字段）进 P0 事件 schema** | 通知疲劳是 day-2 留存头号杀手；分级/digest 依赖事件携带优先级，事后补=全量迁移 |
| 18 | **【v2.4】急停采用 SIGSTOP 三阶段（FREEZE→SNAPSHOT→DECIDE）而非直接杀** | 直接杀丢失进行中轮次且现场残缺；SIGSTOP 瞬时静止（<100ms）保证快照干净，「杀不杀」交还用户决定（设计 §3.1） |
| 19 | **【v2.4】checkpoint 用 git ref 原语（write-tree/commit-tree/update-ref），不用 stash/branch** | 零打扰（不动 HEAD 不污染分支）、结构化 message 可与 U3 叙事对齐、内容级去重存储成本低（设计 §4.1-4.3） |
| 20 | **【v2.4】daemon 重启绝不收养 Worker，一律杀 + suspended(DaemonCrash)** | session ref + checkpoint 已保证可恢复，收养的复杂度（重建流/PTY）不值；孤儿清理是 I3 不变量（设计 §2.5） |
| 21 | **【v2.4】T6 双路径按「交互性」分流：非交互 fan-out 走 Batch，可延迟交互任务走 off-peak** | Batch 的 24h SLA 只适合可并行独立请求；off-peak 保持交互性只是挪时间——eligibility 判断错会让 deadline 全违约（设计 §5） |
| 22 | **【v2.4】T6 的省 token 报告只对比「同模型同路径牌价」，升级多花的钱如实计入** | 防「错峰报告」夸大——成本报告的诚实性是 T4 账本的信用底线（设计 §8-9） |

## 九、下一步

### 9.1 开工序列

1. P0.1 cargo workspace 骨架（四 crate + CI）+ **测试基建**（MockClock/FakeAdapter/脚本 Worker/pgid 沙箱，设计 §12——用例组 A/B 全依赖它）——**落地指南（目录结构/依赖清单/CI/自查清单）见 [P0_SETUP.md](./P0_SETUP.md)**
2. P0.2 protocol schema 契约先行（10 API + 事件类型 + token 账本事件 + UX 埋点 + **suspend/resume/emergency 事件与 API，按 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md) §2.3/§3.2**）

### 9.2 调研任务清单（R1~R5）

| # | 任务 | 关键问题 | 方法 | 交付物 | 影响决策 | 完成门 |
|---|---|---|---|---|---|---|
| R1 | rquickjs 嵌入摸底 | 嵌入 API/异步桥/二进制增量实测是否 ~2MB | 最小示例嵌入 + 测体积 | 技术备忘 + go/no-go（备选：wasmtime wasi-mini） | P1-B6 Code Mode 选型 | P1 Sprint B 开工前 |
| R2 | 小模型 headless CLI 可用性 | Haiku/Flash/4o-mini 经各家 CLI headless 调用的支持度与 stream-json 格式差异 | 各 CLI 实测 | 适配器矩阵表 → 回填 0.6/B7 适配器计划 | P1-A6 模型路由 L1 池 | P1 Sprint A 开工前 |
| R3 | 多轮驱动验证 | `claude -p` 单轮 + `--resume` 续接：上下文保持度？每轮成本递增？轮间可注入 steering？并发 resume 行为？ | **验证协议见 [R3_PROTOCOL.md](./R3_PROTOCOL.md)**：9 场景矩阵（Block 项/记录项分级）+ 提前止损顺序（S1→S3→S7c 先跑）+ 报告模板 | 验证报告 → 0.15 形态 go/no-go/调整；失败出口已定义（回退单轮长跑/单写者锁等） | P0 0.15/0.16（**P0 内最优先**） | P0.15 开工前。**2026-10-01 进展：机制层 harness 完成**（testkit::r3 Driver + mock CLI，S1/S2/S3+S4/S6/S7a/S7c 6 场景绿；真实 CLI 协议待 API key，Driver 参数已兼容） |
| R4 | **Gemini off-peak 触发机制**（设计 §5 前提） | ① 折扣是自动计费（按时窗）还是需参数/API 开关？② 窗口定义与单位（UTC？本地？滚动？）③ 适用 SKU 与比例（是否全线 -50%）④ 与 Gemini 自家 Batch 的叠加规则 | 官方定价页/API 参考文档研究 + 活探针：测试 key 在窗内/窗外各发最小请求，对比 usageMetadata 与计费字段 | `discount-catalog.toml` 首条实测数据 + 设计 §5 表更新为「已验证」 | **2.17 off-peak 窗口管理实现方式**（自动计费→daemon 只排时间；参数开关→适配器传参）；A4 审批门 ETA 的时区计算 | P2 2.17 开工前；文档部分可立即做 |
| R5 | **Batch API 工具调用支持边界**（设计 §7.2 前提） | ① batch item 是否支持 tools/tool_choice？② 单 item 能否多轮工具循环，还是一问一答？③ 输出文件中 tool_calls 格式 ④ 限额（OpenAI 50k 请求/200MB/batch）与分片规则 ⑤ 已提交 batch 能否撤销 ⑥ item 级失败语义与计费 | 文档 + 活探针：提交 3-item 批次（1 个带工具调用、1 个多轮、1 个普通），解析输出文件 | eligibility 检查清单（哪些步骤形态可走 batch）→ 回填 2.17 deferrable 分类器；设计 §7.2 约束表改「已验证」 | **batch 路径宽度**（若工具不支持→收窄为纯分析步骤，仍有价值）；**急停与在途 batch 的交互**（若不可撤销→处置定为「标记+轮询」而非「取消」，用例 B14 按此落地） | P2 2.18 开工前；结论影响设计 §3/§7 两处 |

> R3 是 P0 唯一的技术风险项，优先级最高；R4/R5 的文档研究部分可提前做，活探针需测试 key。
