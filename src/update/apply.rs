//! Staged auto-update.
//!
//! macOS:
//!   1. Download the universal DMG to `~/.crane/update/crane.dmg`.
//!   2. `hdiutil attach` mounts it.
//!   3. Mounted `Crane.app` → `~/.crane/update/Crane.app` (staging dir).
//!   4. `hdiutil detach`.
//!   5. Strip `com.apple.quarantine` so Gatekeeper doesn't re-prompt.
//!   6. User clicks "Restart now" → write `/tmp/crane-swap-<pid>.sh`,
//!      spawn detached. Script waits for our PID, swaps the bundle,
//!      relaunches via `open`.
//!
//! Linux (self-managed installs only — see `LinuxInstallKind`):
//!   1. Download `crane-<ver>-x86_64-linux.tar.gz` to
//!      `~/.crane/update/crane.tar.gz`.
//!   2. Extract via `tar -xzf` to `~/.crane/update/staging/`.
//!   3. Locate the new `crane` binary inside the extracted dir.
//!   4. Swap script: wait for PID, `cp -f` over `current_exe()`,
//!      `chmod +x`, re-exec detached via `setsid`.
//!
//! Linux Snap / Flatpak / apt-installed binaries are detected up
//! front; their auto-update path is "use your package manager",
//! never an in-app overwrite (no privileges, would race the
//! distro's update mechanism).
//!
//! Windows: not yet — falls back to opening the release page.

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

/// Linux install provenance. Only `SelfManaged` is safe to
/// auto-overwrite — every other kind is owned by a package manager
/// or a sandbox and would either fail or fight the distro's own
/// update mechanism.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxInstallKind {
    /// Snap install (e.g. Ubuntu Software). Read-only squashfs.
    /// Updates flow via `snapd` / `snap refresh`.
    Snap,
    /// Flatpak install. Sandboxed, immutable. Updates via `flatpak update`.
    Flatpak,
    /// apt / dpkg / dnf / pacman / etc. Binary owned by root in a
    /// system path. Requires user's package manager.
    SystemPackage,
    /// Tarball drop into a user-writable location (`~/.local/bin`,
    /// `/opt/crane/bin`, `~/Applications`, …). Auto-update works.
    SelfManaged,
}

#[cfg(target_os = "linux")]
fn linux_install_kind() -> LinuxInstallKind {
    // Snap sets $SNAP to its install root inside the sandbox.
    if std::env::var_os("SNAP").is_some() {
        return LinuxInstallKind::Snap;
    }
    // Flatpak's runtime-spawned processes inherit $FLATPAK_ID.
    if std::env::var_os("FLATPAK_ID").is_some() {
        return LinuxInstallKind::Flatpak;
    }
    let Ok(exe) = std::env::current_exe() else {
        // Can't resolve the binary path — assume system to fail safe.
        return LinuxInstallKind::SystemPackage;
    };
    // Probe the parent dir's writability rather than the file itself —
    // the swap step needs to create / replace a file IN that dir, and
    // a write-bit on the binary alone wouldn't tell us about a
    // read-only mount or a dir with a sticky bit.
    if !parent_dir_writable(&exe) {
        return LinuxInstallKind::SystemPackage;
    }
    LinuxInstallKind::SelfManaged
}

#[cfg(target_os = "linux")]
fn parent_dir_writable(p: &Path) -> bool {
    let Some(parent) = p.parent() else {
        return false;
    };
    // Sentinel-file probe: honours real fs ACLs / immutable bits /
    // nosuid mounts the way the upcoming write would actually fail.
    let probe = parent.join(".crane-update-probe");
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
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

    /// True when this binary's install context allows in-app update.
    pub fn is_supported_platform() -> bool {
        #[cfg(target_os = "macos")]
        {
            true
        }
        #[cfg(target_os = "linux")]
        {
            matches!(linux_install_kind(), LinuxInstallKind::SelfManaged)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// When in-app update is unavailable on a platform we recognise,
    /// a plain-language reason the user can act on. None for fully-
    /// supported builds and on Windows (where the generic "open the
    /// release page" fallback is the right UX anyway).
    pub fn unsupported_reason() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            return match linux_install_kind() {
                LinuxInstallKind::Snap => Some(
                    "Crane was installed via Snap. Updates come through \
                     `snap refresh crane` or Snap's auto-refresh."
                        .to_string(),
                ),
                LinuxInstallKind::Flatpak => Some(
                    "Crane was installed via Flatpak. Update with \
                     `flatpak update crane`."
                        .to_string(),
                ),
                LinuxInstallKind::SystemPackage => Some(
                    "Crane was installed via your system package manager. \
                     Update with `sudo apt upgrade crane` (or your \
                     distro's equivalent)."
                        .to_string(),
                ),
                LinuxInstallKind::SelfManaged => None,
            };
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// Kick off a background download-and-stage for a list of candidate
    /// asset URLs. Tries each in order, stops at the first 200. Lets
    /// callers supply arch-specific + universal URLs and fall through
    /// whichever the release actually shipped.
    pub fn start(&self, urls: Vec<String>, ctx: egui::Context) {
        if !Self::is_supported_platform() {
            *self.state.lock() = UpdateState::Failed(
                Self::unsupported_reason().unwrap_or_else(|| {
                    "In-app update isn't supported on this platform yet. \
                     Open the Releases page to download the installer."
                        .to_string()
                }),
            );
            return;
        }
        if urls.is_empty() {
            *self.state.lock() =
                UpdateState::Failed("No download URL available for this build.".into());
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
            let mut last_err: Option<String> = None;
            for url in &urls {
                match do_download_and_stage(url, &state, &ctx2) {
                    Ok(path) => {
                        *state.lock() = UpdateState::Ready {
                            staged_bundle: path,
                        };
                        ctx2.request_repaint();
                        return;
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                        // Reset to Downloading for the next candidate.
                        *state.lock() = UpdateState::Downloading { bytes: 0 };
                    }
                }
            }
            *state.lock() = UpdateState::Failed(
                last_err.unwrap_or_else(|| "no candidate URL succeeded".into()),
            );
            ctx2.request_repaint();
        });
    }

    /// Spawn the swap script and exit. No-ops if state is not Ready.
    pub fn apply_and_exit(&self) {
        let staged = match self.state.lock().clone() {
            UpdateState::Ready { staged_bundle } => staged_bundle,
            _ => return,
        };
        let Some(target) = current_install_path() else {
            *self.state.lock() =
                UpdateState::Failed("Couldn't locate running Crane install".into());
            return;
        };
        if let Err(e) = write_and_spawn_swap_script(&target, &staged) {
            *self.state.lock() = UpdateState::Failed(format!("spawn swap: {e}"));
            return;
        }
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn update_dir() -> std::io::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| std::io::Error::other("no HOME"))?;
    let dir = PathBuf::from(home).join(".crane").join("update");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn download_to(
    url: &str,
    dest: &Path,
    state: &Arc<Mutex<UpdateState>>,
    ctx: &egui::Context,
) -> std::io::Result<()> {
    // Explicit timeouts so a stalled TCP / slow server doesn't freeze
    // the update worker thread indefinitely.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(300))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| std::io::Error::other(format!("GET failed: {e}")))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest)?;
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
    Ok(())
}

fn spawn_swap_script(script_path: &Path, script: &str, interpreter: &str) -> std::io::Result<()> {
    std::fs::write(script_path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(script_path)?.permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(script_path, p)?;
    }
    std::process::Command::new(interpreter)
        .arg(script_path)
        .spawn()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// macOS path
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn do_download_and_stage(
    url: &str,
    state: &Arc<Mutex<UpdateState>>,
    ctx: &egui::Context,
) -> std::io::Result<PathBuf> {
    let dir = update_dir()?;
    let dmg_path = dir.join("crane.dmg");
    download_to(url, &dmg_path, state, ctx)?;

    *state.lock() = UpdateState::Installing;
    ctx.request_repaint();

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
    // Mount point is the last whitespace-separated token on the last
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

    // codesign --verify --deep --strict catches tampering between our
    // build and the user's download (flipped bits, MITM, partial
    // download that got unzipped) — even for ad-hoc-signed builds.
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
    spawn_swap_script(&script_path, &script, "/bin/bash")
}

// ---------------------------------------------------------------------------
// Linux path
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn do_download_and_stage(
    url: &str,
    state: &Arc<Mutex<UpdateState>>,
    ctx: &egui::Context,
) -> std::io::Result<PathBuf> {
    let dir = update_dir()?;
    let tar_path = dir.join("crane.tar.gz");
    let staging = dir.join("staging");
    // Fresh staging each attempt — leftovers from a previous failed
    // run could shadow the new binary (or change which one
    // `find_extracted_binary` picks up first).
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    download_to(url, &tar_path, state, ctx)?;

    *state.lock() = UpdateState::Installing;
    ctx.request_repaint();

    let extract = std::process::Command::new("tar")
        .args([
            "-xzf",
            tar_path.to_string_lossy().as_ref(),
            "-C",
            staging.to_string_lossy().as_ref(),
        ])
        .output()?;
    if !extract.status.success() {
        return Err(std::io::Error::other(format!(
            "tar extract failed: {}",
            String::from_utf8_lossy(&extract.stderr)
        )));
    }
    let _ = std::fs::remove_file(&tar_path);

    // Workflow tarball expands to `crane-<ver>-x86_64-linux/` with the
    // binary inside. We don't bake the version into the path here so
    // a future tarball naming change doesn't silently break updates.
    let bin = find_extracted_binary(&staging)
        .ok_or_else(|| std::io::Error::other("extracted tarball had no `crane` binary"))?;

    // Tar should preserve +x but if the runner stripped modes,
    // restore +x explicitly so the swap script's exec succeeds.
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&bin)?.permissions();
    if perms.mode() & 0o111 == 0 {
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms)?;
    }
    Ok(bin)
}

#[cfg(target_os = "linux")]
fn find_extracted_binary(staging: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(staging).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            let candidate = path.join("crane");
            if candidate.is_file() {
                return Some(candidate);
            }
        } else if path.is_file()
            && path.file_name().and_then(|n| n.to_str()) == Some("crane")
        {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn current_install_path() -> Option<PathBuf> {
    // On Linux we overwrite the binary in place. `current_exe()`
    // returns the canonicalised path to the running binary (not the
    // symlink that started us), which is exactly what the swap needs.
    std::env::current_exe().ok()
}

#[cfg(target_os = "linux")]
fn write_and_spawn_swap_script(target: &Path, staged: &Path) -> std::io::Result<()> {
    let pid = std::process::id();
    let script_path = std::env::temp_dir().join(format!("crane-swap-{pid}.sh"));
    let target_esc = target.to_string_lossy().replace('"', "\\\"");
    let staged_esc = staged.to_string_lossy().replace('"', "\\\"");
    // POSIX sh — bash isn't guaranteed on minimal distros (Alpine ships
    // ash by default). `setsid` detaches the new Crane from the
    // script's process group so it survives the script exiting; we
    // fall back to `nohup` if setsid is missing on some embedded
    // distros.
    let script = format!(
        r#"#!/bin/sh
set -eu
TARGET="{}"
STAGED="{}"
PID="{}"

i=0
while [ "$i" -lt 50 ]; do
  if ! kill -0 "$PID" 2>/dev/null; then
    break
  fi
  i=$((i + 1))
  sleep 0.2
done

cp -f "$STAGED" "$TARGET"
chmod +x "$TARGET"

if command -v setsid >/dev/null 2>&1; then
  setsid "$TARGET" </dev/null >/dev/null 2>&1 &
else
  nohup "$TARGET" </dev/null >/dev/null 2>&1 &
fi

rm -rf "$(dirname "$STAGED")"
"#,
        target_esc, staged_esc, pid
    );
    spawn_swap_script(&script_path, &script, "/bin/sh")
}

// ---------------------------------------------------------------------------
// Stubs for other platforms
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn do_download_and_stage(
    _url: &str,
    _state: &Arc<Mutex<UpdateState>>,
    _ctx: &egui::Context,
) -> std::io::Result<PathBuf> {
    Err(std::io::Error::other(
        "in-app update not implemented on this platform",
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_install_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn write_and_spawn_swap_script(_target: &Path, _staged: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "in-app swap not implemented on this platform",
    ))
}
