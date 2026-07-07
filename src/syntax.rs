//! Standalone syntax/highlighting + editor string helpers, extracted from
//! the (soon-to-be-removed) egui `src/views/` layer so the warpui frontend
//! no longer depends on it. Pure syntect (SyntaxSet/ThemeSet loaders) plus
//! pure string ops for comment toggling / trailing-whitespace trimming — no
//! egui in this module.

use std::path::Path;
use std::sync::OnceLock;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

pub fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(|| {
        // two-face ships ~250 Sublime-grade syntaxes: TypeScript, TSX, JSX,
        // Dockerfile, Astro, Svelte, GraphQL, Prisma, Nix, Zig, etc. — a
        // big step up from syntect's bundled set for modern dev work.
        let mut builder = two_face::syntax::extra_newlines().into_builder();
        // User-dropped packages still fold in on top.
        if let Ok(home) = std::env::var("HOME") {
            let dir = std::path::PathBuf::from(format!("{home}/.crane/syntaxes"));
            if dir.is_dir() {
                let _ = builder.add_from_folder(&dir, true);
            }
        }
        builder.build()
    })
}

/// Guaranteed-present fallback used when the user's requested theme
/// (and every named fallback) is missing and the ThemeSet happens to
/// be empty — e.g. if a future two_face version drops an embedded
/// theme or a user strips themes via config. Returning this instead
/// of panicking keeps the editor usable with default (uncolored)
/// syntax output.
pub fn fallback_theme() -> &'static syntect::highlighting::Theme {
    static FALLBACK: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    FALLBACK.get_or_init(syntect::highlighting::Theme::default)
}

pub fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(|| {
        let mut set = ThemeSet::load_defaults();
        let extras = two_face::theme::extra();
        for name in two_face::theme::EmbeddedLazyThemeSet::theme_names() {
            let key = format!("{name:?}"); // enum Debug prints the variant name, e.g. "VisualStudioDarkPlus"
            set.themes.insert(key, extras.get(*name).clone());
        }
        set
    })
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
