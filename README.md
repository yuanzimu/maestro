<div align="center">

# Maestro

**Tell AI what you want, then grab a coffee ☕**

English | [中文](README.zh-CN.md)

</div>

---

## What is it?

Maestro is an AI task commander. You say "fix that bug" and it tells a squad of AI agents to go do it — then it calls you back when done.

## Before vs After

**Before (you stay glued to the screen):**

```
You ──→ open AI tool ──→ wait ──→ AI gets stuck, asks you ──→ answer ──→ wait...
        (window stays open, you can't leave)
```

**After (assign and walk away):**

```
You ──→ type one sentence ──→ ☕ grab coffee
                            │
                            ▼
                       Maestro runs in background:
                       ① break down task ② assign to AI
                       ③ watch it work ④ auto-test
                       ⑤ done!
                            │
                            ▼
                       Notification: come collect result ✅
```

## Three Things It Does Best

**1. Close the window, work keeps running** 💤

```
Close laptop / lose internet / app crashes
            ↓
       Task state saved exactly as-is
            ↓
       Re-open, resume where you left off
```

**2. Saves money** 💰

```
Simple tasks → send to cheap AI
Hard tasks   → use the expensive one
Bad output   → auto-retry with a better one
                ↓
       Goal: cut cost in half vs using the best AI for everything,
       with same quality
```

**3. One "inbox" for everything** 📮

All questions from all AI agents are gathered in one list. Handle them like messages — click through one by one.

## Something went wrong? Hit "brake" and "undo" 🛑

```
AI going off track
        ↓
Hit emergency stop
        ↓
All AI agents frozen instantly (<0.1s), snapshot taken
        ↓
You decide: keep going / roll back / give up
```

## Which AI agents can it drive?

Claude Code · Codex · Gemini · Amp · OpenCode — all supported, hot-swappable.

## 30-second start

```bash
# install (needs Rust)
git clone https://github.com/yuanzimu/maestro.git
cd maestro && cargo build --workspace

# start daemon
cargo run -p maestro-daemon

# assign a task
cargo run -p maestro-cli -- task create \
  --title "Fix login page" \
  --prompt "Make the login button rounded" \
  --workdir /your/project/path

# ...then go grab coffee ☕
# Check progress later:
cargo run -p maestro-cli -- status
```

## What it looks like

```
$ maestro status

Tasks
────────────────────────────────────────
 Fix login      working 🔨  round 3  spent $0.12
 Write tests    needs you ❗  click to resolve
 Refactor DB    done ✅      tests 12/12 passed
```

## Where are we?

| Stage | What | Status |
|---|---|---|
| P0 | Backend engine + command line | ✅ Done (188 tests, all passing) |
| P1 | Desktop app (Mac/Win/Linux) + cost saving features | 🚧 In progress |
| P2 | Server edition (for teams) | 📋 Planned |

## Want more?

Design docs are in [docs/](docs/) (Chinese, detailed).

## Buy Me a Coffee ☕

Maestro is free and open source, built in spare time. If it saves you time, a coffee is always appreciated — totally optional:

| Channel | Best for | Link |
|---|---|---|
| afdian (爱发电) | Chinese users (WeChat/Alipay) | [afdian.net/a/your-username](https://afdian.net/a/your-username) |
| GitHub Sponsors | International (credit card) | [github.com/sponsors/yuanzimu](https://github.com/sponsors/yuanzimu) |
| Crypto | On-chain, no signup | Addresses below |

```text
USDT (TRC-20):  [REPLACE with your USDT TRC-20 address]
ETH / ERC-20:   [REPLACE with your ETH address]
BTC:            [REPLACE with your BTC address]
```

> Double-check the address before sending. The project stays free under Apache-2.0 — no donation required.

## License

Apache-2.0 — use freely, including commercially.
