//! Identity of one File Tab in the Files Pane.
//!
//! A Files Pane's tab strip holds two kinds of tab: an editable document and a
//! read-only diff. They share one strip (see
//! `docs/superpowers/specs/2026-07-25-diff-tabs-in-files-pane-design.md`), so
//! they need one identity type — and that identity is what stops a second
//! click on the same changed file from appending a duplicate tab.
//!
//! The distinction that matters: a tab's identity is NOT its path. A file tab
//! and a diff tab of the same file are different tabs, and two diffs of one
//! file across different commit ranges are different tabs again. Everything
//! filesystem-facing (delete sweeps, buffer refcounts) must therefore key off
//! [`TabKey::path`] rather than the key itself — keying those off the whole
//! key would leave a live diff tab open over a file that no longer exists.

use std::path::{Path, PathBuf};

/// Which two trees a diff tab compares.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiffSpec {
    /// `HEAD` vs the working tree — what the Right Panel's Changes rows open.
    WorkingTree,
    /// One commit against its first parent — what the Git Log's changed-files
    /// list opens. Stored as the full SHA; the label shortens it.
    Commit(String),
}

impl DiffSpec {
    /// Short label for the tab strip (`HEAD–WT`, or the commit's short SHA).
    pub fn label(&self) -> String {
        match self {
            DiffSpec::WorkingTree => "HEAD–WT".to_string(),
            DiffSpec::Commit(sha) => sha.chars().take(8).collect(),
        }
    }
}

/// One tab in the Files Pane's strip.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TabKey {
    /// An editable document (editor / markdown / image / pdf — the route is
    /// decided by extension elsewhere; from the strip's view it's one tab).
    File(PathBuf),
    /// A read-only diff of `path` over `spec`.
    Diff { path: PathBuf, spec: DiffSpec },
}

impl TabKey {
    /// The on-disk path both variants refer to. Every filesystem-facing sweep
    /// keys off THIS, never off the `TabKey` itself.
    pub fn path(&self) -> &Path {
        match self {
            TabKey::File(p) => p,
            TabKey::Diff { path, .. } => path,
        }
    }

    /// True for the editable-document variant.
    pub fn is_file(&self) -> bool {
        matches!(self, TabKey::File(_))
    }

    /// The same tab pointed at a new path, keeping its variant and spec — how
    /// a rename retargets an open tab. A renamed file's DIFF tab has to follow
    /// the file too, or the tab silently points at a path that no longer
    /// exists.
    pub fn with_path(self, path: PathBuf) -> Self {
        match self {
            TabKey::File(_) => TabKey::File(path),
            TabKey::Diff { spec, .. } => TabKey::Diff { path, spec },
        }
    }

    /// The file name shown on the tab (diff tabs add their spec separately).
    pub fn file_name(&self) -> String {
        self.path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path().display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// The dedup rule the tab strip relies on: re-clicking the same changed
    /// file must FOCUS its tab, so equal keys must compare equal.
    #[test]
    fn same_path_and_spec_is_one_tab() {
        let a = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::WorkingTree };
        let b = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::WorkingTree };
        assert_eq!(a, b);
        let tabs = vec![TabKey::File(p("src/x.rs")), a.clone()];
        assert_eq!(tabs.iter().position(|t| *t == b), Some(1));
    }

    /// A file tab and a diff tab of ONE path coexist — the strip shows both.
    #[test]
    fn file_and_diff_of_one_path_are_distinct_tabs() {
        let f = TabKey::File(p("src/a.rs"));
        let d = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::WorkingTree };
        assert_ne!(f, d);
        // …but they name the same file, which is what the delete sweep uses.
        assert_eq!(f.path(), d.path());
    }

    /// Two ranges of one file are two tabs (the Phase-2 case the Git Log hits).
    #[test]
    fn different_specs_of_one_path_are_distinct_tabs() {
        let wt = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::WorkingTree };
        let c1 = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::Commit("abc123".into()) };
        let c2 = TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::Commit("def456".into()) };
        assert_ne!(wt, c1);
        assert_ne!(c1, c2);
        assert_eq!(c1, TabKey::Diff { path: p("src/a.rs"), spec: DiffSpec::Commit("abc123".into()) });
    }

    /// Delete sweeps match `starts_with` on the PATH, so a diff tab under a
    /// deleted directory is swept exactly like a file tab under it.
    #[test]
    fn path_drives_directory_sweeps_for_both_variants() {
        let tabs = vec![
            TabKey::File(p("src/deep/a.rs")),
            TabKey::Diff { path: p("src/deep/b.rs"), spec: DiffSpec::WorkingTree },
            TabKey::File(p("other/c.rs")),
        ];
        let doomed: Vec<usize> = tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.path().starts_with(p("src/deep")))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(doomed, vec![0, 1]);
    }

    #[test]
    fn labels_are_short_and_stable() {
        assert_eq!(DiffSpec::WorkingTree.label(), "HEAD–WT");
        assert_eq!(DiffSpec::Commit("0a0d16f0abcdef".into()).label(), "0a0d16f0");
        assert_eq!(
            TabKey::Diff { path: p("src/a/b.rs"), spec: DiffSpec::WorkingTree }.file_name(),
            "b.rs"
        );
    }
}
