---
name: "maestro-release-pipeline"
description: "Maestro Windows 客户端的同步-审计-构建-测试-发布闭环。当用户要求拉取远程最新、审计修复、出安装包、发版本，或说『按流程走一遍/发个新版』时调用。"
---

# Maestro 发布流水线（Windows ARM64）

一句话执行的完整闭环：**同步远程 → 审计 → 构建 → 测试 → 发布**。未经用户明确要求不要创建 git commit/tag/release；但用户说「直接执行/发新版」时一路做完。

## 环境事实（务必遵守）

- **Git 仓库（挂载盘）**：`c:\Mac\Home\Documents\windows_trae_projects\maestro`
  - ⚠️ 此盘 **cargo build 报 os error 87，不可构建**；只做 git 提交与编辑。
- **构建镜像**：`C:\dev\maestro`（源码同步副本，cargo 在此构建）。
- **跨盘复制**：用 .NET 方法，最可靠：
  `[System.IO.File]::Copy($src,$dst,$true)`
  - 不要用 `tar`（无法穿透挂载符号链接）；不要用管道式 tar 中转（中文变 `?`）。
  - 若 Copy-Item 被去重/取消，换 `[System.IO.File]::Copy`。
- **git 路径**：`C:\Program Files\Git\cmd\git.exe`（PATH 里可能没有）；所有命令加 `--no-pager`。
- **MSVC 环境**：先 `. C:\dev\maestro\desktop\scripts\build-env.ps1`（cl 指向 Hostarm64\ARM64）。
- **Node**：`C:\Users\a1234\node-v22.14.0-win-arm64\node.exe`。
- **CARGO_TARGET_DIR**：workspace=`C:\cargo-target\maestro`；desktop=`C:\cargo-target\maestro-desktop`。
- **PowerShell 5.1 坑**：不支持 `&&`/heredoc；脚本必须 **ASCII-only**（无 BOM，中文注释会被按 GBK 误读吞换行）；commit message 用 `-F <file>`，不要内联多行。
- **Wire 协议**：daemon 是**裸 TCP + 换行 JSON**，不是 HTTP（PowerShell `Invoke-RestMethod`/Node `http` 都会报协议错）；测试用 Node `net` 直连。

## 步骤 0：Actions 额度预检（跑任何 dry-run / 触发 CI 前）

私有仓库 Actions 按倍率计费（Linux 1x / Windows 2x / **macOS 10x**），免费版每月仅含 2000 分钟；烧尽且 spending limit 为 0 时 job **全部零日志秒失败**（2026-10-06 实证，annotation 报 "spending limit needs to be increased"）。一次三平台 dry-run ≈ 100~200 计费分钟。

```powershell
& "C:\Users\a1234\node-v22.14.0-win-arm64\node.exe" tools\check-actions-quota.cjs
```

- 退出码 1 = 额度不足，**先停手**：到 Billing & plans 提高 spending limit / 结清欠费，或临时转公开仓库（公共仓库免费），再重跑。
- 估算模式比实际偏低约两成（实证：估算 1601 时已被掐）——数字贴阈值时按不足处理，可 `--min-left 400` 提高门槛。
- 令牌加 `admin:billing` scope 后自动切 billing API 权威模式（当前 404 = 缺 scope）。

## 步骤 1：同步远程

```powershell
# cwd = 挂载盘仓库
& "C:\Program Files\Git\cmd\git.exe" fetch origin --tags
# 有本地提交则 rebase（远程由 Mac 侧频繁更新）：
& "C:\Program Files\Git\cmd\git.exe" rebase origin/master
```
冲突优先保留双方意图（如本地 P0 修复 + 远程新增字段），手动合并。随后把挂载盘新增/改动文件用 `[System.IO.File]::Copy` 同步到 `C:\dev\maestro`（只补缺失/更新，不覆盖镜像里已验证但未回写的改动；可先比对两侧文件清单）。

## 步骤 2：审计

- **UTF-8 切字隐患**：搜 `on("data")` 后字符串累加（应 Buffer 拼接或用 `.json()`）；搜字节切片转字符串、`bytes[start..]`（start 可能落在字符中间 panic）。
- **分模块通读**，重点：并发/锁（临时 MutexGuard 是否贯穿含阻塞调用的链式语句）、序号生成（GC 后 `len+1` 会碰撞）、整数溢出（`checked_*`）、错误是否被静默 `let _=`/`continue` 吞掉、子进程/句柄泄漏。
- 可用并行子代理分模块审查，但**每个候选问题必须亲自核验后再改**，避免误报。
- 优先编辑 `C:\dev\maestro` 下文件（编译验证），完成后回写挂载盘。

## 步骤 3：构建

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "C:\dev\maestro\desktop\scripts\rebuild-all.ps1"
```
串联 `prepare-sidecars.ps1`（release daemon/rounder/mock-cli 三件套→`src-tauri\bin\*-aarch64-pc-windows-msvc.exe`）+ `build-installer.ps1`（Tauri build → NSIS）。放后台跑并等结束标记。
产物：`C:\cargo-target\maestro-desktop\release\bundle\nsis\Maestro_<ver>_arm64-setup.exe`。

## 步骤 4：测试

- **Rust 全量**：`. build-env.ps1; $env:CARGO_TARGET_DIR='C:\cargo-target\maestro'; cargo test`（日志 Tee 到文件，检查 `CARGO_EXIT_CODE=0`、无 `FAILED`）。
- ⚠️ **e2e 盲区（2026-10-05 实证教训）**：e2e 全部 `#![cfg(unix)]`，Windows 本地**编译都不覆盖**其内容 —— daemon 行为改动本地全绿 ≠ e2e 绿。改了 daemon/协议就必须**推送后盯 CI 的 ubuntu/macOS job 结果**再收尾（例：急停持久化修复曾因 persistence 用例依赖旧 bug 在 CI 双平台挂）。
- **GUI 回归**（需要时）：先结束 maestro-desktop/daemon，静默安装 `setup.exe /S`；带 CDP 启动：
  `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'`
  playwright-core 已装在 `C:\dev\maestro\desktop`；回归脚本 `scripts\uitest-regress.cjs`（4 项：事件重放/任务详情/急停恢复/轮数轻推）。
  - 安装目录：`C:\Users\a1234\AppData\Local\Maestro\`；数据：`%APPDATA%\maestro-desktop\data\`。

## 步骤 5：发版

1. **升版本号**（桌面发布版，C0-3 单源化）：`tauri.conf.json` 是唯一源，用脚本一键同步四处：
   `node desktop\scripts\sync-version.cjs <ver>`（同步 package.json / package-lock.json 2 处 / src-tauri\Cargo.toml；幂等，可重复跑）。workspace 根 crate 版本（0.1.1）与发布版解耦，**不动**。同步到 C:\dev。
2. commit（`-F` 文件），打 annotated tag：`git tag -a v<ver> -F <msgfile>`。
3. push 前再 `fetch`，远程前进则先 rebase；`git push origin master` + `git push origin v<ver>`。
4. **GitHub Release**：取令牌（不落盘）：
   `git credential fill`（输入 `protocol=https`+`host=github.com`）→ 取 `password=`。
   用 Node + `https` 调 API：POST `/repos/yuanzimu/maestro/releases`（已存在则 PATCH），上传资产走 `uploads.github.com`（`content-type: application/octet-stream`）。令牌只走环境变量。
   参考脚本 `C:\dev\maestro\desktop\scripts\gh-release.cjs` 已参数化，版本/notes 走命令行参数，无需手改：
   `node gh-release.cjs <version> <notes.md>`，如 `node gh-release.cjs 0.2.3 C:\dev\release-v023.md`（notes 为 Markdown 文件，含中文无碍）。

## 已知遗留（不必每次处理）

- ~~daemon 重启后急停状态不持久化~~（2026-10-05 已修，a6953e2：EmergencyResumed
  事件 + Authority.emergency_frozen 派生 + Core::recover 重建相位；若协议再加
  新事件类型，注意 macOS Swift 端靠 switch/default 容错，旧端安全忽略）。
- ~~急停时 git 快照偶发失败~~（v0.2.4 已修：checkpoints 锁冲突退避重试）。
- ~~Windows 无进程组/信号，freeze 为 no-op~~（Sprint C C1 已落地 Job Objects 真冻结）。
- C1-7 mock-cli 是否出发布包待产品决策（当前保留：演示模式内置 worker）。
- C1-6 Authenticode 需真实证书（sign-windows.ps1 降级契约已就绪）。
- release.yml 的 windows-11-arm job 依赖 GitHub ARM64 runner 配额（私有仓库
  注意 larger-runner 配置；首跑 3 失败已修：mock-cli manifest-path / Linux 复用
  build-linux.sh / apt 依赖清单）。
