//! Command-history store for terminal panes. One append-only JSONL log at
//! `~/.crane/history/history.jsonl`; each entry records the command, cwd,
//! exit code, the owning session, and timestamps. Ranking (not filtering)
//! decides up-arrow order — current/restored session first, current dir next,
//! then recency, deduped keeping the latest occurrence. Mirrors Warp's model
//! (see vendor/warp `HistoryOrder`), plus a pwd tie-break Warp omits.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    pub command: String,
    pub pwd: String,
    pub exit_code: Option<i32>,
    pub session_id: u64,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub struct HistoryStore {
    entries: Vec<HistoryEntry>,
    path: PathBuf,
}

/// Milliseconds since the Unix epoch. `0` if the clock is before the epoch
/// (never in practice) — a monotonic-enough ordering key without `chrono`.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl HistoryStore {
    /// Load every well-formed JSONL entry from `path`; a missing file is an
    /// empty store, and a corrupt line is skipped (never aborts the load).
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self { entries, path }
    }

    /// Append `entry` to memory and to disk (one JSON line, `O_APPEND` so
    /// concurrent terminals never interleave a partial line). Disk failure is
    /// swallowed — history is best-effort, never blocks the terminal.
    pub fn append(&mut self, entry: HistoryEntry) {
        if let Ok(line) = serde_json::to_string(&entry) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        self.entries.push(entry);
    }

    /// Ranked, deduped view for up-arrow. Order:
    /// 1. session tier — `current` or in `restored` outrank everyone else;
    /// 2. directory — within a tier, `pwd`-match outranks non-match;
    /// 3. recency — later `start_ms` first;
    /// then duplicates by command text collapse to their latest occurrence.
    pub fn rank(&self, current: u64, restored: &HashSet<u64>, pwd: &str) -> Vec<&HistoryEntry> {
        let tier = |e: &HistoryEntry| -> u8 {
            if e.session_id == current || restored.contains(&e.session_id) { 1 } else { 0 }
        };
        let mut idx: Vec<usize> = (0..self.entries.len()).collect();
        idx.sort_by(|&a, &b| {
            let (ea, eb) = (&self.entries[a], &self.entries[b]);
            // All keys descending (higher = ranks first).
            tier(eb)
                .cmp(&tier(ea))
                .then_with(|| {
                    let (pa, pb) = ((ea.pwd == pwd) as u8, (eb.pwd == pwd) as u8);
                    pb.cmp(&pa)
                })
                .then_with(|| eb.start_ms.cmp(&ea.start_ms))
        });
        let mut seen: HashSet<&str> = HashSet::new();
        idx.into_iter()
            .map(|i| &self.entries[i])
            .filter(|e| seen.insert(e.command.as_str()))
            .collect()
    }
}

/// Process-wide store, lazily loaded from `~/.crane/history/history.jsonl`.
pub fn store() -> &'static Mutex<HistoryStore> {
    static STORE: OnceLock<Mutex<HistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".crane")
            .join("history")
            .join("history.jsonl");
        Mutex::new(HistoryStore::load(path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cmd: &str, pwd: &str, sess: u64, ts: u64) -> HistoryEntry {
        HistoryEntry {
            command: cmd.into(),
            pwd: pwd.into(),
            exit_code: Some(0),
            session_id: sess,
            start_ms: ts,
            end_ms: ts + 1,
        }
    }

    #[test]
    fn rank_puts_current_session_above_others_then_by_recency() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("old_other", "/a", 2, 100),
            entry("cur_early", "/a", 1, 200),
            entry("cur_late", "/a", 1, 300),
        ];
        let restored: HashSet<u64> = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        // Current session (1) first, most-recent-first within the tier; other last.
        assert_eq!(ranked, vec!["cur_late", "cur_early", "old_other"]);
    }

    #[test]
    fn rank_promotes_restored_session_into_the_current_tier() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("live", "/a", 1, 100),
            entry("restored", "/a", 9, 200),
            entry("stranger", "/a", 5, 300),
        ];
        let mut restored = HashSet::new();
        restored.insert(9);
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        // restored(9) ranks in the current tier above stranger(5), by recency.
        assert_eq!(ranked, vec!["restored", "live", "stranger"]);
    }

    #[test]
    fn rank_breaks_ties_by_matching_pwd_within_a_tier() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("here", "/proj", 1, 100),
            entry("elsewhere", "/other", 1, 200), // newer but wrong dir
        ];
        let restored = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/proj").iter().map(|e| e.command.as_str()).collect();
        // Same session tier: current-dir wins over a newer different-dir command.
        assert_eq!(ranked, vec!["here", "elsewhere"]);
    }

    #[test]
    fn rank_dedupes_keeping_the_latest_occurrence() {
        let mut s = HistoryStore { entries: vec![], path: "/dev/null".into() };
        s.entries = vec![
            entry("ls", "/a", 1, 100),
            entry("ls", "/a", 1, 300),
            entry("pwd", "/a", 1, 200),
        ];
        let restored = HashSet::new();
        let ranked: Vec<&str> = s.rank(1, &restored, "/a").iter().map(|e| e.command.as_str()).collect();
        assert_eq!(ranked, vec!["ls", "pwd"], "duplicate ls collapses to its latest");
    }

    #[test]
    fn append_then_reload_roundtrips_and_skips_corrupt_lines() {
        let dir = std::env::temp_dir().join(format!("crane-hist-test-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        {
            let mut s = HistoryStore::load(path.clone());
            s.append(entry("cargo build", "/proj", 1, 100));
            s.append(entry("cargo test", "/proj", 1, 200));
        }
        // Inject a corrupt line between good ones.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{ not valid json").unwrap();
        }
        let reloaded = HistoryStore::load(path.clone());
        assert_eq!(reloaded.entries.len(), 2, "corrupt line skipped, good lines kept");
        assert_eq!(reloaded.entries[1].command, "cargo test");
        std::fs::remove_dir_all(&dir).ok();
    }
}
