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
            IMAGE_EXTS.contains(&e.as_str())
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
    if let Some(home) = crate::util::home_dir()
        && let Ok(rel) = Path::new(path).strip_prefix(&home)
    {
        return format!("~{}", rel.to_string_lossy());
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

/// Platform-appropriate label for the "reveal file" context-menu item.
pub fn reveal_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Reveal in Finder"
    }
    #[cfg(target_os = "linux")]
    {
        "Reveal in Files"
    }
    #[cfg(target_os = "windows")]
    {
        "Reveal in Explorer"
    }
}

/// Return the line-comment prefix for the file at `path`, based on its
/// extension. Falls back to `"//"` when the language isn't recognised.
pub fn comment_prefix(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "go" | "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs"
        | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "hh" | "hxx"
        | "java" | "kt" | "kts" | "swift" | "dart" | "scala"
        | "zig" | "glsl" | "hlsl" | "wgsl" | "proto" => "//",
        "py" | "pyi" | "sh" | "bash" | "zsh" | "fish" | "yaml" | "yml"
        | "toml" | "rb" | "rake" | "pl" | "r" | "ps1" | "lua"
        | "conf" | "cfg" | "ini" | "env" | "dockerfile" => "#",
        "sql" => "--",
        "hs" | "lhs" => "-- ",
        "ex" | "exs" => "#",
        "clj" | "cljs" => ";; ",
        _ => "//",
    }
}

/// Toggle line comments on all lines intersecting the byte range
/// `[sel_start..sel_end]` (char indices) in `content`. Adds the
/// prefix if any line in the range is uncommented, removes it if all
/// are commented.
pub fn toggle_line_comments(
    content: &mut String,
    sel_start: usize,
    sel_end: usize,
    prefix: &str,
) {
    let start_byte = crate::format::char_idx_to_byte(content, sel_start);
    let end_byte = crate::format::char_idx_to_byte(content, sel_end);
    let bytes = content.as_bytes();

    // Find first and last line boundaries
    let first_line_start = bytes[..start_byte]
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let last_line_end = bytes[end_byte..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|i| end_byte + i)
        .unwrap_or(content.len());

    // Collect (content_start, line_start) for each line
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut pos = first_line_start;
    while pos <= last_line_end {
        let line_end = bytes[pos..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| pos + i)
            .unwrap_or(content.len());
        let trimmed = bytes[pos..line_end]
            .iter()
            .position(|b| *b != b' ' && *b != b'\t')
            .unwrap_or(line_end - pos);
        let content_start = pos + trimmed;
        lines.push((content_start, pos));
        if line_end >= content.len() { break; }
        pos = line_end + 1;
    }

    let all_commented = lines
        .iter()
        .all(|&(cs, _)| content[cs..].starts_with(prefix));

    if all_commented {
        for &(cs, _) in lines.iter().rev() {
            if content[cs..].starts_with(prefix) {
                content.replace_range(cs..cs + prefix.len(), "");
            }
        }
    } else {
        for &(cs, _) in lines.iter().rev() {
            content.insert_str(cs, prefix);
        }
    }
}
