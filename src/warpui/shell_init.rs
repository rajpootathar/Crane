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

/// Every file the zsh shim directory must contain. `ZDOTDIR` *replaces* the
/// set of startup files zsh reads from `$HOME`, so each missing name here is
/// one of the user's own startup files silently no longer loading.
const ZSH_SHIM_FILES: [&str; 5] = [
    "crane-init.zsh",
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
];

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

/// The `ZDOTDIR` to hand a spawned zsh — `None` when the shims are not on
/// disk, which is the *only* safe answer in that case.
///
/// [`install_shell_scripts`] is best-effort: a read-only or full `$HOME`, a
/// sandboxed home, or an unset `HOME` (which makes [`shell_root`] relative)
/// all leave it having written nothing, and [`ensure_installed`] never
/// retries. Because `ZDOTDIR` replaces the user's startup files rather than
/// adding to them, exporting it at a directory that does not exist yields a
/// shell with no prompt, no aliases and no PATH additions — strictly worse
/// than not integrating at all. Degrading to a plain, fully-working shell is
/// the contract.
pub fn installed_zsh_zdotdir() -> Option<PathBuf> {
    installed_zsh_zdotdir_at(&shell_root())
}

/// [`installed_zsh_zdotdir`] against an explicit install root.
fn installed_zsh_zdotdir_at(root: &Path) -> Option<PathBuf> {
    let dir = zsh_zdotdir_at(root);
    ZSH_SHIM_FILES
        .iter()
        .all(|name| dir.join(name).is_file())
        .then_some(dir)
}

/// The value to hand a spawned zsh as `CRANE_OLD_ZDOTDIR`: the `ZDOTDIR` this
/// process inherited, but ONLY when it is genuinely the user's own.
///
/// Inside a Crane terminal, `ZDOTDIR` already points at [`zsh_zdotdir`], and
/// that value is inherited by anything launched from there — including another
/// Crane. Passing it straight through would tell the shim that Crane's own
/// directory *is* the user's, so every `source "$CRANE_USER_ZDOTDIR/.zsh*"`
/// would source the shim running it. The shim guards against that, but the
/// guard's fallback is `$HOME`, and the mere presence of `CRANE_OLD_ZDOTDIR`
/// also makes the shim pre-set `ZDOTDIR` before sourcing the user's `.zshenv`
/// — which defeats the guarded `export ZDOTDIR="${ZDOTDIR:-…}"` idiom users
/// relocate their config with, leaving them at a prompt with no rc at all.
///
/// `None` is the useful answer in that case: it routes the shim down its
/// `unset ZDOTDIR` path, which is exactly what a shell launched outside Crane
/// would have seen.
pub fn inherited_user_zdotdir() -> Option<std::ffi::OsString> {
    user_zdotdir_excluding(std::env::var_os("ZDOTDIR"), &zsh_zdotdir())
}

/// [`inherited_user_zdotdir`] against an explicit "Crane's own" directory, so
/// the comparison is testable without touching `HOME` or the real install.
fn user_zdotdir_excluding(
    inherited: Option<std::ffi::OsString>,
    crane: &Path,
) -> Option<std::ffi::OsString> {
    let value = inherited?;
    // An empty ZDOTDIR means "$HOME" to zsh, and the shim already defaults
    // there; forwarding it would only flip on the pre-set branch above.
    let path = Path::new(&value);
    if value.is_empty() || same_dir(path, crane) || is_crane_shim_dir(path) {
        return None;
    }
    Some(value)
}

/// Whether two paths name the same directory. Literal equality first (works
/// before the install exists), then `canonicalize` so a trailing slash, a `..`,
/// or a symlinked `HOME` can't smuggle Crane's own dir past the check.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Whether `dir` is *some* Crane's zsh shim directory, judged by content
/// rather than by path.
///
/// [`same_dir`] compares against this process's [`zsh_zdotdir`], which is
/// `HOME`-derived — so a Crane running under a different `HOME` (`sudo -E`, a
/// second install, a sandboxed home) has a shim dir neither the literal nor
/// the canonical compare recognises. Forwarding that as `CRANE_OLD_ZDOTDIR`
/// tells our shim that a shim directory is the user's config, stranding the
/// real one exactly as the nested-Crane case does. `crane-init.zsh` is ours
/// and is never a file a user's own ZDOTDIR would hold.
fn is_crane_shim_dir(dir: &Path) -> bool {
    dir.join("crane-init.zsh").is_file()
}

/// The `--rcfile` Crane starts bash with, against an explicit install root.
fn bash_rcfile_at(root: &Path) -> PathBuf {
    root.join("crane-init.bash")
}

/// The `--rcfile` to start bash with — `None` when it was never written.
/// `--rcfile` replaces `~/.bashrc` rather than adding to it (our script
/// sources the user's itself), so aiming it at a missing file is the same
/// self-inflicted breakage as a missing `ZDOTDIR`; see
/// [`installed_zsh_zdotdir`].
pub fn installed_bash_rcfile() -> Option<PathBuf> {
    installed_bash_rcfile_at(&shell_root())
}

/// [`installed_bash_rcfile`] against an explicit install root.
fn installed_bash_rcfile_at(root: &Path) -> Option<PathBuf> {
    let file = bash_rcfile_at(root);
    file.is_file().then_some(file)
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
        for name in ZSH_SHIM_FILES {
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

    /// The nested-Crane trap: a Crane launched from inside a Crane terminal
    /// inherits `ZDOTDIR` already pointing at the shim dir. Forwarding it as
    /// `CRANE_OLD_ZDOTDIR` would make the shim treat its own directory as the
    /// user's; `None` routes it down the `unset ZDOTDIR` path instead.
    #[test]
    fn crane_own_zdotdir_is_never_forwarded_as_the_users() {
        let crane = PathBuf::from("/home/u/.crane/shell/zsh");

        assert_eq!(
            user_zdotdir_excluding(Some(crane.clone().into_os_string()), &crane),
            None,
            "nested Crane must not hand its own ZDOTDIR back as the user's"
        );
        assert_eq!(user_zdotdir_excluding(None, &crane), None);
        assert_eq!(user_zdotdir_excluding(Some("".into()), &crane), None);
    }

    /// A *different* Crane's shim dir (second install, or `sudo -E crane`
    /// under another `HOME`) matches neither the literal nor the canonical
    /// compare against this process's `HOME`-derived dir, so it has to be
    /// recognised by its contents — otherwise it gets forwarded as the user's
    /// and strands their real config exactly like the nested-Crane case.
    #[test]
    fn another_cranes_shim_dir_is_not_mistaken_for_the_users() {
        let root = temp_root("other-crane");
        install_shell_scripts_at(&root);
        let other = zsh_zdotdir_at(&root);
        // "Our" dir is somewhere else entirely — a different HOME.
        let ours = PathBuf::from("/home/someone-else/.crane/shell/zsh");

        assert_eq!(
            user_zdotdir_excluding(Some(other.clone().into_os_string()), &ours),
            None,
            "a shim dir belonging to another Crane is still not the user's"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// FIX: without an existence guard, a failed install (read-only `$HOME`,
    /// quota, sandboxed or unset `HOME`) still had Crane export `ZDOTDIR` /
    /// pass `--rcfile` at paths that were never written. Both mechanisms
    /// REPLACE the user's startup files, so the shell came up with no prompt,
    /// no aliases and no PATH — worse than no integration. Missing install
    /// must mean "spawn a plain shell".
    #[test]
    fn a_missing_install_yields_no_shell_env_at_all() {
        let root = temp_root("missing");
        assert!(!root.exists());

        assert_eq!(installed_zsh_zdotdir_at(&root), None);
        assert_eq!(installed_bash_rcfile_at(&root), None);

        // A partial install is just as fatal: ZDOTDIR redirects *every* startup
        // file, so one shim missing is one of the user's rc files silently
        // never loading. All-or-nothing.
        let zdot = zsh_zdotdir_at(&root);
        std::fs::create_dir_all(&zdot).unwrap();
        for name in ZSH_SHIM_FILES.iter().skip(1) {
            std::fs::write(zdot.join(name), "x").unwrap();
        }
        assert_eq!(
            installed_zsh_zdotdir_at(&root),
            None,
            "a shim dir missing one file must not be used"
        );

        // And a real install is accepted.
        install_shell_scripts_at(&root);
        assert_eq!(installed_zsh_zdotdir_at(&root), Some(zdot));
        assert_eq!(
            installed_bash_rcfile_at(&root),
            Some(bash_rcfile_at(&root))
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A genuinely user-set ZDOTDIR still reaches the shim — without it, a user
    /// who relocated their config outside `.zshenv` loses it entirely.
    #[test]
    fn a_users_own_zdotdir_is_forwarded() {
        let crane = PathBuf::from("/home/u/.crane/shell/zsh");
        assert_eq!(
            user_zdotdir_excluding(Some("/home/u/.config/zsh".into()), &crane),
            Some("/home/u/.config/zsh".into())
        );
    }

    /// Symlink/trailing-slash spellings of Crane's own directory are still
    /// Crane's own directory.
    #[test]
    fn crane_zdotdir_is_matched_through_a_symlink() {
        let root = temp_root("symlink");
        let crane = zsh_zdotdir_at(&root);
        std::fs::create_dir_all(&crane).unwrap();
        let link = root.join("link-to-zsh");
        std::os::unix::fs::symlink(&crane, &link).unwrap();

        assert_eq!(
            user_zdotdir_excluding(Some(link.into_os_string()), &crane),
            None,
            "a symlink to Crane's shim dir is still Crane's shim dir"
        );

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
