//! Hunk-level git staging helpers used by the Diff Pane
//! (`src/warpui/diff_view.rs`). Everything else that used to live here —
//! status, commit/push/pull, worktree management, branch listing, etc. —
//! has been superseded by `src/warpui/git.rs` and was removed as dead code.
//!
//! `parse_hunks` is kept despite having no production caller: it's pinned
//! by two regression tests below that guard a specific historical patch-
//! formatting bug, and deleting the function would mean deleting that
//! coverage.

use std::path::Path;
use std::process::Command;

/// Stage a single hunk by piping a unified-diff patch through
/// `git apply --cached`. The patch must be a valid hunk fragment
/// including its `@@ ... @@` header and trailing context.
pub fn stage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    apply_hunk(repo, patch, false)
}

/// Unstage a single hunk by piping the patch through
/// `git apply --reverse --cached`.
pub fn unstage_hunk(repo: &Path, patch: &str) -> Result<(), String> {
    apply_hunk(repo, patch, true)
}

/// Probe whether the hunk represented by `patch` (a HEAD→working-tree
/// hunk) is already present in the index. We test by asking git
/// whether the patch can be reverse-applied to the index: if it can,
/// the index already contains the new lines, so the hunk is staged.
pub fn is_hunk_staged(repo: &Path, patch: &str) -> bool {
    let mut child = match Command::new("git")
        .args([
            "apply",
            "--reverse",
            "--cached",
            "--check",
            "--unidiff-zero",
            "-",
        ])
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(patch.as_bytes());
    }
    child
        .wait_with_output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn apply_hunk(repo: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    // --unidiff-zero is required for the unified=0 patches that
    // file_diff_raw generates: git apply's default safety check
    // rejects context-less patches with "patch does not apply" even
    // when the line numbers are correct.
    let args = if reverse {
        vec!["apply", "--reverse", "--cached", "--unidiff-zero", "-"]
    } else {
        vec!["apply", "--cached", "--unidiff-zero", "-"]
    };
    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(patch.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

/// Get the unified diff between HEAD and the working tree for a file
/// — includes both staged and unstaged changes. Uses `--unified=0` so
/// every atomic change region becomes its own hunk; without that, git
/// merges changes within 3 lines of each other into a single hunk and
/// the diff view can't offer per-region stage actions (jetbrains-style
/// hunk split). git apply accepts unified=0 patches via line numbers,
/// so stage / unstage / is-staged probes all keep working.
pub fn file_diff_raw(repo: &Path, rel_path: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["diff", "--unified=0", "HEAD", "--", rel_path])
        .current_dir(repo)
        .output()
        .ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Parsed unified-diff hunk with line ranges. `new_start`/`new_count`
/// are taken from the `@@ -old,n +new,m @@` header and used by
/// callers to match each git hunk against an in-memory diff view's
/// hunk regions (line-number match, not array index — the two
/// diffing algorithms group adjacent changes differently).
pub struct ParsedHunk {
    pub patch: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let inner = line.strip_prefix("@@ ")?;
    let end = inner.find(" @@")?;
    let ranges = &inner[..end];
    let mut it = ranges.split(' ');
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let parse_range = |s: &str| -> Option<(usize, usize)> {
        let mut p = s.splitn(2, ',');
        let start: usize = p.next()?.parse().ok()?;
        let count: usize = match p.next() {
            Some(c) => c.parse().ok()?,
            None => 1,
        };
        Some((start, count))
    };
    let (os, oc) = parse_range(old)?;
    let (ns, nc) = parse_range(new)?;
    Some((os, oc, ns, nc))
}

/// Parse a unified diff into individual hunks with line ranges.
pub fn parse_hunks_detailed(diff: &str) -> Vec<ParsedHunk> {
    let mut out: Vec<ParsedHunk> = Vec::new();
    let header_end = diff.find('\n').unwrap_or(0);
    let header = if diff.starts_with("diff --git") {
        &diff[..header_end]
    } else {
        ""
    };
    let mut first_hunk = 0;
    for (i, line) in diff.lines().enumerate() {
        if line.starts_with("@@") {
            first_hunk = i;
            break;
        }
    }
    let prefix: &str = if !header.is_empty() {
        &diff[..diff
            .lines()
            .take(first_hunk)
            .map(|l| l.len() + 1)
            .sum::<usize>()
            .min(diff.len())]
    } else {
        ""
    };
    let lines: Vec<&str> = diff.lines().collect();
    let mut i = first_hunk;
    while i < lines.len() {
        if lines[i].starts_with("@@") {
            let start = i;
            let ranges = parse_hunk_header(lines[i]).unwrap_or((0, 0, 0, 0));
            i += 1;
            while i < lines.len() && !lines[i].starts_with("@@") {
                i += 1;
            }
            let hunk_content: String = lines[start..i].join("\n");
            let mut patch = if prefix.is_empty() {
                hunk_content
            } else {
                format!("{}{}", prefix, hunk_content)
            };
            if !patch.ends_with('\n') {
                patch.push('\n');
            }
            out.push(ParsedHunk {
                patch,
                old_start: ranges.0,
                old_count: ranges.1,
                new_start: ranges.2,
                new_count: ranges.3,
            });
        } else {
            i += 1;
        }
    }
    out
}

/// Parse a unified diff into individual hunk patches. Each patch
/// includes the `diff --git` header, the hunk header `@@ ... @@`,
/// and the content lines. Returns (hunk_index, patch_text) pairs.
///
/// Superseded in production by `parse_hunks_detailed`, which also
/// carries line ranges. Kept `#[allow(dead_code)]` because the two
/// tests below pin a specific historical patch-formatting bug fix
/// (a stray blank line before `@@` that made `git apply` reject the
/// patch); deleting the function would delete that regression coverage.
#[allow(dead_code)]
pub fn parse_hunks(diff: &str) -> Vec<(usize, String)> {
    let mut hunks = Vec::new();
    // Find the diff header line (first line starting with "diff --git")
    let header_end = diff.find('\n').unwrap_or(0);
    let header = if diff.starts_with("diff --git") {
        &diff[..header_end]
    } else {
        ""
    };
    // Find old/new mode lines or index lines between header and first hunk
    let mut first_hunk = 0;
    for (i, line) in diff.lines().enumerate() {
        if line.starts_with("@@") {
            first_hunk = i;
            break;
        }
    }
    let prefix: &str = if !header.is_empty() {
        // Include header + any index/mode lines before first hunk
        &diff[..diff
            .lines()
            .take(first_hunk)
            .map(|l| l.len() + 1)
            .sum::<usize>()
            .min(diff.len())]
    } else {
        ""
    };

    let lines: Vec<&str> = diff.lines().collect();
    let mut i = first_hunk;
    let mut hunk_idx = 0;
    while i < lines.len() {
        if lines[i].starts_with("@@") {
            // Collect this hunk's lines until next hunk or end
            let start = i;
            i += 1;
            while i < lines.len() && !lines[i].starts_with("@@") {
                i += 1;
            }
            let hunk_content: String = lines[start..i].join("\n");
            // `prefix` is a byte slice taken up to (but not including)
            // the first `@@` line — it ALREADY ends with the trailing
            // newline of the last header line. Adding another `\n`
            // between prefix and hunk_content inserts a blank line
            // between the header and the hunk, which git apply
            // rejects as "patch with only garbage at line 5".
            let mut patch = if prefix.is_empty() {
                hunk_content
            } else {
                format!("{}{}", prefix, hunk_content)
            };
            // git apply also wants a trailing newline; without it the
            // last hunk of a file silently fails to apply.
            if !patch.ends_with('\n') {
                patch.push('\n');
            }
            hunks.push((hunk_idx, patch));
            hunk_idx += 1;
        } else {
            i += 1;
        }
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hunks_produces_well_formed_patch() {
        let diff = concat!(
            "diff --git a/foo.rs b/foo.rs\n",
            "index 0000001..0000002 100644\n",
            "--- a/foo.rs\n",
            "+++ b/foo.rs\n",
            "@@ -1,3 +1,4 @@\n",
            " line1\n",
            "+added\n",
            " line2\n",
            " line3\n",
        );
        let hunks = parse_hunks(diff);
        assert_eq!(hunks.len(), 1);
        let patch = &hunks[0].1;
        // Header must be immediately followed by the @@ line — no
        // blank line in between. The earlier bug inserted "\n\n" at
        // the boundary, causing `git apply` to reject the patch with
        // "patch with only garbage at line 5".
        assert!(
            !patch.contains("\n\n@@"),
            "patch has a blank line before the @@ hunk header:\n{patch}"
        );
        assert!(patch.starts_with("diff --git"));
        assert!(patch.ends_with('\n'));
    }

    #[test]
    fn parse_hunks_appends_trailing_newline_when_missing() {
        let diff = concat!(
            "diff --git a/foo.rs b/foo.rs\n",
            "index 0000001..0000002 100644\n",
            "--- a/foo.rs\n",
            "+++ b/foo.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new",
        );
        let hunks = parse_hunks(diff);
        assert_eq!(hunks.len(), 1);
        assert!(
            hunks[0].1.ends_with('\n'),
            "patch must end with a newline for git apply to accept it"
        );
    }
}
