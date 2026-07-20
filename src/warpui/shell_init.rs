//! Installs Crane's bundled shell-integration scripts under `~/.crane/shell/`
//! at startup and reports the env a PTY needs to load them. The scripts are
//! embedded at compile time (`include_str!`) and rewritten on every launch so
//! a Crane upgrade always ships the current hooks. Editing the user's own
//! rc files is deliberately avoided — zsh loads ours via a ZDOTDIR shim.

use std::path::PathBuf;

const ZSH_INIT: &str = include_str!("../../assets/shell/crane-init.zsh");
const ZSH_RC: &str = include_str!("../../assets/shell/zshrc");
const BASH_INIT: &str = include_str!("../../assets/shell/crane-init.bash");

fn shell_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".crane")
        .join("shell")
}

/// The `ZDOTDIR` Crane points zsh at (contains our `.zshrc` shim).
pub fn zsh_zdotdir() -> PathBuf {
    shell_root().join("zsh")
}

/// The `--rcfile` Crane starts bash with.
pub fn bash_rcfile() -> PathBuf {
    shell_root().join("crane-init.bash")
}

/// Write (or overwrite) the bundled scripts. Idempotent, best-effort — a write
/// failure just means shell integration is unavailable this run.
pub fn install_shell_scripts() {
    let zdot = zsh_zdotdir();
    let _ = std::fs::create_dir_all(&zdot);
    let _ = std::fs::write(zdot.join("crane-init.zsh"), ZSH_INIT);
    let _ = std::fs::write(zdot.join(".zshrc"), ZSH_RC);
    let _ = std::fs::write(bash_rcfile(), BASH_INIT);
}

/// Install once per process, on first use. Cheap enough to call per PTY spawn.
pub fn ensure_installed() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(install_shell_scripts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_all_three_scripts() {
        // Redirect HOME to a temp dir so we don't touch the real ~/.crane.
        let tmp = std::env::temp_dir().join(format!(
            "crane-shellinit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("HOME");
        // SAFETY: single-threaded test; restored below.
        unsafe { std::env::set_var("HOME", &tmp); }

        install_shell_scripts();

        assert!(tmp.join(".crane/shell/zsh/crane-init.zsh").exists());
        assert!(tmp.join(".crane/shell/zsh/.zshrc").exists());
        assert!(tmp.join(".crane/shell/crane-init.bash").exists());
        assert_eq!(zsh_zdotdir(), tmp.join(".crane/shell/zsh"));

        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
