# Maestro 三端并行开发计划（Sprint C 收官轮）

> 2026-10-06 制定。仓库已转 **public**（CI 三平台免费不限量），Windows 侧引擎核心已连续六轮迭代全绿，剩余任务按端拆分、并行收口。
> 本文是三端并行的**分工与协作契约**；任务详细定义沿用 [DEV_PLAN.md](./DEV_PLAN.md) 既有 ID（注意功能面 C1~C7 与平台面 C0~C5 两套编号并存，下文均已标注）。

## 一、基线（2026-10-06，master = 62f6f42）

| 已交付 | 证据 |
|---|---|
| U5 结果卡 + inline diff + hunk 级部分接受（功能 C1/C2） | ResultCard.tsx + task_diff/task_diff_revert，GUI 全链路 PASS |
| C3 断点续跑 v1（功能 C3） | 轮 RoundStart 锚点 + api_resume 放行 Failed（回滚→重排队→session 续接），daemon 109 测试全绿 |
| C5 反馈闭环 v1（功能 C5，U7） | task.feedback 👍/👎 + 理由落 MAESTRO_MEMORY.md + FeedbackRecorded 事件，GUI 9/9 PASS |
| C0 发布基建 + C1 Windows 真冻结（平台面） | windows-sys Job Objects 真冻结/整组终止/两级关闭 + CI windows test 转正 + release.yml dry-run 三平台全绿 |
| C3 托盘（平台面，三端共享） | tray.rs：菜单六项 + 关窗到托盘 + 事件驱动角标 |
| 版本 v0.2.4 安装包 + CCR 网关 + 模型目录 | 见 Release 与 docs/MODEL_CATALOG.md |
| Linux 端 C4 代码面（C4-1/2/3 降级与集成 + CI deb 冒烟） | XDG 数据目录修复、托盘 Result 化 + 关窗降级分支、单实例、.desktop 模板 + deb/rpm depends、Alt+M + Wayland 设置页引导、ci.yml bundle-smoke-linux；实测面（GNOME/KDE、fcitx5、三形态全链）留手动清单 —— DEV_PLAN 第七轮 |
| macOS 端 universal 双架构出包 + 签名链无凭据部分 + 新事件认知 | 见 928d74c |
| C6 缓存三件套 + 省 token 报告（功能 C6） | ledger_summary 双口径 API + 完成通知带花费/节省 + 30 天省钱徽标，daemon 114 测试全绿，CI 37417546943 全绿（fac520e） |
| C7 基准集 v0（功能 C7 Phase 1+2） | crates/maestro-bench：10 任务套件（6 Rust + 4 Node）+ 红绿不变量自校验 + mock 全链 runner（daemon+rounder+bench-worker）+ JSON/MD 报告，10/10 PASS 验收率 100% |
| 测试基线 | workspace 全量 0 失败；CI 三平台 6 job 全绿（37409164137） |

## 二、三端任务分配

### Windows 端（本机）——引擎核心 + Windows 平台收尾

| 优先 | 任务 | ID | 落点 | 验收 |
|---|---|---|---|---|
| P0 | **三级记忆 v1**：MAESTRO_MEMORY.md 注入 prompt（记忆优先）—— C5 反馈数据已落盘，注入让闭环生效；用户级 ~/.maestro/memory.md + 项目级 + 任务级三层 | 功能 C4（v1 范围） | daemon core.rs spawn_worker_for 组装 prompt 前缀 | 有记忆的任务 prompt 含项目记忆段；e2e 断言注入内容 |
| P0 | **缓存三件套 + 省 token 报告**：prompt 前缀稳定化（系统提示固定前置）/ 结果缓存 / 会话缓存利用统计 + 通知带节省数据 + 月度汇总 | 功能 C6 | daemon llm.rs/core.rs + ResultCard/NotificationCenter | Sprint C 出口断言：token 降 ≥50% 有账本数据支撑 |
| P1 | **效果基准集**：20 任务基准 + CI 每次发版跑 + 协议 generation 冻结 | 功能 C7 | testkit 或新 benchmarks crate + ci.yml job | 基准可复跑；验收通过率 ≥98% 报告产出 |
| P2 | Win11 Mica/Acrylic + 跟随系统深浅色 | B2-2 | tauri.conf 窗口效果 | Win11 生效、旧版自动降级 |
| P2 | WebView2 IME composition 实测（拼音组词不丢字/回车不误提交） | B2-4 | 实测记录 uitest | 中文输入全链正常 |
| P2 | Alt+M 全局快捷键 Windows 注册 | B2-3 | src-tauri | 任意界面可唤起 |
| 阻塞 | Authenticode 实签（证书到位即做，脚本契约已就绪） | 平台 C1-6 | sign-windows.ps1 + release.yml | SmartScreen 无未知发布者 |
| 阻塞 | mock-cli 发布语义决策（保留演示模式 / dev-only / 移除） | 平台 C1-7 | 产品决策 → 实施 | 决策后包体 <20MB 复测 |

### macOS 端——签名公证链 + Swift 客户端跟进

| 优先 | 任务 | ID | 落点 | 验收 |
|---|---|---|---|---|
| P0 | **凭据落实**：Apple Developer Program + Developer ID Application 证书 + notarytool 凭据（App-specific password 或 API Key），按 .env.example 占位注入 | C0-2（mac 侧） | CI secrets / 本地 keychain profile | `security find-identity` 可见证书 |
| P0 | **签名公证链**（凭据前可先做 1~4 无凭据部分）：①Bundle ID 统一 com.yuanzimu.maestro ②hardened runtime + entitlements ③sidecar 逐二进制由内向外签名 ④universal lipo（Rust 双架构 lipo 三件套 + Swift 双 arch）⑤notarytool submit + staple ⑥DMG 签名公证 | 平台 C2-1~6 | clients/macos/build.sh + Maestro.entitlements | 全新用户机 `xattr` 干净、`spctl -a -vvv` 接受、双架构实机可跑 |
| P1 | ScenarioRunner 100 场景在签名 universal 包回归（Apple Silicon + Rosetta x86_64） | 平台 C2-7 | Package.swift harness | 双架构关键旅程通过 |
| P1 | **Swift 端新事件认知跟进**：AppState 认 emergency_resumed；typeNames 补标签：feedback_recorded（反馈 👍/👎）、task_requeued（断点续跑重排队）、context_compacted（上下文已压缩）、round_start 锚点类 checkpoint 变化 | 随功能 C3/C5 | AppKitUI.swift + typeNames 表 | 新事件在 mac 客户端有人话标签，无 unknown 噪声 |
| P2 | Checkpoint 时光机 UI v0：任务级时间线（checkpoint × 叙事标签）+ 恢复到此点（pre-rollback 确认）—— C3 落地后 RoundStart 锚点已有谱系素材 | B10 | Swift 客户端 | 时间线可扫视、恢复可撤销 |
| P2 | 深浅色自动（system colors 替换硬编码） | B3-2 | AppKitUI.swift | 切换系统外观即时生效 |

### Linux 端——桌面集成收尾 + 安装体验

| 优先 | 任务 | ID | 落点 | 验收 |
|---|---|---|---|---|
| P0 | SNI 托盘实测：GNOME/KDE（AppIndicator 兼容）可见可用；无托盘宿主（精简 WM）优雅降级不报错 | 平台 C4-1 / B4-3 | bundle/desktop + 实测记录 | GNOME/KDE 托盘菜单六项可用 |
| P0 | `.desktop` 元数据收尾：Categories/StartupNotify/单实例 + deb/rpm 依赖（libwebkit2gtk-4.1-0、libgtk-3）postinst/remove 干净 | 平台 C4-2 | tauri.conf linux 段 | 菜单可启动、卸载无残留 |
| P1 | 全局快捷键 XDG 实测：X11 与 Wayland 下 Alt+M（Wayland 无全局注册时给设置页引导而非静默失败） | 平台 C4-3 / B4-4 | src-tauri + 实测 | 两协议行为明确 |
| P1 | fcitx5 输入法实测（含 xrdp/XFCE）：拼音组词不丢字、回车不误提交 | 平台 C4-4 / B4-5 | 实测记录 | 中文输入全链正常 |
| P1 | 三形态全链复验：AppImage 免安装直跑（无 FUSE 给 --appimage-extract 指引）/ deb / rpm 装→派活→急停→收通知→卸载 | 平台 C4-5 / B4-6 | 实测 + CI ubuntu 安装包冒烟 | 全链通过 + 产物大小记录 |
| P2 | CI ubuntu job 增安装包冒烟（build-linux.sh 产物装包→doctor→跑一任务） | 平台 C5-3 前置 | ci.yml | 每次发版 Linux 包自动验证 |

### 三端联合（平台任务全完后）

| 任务 | ID | 说明 |
|---|---|---|
| 干净环境矩阵演练：三端各全新 VM/容器 按 Release 页命令全链（下载→校验→安装→doctor→派活→急停/恢复→通知→卸载） | 平台 C5-1 | 检查表三端勾选、问题清零 |
| tag 发版 + Release 页三端命令与校验（sha256/签名指纹/xattr 说明） | 平台 C5-2 | v0.3.0：Sprint C 出口断言全部成立后 |
| 基准集证明 token 降 ≥50% 且验收通过率 ≥98%（Windows 建 C7，ubuntu CI 跑批） | Sprint C 出口 | 开源 announce 时机 |

## 三、并行协作规则（三端共守）

1. **Git 流**：小任务（≤1 天）直接 master——push 前 `git fetch origin && git rebase origin/master`，冲突优先保留双方意图；长任务（>1 天，如 mac C2 签名链、win C6）开 `feat/<端>-<任务>` 分支（如 `feat/macos-c2-signing`），完成后再 rebase 回 master。
2. **CI 硬门**：push 后**必须盯三平台 6 job 全绿**才收尾（仓库已 public，CI 免费）。⚠️ e2e 全部 `#![cfg(unix)]`——daemon/协议改动 Windows 本地全绿 ≠ CI 绿，以 ubuntu/macOS job 为准（实证教训：急停持久化修复曾本地全绿、CI 双平台挂）。
3. **协议双轨制**：events.rs/api.rs 只追加不删除；新增事件类型时 mac Swift 端 switch/default 安全忽略，但需在 typeNames 补人话标签（mac P1 任务）。改动 daemon/协议的 commit message 里注明「protocol: 新增 X」。
4. **冲突高危区**：workspace 根 Cargo.toml（加依赖）与 daemon core.rs（win 端 C4/C6 串行内部消化）；desktop/ 由 Windows 主导、Linux 动 tauri.conf linux 段前先 fetch；DEV_PLAN.md 勾稽表各端只在对应轮次小节追加。
5. **勾稽**：每完成一个 ID 在 [DEV_PLAN.md](./DEV_PLAN.md) 进度表勾稽 + 测试同步新增；重要成果在本文件「基线」表补一行。
6. **提交规范**：`feat(scope):` / `fix(scope):` 中文主题行（与仓库历史一致）；commit message 经 `-F <file>` 无 BOM 写入。
7. **配额**：CI 已免费不限量；dry-run/发版仍先跑 `node tools/check-actions-quota.cjs` 习惯（public 后该检查天然通过，作为断言保留）。

## 四、里程碑

| 里程碑 | 内容 | 判据 |
|---|---|---|
| M1 | win C4 三级记忆 + C6 缓存落地；mac C2-1~4 无凭据部分 + 凭据申请；linux C4-1~2 托盘与 .desktop | 各端 P0 首项完成 |
| M2 | mac C2-5~7 实签公证 universal；linux C4-3~5 全链；win C7 基准集 v0 | 平台面 C2/C4 全勾 |
| M3 | C5-1 三端干净环境演练 + C5-2 发版 | v0.3.0 tag + Release |
| M4 | Sprint C 出口断言：基准 token 降 ≥50% / 验收通过率 ≥98% / 三平台包 <20MB | 开源 announce |

## 五、外部阻塞清单（用户动作）

| 项 | 阻塞谁 | 动作 |
|---|---|---|
| Windows 代码签名证书（Authenticode） | win 平台 C1-6 | 采购证书 → .env 真值 |
| Apple Developer Program + Developer ID + notarytool 凭据 | mac 平台 C2 全链 | 加入开发者计划、生成证书与 App-specific password |
| ~~mock-cli 发布语义~~ **已决策（2026-10-06）：dev-only** | ~~win 平台 C1-7~~ | 决策 33：仓库/CI 保留、发行包剔除。**已实施（2026-10-07）**：mock-cli 抽为根 workspace crate + tauri.release.conf.json overlay 三端出包（desktop crate 内 bin 会被 Tauri 全量打包，externalBin 剔除不了）；包体复测留 release 实跑 |
