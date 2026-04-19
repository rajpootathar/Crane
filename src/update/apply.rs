//! Staged auto-update.
//!
//! Flow (macOS is the fully-supported path today):
//!   1. Background thread downloads the universal DMG to
//!      `~/.crane/update/crane.dmg`, streaming bytes for progress UI.
//!   2. `hdiutil attach` mounts the image.
//!   3. Mounted `Crane.app` → `~/.crane/update/Crane.app` (staging dir).
//!   4. `hdiutil detach`.
//!   5. Strip `com.apple.quarantine` from the staged bundle so Gatekeeper
//!      doesn't re-prompt on launch.
//!   6. User clicks "Restart now" → we write a short shell script to
//!      `/tmp/crane-swap-<pid>.sh` and spawn it detached. The script
//!      waits for our PID to die, moves the staged bundle over the
//!      current one, and re-launches via `open`.
//!
//! Linux (.deb) and Windows (.zip) paths would each need platform-specific
//! swap logic (dpkg / cp / rename-with-.new-trick). Both are left as
//! follow-ups; today's update button falls back to opening the browser.

use parking_lot::Mutex;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum UpdateState {
    Idle,
    Downloading { bytes: u64 },
    Installing,
    Ready { staged_bundle: PathBuf },
    Failed(String),
}

pub struct Updater {
    state: Arc<Mutex<UpdateState>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(UpdateState::Idle)),
        }
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().clone()
    }

    pub fn is_supported_platform() -> bool {
        cfg!(target_os = "macos")
    }

    /// Kick off a background download-and-stage for a DMG url. Must be a
    /// macOS build for the full flow to succeed; other platforms flip to
    /// `Failed` immediately so the UI can fall back to opening a browser.
    pub fn start(&self, url: String, ctx: egui::Context) {
        if !Self::is_supported_platform() {
            *self.state.lock() = UpdateState::Failed(
                "In-app update supported on macOS only today. Open the Releases \
                page to download the installer for your platform."
                    .to_string(),
            );
            return;
        }
        {
            let mut s = self.state.lock();
            if matches!(
                *s,
                UpdateState::Downloading { .. }
                    | UpdateState::Installing
                    | UpdateState::Ready { .. }
            ) {
                return;
            }
            *s = UpdateState::Downloading { bytes: 0 };
        }
        let state = self.state.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            match do_download_and_stage(&url, &state, &ctx2) {
                Ok(path) => {
                    *state.lock() = UpdateState::Ready { staged_bundle: path };
                }
                Err(e) => {
                    *state.lock() = UpdateState::Failed(e.to_string());
                }
            }
            ctx2.request_repaint();
        });
    }

    /// Spawn the swap script and exit. Safe to call even if the state is
    /// not Ready (will no-op).
    pub fn apply_and_exit(&self) {
        let staged = match self.state.lock().clone() {
            UpdateState::Ready { staged_bundle } => staged_bundle,
            _ => return,
        };
        let Some(target) = current_bundle_path() else {
            *self.state.lock() =
                UpdateState::Failed("Couldn't locate running Crane.app".into());
            return;
        };
        if let Err(e) = write_and_spawn_swap_script(&target, &staged) {
            *self.state.lock() = UpdateState::Failed(format!("spawn swap: {e}"));
            return;
        }
        std::process::exit(0);
    }
}

fn do_download_and_stage(
    url: &str,
    state: &Arc<Mutex<UpdateState>>,
    ctx: &egui::Context,
) -> std::io::Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| std::io::Error::other("no HOME"))?;
    let dir = PathBuf::from(home).join(".crane").join("update");
    std::fs::create_dir_all(&dir)?;
    let dmg_path = dir.join("crane.dmg");

    let resp = ureq::get(url)
        .call()
        .map_err(|e| std::io::Error::other(format!("GET failed: {e}")))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&dmg_path)?;
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        total += n as u64;
        *state.lock() = UpdateState::Downloading { bytes: total };
        ctx.request_repaint();
    }
    drop(file);

    *state.lock() = UpdateState::Installing;
    ctx.request_repaint();

    // Mount the DMG.
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
    // The mount point is the last whitespace-separated token on the last
    // line that starts with `/Volumes/`.
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

    // Strip download quarantine so macOS doesn't re-prompt on first launch.
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

fn current_bundle_path() -> Option<PathBuf> {
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

fn write_and_spawn_swap_script(target: &Path, staged: &Path) -> std::io::Result<()> {
    let pid = std::process::id();
    let script_path = std::env::temp_dir().join(format!("crane-swap-{pid}.sh"));
    let script = format!(
        r#"#!/bin/bash
set -e
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

# Relaunch + clean up.
open "$TARGET"
rm -rf "$STAGED"
rm -rf "${{TARGET}}.old"
"#,
        target.to_string_lossy(),
        staged.to_string_lossy(),
        pid
    );
    std::fs::write(&script_path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&script_path)?.permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&script_path, p)?;
    }
    std::process::Command::new("/bin/bash")
        .arg(&script_path)
        .spawn()?;
    Ok(())
}
