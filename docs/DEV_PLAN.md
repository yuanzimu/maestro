# Maestro 开发计划

> 版本 v2.7 · 2026-10-05
> v2.7：新增 Sprint C 详细任务书（C0~C5：Windows Job Objects 真冻结 + Authenticode、macOS Developer ID 签名/公证/universal、Linux 托盘 SNI 收尾）+ 决策 30~32；Sprint B 收尾勾稽
> v2.6：新增 10.5 模型目录同步工具（运行一次获取免费/低价 API+模型+填写示例）
> v2.0：双产品矩阵 + Token 经济第一等公民 + 轻量硬约束
> v2.1：新增 U 组体验层重大改进（8 项，源自 [UX_DEEP_DIVE.md](./UX_DEEP_DIVE.md) 第一部分）
> v2.2：以 U 组信任旅程为主线重排 P0/P1/P2——P0 三个 UX 地基（LLM client / 多轮驱动 / 事件埋点），P1 三个 Sprint（入口信心→过程掌控→回顾成长）
> v2.3：实际使用视角增补 U9~U14（第二部分）——P0 新增 suspended 语义、急停 API、doctor 命令；新增 T6 错峰执行
> v2.4：**U9~U14 细化为可执行任务**合并进各阶段；U10/T6 完成详细设计（见 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md)：三阶段急停 / git ref 快照原语 / T6 双路径调度与 Batch Manager）
> v2.5：**互补项目 + nginx + 三端 GUI 调研落地**——省钱引擎对接 CCR（不自研路由）、Linux GUI 复用 Tauri/React、Server 放弃 nginx 补丁走 Rust 独立实现；新增第十~十二章与决策 23~27
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
| ✅ | 0.16 steering 队列 | jsonl 持久化 + flush/hold + kill -9 不丢 + 重试轮回喂投递；**R23 投递语义修正**（活 worker poll / respawn 前置注入 / 终态 Dropped 不静默）；**R32 at-least-once**：poll 进 inflight 未确认重投、TaskSteerAck 消费确认（仅当前 worker）、inflight 持久化跨重启、CLI 已跑过的失败轮也确认（防网络故障指令死循环重投） |
| ✅ | 0.17 suspended 状态机 | 七值枚举 + 自动恢复矩阵（30s→5m 退避，10 次升 blocked）+ 孤儿清理（绝不收养） |
| ✅ | 0.18 emergency_stop | FREEZE→SNAPSHOT→DECIDE 三阶段 + 竞态修复（过期退出误杀/死 worker 假恢复，R14） |
| ✅ | 0.19 checkpoint | git plumbing 零打扰 + pre-rollback 安全垫 + pinned（baseline/验收点）+ 滚动 GC |
| 🔶 | 0.6 适配器 | **R25 框架完成**：adapter.rs 三件套 —— stream-json 解析（RoundOutcome：session/answer/usage/model/工具调用/subtype）+ 退出分类（Disconnect≠failed，认证/配额 Fatal 优先，防误恢复烧钱）+ 方言 trait（claude 默认，MAESTRO_CLI_DIALECT 可插拔，B7 Codex/Gemini 预留）；rounder/core 已接线；**R28 补结构化错误**（result.errors/api_error_status 优于 stderr：429/5xx → Suspended 自动恢复，error_max_turns 不算错误）；**真实 claude CLI 端到端待 API key** |
| ⬜ | 0.8 落地 / 0.9 UI / 真实 CLI 验证 | 模型路由实测（L1 池 + 升级路径）、账本 UI 呈现、claude CLI 真实验证 —— 均待 API key / P1 前置调研（R2 适配器矩阵） |

**R28 调研落地（竞品对标，来源见 git log 提交）**：
- **计价修正**：cache_creation（写 cache）单独计价 —— Anthropic 1.25x/2x 输入价、OpenAI = 输入价不加价（测试抓到「计 0 价」错误实现并修正）；UsageEntry/轮账参数加 cache_creation_tokens，三桶互斥计价
- **结构化错误**：Claude Code 官方错误源是 result 事件的 errors[] + api_error_status（非 stderr）—— rounder 已转译，可重试（429/5xx）→ Suspended 自动恢复，防过载烧光轮数预算
- **P1 待办（调研发现，未实现）**：① stream-json stdin 常驻会话（官方 steering 通道，工具边界注入 —— Copilot「Steer with Message」/OpenClaw 六模式对标）；② ~~长任务上下文轮转~~ **R36 调研 + R37 机制层落地**（轮边界占用 ≥ MAESTRO_CONTEXT_LIMIT 默认 200k → 注入 `/compact` 指令轮，官方受支持路径；压缩轮防抖跳检防死循环；轻推优先于压缩；ContextCompacted 事件进 U3 叙事；e2e 混沌⑪ 全链：膨胀 200/轮 → 600 触发 → 压缩回落 → 同 session 继续）；③ ~~steering 队列 at-least-once~~ **R32 已落地**（poll/ack/inflight 持久化）；④ ~~total_cost_usd 增量对账~~ **R34 已落地**（adapter 解析 result.total_cost_usd → daemon 与牌价计费比对，漂移 >25% 发 CostDrift(Warning) 事件；入账仍以 daemon 计费为准）；⑤ ~~CLI schema 漂移检测~~ **R35 已落地**（schema_drift：system.session_id/result.result/usage 三要素 feature-detect，缺任一 → rounder exit 3 带人话诊断 → TaskFailed；未知新事件类型前向兼容不算漂移）

**R36 跨任务会话隔离（混沌⑩证伪驱动的真 bug 修复）**：rounder 状态目录从 `.maestro/` 迁移到 `.maestro/<task_id>/` —— 之前同 workdir 串行两任务，B 任务 resume A 的 session（跨任务上下文泄漏：A 通读后 B 无需通读即可召回 FACT）。证伪素材：mock 的 FACT 召回需会话内 read 标记。同任务 respawn/崩溃恢复读同一路径，续接语义不变。

**R38~R49 增量（20 轮后续迭代，详见 git log）**：
- **R39 轻推/压缩竞争修复**（混沌⑫证伪）：轻推占位时 compact_pending 作废（不得把轻推轮记账成压缩轮）；context_used 三桶合计（in + cache_read + cache_creation，漏 cache_creation 会低估占用）
- **R40 R2 适配器矩阵调研**：Amp/CodeBuddy 事件格式 Claude 兼容可直接套 claude 方言；Codex（`codex exec` JSON）/Gemini（stream-json，退出码 42=输入错/53=轮次上限）/OpenCode 需独立解析器；Crush 无 JSON 不可行
- **R41 AmpDialect**：Dialect trait 加 `parse_round` 钩子（默认 claude stream-json），Amp 走 `-x` 位置参数 + `threads continue <tid>` 续接
- **R42/R43 轮数预算耗尽语义**：MAX_ROUNDS 到顶 ≠ 完成 —— exit 5 + `ROUNDS_EXHAUSTED` 标记 → blocked(RoundsExhausted) 进收件箱（steering 不 drop），resume → requeue → respawn 同 session 续接（每次 respawn 新预算）
- **R44 负载混沌**：8 任务×4 槽混合场景全绿；教训：循环内 drop TempDir → workdir 幽灵目录（inode 活着可写但外部读不到），须保活
- **R45/R46/R47 B1 叙事三层落地**：`events --human`（12 类事件可读渲染）→ narrative.rs 降级模板（TaskVitals 事件流单遍聚合 + progress_line「第 N 轮：{top3 工具}×{次数}，累计 {成本}，耗时 {t}｜最近：{摘要}」，task get API 内嵌）→ CLI `task get` 人话渲染（--json 保留原输出）
- **R48 round 推进修复**（审计证伪）：TaskRecord.round 此前不随 RoundProgress 更新（无轻推/检查点的任务 ROUND 列恒 0）；修复用 max() 单调推进（respawn 重计不回退）
- **R49 过期/孤儿 rounder 自杀**（混沌⑭）：poll -403（易主）→ exit 6 只烧 1 轮；daemon 失联容忍 1 轮（重启窗口）、连续 2 轮 exit 7 —— pre-pidfile 孤儿重启后 reap 扫不到，只能自杀止损
- **R50 .maestro 状态目录滚动 GC**：只删权威库已终态（Done/Failed/Cancelled）任务的目录，每 workdir 按 mtime 保留最近 10 个；非终态（resume 凭据）与未知目录不碰；daemon 启动执行一次。dir_name 与 rounder 共用（taskstate.rs，防定位分歧）
- **R51 Codex/Gemini 方言落地**（R2 适配器矩阵实现，调研见 B7 行）：CodexDialect（thread.started/item.*/turn.completed 事件 + `exec [--json] resume <tid>` 续接 + **拆桶口径**：input_tokens 是总量已含 cached/cache_write，解析侧拆回互斥三桶防轮转检测双计）；GeminiDialect（init/message/tool_use/result.stats + `--resume` + cached 合并值入 cache_read + **退出码 53=轮次上限**经 accepts_exit 钩子视为正常轮出口继续循环）；schema_drift 方言化（各自要素清单，claude 格式喂错方言 → 人话漂移诊断 + exit 3）；mock CLI 按参数形态自动切换输出格式（exec→codex / -p 且无 --max-turns→gemini），usage 三方言同源可对账
- **R52 OpenCode 方言**（四方言矩阵齐备）：NDJSON 每行 sessionID（无 init 事件，首见为准）+ text 聚合 + step_finish 计量 + **自报 cost（USD，四方言唯一）**；续接 `run -s <sid>`。教训：`-p maestro-e2e` 单独跑不重建 rounder bin → 旧二进制未注册方言回落 claude（方言测试前须 build --bins）
- **R53~R56 Web 指挥台 + 可用性**（浏览器实测驱动）：CLI status/steer/pause/cancel 人话化；`maestro ui`（零依赖 HTTP + 内嵌单页：任务卡 narrative 进度 + 轻推/恢复/放弃就地操作 + 事件流人话）；daemon 加 `--worker` clap 参数（修复命令行被静默忽略 → echo worker → 假完成 3 振的首跑坑）；应用场景矩阵 S1~S7（首跑/监控+轻推/预算耗尽决策/自动恢复/多任务面板/查账/UI 先于 daemon）；round 显示口径统一为累计轮数。**发现的真实缺陷**：① daemon 不认 `--worker` 命令行 ② TaskList 不带 narrative ③ 事件订阅 from_seq=0 不重放（UI 换任何时机连上都应 from=1）④ round 双口径分裂（respawn 重计 vs 累计）—— 四个都在浏览器首测时暴露
- **R57 跨平台矩阵（GitHub CI）**：三平台（ubuntu/windows/macos）build+clippy 全绿 + test 矩阵（linux 全量阻塞锚点；cross 试验性 continue-on-error）。改造面：① `maestro_client::transport` 跨平台 IPC 抽象（Unix socket ↔ Windows TCP 回环 + 端口文件发现，零新依赖，JSONL 线协议不变）② worker.rs 平台 `imp` 分裂（Linux 完整进程组+/proc 双因子；macOS kill 探测降级；Windows Child 注册表降级，freeze/unfreeze 占位 no-op 待 Job Objects）③ nix 移入 `[target.'cfg(unix)'.dependencies]` ④ serve 改为 bind 后返回 Addr（socket_path 回填时序：Windows TCP 端口 bind 后才确定）⑤ 默认数据目录 `/tmp/maestro` → `std::env::temp_dir()/maestro` ⑥ e2e/testkit Unix 模块 cfg 门控（Windows 测试面 = protocol/client/cli + daemon 单测）。**推送链路**：用户在 Trae 配置 GitHub 源代码管理后 `GIT_ASKPASS` 注入全部 git 子进程 —— HTTPS+token 推送/api 查询 CI 全通（token scopes: repo/workflow）。**网络结论**：github.com 主站 443 间歇阻断（推送需重试）；SSH 22 通但账号无公钥（token 无 admin:public_key scope 加不了）；api.github.com 恒通（CI 状态查询走它）
- **R57 CI 实战（首轮 6/6 失败 → 逐轮收敛，「更容易发现问题」的价值兑现）**：首轮暴露四类真问题——① CI rustc 1.99 新 clippy lint 24 处（本地 1.98 无）② transport.rs `SocketAddr::from` 非 const（E0658，1.99 收紧）③ `cargo test` 只构建 deps/ 带hash测试形态二进制，不生成 `target/debug/maestro-rounder` 常规路径 → CI 裸跑 S1~S7 spawn ENOENT（**R51 教训的 CI 版**；修复：e2e lib 统一 `rounder_bin()` 自举——不存在则 `cargo build -p maestro-daemon --bin maestro-rounder`）④ macOS 兼容：/etc 是符号链接（canonicalize → /private/etc 绕过 deny-list 等值比较 → 敏感目录**原串先拦 + canonicalize 后再拦**双道检查）、BSD touch 无 -d（→ nix::utimensat）、ps stat 带后缀（"S+"/"T+" → 首字母匹配；BSD 不可中断态 U）、accept 继承 listener O_NONBLOCK（Linux 不继承 → macOS 上连接流 WouldBlock 即断，accept 后显式恢复阻塞）。**方法论教训（记档防重蹈）**：① `cargo clippy --fix` 被编译错误中断时修复**不落盘**——先确保编译过再 fix；② clippy/cargo 增量缓存会骗验证——验证 `-D warnings` 前必须 `cargo clean -p`（连续被骗三轮）；③ **Linux 上无法交叉编译 Windows**（sqlite bundled 需 MSVC lib.exe）——"本地交叉 check 三平台零告警"曾是缓存+grep 判定不严的双重假象，三平台编译验证**只能依赖 CI**；④ CI test 加 `--no-fail-fast`：一次暴露该平台全部失败（macOS fail-fast 曾把 b_emergency 后面的测试挡了两轮）
- **R58 Linux 安装包 + 全链验证 + GitHub Release（v0.1.1）**：`dist/linux/` 三脚本——package.sh（release 构建 → 收集三二进制 + 安装脚本 → MANIFEST sha256 → tar.gz，版本取 `git describe --tags`）、install.sh（无 root 单用户 `--prefix ~/.local`，完整性校验 + PATH 提示 + `maestro doctor` 引导）、uninstall.sh（不碰用户数据目录）。**安装包验证暴露 4 个真 bug（又一批「浏览器首测式」收获）**：① package.sh 没把 install/uninstall 打进包（首验即抓）② `events` 非 follow 模式永久阻塞在 read_line（协议加 `live` 字段：daemon 对非跟随连接 drain 重放后**主动关连接**，老客户端不带字段默认 true 前向兼容；CLI `--from` 默认 0→1 全量重放，与 UI 一致）③ 非 follow 只回放首条（CLI 回调返回 follow 值导致第一条即 break → 恒 true，流边界由 daemon 决定——**修 bug 时引入的 bug，第二轮验证才抓到**）④ 任务完成后 worker 记录永远 working（exit-0 成功路径只更新 task；`Authority.apply` 六个终态分支补 `finish_worker_of`——在线/重放共用单一事实源，daemon 重启 replay 路径同步验证）⑤ 终态任务叙事误报「尚未开始」（`progress_line_with_state` 按状态给终态叙事）。附带：doctor 尊重 `--data-dir` + 失败 exit 2（脚本可判定）、验收门 Passed/Failed 事件补人类可读渲染。**验证链路**（可复用的发布前清单）：解压→install→doctor（daemon 未起 fail+exit2）→起 daemon（写产物的假 worker 过验收 readback）→task create→done（实时+重启 replay 双路径）→events 完整回放 8 条→ledger/inbox/workers/steer（-409 为设计行为：终态拒绝入队）→UI Playwright 5/5（卡片/按钮/事件流/连接态）→shutdown→uninstall 零残留。**Release**：tag v0.1.1 + API 创建 + uploads.github.com 传 tar.gz/sha256（3.2MB），api.github.com 资产端点下载 sha256 比对一致；注意 `.gitignore` 用 `dist/*` + `!dist/linux/`（`dist/` 整目录排除后 `!` 无法重新包含——git 不进入被排除目录）

**测试**：191 项全绿（R1~R58，含 app_journeys 7 场景 + UI HTTP e2e + events 非 follow 回归，clippy 零告警；CI 三平台矩阵 6/6 全绿）。体积：release 包 3.2MB（daemon 4.6M / CLI 1.8M / rounder 622K，<30MB 约束 7x 余量）。里程碑见 git log。**已发布**：GitHub Release v0.1.1（Linux x86_64 安装包）。**已知缺陷（v0 接受）**见 [acceptance.rs](../crates/maestro-daemon/src/acceptance.rs) 模块头：无产物型任务误判（0.15 结构化验收断言接管）、workdir 外产物不可见。

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
| B1 | 里程碑叙事：事件流经 L1 压缩为人类可读进度 + ETA（历史时长统计）+ 实时成本 | U3 | 0.14 LLM client + 埋点。**R45 调研完成**：Gemini「Topic & Update」模式最直接先例（每轮压成主题+更新）；各家均无 ETA 产品先例（建议历史轮均耗时滑动估算+置信度标注）；输出双层——一句话进度 + 可展开阶段列表；完成摘要卡「一句话+diff 就绪+花费」（Cursor/Devin 已验证）；LLM 不可用降级模板「第 N 轮：{最频繁工具}×{次数}，累计 ${cost}」。**CLI 前菜已落地（R45~R47）**：`events --human` 12 类事件渲染 + `task get` narrative 一句话进度（降级模板）+ 人话渲染 |
| B2 | **轻推 steering v1**：运行中任务接收补充指示，下一轮生效（UI 入口 + 事件回显） | U4 v1 | 0.15/0.16 |
| B3 | Blocked 收件箱 + 桌面通知 | G1/G5 | P0 inbox |
| B4 | **通知治理 v1（U9）**：分级——仅 blocked/done/failed/预算触顶 弹系统通知，其余进活动流；digest 批处理（低优先级 30 分钟聚合一条）；任务级静音（「做完叫我」）；深链直达（点通知直接落到该 blocked 的决策界面） | U9 | P0 priority 埋点 + B3 |
| B5 | 指挥台 UI：Worker 状态栏 + 任务列表 + 进度叙事呈现 + DAG 可视化 v1 | G4/U3 呈现 | B1 |
| B6 | Code Mode 沙箱（rquickjs）+ 编排启发式 | T2/D4 | — |
| B7 | Codex/Gemini 适配器 + worktree 隔离 | B6/B11 | 0.6 适配器框架。**方言层已落地（R51/R52）**：claude/amp/codex/gemini/opencode 五方言，Codex/Gemini/OpenCode 全链 e2e（多轮续接/计量对账/53 轮次上限/漂移防护）；OpenCode 自报 cost（USD）天然对账素材；真实 CLI 端到端待 API key；worktree 隔离未做 |
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

## 八、v2.5 互补项目调研与引擎增强（Sprint A 依据）

> 调研 30+ 项目（2026-10-05，GitHub 一手数据）。结论：**能复用的不造轮子，能借协议的不追版本**。

### 8.1 采纳清单

| 项目 | License | 采纳方式 | 阶段 |
|---|---|---|---|
| **Claude Code Router (CCR, 37.6k★)** | MIT | 本地网关直连：daemon 把 worker 与自身 LLM 调用统一指向 `http://127.0.0.1:3456`，路由/key 池/fallback/成本核算交给 CCR | **Sprint A** |
| **Foreman** | 自定义（只借设计不抄码） | turn/cost/time 硬预算闸门 + `touches` 文件足迹 + hash-sealed 审批 | **Sprint A**（预算）/ P2（足迹、封签） |
| **AgentSmith (MIT)** | MIT | evidence bundle：验收必须留证据（失败测试→修复→通过→回执），根治谎报 done | Sprint A 轻量 / P1 完整 |
| **OpenLLMetry + Langfuse (MIT)** | Apache / MIT | OTel Rust SDK 导出，观测 UI 不自建 | P2 |
| **ACP（Agent Client Protocol，官方 Rust crate）** | Apache | adapter 双轨：stream-json 拿细粒度 + ACP 扩 agent 生态 | P1 立项 |
| **goose（Rust, 55k★）** | Apache | 作为可指挥的 Rust agent 后端候选 | P2+ |

### 8.2 CCR 对接技术事实（已核实）

- CCR 是本地 HTTP 网关，默认 `HOST=127.0.0.1`、`PORT=3456`；同端口提供 Anthropic `/v1/messages`、OpenAI `/v1/chat/completions`、OpenAI Responses、FIM 端点。
- Claude Code 接入：`ANTHROPIC_BASE_URL=http://127.0.0.1:3456` + `ANTHROPIC_AUTH_TOKEN=dummy`（CCR 自鉴权，真 key 在 CCR Providers 内）。
- 模型名用 `provider,model` 语法；`/model` 可会话内热切。
- Maestro 侧链路已验证可行：daemon 经 `worker_env` 注入环境 → rounder 用 `Command::new` 不清环境 → 内层 claude CLI 继承生效。

### 8.3 风险与纪律

- **供应链**：gpt-pilot 2025-08 投毒事件 → 第三方 agent 拉起必须完整性校验 + 路径白名单（已有 security 基线，CI 脚本纳入校验）。
- **License 雷区**：Claude Squad（AGPL）、Phoenix（ELv2）、gpt-pilot/Crush（FSL）只看不抄；可复用限 MIT/Apache。
- 退潮项目（gpt-engineer、Devina、Swarm、Crystal）不投入。

## 九、Server 版技术路线：不打 nginx 补丁

调研 nginx 源码（`src/core|event|http|os`、master-worker、11 phase、`ngx_module_t`、`--add-module`/`--add-dynamic-module`、njs/OpenResty）后的决策：

1. **放弃真打补丁 / 写 C 模块**：任务/agent 生命周期编排（fork 子进程、多轮、验收、急停）触碰 worker「单线程非阻塞」红线；且 nginx 构建链/ABI 与「<30MB 单二进制 + install.sh」承诺冲突；mainline 无稳定 ABI、补丁需长期追版。
2. **主线 = 借鉴架构、Rust 独立实现**：移植 master/worker（SIGHUP 热加载、graceful drain）、phase 管线（auth→quota→classify→route→upstream→audit）、upstream 抽象（健康检查/failover/SSE 透传）、token 维度 limit_req。
3. **共存契约**：Maestro 对外标准 HTTP + `/health` + `/metrics`，企业现有 nginx/OpenResty 直接 `proxy_pass`，零 nginx 代码。三档部署：纯单二进制（默认）/ nginx+N Maestro / K8s ingress。
4. 对标 F5 AI Gateway（2026-08：模型目录/预算实时扣减/MCP 工具授权/Guardrails fail-closed），以「托管整个 agent 任务生命周期」区别于 LiteLLM/Portkey 的请求代理。

## 十、三端 GUI 优化计划（参照 Trae IDE）

### 10.1 Trae IDE 对标要素

左活动栏 + 可折叠侧边栏；顶部模式切换（Code/SOLO）；对话流 + inline diff + 审批门一体；命令面板（`Ctrl/Cmd+K`、`⇧⌘P`）；深浅主题 + 平台原生窗口控件；全局进度 + 通知中心；空状态引导。

### 10.2 三端共性任务（一次投入三端受益）

| # | 任务 | Sprint |
|---|---|---|
| C-1 | 统一设计语言：spacing/色板/状态色/字体阶梯（对齐现有 [styles.css](../desktop/src/styles.css)，macOS 硬编码改 system colors） | B |
| C-2 | 布局改 Trae 式：左活动栏（任务/收件箱/事件/设置）+ 主区 + 右可折叠详情 | B |
| C-3 | 命令面板 `Ctrl/Cmd+K` + 全局快捷输入 `Alt+M`（U1） | B |
| C-4 | 计划审批门卡片：步骤 + 成本预估（U8）+ 批准/编辑/拒绝 | B |
| C-5 | 结果卡 v2：摘要 + 验收状态 + 花费 + 节省比例 + inline diff（U5） | B |
| C-6 | 通知中心：非打扰事件统一收纳，仅 blocked/done/异常系统通知（U9） | B |
| C-7 | 空状态/断连态/首跑引导（U2）+ doctor 入口前置 | A/B |

### 10.3 平台专项

- **Windows（Tauri，最完整）**：sidecar 状态可视化；Win11 Mica/深浅色；`Alt+M` 注册 + 剪贴板识别；包体 <20MB（发布剔除 mock-cli）；急停用 Job Objects 补 freeze（现为 no-op）；IME composition 验证。
- **macOS（AppKit 原生）**：SF Symbols 活动栏 + NSToolbar；深浅色自动；NSStatusItem 菜单栏 + 全局菜单 + `⌘N/⌘K`；**notarytool 公证 + Developer ID 签名**（现仅 ad-hoc）；Combine 推送替代 1.5s Timer；universal 二进制。
- **Linux（方案 A，复用 Tauri/React）**：同一 Tauri 工程产出 Linux 包；**AppImage（主推）+ .deb + .rpm**；`.desktop` + 托盘（SNI）+ 跟随 GNOME/KDE 深浅色；全局快捷键走 XDG portals（X11/Wayland）；fcitx5 输入法验证（含 xrdp 场景）。

### 10.4 Sprint 排期

| Sprint | 周期 | 出口断言 |
|---|---|---|
| **A 省钱落地** | 2 周 | 真实任务经 CCR 路由，成本可量化；硬预算超限即挂起 |
| **B 三端体验对齐** | 2~3 周 | 三端同一套界面语言；Linux GUI 可安装 |
| **C 平台打磨** | 2 周 | Win Job Objects / mac 公证+菜单栏 / Linux AppImage+托盘；三端可分发 |

#### Sprint A 实施进度（2026-10-05，机制层已完成）

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | [gateway.rs](../crates/maestro-daemon/src/gateway.rs) | CCR 配置/环境注入（Anthropic+OpenAI 四变量）/ daemon LLM client 指向网关 / 探活；命令行 `--ccr-url` `--ccr-token` + 环境 `MAESTRO_CCR_URL` `MAESTRO_CCR_TOKEN`（空串关闭） |
| ✅ | [budget.rs](../crates/maestro-daemon/src/budget.rs) | Foreman 式 cost/wall 硬预算；环境 `MAESTRO_BUDGET_CENTS` `MAESTRO_BUDGET_WALL_MS`（0=不限） |
| ✅ | Core 接线 | worker spawn 注入网关环境（同名键覆盖）；轮账入账后强制预算 → SIGSTOP + Suspended(BudgetExceeded) + 叙事快照 |
| ✅ | 测试 | 新增 2 个预算 e2e（[budget_enforcement.rs](../crates/maestro-e2e/tests/budget_enforcement.rs)）；全 workspace **205 项全绿**；clippy `-D warnings` 零告警；生产 daemon 冒烟通过（默认网关 / `--ccr-url ""` 直连 / status / shutdown） |
| ⬜ | 真实链路联调 | 需本机安装并配置 CCR（`npm i -g @musistudio/claude-code-router`）+ 真实 provider key，验证真实 claude CLI 经 CCR 路由与成本量化；机制层已用 mock 全量验证 |

#### Sprint B 详细任务书（2026-10-05 规划，尚未开发——待三端分别下发）

> 目标：三端同一套界面语言对齐 Trae IDE；Linux GUI 首次出包。
> **本 Sprint 只写计划不写代码。** 任务按「B0 共享契约 → B1 Tauri/React 共享前端（Win+Linux）/ B3 macOS 原生对齐（可并行）→ B2/B4 平台打包」组织，每条带唯一 ID，便于在 Windows、Linux、macOS 三端分别下发与追踪。

**出口断言（Sprint B 完成时必须全部成立）**

1. Win / Linux（Tauri）与 macOS（AppKit）呈现一致的信息架构与视觉语言，普通用户切换平台零学习成本；
2. Linux 产出可安装的 GUI 包（AppImage 主推 + .deb + .rpm），安装后能拉起引擎、派活、看叙事、收通知；
3. 审批门 / 结果卡 / 命令面板 / 通知中心四类核心界面在三端行为一致；
4. 三端包体与内存不恶化（Win/Linux <20MB，macOS 无新增重依赖）；全量测试与 clippy 保持全绿。

##### B0 · 共享设计契约（**必须最先做，三端共同依据**）

| ID | 任务 | 落点 / 产出 | 验收 |
|---|---|---|---|
| B0-1 | 设计 token 单一事实源：spacing（4/8 基准）、色板（含亮+暗双主题）、8 种状态色、字体阶梯、圆角、阴影、动效时长 | 以现有 [styles.css](../desktop/src/styles.css) 的 CSS 变量为底补齐亮色主题并冻结命名；macOS 侧给出同名 token 到 NSColor 的映射表 | token 清单三端各一份且命名/数值一一对应；无硬编码颜色残留 |
| B0-2 | 信息架构（IA）：左活动栏四图标（任务 / 收件箱 / 事件 / 设置）+ 主区 + 右可折叠详情；顶部只留模式切换（专注单 Agent / 并行多 Worker）与全局动作 | IA 线框 + 导航状态机（含收件箱未读角标） | 三端导航路径与默认落点一致；替代当前顶栏 Tab |
| B0-3 | 核心组件契约：TaskCard / TaskDetail / CommandPalette / ApprovalGate / ResultCard / NotificationCenter 的数据字段、状态、动效、空态 | 组件契约表（props / 状态 / 事件，三端各自实现） | 同一任务数据在三端渲染结构等价 |
| B0-4 | 文案与术语表：状态、按钮、空状态、错误引导的统一中英文案（含「下一步」提示） | 术语表 | 三端同义不同译的情况清零 |

##### B1 · Tauri/React 共享前端改造（**Windows 与 Linux 共用，一次开发两端受益**）

> 全部落在 [desktop/src](../desktop/src)；仅在共享前端完成后，Win/Linux 的平台打包（B2/B4）才有意义。

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| B1-1 | 布局改 Trae 式：新增 ActivityBar 组件 + 主区 + 右侧可折叠详情；移除/改造 [StatusBar](../desktop/src/components/StatusBar.tsx) 的导航职责 | 新 `components/ActivityBar.tsx`；改 [App.tsx](../desktop/src/App.tsx)、styles | 四图标导航可用、角标正确、窗口缩放无破版 |
| B1-2 | 命令面板：`Ctrl/Cmd+K` 唤起，支持新建/搜索任务/执行急停/跳设置；模糊匹配 + 键盘上下/回车 | 新 `components/CommandPalette.tsx` | 纯键盘可完成派活；Esc 关闭 |
| B1-3 | 全局快捷输入 `Alt+M`（U1）：任意界面唤起输入条 + 剪贴板智能识别（错误日志/URL/代码） | 前端入口 + Tauri 快捷键注册（B2/B4 平台接线） | 剪贴板类型识别并附为上下文 |
| B1-4 | 计划审批门卡片（U8+C-4）：步骤清单 + 成本预估 + 批准 / 编辑 / 拒绝 | 新 `components/ApprovalGate.tsx` | 审批前不执行；成本一目了然 |
| B1-5 | 结果卡 v2（U5+C-5）：一句话摘要 + 验收 12/12 + 花费 + **节省比例** + inline diff 入口 | 改 TaskDetail / 新 `components/ResultCard.tsx` | 5 秒可扫视；节省数据显著 |
| B1-6 | 通知中心（U9+C-6）：非打扰事件统一收纳 + 仅 blocked/done/异常走系统通知 + 全部已读 | 新 `components/NotificationCenter.tsx` | 非关键事件不弹系统通知 |
| B1-7 | 空状态 / 断连重连 / 首跑引导（U2+C-7）：示例任务 + doctor 入口前置 | 改 App 空态与各空组件 | 新用户首屏有明确下一步 |
| B1-8 | 亮 / 暗主题切换 + 跟随系统 | styles 双主题 + 设置项 | 切换无闪烁、持久化 |

##### B2 · Windows 平台专项（Tauri，依赖 B1）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| B2-1 | **Sprint A 网关参数透传**：Tauri 拉起 sidecar 时支持把 CCR url/token 传给 daemon（当前 [daemon.rs](../desktop/src-tauri/src/daemon.rs) 只传 worker/data-dir） | 改 src-tauri/src/daemon.rs、settings | 引擎日志显示正确网关；可在设置中关闭 |
| B2-2 | Win11 原生观感：Mica/Acrylic、跟随系统深浅色 | tauri 配置 + 窗口效果 | Win11 生效、旧版自动降级不报错 |
| B2-3 | 全局快捷键 `Alt+M` 的 Windows 注册 | src-tauri | 全应用可唤起 |
| B2-4 | IME composition（中文输入法）在 WebView2 下验证 | 实测 | 拼音组词不丢字、回车不误提交 |
| B2-5 | 发布包剔除 mock-cli（仅开发期需要）+ NSIS 包体 **<20MB** | tauri externalBin 条件化 | 安装包达标且功能完整 |

##### B3 · macOS 原生对齐（AppKit，**可与 B1 并行**）

> 依据 B0 契约用原生 AppKit 实现，不引入 SwiftUI 重依赖；公证 / Developer ID 签名留 Sprint C。

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| B3-1 | SF Symbols 活动栏 + `NSToolbar` 替代自绘顶栏 | 改 [AppKitUI.swift](../clients/macos/Sources/MaestroGui/AppKitUI.swift)、[main.swift](../clients/macos/Sources/MaestroGui/main.swift) | 四图标导航 + 角标与 B0-IA 一致 |
| B3-2 | 深浅色自动：状态色全部走 system colors / B0 token 映射 | AppKitUI.swift | 切换系统外观即时生效 |
| B3-3 | `NSStatusItem` 菜单栏常驻：未读角标 + 急停 / 新建任务全局菜单 | main.swift | 菜单栏可完成常用动作 |
| B3-4 | 快捷键体系 `⌘N / ⌘K / ⌘M` | 菜单 + key equivalent | 与 Win/Linux 功能对等（仅修饰键差异） |
| B3-5 | 审批门 / 结果卡 / 命令面板的 AppKit 原生实现（对齐 B1-4/5/2 行为） | 新增对应视图 | 与 Tauri 版行为等价 |

##### B4 · Linux GUI 出包（方案 A 复用 Tauri，依赖 B1）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| B4-1 | Tauri bundle target 增加 Linux：`appimage` / `deb` / `rpm` | [tauri.conf.json](../desktop/src-tauri/tauri.conf.json) | 三产物均可构建 |
| B4-2 | Linux sidecar 准备脚本：产出/收集 maestro-daemon、maestro-rounder（对齐 Windows 的 prepare-sidecars.ps1） | 新增 `desktop/scripts/prepare-sidecars.sh` | 打包前 sidecar 就位、可执行位正确 |
| B4-3 | Freedesktop 集成：`.desktop` 文件 + 托盘图标（SNI）+ 跟随 GNOME/KDE 深浅色 | bundle 配置 + 前端 | GNOME/KDE 菜单可启动、托盘可用 |
| B4-4 | 全局快捷键走 XDG：兼容 X11 与 Wayland | src-tauri | 两协议下 `Alt+M` 可用 |
| B4-5 | fcitx5 输入法验证（含 xrdp/XFCE 场景） | 实测 | 中文输入不丢字 |
| B4-6 | 产物验收：AppImage 单文件免安装可跑；.deb/.rpm 安装卸载干净 | 实测三形态 | 装→派活→看叙事→收通知→卸载全链通 |

**下发顺序与依赖**

```
B0 共享契约（先做，一次性）
   ├── B1 Tauri/React 共享前端 ──┬── B2 Windows 打包/专项
   │                            └── B4 Linux GUI 出包
   └── B3 macOS AppKit 对齐（可与 B1 并行，只依赖 B0）
```

- 建议：三端同时开工时，**先在一处完成 B0 并合入**，Win/Linux 走 B1→B2/B4，mac 走 B3；
- B1 与 B3 完成后做一次「三端一致性走查」（同一组任务数据对照 B0-3 契约），再进入 B2/B4 的安装包验证；
- 每个 ID 完成即在下表勾稽；测试随任务同步新增（共享前端行为测试 + Linux 安装包冒烟可纳入 CI 的 ubuntu job）。

#### Sprint B 实施进度（2026-10-05，Linux 出包补齐 + 编译修复）

> 远端 master 已以「改造既有组件」的等价形态落地 B1/B2/B3（活动栏 / 命令面板 / 审批门 / 结果卡 / 通知中心 / macOS B3）；本地独有的增量是 **B4 Linux 出包** 与远端审计提交遗留的编译修复。

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | B1/B2/B3（远端等价实现） | 活动栏 + 命令面板 + 审批门 + 结果卡 + 通知中心；macOS B3 直接落在 [main.swift](../clients/macos/Sources/MaestroGui/main.swift) / [AppKitUI.swift](../clients/macos/Sources/MaestroGui/AppKitUI.swift)；事件桥首订阅就绪门控 |
| ✅ | B4-1 [tauri.conf.json](../desktop/src-tauri/tauri.conf.json) | `bundle.icon` 补全跨平台 PNG/ICO 清单；Linux 形态经 CLI `--bundles appimage,deb,rpm` 覆盖（顶层 targets 仍 nsis） |
| ✅ | B4-2 [prepare-sidecars.sh](../desktop/scripts/prepare-sidecars.sh) | 先占位（Tauri build script 校验三件套齐全）再 cargo 构建 daemon（含 rounder）+ mock-cli，最后以 `-x86_64-unknown-linux-gnu` 后缀覆盖 |
| ✅ | B4-6 [build-linux.sh](../desktop/scripts/build-linux.sh) + `npm run bundle:linux` | 一键出 AppImage / deb / rpm；产物大小与全链实测见提交说明 |
| ✅ | 编译修复 | [core.rs](../crates/maestro-daemon/src/core.rs)：`api_steer` 与验收反思 push 的 `Result<SteeringMsg,_>` 解包；[worker.rs](../crates/maestro-daemon/src/worker.rs)：spawn 失败兜底 `SendError<Child>.0` 解包；[llm.rs](../crates/maestro-daemon/src/llm.rs)：1.99 clippy needless_question_mark |
| ✅ | 验证 | `cargo clippy --workspace --all-targets -D warnings` 零告警；全 workspace **207 项全绿**；`desktop` tsc+vite 构建通过 |
| ⬜ | B2-2 / B2-4 / B4-3 / B4-4 / B4-5 | Win11 Mica、WebView2 IME、Linux SNI 托盘、XDG 全局快捷键、fcitx5/xrdp 实测——并入 Sprint C 平台收尾（C3/C5） |

#### Sprint C 详细任务书（2026-10-05 规划，**只写计划不写代码**——待三端分别下发）

> 目标：把 Sprint B「能用」的三端做成「敢分发」——Windows 急停真冻结（Job Objects）+ 安装包签名，macOS Developer ID 签名 / 公证 / universal，Linux 托盘与安装体验收尾。
> Sprint B 留下的技术债经调研已定位（见下「现状基线」），任务按「C0 发布基建与凭据 → C1 Windows / C2 macOS（可并行）→ C3 共享前端托盘收口 → C4 Linux 收尾 → C5 三端发版演练」组织，每条带唯一 ID。
>
> **📖 2026-10-06 起三端并行开发**：mac / windows / linux 三端任务分配、协作规则（分支/CI 硬门/协议双轨制/冲突区）与里程碑见 [PARALLEL_PLAN.md](./PARALLEL_PLAN.md)——各端开工前先读。

**现状基线（2026-10-05 调研核实，Sprint C 的改造起点）**

| 平台 | 现状 | 缺口 |
|---|---|---|
| Windows 进程控制 | [worker.rs](../crates/maestro-daemon/src/worker.rs) `#[cfg(windows)] mod imp`：Child 注册表 + waiter 轮询；`freeze_group`/`unfreeze_group` 为 **no-op**；graceful=直接 TerminateProcess；无 windows-sys 直接依赖 | 急停 FREEZE 在 Win 上不静止 → [emergency.rs](../crates/maestro-daemon/src/emergency.rs) 快照前提（FS 静止）不成立；孙进程泄漏；无三级优雅关闭 |
| Windows 安装包 | Tauri 仅 `nsis`，currentUser 免管理员；prepare-sidecars 默认 triple 为 aarch64 | 无 Authenticode → SmartScreen 告警；x64/arm64 矩阵未统一；mock-cli 未剔除 |
| macOS 原生 | [build.sh](../clients/macos/build.sh) 手搓 .app + 内嵌三个 Rust bin + ad-hoc 签名 + DMG；单架构（uname -m）；菜单栏常驻已实现 | 无 Developer ID / hardened runtime / entitlements / notarytool / staple；DMG 未签；无 lipo universal；Bundle ID 与 Tauri 不一致 |
| Tauri 壳 | [lib.rs](../desktop/src-tauri/src/lib.rs) 仅 setup+invoke；Cargo.toml 未启用 `tray-icon` feature | 无托盘、关窗即退（与「关窗任务照跑」承诺不符）；tools.rs 为 Windows 中心写法（.exe / `;`）；无 macOS sidecar 脚本 |
| Linux | AppImage/deb/rpm 已可出（复用 Tauri） | 无 SNI 托盘；无 deb/rpm GPG；fcitx5/xrdp 与 X11/Wayland 快捷键未实测 |

**出口断言（Sprint C 完成时必须全部成立）**

1. Windows 急停走 Job Objects：FREEZE 整组（含孙进程）<100ms 静止，SNAPSHOT 期间产物不变化，恢复后任务继续；`b_emergency` 等价行为在 Windows 有自动化守护；
2. 三端安装包均通过系统安全门：Windows Authenticode 签名（SmartScreen 无未知发布者）、macOS Developer ID 签名 + 公证 + staple（新机 `xattr` 干净、双击即开）、Linux 仓库 GPG 元数据齐备；
3. macOS 一个 universal DMG 覆盖 arm64 + x86_64（App 与三个 sidecar 均 universal）；
4. Tauri 壳三端托盘齐备：关窗最小化到托盘、菜单含新建/收件箱/急停/退出、未读角标，与 macOS 原生菜单栏行为等价；
5. 全新虚拟机/容器内按 Release 说明完成「下载 → 安装 → 派活 → 急停/恢复 → 收通知 → 卸载」全链；全量测试 + clippy 保持全绿，包体约束不回退。

##### C0 · 发布基建与凭据（**必须最先做，C1/C2 共同前置**）

| ID | 任务 | 落点 / 产出 | 验收 |
|---|---|---|---|
| C0-1 | 三端发布矩阵定义：Win(x64+arm64) / macOS(universal) / Linux(x64) 的 sidecar triple 与产物清单统一成一份矩阵 | 矩阵表（仓库内文档或 CI 变量）；prepare-sidecars 两脚本默认值对齐 | 任一端构建脚本只读矩阵即可产出，triple 无硬编码分叉 |
| C0-2 | 签名凭据管理方案：Windows 代码签名证书、macOS Developer ID Application + App-specific password/API Key（notarytool）经 CI secrets 注入，本地构建文档化 | CI secrets 清单 + 本地 `.env.example`（**不入库真值**） | 无凭据时构建给出明确降级提示（ad-hoc/未签）而非静默产出 |
| C0-3 | 版本号单源化：tauri.conf / package / Cargo / DMG 版本统一由 `git describe --tags`（或单一脚本）注入 | 版本同步脚本 | 改一处四端版本一致，消除手工漏同步 |
| C0-4 | 发版流水线骨架：CI 增加可选 `release` workflow（tag 触发），矩阵构建 → 签名 →（mac）公证 → 附件归档 | `.github/workflows/release.yml`（计划阶段只定结构） | dry-run 能跑通到「待签名」前各步 |

##### C1 · Windows 真冻结与签名（依赖 C0，**与 C2 可并行**）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| C1-1 | 新增 `windows-sys`（仅 `[target.'cfg(windows)'.dependencies]`，feature 最小集：Win32_System_JobObjects/Threading/ProcessStatus） | daemon Cargo.toml | 非 Win 构建零影响；clippy 全绿 |
| C1-2 | Job Object 生命周期：spawn 时建（或取每 daemon 一个）kill-on-close Job，worker 及子孙进程 Assign 进 Job；句柄存入现有 Child 注册表 | worker.rs `#[cfg(windows)] imp`（门面与 emergency 调用方零改动） | 内层 CLI 派生的孙进程可被整组枚举/终止 |
| C1-3 | 真冻结：`freeze_group` 枚举 Job 内进程逐线程挂起（NtSuspendThread 等价 win-sys 调用）；`unfreeze_group` 逆操作；记冻结代次防重复 | worker.rs Win imp | FREEZE 后 100ms 内组内无 CPU/FS 变化；恢复后进程继续 |
| C1-4 | 优雅关闭三级化：先 CTRL_BREAK/关闭事件投递，超时再 Terminate Job；替代当前直接 Terminate | worker.rs Win imp | worker 有保存现场窗口；超时兜底确定 |
| C1-5 | emergency 快照前提守护：FREEZE 后加「静止确认」（短轮询无新句柄/无 FS mtime 变化）再进 SNAPSHOT | emergency.rs（平台无关的可选钩子） | Win 上快照干净；Linux 行为不回退 |
| C1-6 | NSIS/EXE Authenticode：signtool 签安装包与未签 sidecar（CI）；时间戳服务 | CI release 步骤 + 本地脚本 | 属性中可见签名者；SmartScreen 不再报未知发布者（声誉积累期另说明） |
| C1-7 | 剔除 mock-cli 出发布包（仅 dev/testkit）；核对 x64 与 arm64 包体 **<20MB** | externalBin 条件化 + bundle 矩阵 | 两架构安装包达标、功能完整 |
| C1-8 | Windows 进程面测试：把 emergency/孤儿回收的关键行为做成 Win 可跑的集成测试（脱离当前 e2e 的 `#![cfg(unix)]` 限制），先 CI 非阻塞、稳定后转阻塞 | testkit/e2e 平台无关化 | 冻结/恢复/整组 kill 在 CI windows job 可自动验证 |

##### C2 · macOS 签名 / 公证 / universal（依赖 C0，**与 C1 可并行**）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| C2-1 | 统一 Bundle ID 为 `com.yuanzimu.maestro`（与 Tauri 一致），更新 Info.plist | build.sh | 两端 identifier 一致 |
| C2-2 | hardened runtime + entitlements（allow-jit / disable-library-validation 按内嵌 CLI 实际需要最小授权） | 新增 `Maestro.entitlements` | `codesign -dv --entitlements -` 符合预期 |
| C2-3 | sidecar 逐二进制签名：Resources/bin 下三个 Rust bin 先各自 Developer ID 签名，再签外层 .app（签名顺序由内向外） | build.sh | bundle 内无未签可执行文件（`codesign --verify --deep` 通过） |
| C2-4 | universal Rust sidecar：cargo 分别构建 arm64/x86_64 后 `lipo -create` 合并三件套；Swift 侧 `swift build --arch arm64 --arch x86_64`（或两构 lipo） | prepare-sidecars 增加 mac 分支 + build.sh | `file/lipo -info` 显示双架构；两架构实机/迁移测试可跑 |
| C2-5 | 公证：`xcrun notarytool submit`（keychain profile 或 API Key）+ 等待 accepted + `xcrun stapler staple`；失败即中断并取日志 | build.sh | `stapler validate` 通过；`spctl -a -vvv` 接受 |
| C2-6 | DMG 签名 + 公证：签名 DMG（Developer ID Application）后一并公证并 staple | build.sh | DMG 双击打开无 Gatekeeper 拦截；全新用户机 `xattr` 无隔离残留 |
| C2-7 | ScenarioRunner 100 场景在签名/universal 包上回归（Apple Silicon 本机 + Rosetta 下 x86_64 路径各跑冒烟） | Package.swift harness | 双架构关键旅程通过 |

##### C3 · 托盘与窗口收口（**三端共享，依赖 B1；可与 C1/C2 并行开发**）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| C3-1 | Cargo.toml 启用 `tauri` 的 `tray-icon` feature；`TrayIconBuilder` 建托盘：菜单（显示窗口 / 新任务 / 收件箱 / 急停 / 设置 / 退出）+ 图标 + 未读角标 | src-tauri Cargo.toml / lib.rs（行为对齐 macOS main.swift 的 NSStatusItem） | 三端托盘菜单动作可用 |
| C3-2 | 关窗最小化到托盘而非退出（daemon 已独立常驻、ensure_daemon 支持接管）；托盘双击/菜单恢复窗口 | lib.rs 窗口事件 | 关窗后任务照跑、可从托盘恢复 |
| C3-3 | 托盘状态随事件刷新：急停态 / 待审批角标 / 恢复态文案（复用现有事件桥，不新增轮询） | lib.rs + 前端事件 | 角标与通知中心一致 |
| C3-4 | tools.rs 跨平台路径补强：`.exe` 与 `:`/`;` 分隔按平台分支，支持 macOS triple 与 sidecar 定位 | src-tauri/src/tools.rs | 三端 sidecar 解析单测覆盖 |

##### C4 · Linux 收尾（依赖 C3）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| C4-1 | SNI 托盘在 GNOME/KDE 实测（AppIndicator 兼容）；无托盘宿主环境（精简 WM）给出优雅降级提示 | bundle/desktop 文件 + 实测 | GNOME/KDE 托盘可见可用；xrdp/XFCE 下不报错 |
| C4-2 | `.desktop` 元数据收尾：Categories/StartupNotify/单实例；deb/rpm 依赖（libwebkit2gtk-4.1-0、libgtk-3）与 postinst/remove 干净 | tauri bundle linux 配置 | 菜单可启动、卸载不留残留菜单 |
| C4-3 | 全局快捷键 XDG 实测：X11 与 Wayland 下 `Alt+M`（Wayland 无全局注册时给出设置页引导而非静默失败） | src-tauri | 两协议行为明确、有降级说明 |
| C4-4 | fcitx5 输入法实测（含 xrdp/XFCE）：拼音组词不丢字、回车不误提交 | 实测记录 | 中文输入全链正常 |
| C4-5 | 三形态全链复验：AppImage 免安装直跑（含无 FUSE 时 `--appimage-extract` 指引）、deb/rpm 装→派活→急停→收通知→卸载 | 实测 + CI ubuntu 冒烟 | 全链通过；产物大小记录（AppImage 自包含 WebKit 属预期，deb/rpm 轻量） |

##### C5 · 三端发版演练（依赖 C1~C4 全部完成）

| ID | 任务 | 落点 | 验收 |
|---|---|---|---|
| C5-1 | 干净环境矩阵验证：各端全新 VM/容器按 Release 页命令完成全链（下载→校验→安装→首跑 doctor→派活→急停/恢复→通知→卸载） | 发版检查表 | 三端检查表全部勾选、问题清零后才打 tag |
| C5-2 | tag 必须指向包含全部修复的提交；Release 页内嵌三端安装命令与校验（sha256 / 签名指纹 / macOS `xattr` 说明） | Release notes | 任一用户照页可独立完成 |
| C5-3 | CI 三平台 build+clippy 全阻塞保持；Windows/macOS 新增平台面测试从 continue-on-error 提升为阻塞（按 C1-8 稳定度推进） | ci.yml | 跨平台回归成为硬门 |

**下发顺序与依赖**

```
C0 发布基建/凭据/版本单源（先做，一次性）
   ├── C1 Windows Job Objects + Authenticode（独立）
   ├── C2 macOS Developer ID/公证/universal（独立，与 C1 并行）
   └── C3 托盘与窗口收口（依赖 B1，可与 C1/C2 并行）
            └── C4 Linux 收尾（依赖 C3）
C1 + C2 + C3 + C4 全部完成 → C5 三端发版演练（一次性验收）
```

- 关键路径在 C2：universal sidecar（C2-4）与公证（C2-5）串行且依赖 Apple 凭据，C0-2 须最早落实；
- C1 的 unsafe FFI（线程枚举/挂起）集中在 `imp` 模块内，先以最小 feature 集引入 windows-sys，避免依赖膨胀；
- C1/C2 各自完成后先做平台内自测，C5 才做跨端一致性与干净环境总验；每完成一个 ID 即在进度表勾稽，测试同步新增。

#### Sprint C 实施进度（2026-10-05，Windows 侧第一轮迭代）

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | C1-1 [Cargo.toml](../crates/maestro-daemon/Cargo.toml) | windows-sys 0.52 target-gated，feature 最小集（Foundation/Security/Console/ToolHelp/JobObjects/Threading），非 Win 构建零影响 |
| ✅ | C1-2 [worker.rs](../crates/maestro-daemon/src/worker.rs) Win imp | 每 worker 一个 kill-on-close Job；`BasicProcessIdList` 枚举整组（孙进程自动继承资格）；Job 失败 best-effort 降级单进程并打日志 |
| ✅ | C1-3 worker.rs Win imp | 真冻结：枚举 Job 内进程逐线程 `SuspendThread`（句柄入注册表，unfreeze 逐个 Resume 配对释放，Suspend 计数不悬空） |
| ✅ | C1-4 worker.rs Win imp | 两级关闭：CTRL_BREAK（子进程作进程组长，CREATE_NEW_PROCESS_GROUP）→ 超时 TerminateJobObject；无共享控制台时自然落二级 |
| ✅ | C1-5 [emergency.rs](../crates/maestro-daemon/src/emergency.rs) + worker.rs | SNAPSHOT 前 `settle_workdir` 静止确认：Win 轮询目录项指纹（80ms×5），Unix 恒 true 直通；不静止 best-effort 继续 + eprintln |
| ✅ | C1-8 worker.rs 测试面 | 5 个 cfg(windows) 进程面测试：生命周期对齐 / 冻结-写入停止-恢复 / 孙进程整组终止 / 两级关闭收敛 / 注册表语义 —— 急停关键行为首次在 Win CI 可自动验证（`test-cross` windows job 从 continue-on-error 转正的候选依据） |
| ✅ | C1-8 后半：CI windows job 转正（2026-10-05 第二轮） | 5 项进程面测试在 GitHub **x64** runner 实证通过（b13d665 run 全绿）→ [ci.yml](../../.github/workflows/ci.yml) 拆出 `test-windows` 独立阻塞 job；macOS 仍 continue-on-error（转正留 C5-3） |
| ✅ | C0-4 release.yml 首跑修复（2026-10-05 第二轮） | v0.2.4 tag 首跑暴露 3 失败：mock-cli 误用 `-p maestro-desktop`（不在 workspace）→ 改 `--manifest-path`；Linux 改复用 B4 [build-linux.sh](../desktop/scripts/build-linux.sh)（单一事实源）+ apt 依赖对齐 Tauri v2 官方清单（ayatana-appindicator） |
| ✅ | C0-3 [sync-version.cjs](../desktop/scripts/sync-version.cjs) | 版本单源：tauri.conf.json 为源，一键同步 package/package-lock(2处)/Cargo.toml；幂等，替代手工四处改 |
| ✅ | C0-2 [.env.example](../../.env.example) + [sign-windows.ps1](../desktop/scripts/sign-windows.ps1) + .gitignore | 凭据模板入库（真值 .env 已 git-ignore）；签名脚本无凭据时大声降级（UNSIGNED + exit 0）而非静默产出，契合「不静默降级」契约 |
| ✅ | C0-4 [release.yml](../../.github/workflows/release.yml) | tag 触发骨架：Win x64+arm64 / macOS / Linux x64 矩阵构建 → 签名槽 → 产物归档 → dry-run 门；发布仍走本地流水线（C5 收口） |
| ✅ | C0-1 发布矩阵 | 落在 release.yml 矩阵注释（CI 变量形态，任务书允许的二选一）；prepare-sidecars 两脚本均已 -Triple 参数化 |
| ⬜ | C1-6 Authenticode 实签 | 脚本/降级契约就绪，**等真实证书**（C0-2 凭据到位即可签） |
| ⬜ | C1-7 mock-cli 剔除 | **计划冲突待决策**：mock-cli 是桌面「演示模式」的内置 worker（[daemon.rs](../desktop/src-tauri/src/daemon.rs) 缺失直接报安装不完整），剔除将破坏无 API key 的体验路径与 GUI 回归基线 —— 需产品层先定义「演示模式」的发布语义（保留 / dev-only 构建 / 移除并改写回归） |
| ⬜ | C1 包体 <20MB 核验 | 当前 arm64 NSIS ≈3.02MiB（含 mock-cli），达标但按 C1-7 决策后需复测 |
| ✅ | C3-1~C3-3 [tray.rs](../desktop/src-tauri/src/tray.rs) + [lib.rs](../desktop/src-tauri/src/lib.rs) + [events.rs](../desktop/src-tauri/src/events.rs) | tray-icon feature 托盘常驻：菜单六项 + 左键恢复；关窗 = prevent_close + hide（任务照跑）；急停态/未读数随事件桥与既有 poller 刷新（无新增轮询），菜单动作经 `maestro://tray` 交前端复用确认流 |
| ✅ | C3-4 [tools.rs](../desktop/src-tauri/src/tools.rs) | 后缀/PATH 分隔符/claude 候选按平台分支（triple 候选在前）；dev 布局双候选目录探测；+2 单测 |
| ⬜ | C2 全部（macOS 签名/公证/universal） | 需 Apple 凭据 + Mac 侧执行（C0-2 模板已含 notarytool 三元组占位）；**由 macOS 端另行开发** |
| ⬜ | C4 Linux 收尾 | **由 Linux 端另行开发**（SNI 托盘在 Tauri 壳由 C3 共享覆盖大半，实测项留 C4） |
| ⬜ | C5 三端发版演练 | 依赖 C2/C4 完成后跨端总验 |

验证（第一轮）：daemon **103 项全绿**（含 5 个新进程面测试）、workspace 全量 **126 项 0 失败**、clippy 零告警。`test-cross` 的 windows job 现在能跑到 worker 进程面测试（此前 cfg 门控下为空转）。

技术债收口（2026-10-05 第二轮迭代，a6953e2）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **急停状态重启持久化**（长期遗留） | 协议新增 `EmergencyResumed` 事件（macOS Swift 端 switch/default 容错未知类型，旧端安全）；`Authority.emergency_frozen` 派生字段（apply/replay 同源）；`Core::recover` 据此重建急停相位 —— 修复「急停后 daemon 重启即忘记急停，冻结期 B12 拦截失效」；+3 测试（wire/派生/两代重启往返） |
| ✅ | CI 转正 + release 骨架修复 | 见上表 C1-8 后半 / C0-4 行 |

验证（第二轮）：daemon **105 项全绿**（+2）、protocol 13 项（+1）、workspace 0 失败、clippy/tsc 零告警。

第三轮迭代（2026-10-05，71b6b42 + c47fe49）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **U5 结果卡**（B1-5，C1 前半） | daemon task_get 增量返回 `workdir`（旧客户端宽松解析）；桌面 `task_diff` 命令（baseline checkpoint → 工作区 stat+patch，96KB 钳制 + UTF-8 边界截断，quotepath=false 防 CJK 转义）；新组件 [ResultCard.tsx](../desktop/src/components/ResultCard.tsx)：一句话摘要 + 验收门状态 + 花费 + 节省比例 + diff 按需加载，挂 TaskDetail 顶部；+2 单测（numstat/UTF-8 截断） |
| ✅ | release dry-run 三跑失败修复 | arm64 Sign 步骤漏 `working-directory: desktop`（仓库根跑 → `$exe` null）→ 补齐；NSIS 产物本身正常（3.10 MiB） |

验证（第三轮）：daemon **105 项全绿**、desktop 4 项（+2）、tsc 0 错；CI push 门（c47fe49）全绿。

第四轮迭代（2026-10-05，C2 hunk 级部分接受）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **C2 inline diff + hunk 级部分接受**（Sprint C 主干，[commands.rs](../desktop/src-tauri/src/commands.rs) + [ResultCard.tsx](../desktop/src/components/ResultCard.tsx)） | 前后端同契约解析 patch（`diff --git` 分文件 / `@@` 分 hunk / 全局序号）；`task_diff_revert` 一条龙：被拒 hunk 拼子 patch → `git apply --reverse` 撤销 → 同 workdir 自动转修正任务（prompt 附被拒段落 + 理由，2KB/hunk 钳制）；UI 逐 hunk 勾选 + 理由必填 + 拒绝后 diff 自动重载 |
| ✅ | **untracked diff 误报修复**（U5 遗留 bug） | 根因：baseline capture `add -A` 烧入 untracked，而 `git diff <ref>` 只比 tracked → untracked 文件全部误报「删除」（demo-result.md 存在却 +0 −365，reverse apply 撞存活文件）→ 临时 index 三步法（read-tree baseline → add -A → write-tree → diff-tree），真 index 零扰动；同时排除 `.maestro/` 运行时噪声 |
| ✅ | 测试面 | desktop 8 单测（+4：split_patch/build_reject 跨文件与越界/followup_prompt）+ GUI 全链路实测脚本 [uitest-c2.cjs](../desktop/scripts/uitest-c2.cjs)（建任务→done→勾选 hunk→拒绝→toast+修正任务卡+diff 重载全 PASS）；done 判定修 `.st.done` class（narrative「第 N 轮完成」含「完成」二字致假阳性） |

验证（第四轮）：desktop 8 项全绿、tsc 0 错、GUI C2 全链路 PASS（零页面错误）。

第五轮迭代（2026-10-06，C5 反馈闭环 U7 v1）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **C5 反馈闭环**（U7 v1，[core.rs](../crates/maestro-daemon/src/core.rs) + [ResultCard.tsx](../desktop/src/components/ResultCard.tsx)） | 协议新增 `task.feedback`（👍/👎 + 理由，👎 必填双保险：daemon 400 + 前端禁用）；daemon `api_task_feedback`：写 `workdir/MAESTRO_MEMORY.md`（标题/理由压平单行防 markdown 注入）+ publish `FeedbackRecorded`；**先写文件后发事件**防「事件入账但记忆缺失」半态；前端结果卡三态反馈区（未反馈/👎 理由输入/已反馈），已反馈从事件流恢复（重开详情不失） |
| ✅ | 测试面 | daemon +1 测试（负面 400 / 未知任务 404 / 事件入库 replay 验证 / 记忆文件内容与换行压平断言）；GUI 全链路脚本 [uitest-c5.cjs](../desktop/scripts/uitest-c5.cjs)：👎 必填→提交→toast+已反馈态+记忆落盘、👍 直发→（无备注）落盘、重开详情事件流恢复，9 项全 PASS（零页面错误） |
| ✅ | 清理 | desktop 移除 C2 遗留 dead code `git_bytes`（release 构建零警告） |

验证（第五轮）：daemon **106 项全绿**（+1）、desktop 8 项全绿零警告、tsc 0 错、GUI C5 反馈流 9/9 PASS。

第六轮迭代（2026-10-06，C3 断点续跑 v1）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **C3 断点续跑 v1**（U6，[core.rs](../crates/maestro-daemon/src/core.rs)） | 轮完成即落 `RoundStart` 锚点（含 .maestro session；滚动 GC 最近 3 个，pinned 不受影响）；`api_resume` 放行 Failed：回滚到最近锚点（无则退 baseline；restore 自带 pre_rollback 安全垫可撤销）→ `TaskRequeued` 重新入队 → respawn 的 rounder 从 session 续接（LLM 记得已完成轮），**只重做失败轮、已完成部分保留**；无锚点（非 git workdir）不回滚照常续跑。GUI 零改动——failed 卡既有「▶ 重跑」按钮从 409 拒绝变为真正可用 |
| ✅ | 测试面 | daemon +3：锚点滚动 GC / 主径回滚（失败轮半成品清除 + session 恢复到上一完成轮 + TaskRequeued 入账）/ 无锚点边界；daemon **109 项全绿**、workspace 0 失败、clippy 零告警 |
| ✅ | 事故沉淀 | Actions 配额预检脚本 + SKILL 步骤 0（spending limit 掐断 CI 零日志秒失败的实证）；仓库转公开后 CI 免费不限量 |

验证（第六轮）：daemon 109 全绿（+3）、clippy 0 告警；CI 三平台 6 job 全绿（37409164137，含 ubuntu/macOS e2e——rounder 续接语义 CI 收尾确认）。

第七轮迭代（2026-10-06，C4 三级记忆 v1）：

| 状态 | 交付 | 说明 |
|---|---|---|
| ✅ | **C4 三级记忆 v1（记忆优先）**（[core.rs](../crates/maestro-daemon/src/core.rs)） | spawn 统一 choke point `compose_spawn_prompt` 注入：用户级 `~/.maestro/memory.md` + 项目级 `<workdir>/MAESTRO_MEMORY.md`（C5 反馈落盘处），每层尾部 8KB（UTF-8 边界安全截断，append-only 尾部 = 最近记忆）；任务级 = session 续接（已有，不重复注入）。顺序 = 记忆 > 轻推 > 正文（记忆是背景上下文，轻推紧贴指令）；两层皆缺零开销；读失败不阻塞 spawn。**反馈→记忆→续跑闭环生效**：C3 respawn 自动带上 👎 教训 |
| ✅ | 三端并行开发启动 | 仓库转 public（CI 免费不限量）；[PARALLEL_PLAN.md](./PARALLEL_PLAN.md) 分工与协作契约上线；81 休眠 fork 归档归类 |

验证（第七轮）：daemon **112 全绿**（+3：read_tail UTF-8 边界 / memory_prefix 分层 / compose 顺序）；CI 待 push 验证（e2e 盲区惯例）。

### 10.5 模型目录：运行一次即获取免费 / 低价 token 的 API 与模型

> 解决「去哪找不要钱和便宜的模型、在客户端怎么填」的问题。
> **同步工具**：[tools/sync-model-catalog.mjs](../tools/sync-model-catalog.mjs)（Node 22，零依赖）
> `node tools/sync-model-catalog.mjs` 一次运行产出：
> - [docs/model-catalog.json](./model-catalog.json)：机器可读（直连 provider / 免费 / 低价三组）
> - [docs/MODEL_CATALOG.md](./MODEL_CATALOG.md)：**每个 API 与模型的填写示例**（BASE_URL / KEY 环境变量 / MODEL + curl 实例）

数据源（2026-10-05 实测）：

| 来源 | 形态 | 当前数量 |
|---|---|---|
| 内置直连 provider 免费层（Groq / Cerebras / Gemini / Mistral / GitHub Models / Cloudflare / SiliconFlow / ModelScope / Cohere / NVIDIA / SambaNova） | 人工核实，含注册地址与限额 | 11 家 |
| OpenRouter 公开目录 `GET /api/v1/models`（**无需 key**，本机直连实测 466 模型） | 按 price=0 切免费 | 22 个 |
| 同上，按输入 < $0.15/M 切低价价值模型 | 按价格排序取前 15 | 15 个 |

关键事实：① OpenRouter 免费名单按 **price**（`pricing.prompt/completion` 双零）筛选比按 `:free` 后缀多出 4 个（含 `openrouter/free` 自动路由器与 2 个预览模型）——dev.to 实测 387 模型时为 21 vs 18；② 免费 roster 高频轮换（旧教程里的 `llama-3.3-70b:free` 等多已转付费），故工具不落死 `:free` id、每次实拉；③ 直连免费档与 daemon 现有 OpenAI 兼容层（llm.rs）直接对接，无需新代码。

后续：GUI「设置 → worker」可一键读 `model-catalog.json` 下拉选模型（B1 任务，填好 BASE_URL/MODEL 即生效）。

## 十一、决策记录（v1→v2，含 v2.2 重排与 v2.3 增补）

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
| 23 | **【v2.5】省钱引擎对接 CCR，不自研模型路由** | CCR（MIT）已有路由/key 池/fallback/成本核算且保持更新；自研重复造轮子且需长期追 provider 协议；以 sidecar/网关复用，daemon 专注编排与预算 |
| 24 | **【v2.5】Linux GUI 选方案 A：复用 Tauri/React 工程** | 三端共用一套前端，避免维护第三套原生 UI；最快补齐 Linux 桌面空白并保证界面统一 |
| 25 | **【v2.5】Server 放弃 nginx 补丁，Rust 借鉴架构独立实现** | 子进程编排触碰 nginx worker 非阻塞红线；构建链/ABI 与单二进制承诺冲突；以标准 HTTP `/health`/`/metrics` 作为与 nginx 共存契约 |
| 26 | **【v2.5】新增 Foreman 式 cost/time/turn 硬预算闸门** | 软预算只报告不阻止，失控时仍烧钱；硬闸门超限即挂起（复用 `SuspendReason::BudgetExceeded`，仅手动恢复），是「敢放手」的硬保障 |
| 27 | **【v2.5】观测走 OpenTelemetry + Langfuse，不自建观测 UI；agent 生态走 ACP** | 标准协议不锁定厂商；ACP 一次接入整个 agent 生态，避免逐 CLI 写方言 |
| 28 | **【v2.6】免费/低价模型走「同步工具实拉目录」而非仓库内置死清单** | 免费 roster 高频轮换（旧 `:free` id 数月即失效）；OpenRouter 公开目录无需 key、按 price 筛选比 `:free` 后缀更全；工具每次实拉 + 内置直连 provider 兜底，仓库只存生成快照 |
| 29 | **【v2.6】事件桥首订阅加「前端就绪」门控（bridge_ready）** | 回归 A 项暴露竞态：daemon 存活的接管路径下，bridge 在 setup 阶段立即 `subscribe(from_seq=1)`，全量重放瞬时爆发，早于 React `listen()` 注册的事件全部丢失（事件流空白；spawn 路径因等 daemon 就绪侥幸避开）。前端监听器注册后 invoke `bridge_ready` 再放行首订阅，从协议层消除竞态 |
| 30 | **【v2.7】Windows 冻结选 Job Objects + 逐线程挂起，不用 NtSuspendProcess 单进程方案** | 急停语义是整组（含内层 CLI 派生的孙进程）静止；仅挂直接子进程会让快照期孙进程继续写盘，emergency 快照前提不成立。Job 统一收纳进程树，句柄挂现有 Child 注册表，`imp` 模块内闭环、门面与 emergency 调用方零改动 |
| 31 | **【v2.7】macOS 走 Developer ID + notarytool 官方公证链，universal 以 lipo 合并产出** | ad-hoc 包在 Apple Silicon 新机被 Gatekeeper 直接拦截，无法公开分发；公证要求 bundle 内全部可执行文件（含三个 sidecar）由内向外签名，故 sidecar 先双架构构建再 lipo，一个 DMG 覆盖 Intel/Apple Silicon |
| 32 | **【v2.7】Tauri 壳启用 tray-icon，关窗最小化到托盘（三端一致）** | daemon 已独立常驻且支持接管，但壳层关窗即退与「关窗任务照跑」承诺矛盾；托盘行为对齐 macOS 已实现的 NSStatusItem，菜单/角标三端统一，复用事件桥刷新而不新增轮询 |

## 十二、下一步

### 9.1 开工序列

1. P0.1 cargo workspace 骨架（四 crate + CI）+ **测试基建**（MockClock/FakeAdapter/脚本 Worker/pgid 沙箱，设计 §12——用例组 A/B 全依赖它）——**落地指南（目录结构/依赖清单/CI/自查清单）见 [P0_SETUP.md](./P0_SETUP.md)**
2. P0.2 protocol schema 契约先行（10 API + 事件类型 + token 账本事件 + UX 埋点 + **suspend/resume/emergency 事件与 API，按 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md) §2.3/§3.2**）

### 9.2 调研任务清单（R1~R5）

| # | 任务 | 关键问题 | 方法 | 交付物 | 影响决策 | 完成门 |
|---|---|---|---|---|---|---|
| R1 | rquickjs 嵌入摸底 | 嵌入 API/异步桥/二进制增量实测是否 ~2MB | 最小示例嵌入 + 测体积 | 技术备忘 + go/no-go（备选：wasmtime wasi-mini） | P1-B6 Code Mode 选型 | P1 Sprint B 开工前 |
| R2 | 小模型 headless CLI 可用性 | Haiku/Flash/4o-mini 经各家 CLI headless 调用的支持度与 stream-json 格式差异 | 各 CLI 实测 | 适配器矩阵表 → 回填 0.6/B7 适配器计划 | P1-A6 模型路由 L1 池 | P1 Sprint A 开工前。**2026-10-04 文档层调研完成**（实测待 key）：**Amp 完全兼容 claude 事件格式**（`--stream-json`，仅换参数构造 `-x` 与续接命令 `threads continue`）、**CodeBuddy 同构**（`-p`/`--output-format stream-json`/`--resume`）；Codex（`codex exec` + `--json` JSONL，`turn.completed` 含 usage，`exec resume <uuid>`）、Gemini（`-p` + `--output-format stream-json`，退出码语义最丰富：42=输入错/53=轮次上限，`--resume <uuid>`）、OpenCode（`run` + `--format json` NDJSON，`--session`/`--fork`）需独立事件解析器；Crush 无 JSON 输出（低优先级）。Dialect trait 四抽象点：参数构造（prompt 位置/flag + 权限 flag + 单轮控制——`--max-turns 1` 为 claude 独有）、输出格式（事件命名/usage 位置/session id 字段三处分叉）、续接语义（flag vs 子命令）、错误分类（退出码映射 + error 事件） |
| R3 | 多轮驱动验证 | `claude -p` 单轮 + `--resume` 续接：上下文保持度？每轮成本递增？轮间可注入 steering？并发 resume 行为？ | **验证协议见 [R3_PROTOCOL.md](./R3_PROTOCOL.md)**：9 场景矩阵（Block 项/记录项分级）+ 提前止损顺序（S1→S3→S7c 先跑）+ 报告模板 | 验证报告 → 0.15 形态 go/no-go/调整；失败出口已定义（回退单轮长跑/单写者锁等） | P0 0.15/0.16（**P0 内最优先**） | P0.15 开工前。**2026-10-01 进展：机制层 harness 完成**（testkit::r3 Driver + mock CLI，S1/S2/S3+S4/S6/S7a/S7c 6 场景绿；真实 CLI 协议待 API key，Driver 参数已兼容） |
| R4 | **Gemini off-peak 触发机制**（设计 §5 前提） | ① 折扣是自动计费（按时窗）还是需参数/API 开关？② 窗口定义与单位（UTC？本地？滚动？）③ 适用 SKU 与比例（是否全线 -50%）④ 与 Gemini 自家 Batch 的叠加规则 | 官方定价页/API 参考文档研究 + 活探针：测试 key 在窗内/窗外各发最小请求，对比 usageMetadata 与计费字段 | `discount-catalog.toml` 首条实测数据 + 设计 §5 表更新为「已验证」 | **2.17 off-peak 窗口管理实现方式**（自动计费→daemon 只排时间；参数开关→适配器传参）；A4 审批门 ETA 的时区计算 | P2 2.17 开工前；文档部分可立即做 |
| R5 | **Batch API 工具调用支持边界**（设计 §7.2 前提） | ① batch item 是否支持 tools/tool_choice？② 单 item 能否多轮工具循环，还是一问一答？③ 输出文件中 tool_calls 格式 ④ 限额（OpenAI 50k 请求/200MB/batch）与分片规则 ⑤ 已提交 batch 能否撤销 ⑥ item 级失败语义与计费 | 文档 + 活探针：提交 3-item 批次（1 个带工具调用、1 个多轮、1 个普通），解析输出文件 | eligibility 检查清单（哪些步骤形态可走 batch）→ 回填 2.17 deferrable 分类器；设计 §7.2 约束表改「已验证」 | **batch 路径宽度**（若工具不支持→收窄为纯分析步骤，仍有价值）；**急停与在途 batch 的交互**（若不可撤销→处置定为「标记+轮询」而非「取消」，用例 B14 按此落地） | P2 2.18 开工前；结论影响设计 §3/§7 两处 |

> R3 是 P0 唯一的技术风险项，优先级最高；R4/R5 的文档研究部分可提前做，活探针需测试 key。
