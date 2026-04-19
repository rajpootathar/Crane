//! Background poller for total WKWebView memory.
//!
//! wry doesn't expose per-webview process ids, so per-tab attribution
//! is impossible from here (would need a private WebKit KVC key).
//! Instead we sum the resident set size of every
//! `com.apple.WebKit.WebContent` process — Apple's per-origin content
//! processes — and expose the total. Browser panes read this each
//! frame to show a chip that turns orange/red above warning
//! thresholds, nudging the user to close heavy tabs.
//!
//! Warn threshold: 1.0 GB  (chip goes orange).
//! Danger threshold: 2.0 GB (chip goes red + prompts).

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
pub const WARN_BYTES: u64 = 1_000_000_000;
pub const DANGER_BYTES: u64 = 2_000_000_000;

#[derive(Clone, Default)]
pub struct Snapshot {
    pub total_bytes: u64,
    pub process_count: u32,
}

pub struct Monitor {
    state: Arc<Mutex<Snapshot>>,
}

impl Monitor {
    pub fn start() -> Self {
        let state: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(Snapshot::default()));
        #[cfg(target_os = "macos")]
        {
            let state = state.clone();
            std::thread::spawn(move || loop {
                if let Some(snap) = sample_webkit_processes() {
                    *state.lock() = snap;
                }
                std::thread::sleep(POLL_INTERVAL);
            });
        }
        Self { state }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().clone()
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
