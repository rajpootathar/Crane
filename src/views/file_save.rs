//! Save / reload / external-change detection for Files pane tabs.
//!
//! Extracted from `file_view.rs`. The save path runs the project's
//! configured formatter, optionally trims trailing whitespace, and
//! notifies the LSP client on success. External-change polling sets
//! `tab.external_change` when disk advanced in a way we didn't cause,
//! so the editor can surface a Reload / Overwrite / Cancel banner.

use super::file_util::trim_trailing_whitespace;
use super::file_view::EditorPrefs;

/// Write `tab.content` to disk. When `force` is false and
/// `tab.external_change` is set, the save is refused — the caller
/// surfaces the banner letting the user pick Reload / Overwrite
/// (force) / Cancel.
pub fn save_tab(
    tab: &mut crate::state::layout::FileTab,
    prefs: EditorPrefs,
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    notify_saved: &dyn Fn(&str, &str),
    force: bool,
) {
    if tab.external_change && !force {
        return;
    }
    if prefs.trim_on_save {
        tab.content = trim_trailing_whitespace(&tab.content);
    }
    if let Some(formatted) = format_before_save(&tab.content, &tab.path) {
        tab.content = formatted;
    }
    if let Err(e) = std::fs::write(&tab.path, &tab.content) {
        eprintln!("save failed: {e}");
        return;
    }
    tab.original_content = tab.content.clone();
    // Invalidate the cached gutter diff so the next frame re-runs
    // `git diff HEAD -- <file>` against the freshly-saved content.
    tab.line_changes_key = 0;
    tab.disk_mtime = std::fs::metadata(&tab.path)
        .and_then(|m| m.modified())
        .ok();
    tab.external_change = false;
    notify_saved(&tab.path, &tab.content);
}

/// Poll the filesystem for external edits. Sets `tab.external_change`
/// when mtime advanced AND the disk bytes differ from what we'd
/// write. Called once per render for the active tab — cheap on SSDs
/// and short-circuited by the mtime check.
pub fn poll_external_change(tab: &mut crate::state::layout::FileTab) {
    if tab.external_change {
        return;
    }
    let Ok(meta) = std::fs::metadata(&tab.path) else {
        return;
    };
    let Ok(disk_mtime) = meta.modified() else {
        return;
    };
    let stale = match tab.disk_mtime {
        Some(prev) => disk_mtime > prev,
        None => true,
    };
    if !stale {
        return;
    }
    // mtime advanced — compare bytes before alarming (some editors
    // rewrite without changing content).
    let Ok(disk_content) = std::fs::read_to_string(&tab.path) else {
        return;
    };
    if disk_content == tab.original_content {
        // Content matches our baseline: silently catch up the mtime.
        tab.disk_mtime = Some(disk_mtime);
        return;
    }
    tab.external_change = true;
}

/// Reload from disk, discarding any unsaved edits.
pub fn reload_tab(tab: &mut crate::state::layout::FileTab) {
    let Ok(disk_content) = std::fs::read_to_string(&tab.path) else {
        return;
    };
    tab.content = disk_content.clone();
    tab.original_content = disk_content;
    tab.line_changes_key = 0;
    tab.disk_mtime = std::fs::metadata(&tab.path)
        .and_then(|m| m.modified())
        .ok();
    tab.external_change = false;
}
