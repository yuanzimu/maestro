//! EventStore：SQLite 事件溯源（WAL + synchronous=NORMAL，fsync 集中在 checkpoint —— 调研坑 4 的官方答案）。

use maestro_protocol::events::Envelope;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get(0)
}

pub struct EventStore {
    conn: Mutex<Connection>,
}

impl EventStore {
    /// 打开/创建事件库（dir/maestro.sqlite）
    pub fn open(dir: &Path) -> rusqlite::Result<Self> {
        std::fs::create_dir_all(dir).ok();
        let conn = Connection::open(dir.join("maestro.sqlite"))?;
        Self::init(conn)
    }

    /// 内存库（测试）
    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        // WAL + NORMAL：前台提交不蹲长 fsync，崩溃窗口由 WAL checkpoint 兜底
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                seq     INTEGER PRIMARY KEY,
                ts      INTEGER NOT NULL,
                priority TEXT NOT NULL,
                json    TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 追加事件（publish 路径的 sink 调用）
    pub fn append(&self, env: &Envelope) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO events (seq, ts, priority, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                env.seq,
                env.ts as i64,
                serde_json::to_string(&env.priority).unwrap_or_default(),
                serde_json::to_string(env).unwrap_or_default(),
            ],
        )?;
        Ok(())
    }

    /// 重放：seq >= from 的全部事件（订阅断点续订用）
    pub fn replay_from(&self, from: u64) -> Vec<Envelope> {
        self.query("SELECT json FROM events WHERE seq >= ?1 ORDER BY seq", from)
    }

    /// 全量重放（daemon 重启恢复用）
    pub fn replay_all(&self) -> Vec<Envelope> {
        self.query("SELECT json FROM events ORDER BY seq", 0)
    }

    fn query(&self, sql: &str, from: u64) -> Vec<Envelope> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                if cfg!(test) { eprintln!("[query] prepare err: {e}"); }
                return vec![];
            }
        };
        // SQL 无占位符时不传参（replay_all 无 WHERE）
        let rows = if sql.contains('?') {
            stmt.query_map(params![from as i64], map_row)
        } else {
            stmt.query_map([], map_row)
        };
        let mut out = vec![];
        match rows {
            Ok(rows) => {
                for r in rows {
                    match r {
                        Ok(json) => match serde_json::from_str::<Envelope>(&json) {
                            Ok(env) => out.push(env),
                            Err(e) => {
                                if cfg!(test) { eprintln!("[query] parse err: {e} json={json}"); }
                            }
                        },
                        Err(e) => {
                            if cfg!(test) { eprintln!("[query] row err: {e}"); }
                        }
                    }
                }
            }
            Err(e) => {
                if cfg!(test) { eprintln!("[query] map err: {e}"); }
            }
        }
        out
    }

    /// 最大 seq
    pub fn max_seq(&self) -> u64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_protocol::events::Event;
    use maestro_protocol::types::TaskId;

    fn ev(seq_hint: u64) -> Event {
        Event::NarrativeSnapshot {
            task: TaskId::new(format!("t{seq_hint}")),
            round: seq_hint as u32,
            milestone: format!("m{seq_hint}"),
        }
    }

    #[test]
    fn append_replay_roundtrip() {
        let store = EventStore::in_memory().unwrap();
        let hub_envs: Vec<Envelope> = (1..=5)
            .map(|i| Envelope::new(i, ev(i)))
            .collect();
        for e in &hub_envs {
            store.append(e).unwrap();
        }
        assert_eq!(store.max_seq(), 5);
        let all = store.replay_all();
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].seq, 1);
        // 断点续订
        let from3 = store.replay_from(3);
        assert_eq!(from3.len(), 3);
        assert_eq!(from3[0].seq, 3);
        // 类型保真
        match &from3[0].event {
            Event::NarrativeSnapshot { task, round, .. } => {
                assert_eq!(task.as_str(), "t3");
                assert_eq!(*round, 3);
            }
            other => panic!("类型失真: {other:?}"),
        }
    }

    /// 重放结果与写入的 wire 格式逐字节一致（事件溯源的根基）
    #[test]
    fn wire_format_stable() {
        let store = EventStore::in_memory().unwrap();
        let env = Envelope::new(1, ev(1));
        store.append(&env).unwrap();
        let back = store.replay_all().pop().unwrap();
        assert_eq!(
            serde_json::to_string(&env).unwrap(),
            serde_json::to_string(&back).unwrap()
        );
    }
}

