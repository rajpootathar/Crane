//! One-off startup helpers: PATH rehydration for GUI launches and old
//! config-directory migration. The egui-era icon / font / style helpers were
//! removed when warpui became the sole frontend (warpui owns its own icon,
//! fonts, and styling).

/// Login-shells aren't sourced by Finder / Dock when launching a GUI
/// app, so PATH ends up stripped down to the system defaults. Heuristic:
/// if none of the common user-ish PATH entries are present but HOME is
/// set, we're probably GUI-launched — spawn `$SHELL -l -c "echo $PATH"`
/// and import it. Login mode (`-l`) is deliberate: `-i` would source
/// `.zshrc` / `.bashrc`, which triggers nvm / brew shellenv / banners
/// and can add seconds of startup time.
///
/// Unix-only: Windows GUI apps inherit PATH correctly from the
/// environment, so this function is a no-op there.
#[cfg(unix)]
pub fn fix_path_for_gui_launch() {
    let original = std::env::var("PATH").unwrap_or_default();
    let looks_gui = !original.contains("/usr/local/bin")
        && !original.contains("/opt/homebrew/bin")
        && !original.contains(".cargo/bin")
        && std::env::var("HOME").is_ok();

    // Start from whatever PATH we have. If we detect a GUI launch,
    // replace with the login-shell PATH — but don't stop there: login
    // shells don't source `.zshrc` / `.bashrc`, so version-manager
    // installs (nvm, asdf, fnm, volta) are invisible. We always append
    // the sprinkle list below so npm / node / etc. installed via those
    // managers still get resolved.
    let mut current = original.clone();
    if looks_gui {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let output = std::process::Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("echo __CRANE_PATH__:$PATH")
            .output();
        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().find(|l| l.starts_with("__CRANE_PATH__:")) {
                let path = line.trim_start_matches("__CRANE_PATH__:").to_string();
                if !path.is_empty() {
                    current = path;
                }
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let mut extras: Vec<String> = vec![
        format!("{home}/.cargo/bin"),
        format!("{home}/.local/bin"),
        format!("{home}/bin"),
        format!("{home}/go/bin"),
        format!("{home}/.volta/bin"),
        format!("{home}/.fnm/aliases/default/bin"),
        format!("{home}/.asdf/shims"),
        format!("{home}/.bun/bin"),
        format!("{home}/n/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    // nvm installs one directory per Node version; glob and include
    // all of them so whatever the user has active is found. Order by
    // mtime so the most-recently-used version wins the PATH race.
    let nvm_dir = std::path::PathBuf::from(format!("{home}/.nvm/versions/node"));
    if let Ok(rd) = std::fs::read_dir(&nvm_dir) {
        let mut versions: Vec<(std::time::SystemTime, String)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path().join("bin");
                if !p.is_dir() {
                    return None;
                }
                let mtime = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                Some((mtime, p.to_string_lossy().into_owned()))
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in versions {
            extras.push(p);
        }
    }

    // De-dup while preserving order (extras come first so they take
    // precedence for binaries that also exist in /usr/bin).
    let mut seen = std::collections::HashSet::new();
    let mut parts: Vec<String> = Vec::new();
    for p in extras.into_iter().chain(current.split(':').map(|s| s.to_string())) {
        if p.is_empty() || !seen.insert(p.clone()) {
            continue;
        }
        parts.push(p);
    }
    // SAFETY: called from main() before any threads spawn.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

/// No-op on non-Unix platforms (Windows inherits PATH correctly).
#[cfg(not(unix))]
pub fn fix_path_for_gui_launch() {}

/// Earlier builds stored config under `~/.config/crane`; we moved to
/// `~/.crane` so Crane's files sit alongside other dev tools the user
/// typically keeps at the home root. One-shot rename at startup.
pub fn migrate_config_dir() {
    let home = match crate::util::home_dir() {
        Some(h) => h,
        None => return,
    };
    let old_dir = home.join(".config").join("crane");
    let new_dir = home.join(".crane");
    if old_dir.is_dir() && !new_dir.exists() {
        let _ = std::fs::rename(&old_dir, &new_dir);
    }
}
