//! 套件完整性离线校验（不依赖 daemon）：
//! ① 清单可加载且字段自洽；
//! ② 红绿不变量：每个任务 fixture 初态验收必须失败（红），应用 solution
//!    后必须通过（绿）—— 这是「验收通过率」口径成立的根基。

use maestro_bench::manifest::{apply_solution, load_suite, materialize_fixture, run_accept};
use std::path::PathBuf;

fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("suite")
}

#[test]
fn red_green_invariant() {
    let tasks = load_suite(&suite_dir()).expect("套件加载");
    let tmp = tempfile::tempdir().unwrap();
    let mut failures: Vec<String> = vec![];

    for t in &tasks {
        let m = &t.manifest;
        let work = tmp.path().join(&m.id).join("repo");

        // 红：fixture 初态验收失败
        materialize_fixture(t, &work).expect("fixture 物化");
        let (red_pass, _red_tail) = run_accept(m, &work);
        if red_pass {
            failures.push(format!("[{}] fixture 初态验收意外通过（应红）", m.id));
            continue;
        }

        // 绿：应用 solution 后验收通过
        apply_solution(t, &work).expect("solution 应用");
        // 诊断：确认源文件确已被 solution 覆盖
        let probe = work.join("src").join("lib.rs");
        let probe_desc = if probe.is_file() {
            let meta = std::fs::metadata(&probe).unwrap();
            let head = std::fs::read_to_string(&probe)
                .unwrap_or_default()
                .lines()
                .nth(1)
                .unwrap_or("")
                .to_string();
            format!("mtime={:?} head={head:?}", meta.modified())
        } else {
            "(no src/lib.rs)".into()
        };
        let (green_pass, green_tail) = run_accept(m, &work);
        if !green_pass {
            failures.push(format!(
                "[{}] solution 应用后验收仍失败（应绿）probe={}：\n{}",
                m.id, probe_desc, green_tail
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "红绿不变量被破坏（{} 项）：\n{}",
        failures.len(),
        failures.join("\n")
    );
}
