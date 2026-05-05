use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct RefEntry {
    pub name: String,
    pub sha: String,
    pub upstream: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefSet {
    pub local: Vec<RefEntry>,
    pub remote: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    pub worktrees: Vec<WorktreeEntry>,
    pub head: Option<String>,
}

const FIELD_SEP: char = '\x1f';

pub fn parse_for_each_ref(stdout: &str) -> RefSet {
    let mut set = RefSet::default();
    for line in stdout.split('\n') {
        if line.is_empty() { continue; }
        let mut fields = line.split(FIELD_SEP);
        let (Some(refname), Some(objectname), Some(upstream)) =
            (fields.next(), fields.next(), fields.next())
            else { continue; };
        let upstream = if upstream.is_empty() { None } else { Some(upstream.to_string()) };
        let entry = RefEntry {
            name: refname.to_string(),
            sha: objectname.to_string(),
            upstream,
        };
        if refname.starts_with("refs/heads/") {
            set.local.push(entry);
        } else if refname.starts_with("refs/remotes/") {
            set.remote.push(entry);
        } else if refname.starts_with("refs/tags/") {
            set.tags.push(entry);
        }
    }
    set
}

pub fn parse_worktree_porcelain(stdout: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    for line in stdout.split('\n') {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let (Some(p), Some(b)) = (cur_path.take(), cur_branch.take()) {
                out.push(WorktreeEntry { path: p, branch: b });
            }
            cur_path = Some(PathBuf::from(rest));
            cur_branch = Some("detached".to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "bare" {
            cur_branch = Some("(bare)".to_string());
        } else if line == "detached" {
            cur_branch = Some("detached".to_string());
        }
    }
    if let (Some(p), Some(b)) = (cur_path, cur_branch) {
        out.push(WorktreeEntry { path: p, branch: b });
    }
    out
}

pub fn load_refs(repo: &Path) -> RefSet {
    let format = format!("--format=%(refname){us}%(objectname){us}%(upstream)", us = '\x1f');
    let out = match Command::new("git")
        .args(["for-each-ref", &format, "refs/heads", "refs/remotes", "refs/tags"])
        .current_dir(repo)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return RefSet::default(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut set = parse_for_each_ref(&stdout);

    if let Ok(o) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
    {
        if o.status.success() {
            let head = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !head.is_empty() {
                set.head = Some(head);
            }
        }
    }

    if let Ok(o) = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
    {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            set.worktrees = parse_worktree_porcelain(&stdout);
        }
    }

    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_line(name: &str, sha: &str, upstream: &str) -> String {
        format!("{name}\x1f{sha}\x1f{upstream}\n")
    }

    #[test]
    fn parses_local_remote_tag_buckets() {
        let stdout = format!(
            "{}{}{}",
            ref_line("refs/heads/main", "aaa", "refs/remotes/origin/main"),
            ref_line("refs/remotes/origin/main", "aaa", ""),
            ref_line("refs/tags/v1.0", "bbb", ""),
        );
        let set = parse_for_each_ref(&stdout);
        assert_eq!(set.local.len(), 1);
        assert_eq!(set.remote.len(), 1);
        assert_eq!(set.tags.len(), 1);
        assert_eq!(set.local[0].name, "refs/heads/main");
        assert_eq!(set.local[0].upstream.as_deref(), Some("refs/remotes/origin/main"));
        assert!(set.tags[0].upstream.is_none());
    }

    #[test]
    fn worktree_branched_then_detached_then_bare() {
        let stdout = "\
worktree /a/main
branch refs/heads/main

worktree /a/feat
branch refs/heads/feat/x

worktree /a/det
HEAD abc
detached

worktree /a/bare
bare
";
        let parsed = parse_worktree_porcelain(stdout);
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].branch, "main");
        assert_eq!(parsed[1].branch, "feat/x");
        assert_eq!(parsed[2].branch, "detached");
        assert_eq!(parsed[3].branch, "(bare)");
    }

    #[test]
    fn empty_input_yields_empty_set() {
        assert_eq!(parse_for_each_ref(""), RefSet::default());
        assert!(parse_worktree_porcelain("").is_empty());
    }

    #[test]
    fn ignores_malformed_ref_lines() {
        let stdout = "refs/heads/main\x1faaa\x1f\nshort_line\nrefs/tags/v1\x1fbbb\x1f\n";
        let set = parse_for_each_ref(stdout);
        assert_eq!(set.local.len(), 1);
        assert_eq!(set.tags.len(), 1);
    }
}
