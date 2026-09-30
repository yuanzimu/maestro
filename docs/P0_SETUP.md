# P0.1 Workspace 与测试基建搭建指南

> 版本 v1.0 · 2026-10-01
> 对应 [DEV_PLAN.md](./DEV_PLAN.md) §9.1-1；测试基建对应 [U10_T6_DESIGN.md](./U10_T6_DESIGN.md) §12（用例组 A/B 全依赖它）

## 一、前置系统依赖

| 依赖 | 用途 | 安装 |
|---|---|---|
| Rust stable（rust-toolchain.toml 锁定） | 全部构建 | rustup |
| cc / build-essential | rusqlite bundled 编译 C 源 | apt install build-essential |
| git ≥ 2.30 | checkpoint 原语直接调 plumbing 命令（不引 libgit2） | 系统自带或 apt |
| claude CLI | 0.6 适配器开发 + [R3 验证](./R3_PROTOCOL.md) | 官方安装脚本 |
| pkg-config | rusqlite/部分构建脚本探测 | apt install pkg-config |

> P0 是 Linux 优先验证；Windows/macOS 交叉在 P1 引入（届时补 windows-sys、named pipe）。

## 二、目录结构

```
maestro/
├── Cargo.toml                  # [workspace] members = crates/* + [workspace.dependencies]
├── rust-toolchain.toml         # 锁定 stable
├── .gitignore                  # target/ data/ *.sqlite-wal
├── .github/workflows/ci.yml    # fmt --check / clippy -D warnings / test
├── crates/
│   ├── maestro-protocol/       # P0.2：纯数据 crate，零 IO
│   │   └── src/
│   │       ├── api.rs          #   方法 schema（schemars）
│   │       ├── events.rs       #   事件类型+UX 埋点+Suspend/Resume/Emergency*（设计 §2.3/§3.2）
│   │       └── types.rs        #   TaskId/WorkerId/SuspendReason/CheckpointRef…
│   ├── maestro-daemon/
│   │   └── src/
│   │       ├── main.rs         #   入口：setsid 脱离 + data_dir 初始化
│   │       ├── clock.rs        #   Clock trait + SystemClock（MockClock 在 testkit）
│   │       ├── server/         #   0.3/0.4：事件循环 mod.rs / ipc.rs / eventhub.rs
│   │       ├── worker/         #   0.5/0.15/0.17：mod.rs / state.rs / recovery.rs
│   │       │                   #         / reaper.rs（孤儿清理）/ multiround.rs（多轮驱动）
│   │       ├── adapters/       #   0.6：mod.rs（Adapter trait）+ claude.rs（stream-json）
│   │       ├── steering/       #   0.16：队列持久化 + 轮间投递
│   │       ├── orchestrator/   #   0.7：队列 v0 + Goal 3 轮
│   │       ├── ledger/         #   0.9：token 账本
│   │       ├── gates/          #   0.10：验收门 v0
│   │       ├── security/       #   0.11：allowlist + session ID 校验
│   │       ├── persist/        #   0.12：事件溯源/去抖/原子写/备份/重放
│   │       ├── checkpoints/    #   0.19：capture/restore/GC（调 git CLI）
│   │       ├── emergency.rs    #   0.18：三阶段 FREEZE/SNAPSHOT/DECIDE
│   │       └── llm/            #   0.14 留位（P0.1 空模块）
│   ├── maestro-client/         # Rust 客户端库（CLI/GUI 共用；连双 socket）
│   ├── maestro-cli/
│   │   └── src/
│   │       ├── main.rs         #   clap
│   │       ├── commands/       #   status/task/worker/inbox/stop/resume
│   │       └── doctor/         #   0.13
│   ├── maestro-testkit/        # 【测试基建】publish = false
│   │   └── src/
│   │       ├── mock_clock.rs   #   MockClock（虚拟时钟+唤醒通知）
│   │       ├── fake_adapter.rs #   注入 stderr/按脚本吐 stream-json/卡任意状态
│   │       ├── script_worker.rs#   脚本 Worker 启动器（独立进程组，记 pgid）
│   │       ├── pgid_sandbox.rs #   进程组隔离/清理（只碰自己 pgid）
│   │       └── fixtures.rs     #   把 fixture 脚本写入 tempdir 并 chmod +x
│   └── maestro-e2e/            # 跨 crate 端到端（publish=false，src/lib.rs 空壳）
│       └── tests/
│           ├── a_suspended.rs  #   A1~A10（设计 §10）
│           ├── b_emergency.rs  #   B1~B14（设计 §11，含 §7.3 在途 batch）
│           └── persistence.rs  #   kill -9 恢复 / steering 不丢
├── docs/                       # 已有
└── reference/herdr/            # 已有（workspace 排除，不参与构建）
```

**三个结构决策**：

1. **testkit 独立 crate**：daemon 的单测与 e2e 都要用 MockClock/FakeAdapter。testkit 正常依赖 daemon（拿 trait），daemon 以 dev-dependencies 反向依赖 testkit——**cargo 允许 dev 依赖成环**（编译 lib 时不需要它）
2. **fixture 脚本不落仓库路径**：`fixtures.rs` 把 `loop_write.sh` 等以字符串内嵌、运行时写 tempdir 并 `chmod +x`——避免跨 crate 路径解析问题
3. **e2e 独立 crate**：workspace 根目录的 `tests/` 不会被 cargo 收集，统一放 `maestro-e2e/tests/`

## 三、依赖清单

### P0.1 立即引入（[workspace.dependencies] 统一版本，cargo add 取最新稳定）

| crate | 用途 | 落点任务 |
|---|---|---|
| serde + serde_json | 全协议序列化 | 全部 |
| schemars | API schema 自动生成（herdr 同款） | 0.2 |
| thiserror + anyhow | 错误分层（库内 thiserror / 边界 anyhow） | 全部 |
| tracing + tracing-subscriber（env-filter） | 日志 | 全部 |
| nix（features: signal, process） | setsid/killpg/waitpid/SIGSTOP | 0.5/0.17/0.18 |
| rusqlite（features: bundled） | 持久化；bundled 免系统 sqlite | 0.12 |
| clap（derive） | CLI | 0.13 |

### P0.1 dev-dependencies

| crate | 用途 |
|---|---|
| tempfile | 临时 worktree/data_dir |
| assert_cmd + predicates | CLI e2e |
| serial_test | **信号/进程组敏感用例必须 `#[serial]`**（A8/B1/B11 等会互相干扰） |
| maestro-testkit | daemon 单测 + e2e |

### 后续任务按需引入（现在不加）

| crate | 引入时机 | 理由 |
|---|---|---|
| ureq（rustls） | 0.14 | daemon 内置 LLM client 用**阻塞式** ureq 而非 reqwest——见下方决策 2 |
| interprocess | P1 三平台 | P0 直接用 `std::os::unix::net`，零依赖完成 Linux 验证 |

### 四个关键依赖决策（与既有架构决策对齐）

1. **不引入 tokio/async 运行时**：架构是「单线程权威事件循环 + 每连接线程 + mpsc」（决策 A3，herdr 验证过），std 线程足够，还符合轻量
2. **LLM client 用 ureq 不用 reqwest**：reqwest 拖 async 运行时；daemon 内部调用（分类/叙事压缩）都是小 JSON 非流式，阻塞式足够
3. **不引 libgit2/git2**：checkpoint 原语走 `git add -A → write-tree → commit-tree → update-ref → reset` 的 CLI plumbing（设计 §4.3），依赖只有系统 git
4. **nix 仅 unix**：P0 Linux 优先；P1 加 `cfg(windows)` + windows-sys（Job Objects 杀进程树）

## 四、测试基建规格

- **MockClock**（`mock_clock.rs`）：内部 `Cell<u64>` 纳秒计数 + condvar 唤醒列表；`advance_secs(n)` 推进并唤醒所有等待者（退避定时器/窗口全部可虚拟推进，CI 零真实睡眠）
- **FakeAdapter**（`fake_adapter.rs`）：实现 daemon 的 `Adapter` trait；可注入 ①stderr 字符串（测断连映射）②脚本化 stream-json 事件序列 ③卡在任意状态 N 虚拟时间
- **script_worker**（`script_worker.rs`）：`CommandExt::process_group(0)` 起独立进程组，返回 `{pid, pgid}`；`stop()` 走 SIGSTOP 供 I1 实测
- **pgid_sandbox**（`pgid_sandbox.rs`）：测试结束兜底 `killpg` 清理自己的进程组；提供「扫描断言本 pgid 无存活」工具（A8/B11 用）
- **fixtures**：`loop_write.sh`（每 5ms 追加写文件，I1/B1）、`loop_http.sh`（循环 curl 本地 mock server，B2）、`hanging.sh`（`sleep infinity`，SIGSTOP 验证）

## 五、CI 要点（.github/workflows/ci.yml）

```yaml
jobs:
  check:
    steps:
      - rustup toolchain 按 rust-toolchain.toml
      - cargo fmt --all -- --check
      - cargo clippy --all-targets -- -D warnings
      - cargo test --workspace          # 或 cargo nextest run（可选）
```

## 六、完成自查清单

- [ ] `cargo build --workspace` 全绿，无 unsafe lint 告警
- [ ] `cargo test --workspace` 冒烟通过（testkit 自测：MockClock 推进唤醒、fixture 写入可执行）
- [ ] `loop_write.sh` 手工跑一次：进程组独立（`ps -o pgid`）、SIGSTOP 后写入停止
- [ ] `maestro --help` / `maestro doctor` 能跑（doctor 此时可只有 daemon 探测一项）
- [ ] `reference/herdr` 未参与构建（workspace excludes）
