//! Small helpers shared by the Files Pane — path munging, extension
//! classification, cursor index arithmetic, OS reveal. Kept out of
//! `file_view.rs` so that module can focus on render composition.

use std::path::Path;

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico"];

pub fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            IMAGE_EXTS.iter().any(|x| *x == e.as_str())
        })
        .unwrap_or(false)
}

/// Display form of `path` — workspace-relative if we have a root, else
/// tilde-ified home, else the absolute path. Used above each file tab.
pub fn short_path(path: &str, workspace_root: Option<&Path>) -> String {
    if let Some(root) = workspace_root
        && let Ok(rel) = Path::new(path).strip_prefix(root)
    {
        return rel.to_string_lossy().to_string();
    }
    if let Ok(home) = std::env::var("HOME")
        && let Some(stripped) = path.strip_prefix(&home)
    {
        return format!("~{stripped}");
    }
    path.to_string()
}

/// Convert an LSP (line, col) pair to an egui `CCursor` char index.
pub fn line_col_to_char(text: &str, line: u32, col: u32) -> usize {
    let mut cur_line = 0u32;
    let mut cur_col = 0u32;
    let mut char_idx = 0usize;
    for ch in text.chars() {
        if cur_line == line && cur_col == col {
            return char_idx;
        }
        char_idx += 1;
        if ch == '\n' {
            cur_line += 1;
            cur_col = 0;
        } else {
            cur_col += 1;
        }
    }
    char_idx
}

/// Reverse of `line_col_to_char` — where does this char index sit in
/// 0-indexed line/column coordinates?
pub fn char_idx_to_line_col(text: &str, char_idx: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut col = 0u32;
    let mut idx = 0usize;
    for ch in text.chars() {
        if idx == char_idx {
            return (line, col);
        }
        idx += 1;
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Strip trailing spaces/tabs from every line in `text` (leaves newlines
/// and final EOF handling alone). Used by the editor's
/// "trim trailing whitespace on save" pref.
pub fn trim_trailing_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(line.trim_end_matches([' ', '\t']));
    }
    out
}

/// Open the OS file-manager at a given path, with the file selected
/// where the OS supports that verb.
pub fn reveal_in_file_manager(path: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "linux")]
    {
        let parent = Path::new(path).parent().unwrap_or_else(|| Path::new("/"));
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn();
}
