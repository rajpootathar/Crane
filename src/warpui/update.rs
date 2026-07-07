//! Background update checker + staged in-app updater for the warpui shell.
//!
//! Ported from the original egui Crane `src/update/check.rs` and
//! `src/update/apply.rs`. Reuses the same `ureq` HTTP client (already in
//! Cargo.toml) to query the GitHub Releases API for the latest published Crane
//! release, parses the `vX.Y.Z` tag, compares it against the compiled-in
//! `CARGO_PKG_VERSION`, and — if a newer stable release exists — stashes the
//! version string behind a shared, lock-free cell.
//!
//! On top of the check, this module owns the **staged auto-update** pipeline:
//! a background download of the release DMG into `~/.crane/update/`, a
//! `hdiutil`-driven stage of the mounted `Crane.app`, signature verification,
//! and a detached swap-and-relaunch script triggered from the UI. All progress
//! is published through a single shared `UpdateState` the shell polls each
//! frame via [`update_state`].
//!
//! Self-contained: no dependency on `src/update/`. Non-blocking: every network
//! call runs on a `std::thread`; accessors never block and never panic on
//! network failure. Consistent with warpui's repaint model, callers hand in a
//! `wake` closure (typically wrapping the shell's repaint waker) instead of an
//! `egui::Context`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_LATEST: &str = "https://api.github.com/repos/rajpootathar/Crane/releases/latest";
const USER_AGENT: &str = "Crane-Update-Checker";

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// The full lifecycle of the staged updater, polled by the shell each frame.
///
/// - `Idle` — nothing in flight; either no check has run or the build is
///   already current.
/// - `Checking` — the background GitHub-releases query is in flight.
/// - `UpdateAvailable { version }` — a newer stable release exists; the user
///   may kick off [`start_download`].
/// - `Downloading { received, total }` — the DMG is streaming to the staging
///   dir. `total` is `0` when the server omitted `Content-Length`.
/// - `Ready { path }` — the new `Crane.app` is staged and verified; the user
///   may call [`apply_and_restart`].
/// - `Failed { msg }` — any step failed; `msg` is user-presentable.
#[derive(Clone, Debug)]
pub enum UpdateState {
    Idle,
    Checking,
    UpdateAvailable { version: String },
    Downloading { received: u64, total: u64 },
    Ready { path: PathBuf },
    Failed { msg: String },
}

/// Shared version cell. `None` until the check finishes; `Some(version)` when a
/// newer stable release exists. Guarded so the background thread can publish and
/// the UI thread can read without data races. Kept independent of
/// [`UpdateState`] so [`latest_available`] stays a cheap, stable accessor for
/// the Settings > About row.
fn cell() -> &'static Mutex<Option<String>> {
    static CELL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Shared updater state cell (the staged-download state machine).
fn state_cell() -> &'static Mutex<UpdateState> {
    static STATE: OnceLock<Mutex<UpdateState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(UpdateState::Idle))
}

/// Guards against launching two downloads at once. Reset to `false` when a
/// download finishes (success or failure) so a `Failed` attempt can be retried.
fn download_in_flight() -> &'static AtomicBool {
    static FLAG: OnceLock<AtomicBool> = OnceLock::new();
    FLAG.get_or_init(|| AtomicBool::new(false))
}

/// Ensures the background check runs at most once per process.
fn started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

fn set_state(new: UpdateState) {
    if let Ok(mut g) = state_cell().lock() {
        *g = new;
    }
}

/// Snapshot of the current staged-updater state. Non-blocking; never panics.
/// The shell polls this each frame to render progress / the "Restart now"
/// affordance.
pub fn update_state() -> UpdateState {
    state_cell()
        .lock()
        .map(|g| g.clone())
        .unwrap_or(UpdateState::Idle)
}

// ---------------------------------------------------------------------------
// Background check (unchanged public surface)
// ---------------------------------------------------------------------------

/// Kick off the background update check. Idempotent: subsequent calls are no-ops.
/// Spawns a detached `std::thread` that queries GitHub and, on success, publishes
/// the newer version into the shared cell, then calls `wake` so the UI thread
/// repaints and Settings > About reflects the result without waiting for an
/// incidental repaint. Never blocks the caller.
///
/// `wake` is typically a closure that clones the shell repaint waker — e.g.
/// `let wake = ui_wake.clone(); move || wake()`.
pub fn spawn_check(wake: impl Fn() + Send + 'static) {
    // Only the first caller wins the OnceLock; others return immediately.
    if started().set(()).is_err() {
        return;
    }
    set_state(UpdateState::Checking);
    std::thread::spawn(move || {
        match fetch_latest() {
            Some(version) => {
                if let Ok(mut guard) = cell().lock() {
                    *guard = Some(version.clone());
                }
                set_state(UpdateState::UpdateAvailable { version });
            }
            None => {
                // No newer stable release — settle back to Idle so any
                // "checking…" affordance clears.
                if matches!(update_state(), UpdateState::Checking) {
                    set_state(UpdateState::Idle);
                }
            }
        }
        // Wake the UI regardless of outcome so a "checking…" state can settle.
        wake();
    });
}

/// Returns `Some(version)` when a newer stable release than the running build is
/// available, else `None`. Non-blocking; returns `None` until the background
/// check completes. Never panics.
pub fn latest_available() -> Option<String> {
    cell().lock().ok().and_then(|g| g.clone())
}

/// Blocking network fetch (runs on the spawned thread). Returns the newer version
/// string (without the leading `v`) or `None` on any failure / no update.
fn fetch_latest() -> Option<String> {
    let release = fetch_release()?;
    if release.draft || release.prerelease {
        return None;
    }
    let tag = release.tag_name.trim_start_matches('v').to_string();
    if is_newer(&tag, CURRENT_VERSION) {
        Some(tag)
    } else {
        None
    }
}

fn fetch_release() -> Option<Release> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build();
    let response = agent.get(GITHUB_LATEST).call().ok()?;
    response.into_json().ok()
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(serde::Deserialize)]
struct Asset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = s.split('.').take(3).collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Staged download
// ---------------------------------------------------------------------------

/// Kick off a background download-and-stage of the latest release's DMG.
/// Idempotent: only one download runs at a time — a call while a download is
/// already in flight (or already `Ready`) is a no-op.
///
/// Re-queries GitHub for the latest release's `.dmg` assets (preferring a
/// universal build), tries each candidate in order, and stops at the first that
/// stages successfully. On completion flips state to `Ready { path }` (the
/// staged `Crane.app`) or `Failed { msg }`. `wake` is invoked on each progress
/// tick and at completion so the shell repaints. Never blocks the caller.
pub fn start_download(wake: impl Fn() + Send + Sync + 'static) {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &wake;
        set_state(UpdateState::Failed {
            msg: "In-app update is only supported on macOS. \
                  Download the latest release from GitHub."
                .to_string(),
        });
        wake();
        return;
    }
    #[cfg(target_os = "macos")]
    {
        // Single-flight: bail if a download is already running or staged.
        if download_in_flight()
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        if matches!(update_state(), UpdateState::Ready { .. }) {
            download_in_flight().store(false, Ordering::SeqCst);
            return;
        }
        set_state(UpdateState::Downloading {
            received: 0,
            total: 0,
        });
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let wake2 = wake.clone();
        std::thread::spawn(move || {
            let urls = fetch_dmg_urls();
            if urls.is_empty() {
                set_state(UpdateState::Failed {
                    msg: "No DMG asset found on the latest release.".to_string(),
                });
                download_in_flight().store(false, Ordering::SeqCst);
                wake2();
                return;
            }
            let mut last_err: Option<String> = None;
            for url in &urls {
                match do_download_and_stage(url, &wake2) {
                    Ok(path) => {
                        set_state(UpdateState::Ready { path });
                        download_in_flight().store(false, Ordering::SeqCst);
                        wake2();
                        return;
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                        // Reset progress for the next candidate URL.
                        set_state(UpdateState::Downloading {
                            received: 0,
                            total: 0,
                        });
                    }
                }
            }
            set_state(UpdateState::Failed {
                msg: last_err.unwrap_or_else(|| "no candidate DMG succeeded".to_string()),
            });
            download_in_flight().store(false, Ordering::SeqCst);
            wake2();
        });
    }
}

/// Candidate DMG download URLs for the latest release, universal build first.
/// Empty when no `.dmg` asset is published or the fetch fails.
fn fetch_dmg_urls() -> Vec<String> {
    let Some(release) = fetch_release() else {
        return Vec::new();
    };
    if release.draft || release.prerelease {
        return Vec::new();
    }
    let mut dmgs: Vec<String> = release
        .assets
        .into_iter()
        .filter(|a| {
            a.name.to_ascii_lowercase().ends_with(".dmg") && !a.browser_download_url.is_empty()
        })
        .map(|a| a.browser_download_url)
        .collect();
    // Prefer a universal DMG (runs on both arches) over an arch-specific one.
    dmgs.sort_by_key(|u| !u.to_ascii_lowercase().contains("universal"));
    dmgs
}

fn update_dir() -> std::io::Result<PathBuf> {
    let home = crate::util::home_dir().ok_or_else(|| std::io::Error::other("no HOME dir"))?;
    let dir = home.join(".crane").join("update");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Stream `url` to `dest`, publishing `Downloading { received, total }` progress
/// on each chunk and waking the UI. `total` is `0` when `Content-Length` is
/// absent.
fn download_to(url: &str, dest: &Path, wake: &Arc<dyn Fn() + Send + Sync>) -> std::io::Result<()> {
    // Explicit timeouts so a stalled TCP / slow server doesn't freeze the
    // update worker thread indefinitely.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(300))
        .user_agent(USER_AGENT)
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| std::io::Error::other(format!("GET failed: {e}")))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest)?;
    let mut buf = [0u8; 64 * 1024];
    let mut received: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        received += n as u64;
        set_state(UpdateState::Downloading { received, total });
        wake();
    }
    drop(file);
    Ok(())
}

// ---------------------------------------------------------------------------
// Apply + relaunch
// ---------------------------------------------------------------------------

/// Swap the running install for the staged `Crane.app` at `path` and relaunch,
/// then exit the current process. Writes a detached swap script that waits for
/// this PID to exit, moves the new bundle into place, and re-opens Crane.
///
/// macOS only — a no-op stub elsewhere (state flips to `Failed`). On success
/// this call does not return (it `exit(0)`s after spawning the script).
#[cfg(target_os = "macos")]
pub fn apply_and_restart(path: &Path) {
    let Some(target) = current_install_path() else {
        set_state(UpdateState::Failed {
            msg: "Couldn't locate the running Crane install.".to_string(),
        });
        return;
    };
    if let Err(e) = write_and_spawn_swap_script(&target, path) {
        set_state(UpdateState::Failed {
            msg: format!("spawn swap: {e}"),
        });
        return;
    }
    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
pub fn apply_and_restart(_path: &Path) {
    set_state(UpdateState::Failed {
        msg: "In-app restart is only supported on macOS.".to_string(),
    });
}

// ---------------------------------------------------------------------------
// macOS staging + swap
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn do_download_and_stage(
    url: &str,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> std::io::Result<PathBuf> {
    let dir = update_dir()?;
    let dmg_path = dir.join("crane.dmg");
    download_to(url, &dmg_path, wake)?;

    let mount_out = std::process::Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-noverify",
            "-noautoopen",
            dmg_path.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !mount_out.status.success() {
        return Err(std::io::Error::other(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&mount_out.stderr)
        )));
    }
    let mount_stdout = String::from_utf8_lossy(&mount_out.stdout).to_string();
    // Mount point is the token after the last tab on the line that names a
    // `/Volumes/` path.
    let volume = mount_stdout
        .lines()
        .filter_map(|l| l.rsplit_once('\t').map(|(_, rhs)| rhs.trim()))
        .find(|p| p.starts_with("/Volumes/"))
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("no /Volumes mount point"))?;

    let src_app = volume.join("Crane.app");
    let dest_app = dir.join("Crane.app");
    if dest_app.exists() {
        let _ = std::fs::remove_dir_all(&dest_app);
    }
    let cp_out = std::process::Command::new("cp")
        .args([
            "-R",
            src_app.to_string_lossy().as_ref(),
            dest_app.to_string_lossy().as_ref(),
        ])
        .output()?;
    // Always detach, even if cp failed.
    let _ = std::process::Command::new("hdiutil")
        .args(["detach", volume.to_string_lossy().as_ref(), "-quiet"])
        .output();
    if !cp_out.status.success() {
        return Err(std::io::Error::other(format!(
            "cp .app failed: {}",
            String::from_utf8_lossy(&cp_out.stderr)
        )));
    }

    // codesign --verify --deep --strict catches tampering between our build and
    // the user's download (flipped bits, MITM, partial unzip) — even for
    // ad-hoc-signed builds.
    let verify = std::process::Command::new("codesign")
        .args([
            "--verify",
            "--deep",
            "--strict",
            dest_app.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !verify.status.success() {
        let _ = std::fs::remove_dir_all(&dest_app);
        return Err(std::io::Error::other(format!(
            "signature verify failed: {}",
            String::from_utf8_lossy(&verify.stderr)
        )));
    }

    // Strip the quarantine bit so Gatekeeper doesn't re-prompt on relaunch.
    let _ = std::process::Command::new("xattr")
        .args([
            "-dr",
            "com.apple.quarantine",
            dest_app.to_string_lossy().as_ref(),
        ])
        .output();

    let _ = std::fs::remove_file(&dmg_path);
    Ok(dest_app)
}

/// Walk up from the running executable to the enclosing `.app` bundle.
#[cfg(target_os = "macos")]
fn current_install_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut cur = exe;
    while cur.pop() {
        if let Some(name) = cur.file_name()
            && let Some(s) = name.to_str()
            && s.ends_with(".app")
        {
            return Some(cur);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn write_and_spawn_swap_script(target: &Path, staged: &Path) -> std::io::Result<()> {
    let pid = std::process::id();
    let script_path = std::env::temp_dir().join(format!("crane-swap-{pid}.sh"));
    let target_esc = target.to_string_lossy().replace('"', "\\\"");
    let staged_esc = staged.to_string_lossy().replace('"', "\\\"");
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail
TARGET="{}"
STAGED="{}"
PID="{}"

# Wait (up to 10 s) for the running Crane to exit.
for i in {{1..50}}; do
  if ! kill -0 "$PID" 2>/dev/null; then
    break
  fi
  sleep 0.2
done

# Back up current, swap in new.
if [ -d "$TARGET" ]; then
  rm -rf "${{TARGET}}.old"
  mv "$TARGET" "${{TARGET}}.old" || true
fi
cp -R "$STAGED" "$TARGET"
xattr -dr com.apple.quarantine "$TARGET" 2>/dev/null || true

open "$TARGET"
rm -rf "$STAGED"
rm -rf "${{TARGET}}.old"
"#,
        target_esc, staged_esc, pid
    );
    std::fs::write(&script_path, script)?;
    use std::os::unix::fs::PermissionsExt;
    let mut p = std::fs::metadata(&script_path)?.permissions();
    p.set_mode(0o755);
    std::fs::set_permissions(&script_path, p)?;
    std::process::Command::new("/bin/bash")
        .arg(&script_path)
        .spawn()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Non-macOS stub
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn do_download_and_stage(
    _url: &str,
    _wake: &Arc<dyn Fn() + Send + Sync>,
) -> std::io::Result<PathBuf> {
    Err(std::io::Error::other(
        "in-app update not implemented on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_major_minor_patch() {
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.3", "0.1.2"));
        assert!(is_newer("0.2.0", "0.1.9"));
    }

    #[test]
    fn not_newer_when_equal_or_older() {
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn bad_versions_are_not_newer() {
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
    }
}
