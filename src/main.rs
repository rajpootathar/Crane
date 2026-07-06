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
    // Login-shells aren't sourced by Finder / Dock when launching a GUI app,
    // so PATH ends up stripped down to the system defaults. Rehydrate it
    // before warpui spawns any PTY.
    startup::fix_path_for_gui_launch();
    // One-shot migration of the old `~/.config/crane` config directory to
    // `~/.crane`. Cheap no-op after the first run.
    startup::migrate_config_dir();
    warpui::run();
}
