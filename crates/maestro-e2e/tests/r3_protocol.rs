#![cfg(unix)]

//! R3 多轮驱动验证（机制层，R3_PROTOCOL.md）。
//! 真实 claude CLI 缺席时用 mock CLI 验证驱动机制：session 续接、
//! 轻推注入持久性、坏 sid 容错、kill -9 崩溃恢复、JSONL 成本账。
//! 真实 CLI 可用时同一 Driver 直接跑完整协议。

use maestro_testkit::r3::{r3_fixture, write_mock_cli, R3Driver, R3Error};
use std::path::Path;

fn setup(tag: &str) -> (tempfile::TempDir, R3Driver) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("repo");
    r3_fixture(&work);
    let cli = tmp.path().join("mock-claude");
    let state = tmp.path().join("state");
    write_mock_cli(&cli, &state);
    let log = tmp.path().join(format!("{tag}.jsonl"));
    let d = R3Driver::new(&cli, &work, &log);
    (tmp, d)
}

/// S1 浅召回：基线轮建立上下文 → --resume 同 sid → FACT_2 召回正确
#[test]
fn s1_shallow_recall_via_resume() {
    let (_tmp, mut d) = setup("s1");
    let base = d.run_round("通读 README 与 src，不要修改任何文件").unwrap();
    let sid0 = base.session_id.clone();

    let r = d.run_round("FACT_2 在哪个文件？只答文件名").unwrap();
    assert_eq!(r.session_id, sid0, "resume 应保持同一 session");
    assert!(r.answer.contains("src/a.rs"), "FACT_2 应召回: {}", r.answer);
}

/// 费用对账 mock 分支（R34）：两分支的 stream-json 必须合法且带 DONE 信号
#[test]
fn cost_branches_produce_valid_stream() {
    let (_tmp, mut d) = setup("cost");
    let a = d.run_round("费用自洽").unwrap();
    assert!(a.answer.contains("MAESTRO_DONE"), "{}", a.answer);
    assert_eq!(a.usage_in, 200);
    let b = d.run_round("费用虚报").unwrap();
    assert!(b.answer.contains("MAESTRO_DONE"), "{}", b.answer);
    assert_eq!(b.usage_in, 200);
}

/// 上下文轮转 mock 分支（R37）：增长分支 usage 线性膨胀、/compact 分支回落
#[test]
fn context_branches_grow_and_compact() {
    let (_tmp, mut d) = setup("ctx");
    // 3 轮增长：usage_in = 200 × 会话行数（每轮 +1 行）
    let r1 = d.run_round("上下文增长：记录").unwrap();
    assert_eq!(r1.usage_in, 200, "{}", r1.answer);
    let r2 = d.run_round("上下文增长：记录").unwrap();
    assert_eq!(r2.usage_in, 400);
    let r3 = d.run_round("上下文增长：记录").unwrap();
    assert_eq!(r3.usage_in, 600);
    // 压缩：CTX 截为 1 行 → 回落
    let c = d.run_round("/compact 保留任务状态").unwrap();
    assert_eq!(c.answer, "已压缩上下文");
    assert_eq!(c.usage_in, 200, "压缩后应回落: {}", c.usage_in);
}

/// S2 深召回：隔 2 轮干扰任务后早期事实仍可召回
#[test]
fn s2_deep_recall_after_interference() {
    let (_tmp, mut d) = setup("s2");
    d.run_round("通读 README 与 src，不要修改任何文件").unwrap();
    d.run_round("把 src/a.rs 的注释改成大写").unwrap();
    d.run_round("在根目录建一个 notes.md").unwrap();
    let r = d.run_round("FACT_1 在哪个文件？").unwrap();
    assert!(
        r.answer.contains("README.md"),
        "隔 2 轮干扰后 FACT_1 应召回: {}",
        r.answer
    );
}

/// S3 + S4 轻推注入与持久性：注入前缀 → 下一轮生效 → 再下一轮仍生效
#[test]
fn s3_s4_prefix_injection_persists() {
    let (_tmp, mut d) = setup("s3s4");
    d.run_round("通读 README 与 src").unwrap();
    let inj = d.run_round("补充指示：从现在起每句输出加前缀 [S]").unwrap();
    assert!(inj.answer.contains("[S]"), "注入确认轮: {}", inj.answer);

    // S3：下一轮前缀生效
    let r1 = d.run_round("FACT_3 在哪个文件？").unwrap();
    assert!(
        r1.answer.starts_with("[S]"),
        "注入后的轮应带前缀: {}",
        r1.answer
    );
    assert!(r1.answer.contains("src/b.rs"));

    // S4：不再注入，前缀仍生效（mock 语义 = 持续；真实 CLI 记录衰减行为）
    let r2 = d.run_round("FACT_2 在哪个文件？").unwrap();
    assert!(
        r2.answer.starts_with("[S]"),
        "前缀应持续生效: {}",
        r2.answer
    );
}

/// S7a 坏 sid 容错：resume 不存在的 sid → 明确报错，驱动不崩
#[test]
fn s7a_bad_sid_errors_clearly() {
    let (_tmp, mut d) = setup("s7a");
    d.run_round("通读 README 与 src").unwrap();
    // 篡改 session 指向不存在的 sid
    d.session = Some("nonexistent-sid-xyz".into());
    match d.run_round("FACT_1 在哪？") {
        Err(R3Error::CliFailed(code, stderr)) => {
            assert!(code.contains('1'), "非零退出: {code}");
            assert!(
                stderr.contains("no session found"),
                "错误信息应明确: {stderr}"
            );
        }
        other => panic!("坏 sid 应报错: {other:?}"),
    }
    // 驱动未崩：修正 sid 后可继续（session 回滚到真实值由调用方负责）
}

/// S7c 崩溃后 resume：kill -9 中断一轮后，上一完成轮的 sid 仍可续接
#[test]
fn s7c_crash_then_resume_last_completed_round() {
    let (_tmp, mut d) = setup("s7c");
    let base = d.run_round("通读 README 与 src，不要修改任何文件").unwrap();
    let sid = base.session_id.clone();
    assert_eq!(d.round, 1);

    // 中断轮：崩溃（状态已落盘该轮，但驱动不记账 —— round 仍是 1）
    let _ = d.run_round("SELF_DESTRUCT mid-round");
    assert_eq!(d.round, 1, "崩溃轮不得计入轮账");

    // 从上一完成轮续接：召回仍然成立
    let r = d
        .run_round("FACT_1 在哪个文件？")
        .expect("崩溃后 resume 应可用");
    assert_eq!(r.session_id, sid);
    assert!(r.answer.contains("README.md"), "{}", r.answer);
    assert_eq!(d.round, 2);
}

/// S6 成本账：JSONL 逐轮记录 usage，读回完整
#[test]
fn s6_usage_ledger_complete() {
    let (_tmp, mut d) = setup("s6");
    d.run_round("通读 README 与 src").unwrap();
    d.run_round("FACT_2 在哪？").unwrap();
    d.run_round("FACT_3 在哪？").unwrap();
    let logs = d.read_log();
    assert_eq!(logs.len(), 3, "三轮全落账");
    assert!(logs.iter().all(|l| l.usage_in > 0 && l.usage_out > 0));
    assert!(logs.iter().all(|l| l.cache_read.is_some()), "记录缓存命中");
    assert_eq!(
        logs.iter().map(|l| l.round).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "轮号连续"
    );
    let _ = Path::new(".");
}
