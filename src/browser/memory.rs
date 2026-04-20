//! On-demand poller for total WKWebView memory.
//!
//! wry doesn't expose per-webview process ids, so per-tab attribution
//! is impossible from here (would need a private WebKit KVC key).
//! Instead we sum the resident set size of every
//! `com.apple.WebKit.WebContent` process — Apple's per-origin content
//! processes — and expose the total.
//!
//! The previous implementation spun a background thread at app start
//! that ran `ps -axo rss=,comm=` every 3 seconds for the entire
//! process lifetime, regardless of whether any Browser Pane was open.
//! That's a leaked thread + a subprocess + a full process-table scan
//! you never asked for.
//!
//! Now: zero threads, zero work unless a Browser Pane actually calls
//! `snapshot()`. Results are cached for `POLL_INTERVAL` so the hot
//! per-frame UI path is still cheap.
//!
//! Warn threshold: 1.0 GB  (chip goes orange).
//! Danger threshold: 2.0 GB (chip goes red + prompts).

use parking_lot::Mutex;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
pub const WARN_BYTES: u64 = 1_000_000_000;
pub const DANGER_BYTES: u64 = 2_000_000_000;

#[derive(Clone, Default)]
pub struct Snapshot {
    pub total_bytes: u64,
    pub process_count: u32,
}

struct Cached {
    snap: Snapshot,
    at: Option<Instant>,
}

pub struct Monitor {
    cache: Mutex<Cached>,
}

impl Monitor {
    pub fn start() -> Self {
        Self {
            cache: Mutex::new(Cached {
                snap: Snapshot::default(),
                at: None,
            }),
        }
    }

    /// Called by the Browser view once per frame while visible. Returns
    /// the cached value unless `POLL_INTERVAL` has elapsed, in which
    /// case it samples inline (single `ps` invocation, ~5 ms). No
    /// background thread, no work when no browser pane is visible.
    pub fn snapshot(&self) -> Snapshot {
        let mut c = self.cache.lock();
        let stale = c.at.is_none_or(|t| t.elapsed() >= POLL_INTERVAL);
        if stale {
            #[cfg(target_os = "macos")]
            if let Some(fresh) = sample_webkit_processes() {
                c.snap = fresh;
            }
            c.at = Some(Instant::now());
        }
        c.snap.clone()
    }
}

#[cfg(target_os = "macos")]
fn sample_webkit_processes() -> Option<Snapshot> {
    // `ps -axo rss=,comm=` avoids headers. rss is in KB on macOS.
    let out = std::process::Command::new("ps")
        .args(["-axo", "rss=,comm="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut bytes = 0u64;
    let mut count = 0u32;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.contains("com.apple.WebKit.WebContent") {
            continue;
        }
        if let Some((rss_str, _)) = trimmed.split_once(char::is_whitespace)
            && let Ok(rss_kb) = rss_str.parse::<u64>()
        {
            bytes += rss_kb * 1024;
            count += 1;
        }
    }
    Some(Snapshot {
        total_bytes: bytes,
        process_count: count,
    })
}

pub fn human_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}
