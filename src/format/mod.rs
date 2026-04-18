//! Project-local formatting conventions discovered from `.prettierrc*` or
//! the `"prettier"` field of `package.json`. Walks up from the edited
//! file's directory so a monorepo with different rules in `dashboard/`
//! vs. `admin/` just works.
//!
//! Only the keys the editor honors live-while-typing are read here:
//! `tabWidth` and `useTabs`. Full Prettier semantics (printWidth,
//! bracketSameLine, trailingComma, …) need actually running Prettier on
//! save, which is a separate story.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct FormatStyle {
    pub tab_width: usize,
    pub use_tabs: bool,
    /// Absolute path of the config file we read (None → defaults).
    pub source: Option<PathBuf>,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            tab_width: 2,
            use_tabs: false,
            source: None,
        }
    }
}

impl FormatStyle {
    /// The string one press of Tab (or one indent level) should insert.
    pub fn indent_unit(&self) -> String {
        if self.use_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_width)
        }
    }
}

const FILE_NAMES: &[&str] = &[
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yaml",
    ".prettierrc.yml",
];

/// Walks up from `file`'s parent directory to find the nearest Prettier
/// config. `.prettierrc` JSON and `package.json`'s `prettier` field are
/// parsed; YAML configs are recognized but fall back to defaults (we
/// avoid pulling serde_yaml in for one feature).
pub fn discover(file: &Path) -> FormatStyle {
    let mut cur: PathBuf = file.parent().unwrap_or(file).to_path_buf();
    loop {
        for name in FILE_NAMES {
            let candidate = cur.join(name);
            if candidate.is_file()
                && let Some(mut s) = parse_rc(&candidate)
            {
                s.source = Some(candidate);
                return s;
            }
        }
        let pkg = cur.join("package.json");
        if pkg.is_file()
            && let Some(mut s) = parse_pkg_field(&pkg)
        {
            s.source = Some(pkg);
            return s;
        }
        if !cur.pop() {
            break;
        }
    }
    FormatStyle::default()
}

/// Run the language-appropriate formatter over `content`, piping through
/// stdin and reading stdout. Returns None if the formatter isn't on PATH
/// or exited non-zero, so callers can fall back to the original text.
///
/// Formatters used:
///   TypeScript / CssLs / HtmlLs  → prettier (uses `--stdin-filepath`)
///   RustAnalyzer                 → rustfmt  (`--emit stdout`)
///   Pyright                      → ruff format -
///   Gopls                        → gofmt
pub fn format_text(
    key: crate::lsp::ServerKey,
    path: &Path,
    content: &str,
) -> Option<String> {
    use std::io::Write;
    use std::process::Stdio;

    let path_str = path.to_str()?;
    let (cmd, args): (&str, Vec<String>) = match key {
        crate::lsp::ServerKey::TypeScript
        | crate::lsp::ServerKey::CssLs
        | crate::lsp::ServerKey::HtmlLs => (
            "prettier",
            vec!["--stdin-filepath".into(), path_str.into()],
        ),
        crate::lsp::ServerKey::RustAnalyzer => {
            ("rustfmt", vec!["--emit".into(), "stdout".into()])
        }
        crate::lsp::ServerKey::Pyright => ("ruff", vec!["format".into(), "-".into()]),
        crate::lsp::ServerKey::Gopls => ("gofmt", vec![]),
        // Eslint's stdin-fix output is JSON-wrapped; parsing it correctly
        // across eslint versions is brittle. Prettier (triggered by the
        // TypeScript key's toggle) already formats the file, and ESLint
        // diagnostics still surface via its LSP. So we skip ESLint here.
        crate::lsp::ServerKey::Eslint => return None,
    };
    let mut child = std::process::Command::new(cmd)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.as_mut()?.write_all(content.as_bytes()).ok()?;
    drop(child.stdin.take());
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn parse_rc(path: &Path) -> Option<FormatStyle> {
    let bytes = std::fs::read(path).ok()?;
    let val = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    Some(style_from_json(&val))
}

fn parse_pkg_field(path: &Path) -> Option<FormatStyle> {
    let bytes = std::fs::read(path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let prettier = v.get("prettier")?;
    Some(style_from_json(prettier))
}

/// Convert a character index (egui's CCursor index) into a byte offset
/// into `s`. Returns `s.len()` for out-of-range indices so callers can
/// safely append.
pub fn char_idx_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Leading-whitespace + should-bump-one-level decision for a new line
/// inserted at `cursor_byte`. Matches the common behaviour: copy the
/// indent of the line the cursor is on, and add one extra level if the
/// last non-whitespace token opens a block (`{`, `(`, `[`, `=>`).
pub fn auto_indent_context(text: &str, cursor_byte: usize) -> (String, bool) {
    let before = &text[..cursor_byte];
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let current_line_before = &text[line_start..cursor_byte];
    let prev_indent: String = current_line_before
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let trimmed = current_line_before.trim_end();
    let bump = trimmed.ends_with('{')
        || trimmed.ends_with('(')
        || trimmed.ends_with('[')
        || trimmed.ends_with("=>");
    (prev_indent, bump)
}

fn style_from_json(v: &serde_json::Value) -> FormatStyle {
    let tab_width = v
        .get("tabWidth")
        .and_then(|x| x.as_u64())
        .unwrap_or(2) as usize;
    let use_tabs = v
        .get("useTabs")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    FormatStyle {
        tab_width: tab_width.clamp(1, 16),
        use_tabs,
        source: None,
    }
}
