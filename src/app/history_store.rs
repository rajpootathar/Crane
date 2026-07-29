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

/// Most-recent N commands kept, in memory and on disk. Bounds both the
/// per-keypress `rank()` sort (O(n log n) over `entries`) and the startup
/// `load()` parse so an append-only log that's grown for months/years never
/// makes either slow — 5000 keeps the sort imperceptible while still
/// retaining plenty of history for up-arrow recall.
const HISTORY_MAX: usize = 5000;

/// Extra headroom above `HISTORY_MAX` the in-memory `entries` Vec is allowed
/// to grow to before `append` trims it back down. Batching the trim over
/// `HISTORY_TRIM_SLACK` appends — instead of re-capping to exactly
/// `HISTORY_MAX` on every single append past the cap — makes the O(n)
/// front-drain genuinely amortized O(1) per append. The on-disk file is
/// unaffected: `load()`'s compaction still caps it at exactly `HISTORY_MAX`.
const HISTORY_TRIM_SLACK: usize = 512;

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
    ///
    /// If the file holds more than `HISTORY_MAX` entries, keep only the
    /// most-recent `HISTORY_MAX` (the file is append-order, so the tail is
    /// newest) and rewrite `path` to just those lines so the file stops
    /// growing without bound across restarts. This runs once at startup (the
    /// UI-thread warm-up), never on the PTY reader hot path, so a one-time
    /// rewrite here is cheap relative to the parse it follows. The rewrite is
    /// best-effort: if it fails, the in-memory store is still capped and
    /// returned — `load()` never panics or aborts because of it.
    pub fn load(path: PathBuf) -> Self {
        let mut entries: Vec<HistoryEntry> = std::fs::read_to_string(&path)
            .ok()
            .map(|text| {
                text.lines()
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<HistoryEntry>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        if entries.len() > HISTORY_MAX {
            let excess = entries.len() - HISTORY_MAX;
            entries.drain(0..excess);
            rewrite_compacted(&path, &entries);
        }
        Self { entries, path }
    }

    /// Append `entry` to memory and to disk (one JSON line, `O_APPEND` so
    /// concurrent terminals never interleave a partial line). Disk failure is
    /// swallowed — history is best-effort, never blocks the terminal.
    ///
    /// The disk file is left untouched past `HISTORY_MAX` — it's only
    /// compacted at the next `load()` — but the in-memory `entries` Vec is
    /// soft-capped at `HISTORY_MAX + HISTORY_TRIM_SLACK`: once it grows past
    /// that, a single `drain` trims the front back down to exactly
    /// `HISTORY_MAX`. Batching the trim over `HISTORY_TRIM_SLACK` appends
    /// this way (rather than re-capping to exactly `HISTORY_MAX` on every
    /// append past the cap, which degenerates to a `remove(0)`-equivalent
    /// full-tail memmove each time) makes the front-drain genuinely
    /// amortized O(1) per append. `rank()` briefly sorting a few hundred
    /// extra entries between trims is negligible.
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
        if self.entries.len() > HISTORY_MAX + HISTORY_TRIM_SLACK {
            let excess = self.entries.len() - HISTORY_MAX;
            self.entries.drain(0..excess);
        }
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

/// Best-effort rewrite of `path` to hold exactly `entries` (one JSON line
/// each), used once at `load()` time to compact a file that grew past
/// `HISTORY_MAX`. Writes to a sibling temp file — named with this process's
/// pid plus a per-process sequence number, so two Crane processes compacting
/// at the same moment can never collide on the same temp path — and renames
/// it over `path` rather than truncating in place, so a concurrent appender
/// (another Crane process writing via `O_APPEND`) can never observe a
/// half-written file — same atomic tmp-then-rename discipline as
/// `persist.rs::write_bytes`. If the rename fails, the temp file is removed
/// so a failed compaction never leaves a stray `history.jsonl.tmp*` file
/// behind. Every failure path (serialize, write, rename, cleanup) is
/// swallowed: this is a housekeeping nicety, not something that may ever
/// fail the load it runs inside of.
///
/// Known accepted race (not fixed here): the pid+seq temp name only
/// prevents two processes' *temp files* from colliding. It does not close a
/// narrower window where a second Crane process compacts this same
/// oversized file at startup in the exact instant a first process appends a
/// line — that appended line can be lost from disk (it still survives in
/// the first process's in-memory `entries` for the rest of its session).
/// Closing that gap needs advisory file locking, a larger change (and a new
/// dependency) than this fix warrants; left as a follow-up.
fn rewrite_compacted(path: &std::path::Path, entries: &[HistoryEntry]) {
    let mut buf = String::new();
    for e in entries {
        let Ok(line) = serde_json::to_string(e) else { continue };
        buf.push_str(&line);
        buf.push('\n');
    }
    let Some(parent) = path.parent() else { return };
    let _ = std::fs::create_dir_all(parent);
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    let tmp = path.with_extension(format!("jsonl.tmp.{pid}.{n}"));
    if std::fs::write(&tmp, buf.as_bytes()).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
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

    #[test]
    fn append_past_history_max_plus_slack_batches_the_trim_back_to_history_max() {
        let dir = std::env::temp_dir().join(format!("crane-hist-test-cap-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        let mut s = HistoryStore::load(path.clone());
        // One append past HISTORY_MAX + HISTORY_TRIM_SLACK is the first point
        // at which append's soft cap actually fires the front-drain.
        let total = HISTORY_MAX + HISTORY_TRIM_SLACK + 1;
        for i in 0..total {
            s.append(entry(&format!("cmd{i}"), "/proj", 1, i as u64));
        }
        assert_eq!(
            s.entries.len(),
            HISTORY_MAX,
            "in-memory entries are trimmed back down to exactly HISTORY_MAX once the slack is exceeded"
        );
        assert_eq!(
            s.entries[0].command,
            format!("cmd{}", HISTORY_TRIM_SLACK + 1),
            "the oldest HISTORY_TRIM_SLACK + 1 entries were dropped from the front"
        );
        assert_eq!(
            s.entries[HISTORY_MAX - 1].command,
            format!("cmd{}", total - 1),
            "the newest entry is retained"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_compacts_an_oversized_file_and_rewrites_it_to_history_max_lines() {
        let dir = std::env::temp_dir().join(format!("crane-hist-test-compact-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            for i in 0..HISTORY_MAX + 25 {
                let line = serde_json::to_string(&entry(&format!("cmd{i}"), "/proj", 1, i as u64)).unwrap();
                writeln!(f, "{line}").unwrap();
            }
        }
        let store = HistoryStore::load(path.clone());
        assert_eq!(store.entries.len(), HISTORY_MAX, "in-memory entries compacted to HISTORY_MAX");
        assert_eq!(store.entries[0].command, "cmd25", "the oldest entries were dropped");
        assert_eq!(
            store.entries[HISTORY_MAX - 1].command,
            format!("cmd{}", HISTORY_MAX + 24),
            "the newest entry survives compaction"
        );

        let on_disk = std::fs::read_to_string(&path).unwrap();
        let line_count = on_disk.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(line_count, HISTORY_MAX, "the file itself is rewritten to exactly HISTORY_MAX lines");

        std::fs::remove_dir_all(&dir).ok();
    }
}
