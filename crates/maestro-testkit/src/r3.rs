//! R3 多轮驱动验证 harness（R3_PROTOCOL.md）。
//!
//! 真实环境无 claude CLI / API key 时，用 mock CLI 验证**驱动机制**：
//! 轮循环、session 捕获、--resume 续接、stream-json 解析、JSONL 落账、
//! kill -9 崩溃恢复。真实 CLI 可用时，同一 Driver 指向真实二进制即可
//! 跑完整 S0~S7 协议（mock/真实仅是 `cli` 参数不同）。
//!
//! mock CLI 语义（与 claude CLI 对齐的子集）：
//! - `-p <prompt>` `--resume <sid>` `--output-format stream-json` `--verbose` `--max-turns N`
//! - 会话状态：每轮一行 JSON 追加到 `$MOCK_STATE_DIR/<sid>.jsonl`
//! - 「记忆」：只有读过基线轮（S0）的会话才能召回 FACT
//! - 注入：`补充指示` 前缀指令写入会话，后续回答持续带前缀（S3/S4）
//! - 崩溃：prompt 含 SELF_DESTRUCT → 状态落盘后 kill -9 自身（S7c）

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 一轮的完整记录（JSONL 一行）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoundLog {
    pub round: u32,
    pub session_id: String,
    pub prompt: String,
    pub usage_in: u64,
    pub usage_out: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    pub latency_ms: u64,
    pub answer: String,
}

/// 轮执行错误
#[derive(Debug)]
pub enum R3Error {
    /// CLI 非零退出（含崩溃/坏 sid）—— stderr 原样带回
    CliFailed(String, String),
    /// stream-json 解析失败
    Parse(String),
}

/// R3 驱动器：指向 mock 或真实 CLI
pub struct R3Driver {
    cli: PathBuf,
    workdir: PathBuf,
    log_path: PathBuf,
    pub round: u32,
    pub session: Option<String>,
}

impl R3Driver {
    pub fn new(cli: &Path, workdir: &Path, log_path: &Path) -> Self {
        Self {
            cli: cli.to_path_buf(),
            workdir: workdir.to_path_buf(),
            log_path: log_path.to_path_buf(),
            round: 0,
            session: None,
        }
    }

    /// 跑一轮：无 session 则新开，有则 --resume
    pub fn run_round(&mut self, prompt: &str) -> Result<RoundLog, R3Error> {
        let t0 = std::time::Instant::now();
        let mut cmd = Command::new(&self.cli);
        cmd.current_dir(&self.workdir)
            .arg("-p")
            .arg(prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--max-turns")
            .arg("1");
        if let Some(sid) = &self.session {
            cmd.arg("--resume").arg(sid);
        }
        // ETXTBSY 瞬态保护：脚本刚落盘即 exec 时（写回未完成）偶发，
        // 短退避重试
        let mut out = None;
        for attempt in 0..5 {
            match cmd.output() {
                Ok(o) => {
                    out = Some(o);
                    break;
                }
                Err(e) if attempt < 4 && e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(R3Error::CliFailed(format!("spawn: {e}"), String::new())),
            }
        }
        let Some(out) = out else {
            return Err(R3Error::CliFailed(
                "spawn: retries exhausted".into(),
                String::new(),
            ));
        };
        if !out.status.success() {
            return Err(R3Error::CliFailed(
                format!("exit={:?}", out.status.code()),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ));
        }
        let latency_ms = t0.elapsed().as_millis() as u64;
        let (session_id, answer, usage_in, usage_out, cache_read) = parse_stream_json(&out.stdout)?;

        let log = RoundLog {
            round: self.round,
            session_id: session_id.clone(),
            prompt: prompt.to_string(),
            usage_in,
            usage_out,
            cache_read,
            latency_ms,
            answer,
        };
        // JSONL 落账（协议 §二）
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|e| R3Error::Parse(e.to_string()))?;
        writeln!(
            f,
            "{}",
            serde_json::to_string(&log).map_err(|e| R3Error::Parse(e.to_string()))?
        )
        .map_err(|e| R3Error::Parse(e.to_string()))?;

        self.round += 1;
        self.session = Some(session_id);
        Ok(log)
    }

    /// 读全部轮账（S6 成本曲线分析用）
    pub fn read_log(&self) -> Vec<RoundLog> {
        let f = std::fs::File::open(&self.log_path).unwrap();
        BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect()
    }
}

/// 解析 stream-json：system 行取 session_id，result 行取 answer + usage
fn parse_stream_json(stdout: &[u8]) -> Result<(String, String, u64, u64, Option<u64>), R3Error> {
    let mut session_id = None;
    let mut answer = None;
    let (mut usage_in, mut usage_out, mut cache_read) = (0, 0, None);
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str() {
            Some("system") => {
                if let Some(s) = v["session_id"].as_str() {
                    session_id = Some(s.to_string());
                }
            }
            Some("result") => {
                answer = v["result"].as_str().map(String::from);
                usage_in = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
                usage_out = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
                cache_read = v["usage"]["cache_read_input_tokens"].as_u64();
            }
            _ => {}
        }
    }
    let sid = session_id.ok_or_else(|| R3Error::Parse("missing system.session_id".into()))?;
    let ans = answer.ok_or_else(|| R3Error::Parse("missing result".into()))?;
    Ok((sid, ans, usage_in, usage_out, cache_read))
}

/// R3 fixture 仓库（协议 §二）：FACT_1~5 植入
pub fn r3_fixture(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("README.md"),
        "# fixture\n\nFACT_1: 项目代号「maestro」的验证夹具仓库。\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/a.rs"),
        "// FACT_2: 模块 a 负责协议序列化\npub mod a {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/b.rs"),
        "/// FACT_3: b 模块是事件总线\npub mod b {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".project-notes"),
        "FACT_5: 隐藏笔记，仅基线轮可见\n",
    )
    .unwrap();
    // FACT_4 进 commit message
    let st = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success());
    for a in [
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
    ] {
        let _ = std::process::Command::new("git")
            .args(&a)
            .current_dir(dir)
            .status();
    }
    let _ = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .status();
    let _ = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "FACT_4: 初始提交", "--allow-empty"])
        .current_dir(dir)
        .status();
}

/// 写 mock CLI（bash 脚本）。state_dir 存会话状态。
pub fn write_mock_cli(path: &Path, state_dir: &Path) {
    std::fs::create_dir_all(state_dir).unwrap();
    let script = format!(
        r#"#!/bin/bash
# mock claude CLI（R3 harness）
STATE_DIR="{state}"
PROMPT=""; SID=""; RESUME_GIVEN=""; MAXTURNS=1
while [ $# -gt 0 ]; do
  case "$1" in
    -p) PROMPT="$2"; shift 2;;
    --resume) SID="$2"; RESUME_GIVEN=1; shift 2;;
    --output-format|--verbose) shift;;
    --max-turns) MAXTURNS="$2"; shift 2;;
    *) shift;;
  esac
done
if [ -z "$SID" ]; then SID="sid-$$-$RANDOM"; fi
CTX="$STATE_DIR/$SID.jsonl"
# --resume 语义对齐真实 CLI：不存在的 sid → 明确报错（S7a）
if [ -n "${{RESUME_GIVEN:-}}" ] && [ ! -f "$CTX" ]; then
  echo "error: no session found to resume for --resume $SID" >&2
  exit 1
fi
touch "$CTX"

# 读过基线（会话里有 read 标记）才能召回 FACT
has_read() {{ grep -q '"read":true' "$CTX" 2>/dev/null; }}
# 前缀注入状态
prefix() {{ grep -o '"prefix":"[^"]*"' "$CTX" 2>/dev/null | tail -1 | sed 's/"prefix":"//;s/"$//'; }}

ANSWER="ok"
case "$PROMPT" in
  *通读*) echo '{{"read":true}}' >> "$CTX"; ANSWER="已通读 README 与 src";;
  *FACT_1*) if has_read; then ANSWER="FACT_1 在 README.md"; else ANSWER="我不知道（无上下文）"; fi;;
  *FACT_2*) if has_read; then ANSWER="FACT_2 在 src/a.rs"; else ANSWER="我不知道（无上下文）"; fi;;
  *FACT_3*) if has_read; then ANSWER="FACT_3 在 src/b.rs"; else ANSWER="我不知道（无上下文）"; fi;;
  *补充指示*)
     P=$(echo "$PROMPT" | grep -o '\[[A-Z]\]' | head -1)
     echo "{{\"prefix\":\"$P\"}}" >> "$CTX"
     ANSWER="已记录前缀 $P";;
  *网络故障*) echo "Error: connection reset by peer" >&2; exit 1;;
  *结束*) ANSWER="MAESTRO_DONE";;
  *SELF_DESTRUCT*)
     echo '{{"crashed":true}}' >> "$CTX"
     kill -9 $$
     ;;
  *) ANSWER="(generic echo) $PROMPT";;
esac

# 应用前缀（注入后持续生效 = S4 持久性语义；含 DONE 信号轮）
P=$(prefix)
if [ -n "$P" ] && [ "$ANSWER" != "已记录前缀 $P" ]; then ANSWER="$P $ANSWER"; fi

# 产物落盘（daemon 验收门读回校验用；对 R3 harness 测试无影响）
echo "$ANSWER" >> out.txt

# usage：平台型（常量 in + cache 命中）；MOCK_REPLAY=1 时线性重放
LINES=$(wc -l < "$CTX")
if [ "$MOCK_REPLAY" = "1" ]; then IN=$((200*LINES)); CR=0; else IN=200; CR=120; fi
OUT=40

printf '%s\n' \
  "{{\"type\":\"system\",\"session_id\":\"$SID\"}}" \
  "{{\"type\":\"assistant\",\"message\":{{\"content\":\"$ANSWER\"}}}}" \
  "{{\"type\":\"result\",\"result\":\"$ANSWER\",\"usage\":{{\"input_tokens\":$IN,\"output_tokens\":$OUT,\"cache_read_input_tokens\":$CR}}}}"
"#,
        state = state_dir.display(),
    );
    std::fs::write(path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
