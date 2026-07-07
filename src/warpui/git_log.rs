//! Git-log graph model for the warpui Git Log pane — the framework-agnostic
//! core that shells out to `git`, parses commits + refs, and computes the
//! classic railroad lane graph. No warpui / egui types leak across this
//! boundary: the shell (`shell.rs`) owns the rendering and maps [`RefKind`] /
//! lane color slots onto concrete theme colors. Ported 1:1 from old Crane's
//! `src/git_log/{data,graph,refs}.rs`, collapsed into one module.
//!
//! Everything here is a pure `git` subprocess + in-memory transform, so the
//! shell runs [`load`] and [`load_detail`] off the UI thread via `ctx.spawn`
//! (background executor) — nothing here blocks the frame.

use std::path::Path;
use std::process::Command;

pub type Sha = String;

// ── Commit records (old data.rs) ──────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    pub sha: Sha,
    pub parents: Vec<Sha>,
    pub author: String,
    /// ISO-8601 commit date (parsed on demand — avoids chrono in the hot path).
    pub date: String,
    /// Relative age ("3 days ago") from `%ar`, for the muted meta column.
    pub relative: String,
    pub subject: String,
    /// Raw `%D` decoration string, e.g. ` (HEAD -> main, origin/main, tag: v1.0)`.
    pub refs_decoration: String,
}

const FIELD_SEP: char = '\x1f';
const RECORD_SEP: char = '\n';

/// Parse `%H<US>%P<US>%an<US>%aI<US>%ar<US>%s<US>%D<LF>` records. Malformed
/// lines (too few fields) are skipped cleanly rather than corrupting the list.
pub fn parse_log_output(stdout: &str) -> Vec<CommitRecord> {
    let mut out = Vec::new();
    for line in stdout.split(RECORD_SEP) {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(FIELD_SEP);
        let (
            Some(sha),
            Some(parents),
            Some(author),
            Some(date),
            Some(relative),
            Some(subject),
            Some(refs),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            continue;
        };
        let parents: Vec<Sha> = if parents.is_empty() {
            Vec::new()
        } else {
            parents.split(' ').map(String::from).collect()
        };
        out.push(CommitRecord {
            sha: sha.to_string(),
            parents,
            author: author.to_string(),
            date: date.to_string(),
            relative: relative.to_string(),
            subject: subject.to_string(),
            refs_decoration: refs.to_string(),
        });
    }
    out
}

/// Run `git log --all --date-order` against `repo` and parse the records.
/// `max_count` caps the walk (pass a large value for the initial load).
/// Empty Vec on any error.
pub fn load_commits(repo: &Path, max_count: usize) -> Vec<CommitRecord> {
    let format = format!(
        "--pretty=format:%H{us}%P{us}%an{us}%aI{us}%ar{us}%s{us}%D",
        us = FIELD_SEP
    );
    let max_count_arg = format!("--max-count={max_count}");
    let out = match Command::new("git")
        .args(["log", "--all", "--date-order", &format, &max_count_arg])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    parse_log_output(&String::from_utf8_lossy(&out.stdout))
}

// ── Refs (old refs.rs, trimmed to what the pills need) ─────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct RefEntry {
    /// Fully-qualified ref name, e.g. `refs/heads/main`.
    pub name: String,
    pub sha: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefSet {
    pub local: Vec<RefEntry>,
    pub remote: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    /// Current HEAD SHA (for the HEAD pill), if resolvable.
    pub head: Option<String>,
}

pub fn parse_for_each_ref(stdout: &str) -> RefSet {
    let mut set = RefSet::default();
    for line in stdout.split('\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split(FIELD_SEP);
        let (Some(refname), Some(objectname)) = (fields.next(), fields.next()) else {
            continue;
        };
        let entry = RefEntry {
            name: refname.to_string(),
            sha: objectname.to_string(),
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

pub fn load_refs(repo: &Path) -> RefSet {
    let format = format!("--format=%(refname){us}%(objectname)", us = FIELD_SEP);
    let out = match Command::new("git")
        .args(["for-each-ref", &format, "refs/heads", "refs/remotes", "refs/tags"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return RefSet::default(),
    };
    let mut set = parse_for_each_ref(&String::from_utf8_lossy(&out.stdout));

    if let Ok(o) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        if o.status.success() {
            let head = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !head.is_empty() {
                set.head = Some(head);
            }
        }
    }
    set
}

// ── Ref pills (old view/log.rs::parse_ref_pills, framework-free) ───────────

/// Category of a decoration ref — the shell maps this to a pill color so the
/// core stays free of any UI-toolkit types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// `HEAD` / `HEAD -> branch` — the current checkout.
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
    /// Categorization couldn't place it (neither a known local nor remote ref).
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefPill {
    pub label: String,
    pub kind: RefKind,
}

/// Split a `%D` decoration (` (HEAD -> main, origin/main, tag: v1.0)`) into
/// categorised pills. Categorisation uses the real [`RefSet`] rather than
/// slash-counting: a local branch may legitimately contain slashes
/// (`feat/foo`), which the old `contains('/')` heuristic misclassified.
pub fn parse_ref_pills(decoration: &str, refs: &RefSet) -> Vec<RefPill> {
    let body = decoration
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    if body.is_empty() {
        return Vec::new();
    }
    let local_names: std::collections::HashSet<&str> = refs
        .local
        .iter()
        .filter_map(|r| r.name.strip_prefix("refs/heads/"))
        .collect();
    let remote_names: std::collections::HashSet<&str> = refs
        .remote
        .iter()
        .filter_map(|r| r.name.strip_prefix("refs/remotes/"))
        .collect();

    let mut out = Vec::new();
    for raw in body.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let pill = if let Some(rest) = raw.strip_prefix("HEAD -> ") {
            RefPill {
                label: format!("HEAD -> {rest}"),
                kind: RefKind::Head,
            }
        } else if raw == "HEAD" {
            RefPill {
                label: "HEAD".to_string(),
                kind: RefKind::Head,
            }
        } else if let Some(t) = raw.strip_prefix("tag: ") {
            RefPill {
                label: t.to_string(),
                kind: RefKind::Tag,
            }
        } else if local_names.contains(raw) {
            RefPill {
                label: raw.to_string(),
                kind: RefKind::LocalBranch,
            }
        } else if remote_names.contains(raw) {
            RefPill {
                label: raw.to_string(),
                kind: RefKind::RemoteBranch,
            }
        } else {
            RefPill {
                label: raw.to_string(),
                kind: RefKind::Unknown,
            }
        };
        out.push(pill);
    }
    out
}

// ── Lane graph (old graph.rs) ─────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    pub sha: Sha,
    pub own_lane: u8,
    /// Lanes the parents occupy. First entry is `own_lane` for the first parent
    /// (linear continuation) — except root commits, where this is empty.
    pub parent_lanes: Vec<u8>,
    /// Lanes active before this row's draw that don't continue past it
    /// (closing branches) — the painter draws lane caps for these.
    pub terminating_lanes: Vec<u8>,
    /// Lanes that pass STRAIGHT THROUGH this row (active before AND after, but
    /// not this commit's own lane). Each pair is `(lane_index, color_slot)`.
    pub passthrough_lanes: Vec<(u8, u8)>,
    /// Color slot (0..8) — approximates "color per branch".
    pub color: u8,
    /// How many lanes remain active after this row.
    pub visible_lanes_after: u8,
}

#[derive(Clone, Debug, Default)]
pub struct LaneFrame {
    pub rows: Vec<LaneRow>,
    pub max_lane: u8,
}

/// Stable color picker keyed on `(lane_index, allocation_epoch)`. Each fresh
/// claim of a lane bumps its epoch; same `(lane, epoch)` → same color.
struct ColorSeeder {
    epochs: Vec<u32>,
}

impl ColorSeeder {
    fn new() -> Self {
        Self { epochs: Vec::new() }
    }
    fn allocate(&mut self, lane: usize) -> u8 {
        while self.epochs.len() <= lane {
            self.epochs.push(0);
        }
        self.epochs[lane] += 1;
        let h = (lane as u32).wrapping_mul(7919) ^ self.epochs[lane].wrapping_mul(31337);
        (h % 8) as u8
    }
    fn current(&self, lane: usize) -> u8 {
        let e = *self.epochs.get(lane).unwrap_or(&1);
        let h = (lane as u32).wrapping_mul(7919) ^ e.wrapping_mul(31337);
        (h % 8) as u8
    }
}

/// Build a [`LaneFrame`] from commits in display order (newest first). Walks
/// newest → oldest tracking lane ownership; each commit either finds its SHA
/// already claimed by a processed child, or allocates a fresh lane.
pub fn layout(commits: &[CommitRecord]) -> LaneFrame {
    if commits.is_empty() {
        return LaneFrame::default();
    }

    let mut active_lanes: Vec<Option<Sha>> = Vec::new();
    let mut seeder = ColorSeeder::new();
    let mut rows: Vec<LaneRow> = Vec::with_capacity(commits.len());

    for c in commits.iter() {
        let lanes_before = active_lanes.clone();

        // 1. Find the lane waiting for this commit (or allocate a new one).
        let own_lane = match active_lanes.iter().position(|l| l.as_ref() == Some(&c.sha)) {
            Some(idx) => idx,
            None => {
                let slot = active_lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(active_lanes.len());
                if slot == active_lanes.len() {
                    active_lanes.push(None);
                }
                seeder.allocate(slot);
                slot
            }
        };

        // 2. First parent claims the same lane (linear continuation), UNLESS it
        //    is already pending in another lane — then terminate our lane and
        //    merge into the existing one.
        let mut parent_lanes: Vec<u8> = Vec::new();
        if let Some(p0) = c.parents.first() {
            let already_tracked = active_lanes
                .iter()
                .enumerate()
                .find(|(i, l)| *i != own_lane && l.as_ref() == Some(p0))
                .map(|(i, _)| i);
            if let Some(other) = already_tracked {
                active_lanes[own_lane] = None;
                parent_lanes.push(other as u8);
            } else {
                active_lanes[own_lane] = Some(p0.clone());
                parent_lanes.push(own_lane as u8);
            }
        } else {
            active_lanes[own_lane] = None; // root commit
        }

        // 3. Subsequent parents → branch off into new lanes, OR merge into an
        //    existing lane already pending for that parent.
        for p in c.parents.iter().skip(1) {
            let already_tracked = active_lanes
                .iter()
                .enumerate()
                .find(|(_, l)| l.as_ref() == Some(p))
                .map(|(i, _)| i);
            if let Some(other) = already_tracked {
                parent_lanes.push(other as u8);
                continue;
            }
            let slot = active_lanes
                .iter()
                .position(Option::is_none)
                .unwrap_or(active_lanes.len());
            if slot == active_lanes.len() {
                active_lanes.push(None);
            }
            active_lanes[slot] = Some(p.clone());
            seeder.allocate(slot);
            parent_lanes.push(slot as u8);
        }

        // 4. Compact trailing frees so visual width stays minimal.
        while matches!(active_lanes.last(), Some(None)) {
            active_lanes.pop();
        }

        let terminating_lanes: Vec<u8> = lanes_before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let still_alive = i < active_lanes.len() && active_lanes[i].is_some();
                if l.is_some() && !still_alive && i != own_lane {
                    Some(i as u8)
                } else {
                    None
                }
            })
            .collect();

        let passthrough_lanes: Vec<(u8, u8)> = lanes_before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let alive_after = i < active_lanes.len() && active_lanes[i].is_some();
                let alive_before = l.is_some();
                if !(alive_before && alive_after) {
                    return None;
                }
                if i as u8 == own_lane as u8 {
                    return None;
                }
                Some((i as u8, seeder.current(i)))
            })
            .collect();

        let color = seeder.current(own_lane);

        rows.push(LaneRow {
            sha: c.sha.clone(),
            own_lane: own_lane as u8,
            parent_lanes,
            terminating_lanes,
            passthrough_lanes,
            color,
            visible_lanes_after: active_lanes.len() as u8,
        });
    }

    let max_lane = rows
        .iter()
        .map(|r| r.visible_lanes_after)
        .max()
        .unwrap_or(1);
    LaneFrame { rows, max_lane }
}

// ── Loaded snapshot ───────────────────────────────────────────────────────

/// One consistent load of the graph — commits + refs + lane geometry. The
/// shell caches this behind an `Rc` and only reloads when the repo's refs
/// change. `Send` (plain data) so `ctx.spawn` can build it on a background
/// thread.
#[derive(Clone, Debug)]
pub struct GraphFrame {
    pub commits: Vec<CommitRecord>,
    pub refs: RefSet,
    pub lanes: LaneFrame,
}

/// Cap on the initial `git log` walk — a huge repo can't blow up the model.
pub const MAX_COMMITS: usize = 5_000;

/// Load the full graph for `repo`. Blocking (subprocess) — call off the UI
/// thread. Returns an empty frame on any error / non-repo.
pub fn load(repo: &Path) -> GraphFrame {
    let commits = load_commits(repo, MAX_COMMITS);
    let refs = load_refs(repo);
    let lanes = layout(&commits);
    GraphFrame {
        commits,
        refs,
        lanes,
    }
}

// ── Commit detail (`git show`) ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `+` added line.
    Add,
    /// `-` removed line.
    Del,
    /// `@@ … @@` hunk header.
    Hunk,
    /// `diff --git` / `index` / `--- ` / `+++ ` / `new file` etc. — file meta.
    FileHeader,
    /// Unchanged context line.
    Context,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// The rendered detail for one selected commit: the header/message block
/// (everything before the first `diff --git`) plus the classified patch body.
#[derive(Clone, Debug, Default)]
pub struct CommitDetail {
    pub header: Vec<String>,
    pub diff: Vec<DiffLine>,
}

/// Classify one raw patch line by its leading character(s).
fn classify(line: &str) -> DiffLineKind {
    if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("new file")
        || line.starts_with("deleted file")
        || line.starts_with("similarity ")
        || line.starts_with("rename ")
        || line.starts_with("old mode")
        || line.starts_with("new mode")
        || line.starts_with("Binary files")
    {
        DiffLineKind::FileHeader
    } else if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with('+') {
        DiffLineKind::Add
    } else if line.starts_with('-') {
        DiffLineKind::Del
    } else {
        DiffLineKind::Context
    }
}

/// `git show --no-color <sha>` split into the message header (before the first
/// `diff --git`) and the classified patch body. Blocking — call off-thread.
/// Returns an empty detail on any error.
pub fn load_detail(repo: &Path, sha: &str) -> CommitDetail {
    let out = match Command::new("git")
        .args(["show", "--no-color", "--stat=0", sha])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return CommitDetail::default(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut header = Vec::new();
    let mut diff = Vec::new();
    let mut in_diff = false;
    for line in text.lines() {
        if !in_diff && line.starts_with("diff --git") {
            in_diff = true;
        }
        if in_diff {
            diff.push(DiffLine {
                kind: classify(line),
                text: line.to_string(),
            });
        } else {
            header.push(line.to_string());
        }
    }
    CommitDetail { header, diff }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(sha: &str, parents: &[&str]) -> CommitRecord {
        CommitRecord {
            sha: sha.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "A".to_string(),
            date: "2026-05-01T10:00:00+00:00".to_string(),
            relative: "1 day ago".to_string(),
            subject: "S".to_string(),
            refs_decoration: String::new(),
        }
    }

    fn line(sha: &str, parents: &str, subject: &str, refs: &str) -> String {
        format!("{sha}\x1f{parents}\x1fAlice\x1f2026-05-01T10:00:00+00:00\x1f1 day ago\x1f{subject}\x1f{refs}")
    }

    #[test]
    fn parses_single_commit_no_parents() {
        let parsed = parse_log_output(&line("abc", "", "Initial", ""));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sha, "abc");
        assert!(parsed[0].parents.is_empty());
        assert_eq!(parsed[0].relative, "1 day ago");
    }

    #[test]
    fn parses_two_parent_merge() {
        let parsed = parse_log_output(&line("m1", "p1 p2", "Merge", ""));
        assert_eq!(parsed[0].parents, vec!["p1".to_string(), "p2".to_string()]);
    }

    #[test]
    fn subjects_with_pipe_chars_dont_corrupt() {
        let parsed = parse_log_output(&line("abc", "", "fix: a | b | c", ""));
        assert_eq!(parsed[0].subject, "fix: a | b | c");
    }

    #[test]
    fn straight_line_no_merges() {
        let commits = vec![
            cr("c3", &["c2"]),
            cr("c2", &["c1"]),
            cr("c1", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);
        assert_eq!(frame.rows.len(), 4);
        for r in &frame.rows {
            assert_eq!(r.own_lane, 0, "row {} not on lane 0", r.sha);
        }
    }

    #[test]
    fn fork_and_merge_two_branches() {
        let commits = vec![
            cr("m", &["c2", "b1"]),
            cr("c2", &["c1"]),
            cr("b1", &["c1"]),
            cr("c1", &[]),
        ];
        let frame = layout(&commits);
        let m_row = frame.rows.iter().find(|r| r.sha == "m").unwrap();
        assert_eq!(m_row.parent_lanes.len(), 2);
    }

    #[test]
    fn merged_branches_do_not_leave_dangling_lane() {
        let commits = vec![
            cr("m", &["c2", "b1"]),
            cr("c2", &["c1"]),
            cr("b1", &["c1"]),
            cr("c1", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);
        let c1_row = frame.rows.iter().find(|r| r.sha == "c1").unwrap();
        assert_eq!(c1_row.visible_lanes_after, 1);
        let root_row = frame.rows.iter().find(|r| r.sha == "root").unwrap();
        assert!(root_row.passthrough_lanes.is_empty());
    }

    #[test]
    fn ref_pills_categorise_head_local_remote_tag() {
        let refs = RefSet {
            local: vec![RefEntry {
                name: "refs/heads/main".into(),
                sha: "a".into(),
            }],
            remote: vec![RefEntry {
                name: "refs/remotes/origin/main".into(),
                sha: "a".into(),
            }],
            tags: vec![],
            head: None,
        };
        let pills = parse_ref_pills(" (HEAD -> main, origin/main, tag: v1.0)", &refs);
        assert_eq!(pills[0].kind, RefKind::Head);
        assert_eq!(pills[1].kind, RefKind::RemoteBranch);
        assert_eq!(pills[2].kind, RefKind::Tag);
    }

    #[test]
    fn detail_classify_splits_patch() {
        assert_eq!(classify("diff --git a/x b/x"), DiffLineKind::FileHeader);
        assert_eq!(classify("@@ -1,2 +1,3 @@"), DiffLineKind::Hunk);
        assert_eq!(classify("+added"), DiffLineKind::Add);
        assert_eq!(classify("-removed"), DiffLineKind::Del);
        assert_eq!(classify(" context"), DiffLineKind::Context);
    }
}
