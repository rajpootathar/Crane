//! Format-on-save support for the Warp editor.
//!
//! Picks the language-appropriate external formatter for a file by its
//! extension, probes that the binary is actually on `PATH`, and runs it as a
//! subprocess: the buffer is fed on **stdin** and the formatted result is read
//! from **stdout**. No formatter is ever allowed to mutate the file on disk
//! itself — the caller reads stdout and applies it, so a formatter crash or a
//! non-zero exit can never corrupt or truncate the file.
//!
//! Mirrors the old egui `format::format_text`, but keyed off the file extension
//! (the Warp editor has no `ServerKey`) and hardened for large files: stdin is
//! written on a helper thread while stdout is drained, so a formatter that
//! streams output while we're still feeding it input can't deadlock the pipe.
//!
//! Formatters used (only when the binary exists on PATH — otherwise `None`):
//!   rust                                → `rustfmt --edition 2024 --emit stdout`
//!   js/jsx/ts/tsx/json/css/html/md/…    → `prettier --stdin-filepath <path>`
//!   py                                  → `ruff format -`  (fallback `black -q -`)
//!   go                                  → `gofmt`

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A resolved formatter invocation. The buffer is always piped on stdin and the
/// formatted text read from stdout; `program` + `args` is the full argv.
pub struct Formatter {
    program: &'static str,
    args: Vec<String>,
    /// Directory to run the subprocess in, so tools that resolve project config
    /// relative to CWD (prettier's `.prettierrc`, ruff's `pyproject.toml`) pick
    /// up the right rules. `None` only for a file with no parent (unreachable in
    /// practice — every open file has one).
    cwd: Option<PathBuf>,
}

/// Resolve the formatter for `path` by extension, or `None` when the extension
/// has no configured formatter **or** the required binary isn't on `PATH`.
///
/// Probing here (up front, on the caller's thread — a cheap `PATH` scan) rather
/// than at spawn time lets the caller skip the whole async round-trip and write
/// straight to disk when nothing would run.
pub fn for_path(path: &Path) -> Option<Formatter> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let cwd = path.parent().map(|p| p.to_path_buf());

    match ext.as_str() {
        "rs" if on_path("rustfmt") => Some(Formatter {
            program: "rustfmt",
            args: vec![
                "--edition".into(),
                "2024".into(),
                "--emit".into(),
                "stdout".into(),
            ],
            cwd,
        }),

        // Prettier's own supported set — feed the real path via --stdin-filepath
        // so it selects the right parser AND discovers the nearest config.
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc"
        | "css" | "scss" | "less" | "html" | "vue" | "svelte" | "md" | "markdown" | "mdx"
        | "yaml" | "yml"
            if on_path("prettier") =>
        {
            Some(Formatter {
                program: "prettier",
                args: vec![
                    "--stdin-filepath".into(),
                    path.to_string_lossy().into_owned(),
                ],
                cwd,
            })
        }

        // Python: prefer ruff (fast), fall back to black. Both format stdin→stdout.
        "py" | "pyi" => {
            if on_path("ruff") {
                Some(Formatter {
                    program: "ruff",
                    args: vec!["format".into(), "-".into()],
                    cwd,
                })
            } else if on_path("black") {
                Some(Formatter {
                    program: "black",
                    // -q silences black's "reformatted / left unchanged" note on
                    // stderr; `-` reads stdin and writes the result to stdout.
                    args: vec!["-q".into(), "-".into()],
                    cwd,
                })
            } else {
                None
            }
        }

        "go" if on_path("gofmt") => Some(Formatter {
            program: "gofmt",
            args: vec![],
            cwd,
        }),

        _ => None,
    }
}

/// Run the formatter over `input`, returning the formatted text on success or
/// `None` on ANY failure (binary vanished between probe and spawn, non-zero
/// exit, non-UTF-8 output, or a suspicious empty result for non-empty input).
/// A `None` return is the caller's signal to keep the original text — the file
/// is never corrupted by a broken formatter.
///
/// Intended to be called from inside a background task (e.g. `ctx.spawn`), never
/// on the UI thread: a formatter can take tens to hundreds of milliseconds.
/// Large files are safe — stdin is written on a helper thread while this thread
/// drains stdout, so a full stdout pipe can't deadlock against a blocked stdin
/// write (the latent bug in the old synchronous `write_all` path).
pub fn run(f: &Formatter, input: &str) -> Option<String> {
    let mut cmd = Command::new(f.program);
    cmd.args(&f.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = &f.cwd {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().ok()?;

    // Feed stdin from a helper thread so we can read stdout concurrently; a large
    // file whose formatted output fills the stdout pipe would otherwise wedge the
    // child (it blocks writing stdout) against us (we block writing stdin).
    let mut stdin = child.stdin.take()?;
    let payload = input.to_string();
    let writer = std::thread::spawn(move || {
        // Ignore write errors: a formatter that closes stdin early (e.g. rejects
        // the input) surfaces as a non-zero exit below, which we already handle.
        let _ = stdin.write_all(payload.as_bytes());
        // Explicit drop closes stdin so the child sees EOF and can terminate.
        drop(stdin);
    });

    let output = child.wait_with_output().ok()?;
    let _ = writer.join();

    if !output.status.success() {
        return None;
    }
    let formatted = String::from_utf8(output.stdout).ok()?;
    // Guard against a formatter that exits 0 but emits nothing for real content
    // — adopting that would silently blank the file.
    if formatted.is_empty() && !input.is_empty() {
        return None;
    }
    Some(formatted)
}

/// True when `bin` is an executable file on `PATH`. A cheap synchronous scan —
/// safe to call on the UI thread to decide whether formatting will run at all.
fn on_path(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(bin)))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}
