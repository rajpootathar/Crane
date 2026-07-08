mod format;
mod git;
mod lsp;
mod startup;
mod syntax;
mod theme;
mod util;
mod warpui;

/// Crane entry point. warpui (GPU-rendered, `src/warpui/`) is the sole
/// frontend — it owns its own NSApplication / event loop. The legacy egui
/// frontend has been removed. `main` performs the shared startup (PATH
/// rehydration for GUI launches, config-dir migration) then hands control to
/// warpui.
fn main() {
    env_logger::init();
    install_crash_logger();
    // Login-shells aren't sourced by Finder / Dock when launching a GUI app,
    // so PATH ends up stripped down to the system defaults. Rehydrate it
    // before warpui spawns any PTY.
    startup::fix_path_for_gui_launch();
    // One-shot migration of the old `~/.config/crane` config directory to
    // `~/.crane`. Cheap no-op after the first run.
    startup::migrate_config_dir();
    warpui::run();
}

/// Install a panic hook that writes the message + location + a full
/// backtrace to `~/.crane/crash.log` before the default hook runs.
///
/// GUI apps launched from Finder/Dock have no attached terminal, so a
/// panic's message — normally printed to stderr — goes nowhere; macOS's own
/// `.ips` crash report captures a raw address backtrace but not the panic
/// text, and the release binary strips symbols, so those addresses don't
/// even resolve to function names without the exact original build. This
/// hook is the only reliable way to see what actually broke.
fn install_crash_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = crane_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::error!("crash logger: could not create {}: {e}", dir.display());
        } else {
            let path = dir.join("crash.log");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bt = std::backtrace::Backtrace::force_capture();
            let entry = format!(
                "\n===== crash at unix {ts} (Crane v{}) =====\n{info}\n{bt}\n",
                env!("CARGO_PKG_VERSION")
            );
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                use std::io::Write;
                let _ = f.write_all(entry.as_bytes());
            }
        }
        // Still run the default hook (stderr message when a terminal IS
        // attached, e.g. debug/dev runs) so nothing regresses there.
        default_hook(info);
    }));
}

fn crane_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".crane"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".crane"))
}
