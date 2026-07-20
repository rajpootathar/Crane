//! Installs Crane's bundled shell-integration scripts under `~/.crane/shell/`
//! at startup and reports the env a PTY needs to load them. The scripts are
//! embedded at compile time (`include_str!`) and rewritten on every launch so
//! a Crane upgrade always ships the current hooks. Editing the user's own
//! rc files is deliberately avoided — zsh loads ours via a ZDOTDIR shim.
//!
//! Every path is derived from a `root` argument rather than read from the
//! environment inside the logic, so tests can point the whole install at a
//! temp directory without mutating the process-global `HOME` (which `cargo
//! test`'s parallel threads share).

use std::path::{Path, PathBuf};

const ZSH_INIT: &str = include_str!("../../assets/shell/crane-init.zsh");
// ZDOTDIR redirects *every* per-user file zsh reads, so all four are shimmed;
// shipping only `.zshrc` silently dropped the user's own `~/.zshenv`.
const ZSH_ENV: &str = include_str!("../../assets/shell/zshenv");
const ZSH_PROFILE: &str = include_str!("../../assets/shell/zprofile");
const ZSH_RC: &str = include_str!("../../assets/shell/zshrc");
const ZSH_LOGIN: &str = include_str!("../../assets/shell/zlogin");
const BASH_INIT: &str = include_str!("../../assets/shell/crane-init.bash");

fn shell_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".crane")
        .join("shell")
}

/// The `ZDOTDIR` Crane points zsh at (contains our four shims).
pub fn zsh_zdotdir() -> PathBuf {
    zsh_zdotdir_at(&shell_root())
}

/// [`zsh_zdotdir`] against an explicit install root.
pub fn zsh_zdotdir_at(root: &Path) -> PathBuf {
    root.join("zsh")
}

/// The `--rcfile` Crane starts bash with.
pub fn bash_rcfile() -> PathBuf {
    bash_rcfile_at(&shell_root())
}

/// [`bash_rcfile`] against an explicit install root.
pub fn bash_rcfile_at(root: &Path) -> PathBuf {
    root.join("crane-init.bash")
}

/// Write (or overwrite) the bundled scripts under the real `~/.crane/shell`.
pub fn install_shell_scripts() {
    install_shell_scripts_at(&shell_root());
}

/// Write (or overwrite) the bundled scripts under `root`. Idempotent,
/// best-effort — a write failure just means shell integration is unavailable
/// this run, never a broken shell.
pub fn install_shell_scripts_at(root: &Path) {
    let zdot = zsh_zdotdir_at(root);
    let _ = std::fs::create_dir_all(&zdot);
    write_atomic(&zdot.join("crane-init.zsh"), ZSH_INIT);
    write_atomic(&zdot.join(".zshenv"), ZSH_ENV);
    write_atomic(&zdot.join(".zprofile"), ZSH_PROFILE);
    write_atomic(&zdot.join(".zshrc"), ZSH_RC);
    write_atomic(&zdot.join(".zlogin"), ZSH_LOGIN);
    write_atomic(&bash_rcfile_at(root), BASH_INIT);
}

/// Replace `path` with `contents` in one step: write a sibling temp file, then
/// `rename` over the target, which is atomic on POSIX.
///
/// A plain `fs::write` truncates before writing, so a second Crane process
/// reinstalling while a shell in the first is mid-`source` can hand that shell
/// a half-written shim. Worst case is a `.zshenv` truncated after the ZDOTDIR
/// bookkeeping but before it sources the user's files — a shell with no config
/// at all. `rename` means a reader sees either the whole old file or the whole
/// new one.
///
/// Best-effort throughout: never panics, never propagates. A leftover temp file
/// is cleaned up on failure.
fn write_atomic(path: &Path, contents: &str) {
    let Some(dir) = path.parent() else { return };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };

    // pid + counter keeps concurrent installers (and repeat calls) off each
    // other's temp file. Leading dot so a stray one stays hidden.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.crane-tmp.{}.{seq}", std::process::id()));

    if std::fs::write(&tmp, contents).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(&tmp);
}

/// Install once per process, on first use. Cheap enough to call per PTY spawn.
pub fn ensure_installed() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(install_shell_scripts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "crane-shellinit-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Note the install root is passed in, not faked via `HOME`: `cargo test`
    /// runs these on parallel threads sharing one process environment, so
    /// setting `HOME` here would race every other test and env reader — and a
    /// losing interleaving would write into the user's live `~/.crane/shell`.
    #[test]
    fn install_writes_every_script() {
        let root = temp_root("all");

        install_shell_scripts_at(&root);

        let zdot = zsh_zdotdir_at(&root);
        assert_eq!(zdot, root.join("zsh"));
        for name in [
            "crane-init.zsh",
            ".zshenv",
            ".zprofile",
            ".zshrc",
            ".zlogin",
        ] {
            assert!(zdot.join(name).exists(), "missing {name}");
        }
        assert_eq!(bash_rcfile_at(&root), root.join("crane-init.bash"));
        assert!(bash_rcfile_at(&root).exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// Reinstalling replaces content rather than appending or leaving the old
    /// bytes, and leaves no `.crane-tmp.*` debris behind for the rename path.
    #[test]
    fn reinstall_replaces_content_and_leaves_no_temp_files() {
        let root = temp_root("reinstall");
        let zdot = zsh_zdotdir_at(&root);
        std::fs::create_dir_all(&zdot).unwrap();
        std::fs::write(zdot.join(".zshrc"), "stale contents").unwrap();

        install_shell_scripts_at(&root);
        install_shell_scripts_at(&root);

        assert_eq!(std::fs::read_to_string(zdot.join(".zshrc")).unwrap(), ZSH_RC);
        let debris: Vec<_> = std::fs::read_dir(&zdot)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("crane-tmp"))
            .collect();
        assert!(debris.is_empty(), "temp files left behind: {debris:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The zsh shims are the load-bearing part of "never break the user's
    /// shell": all four must be present, and .zshenv must both source the
    /// user's own .zshenv and hand ZDOTDIR back to Crane's directory.
    #[test]
    fn zshenv_shim_sources_user_file_and_restores_zdotdir() {
        assert!(ZSH_ENV.contains("$CRANE_USER_ZDOTDIR/.zshenv"));
        assert!(ZSH_ENV.contains("ZDOTDIR=\"$CRANE_ZDOTDIR\""));
        // Crane's own hooks load last, from the .zshrc shim.
        assert!(ZSH_RC.contains("crane-init.zsh"));
    }
}
