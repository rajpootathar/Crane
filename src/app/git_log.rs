//! Git-log graph model for the warpui Git Log pane — the framework-agnostic
//! core that shells out to `git`, parses commits + refs, and computes the
//! classic railroad lane graph. No warpui / egui types leak across this
//! boundary: the shell (`shell.rs`) owns the rendering and maps [`RefKind`] /
//! lane color slots onto concrete theme colors. Ported 1:1 from old Crane's
//! `src/git_log/{data,graph,refs}.rs`, collapsed into one module.
//!
//! Everything here is a pure `git` subprocess + in-memory transform, so the
//! shell runs [`load_graph_for`] and [`load_detail`] off the UI thread via
//! `ctx.spawn` (background executor) — nothing here blocks the frame. The
//! in-memory transforms ([`apply_filters`] / [`filtered_frame`],
//! [`ref_groups`], [`step_selection`]) are pure and cheap enough to run on
//! the UI thread at interaction time.

use std::path::Path;
use std::process::Command;

pub type Sha = String;

// ── Commit records (old data.rs) ──────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct CommitRecord {
    pub sha: Sha,
    pub parents: Vec<Sha>,
    pub author: String,
    /// Author email (`%ae`) — the attribution line renders it beside the name.
    pub email: String,
    /// ISO-8601 commit date (parsed on demand — avoids chrono in the hot path).
    pub date: String,
    /// Pre-formatted local date for the log's Date COLUMN
    /// (`31/07/2026, 12:07 pm`). Formatted by `git` itself via `--date=format:`
    /// rather than parsed here: no chrono, no hand-rolled calendar maths, and
    /// the user's own locale/timezone rules apply.
    pub display_date: String,
    /// Relative age ("3 days ago") from `%ar`.
    pub relative: String,
    /// Author timestamp, Unix seconds (`%at`) — what the Date filter compares.
    /// A number rather than the ISO string because comparing ISO text across
    /// mixed timezone offsets silently misorders commits.
    pub timestamp: i64,
    pub subject: String,
    /// Raw `%D` decoration string, e.g. ` (HEAD -> main, origin/main, tag: v1.0)`.
    pub refs_decoration: String,
}

const FIELD_SEP: char = '\x1f';
const RECORD_SEP: char = '\n';

/// Parse `%H<US>%P<US>%an<US>%ae<US>%aI<US>%ad<US>%ar<US>%at<US>%s<US>%D<LF>`
/// records. Malformed lines (too few fields) are skipped cleanly rather than
/// corrupting the list.
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
            Some(email),
            Some(date),
            Some(display_date),
            Some(relative),
            Some(timestamp),
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
            email: email.to_string(),
            date: date.to_string(),
            display_date: display_date.to_string(),
            relative: relative.to_string(),
            // A record whose timestamp won't parse still belongs in the list;
            // it just sorts as epoch and never matches a "since" filter.
            timestamp: timestamp.parse().unwrap_or(0),
            subject: subject.to_string(),
            refs_decoration: refs.to_string(),
        });
    }
    out
}

/// Run `git log --date-order` against `repo` and parse the records, with an
/// optional ref scope. `Some("main")` walks only the commits reachable from
/// that ref (`git log <ref>`) — the refs-column branch / tag filter, matching
/// old Crane's `FilterState::branch` semantics — while `None` keeps the full
/// `--all` walk. `max_count` caps the walk (pass a large value for the
/// initial load). Empty Vec on any error, including a ref name git can't
/// resolve.
pub fn load_commits_for(
    repo: &Path,
    max_count: usize,
    ref_filter: Option<&str>,
) -> Vec<CommitRecord> {
    let format = format!(
        "--pretty=format:%H{us}%P{us}%an{us}%ae{us}%aI{us}%ad{us}%ar{us}%at{us}%s{us}%D",
        us = FIELD_SEP
    );
    // `%ad` renders through this; `%aI` stays ISO regardless. Day/month order
    // and the 12-hour clock match the reference JetBrains layout.
    const DATE_FORMAT: &str = "--date=format:%d/%m/%Y, %I:%M %p";
    let max_count_arg = format!("--max-count={max_count}");
    let mut args: Vec<&str> = vec!["log", DATE_FORMAT];
    match ref_filter {
        Some(r) => args.push(r),
        None => args.push("--all"),
    }
    args.extend(["--date-order", &format, &max_count_arg]);
    if ref_filter.is_some() {
        // `--` terminates the revision list so a ref named like a path
        // (`docs`, `src`) still reads as a revision, never a pathspec.
        args.push("--");
    }
    let out = match Command::new("git")
        .args(&args)
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

// ── Refs column listing (old view/refs.rs, framework-free) ─────────────────

/// One display-ready row for the refs column: prefix-stripped name, tip SHA
/// (clicking becomes the ref filter / scroll target), and a HEAD marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefItem {
    /// Prefix-stripped display name (`main`, `origin/main`, `v1.0`).
    pub display: String,
    /// Tip SHA the ref points at.
    pub sha: String,
    /// True when this ref's tip IS the current HEAD (the asterisk row in the
    /// old refs column).
    pub is_head: bool,
}

/// One LOCAL / REMOTE / TAGS section of the refs column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefGroup {
    pub title: &'static str,
    pub items: Vec<RefItem>,
}

/// Group a [`RefSet`] into the LOCAL / REMOTE / TAGS sections the refs column
/// renders (old `view/refs.rs::ref_section`): fully-qualified names strip to
/// display names, rows sort case-insensitively inside each group, and empty
/// groups drop out so the column never paints a bare header.
pub fn ref_groups(refs: &RefSet) -> Vec<RefGroup> {
    let section = |title: &'static str, entries: &[RefEntry], prefix: &str| -> Option<RefGroup> {
        if entries.is_empty() {
            return None;
        }
        let mut items: Vec<RefItem> = entries
            .iter()
            .map(|e| RefItem {
                display: e
                    .name
                    .strip_prefix(prefix)
                    .unwrap_or(e.name.as_str())
                    .to_string(),
                sha: e.sha.clone(),
                is_head: refs.head.as_deref() == Some(e.sha.as_str()),
            })
            .collect();
        items.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
        Some(RefGroup { title, items })
    };
    [
        section("LOCAL", &refs.local, "refs/heads/"),
        section("REMOTE", &refs.remote, "refs/remotes/"),
        section("TAGS", &refs.tags, "refs/tags/"),
    ]
    .into_iter()
    .flatten()
    .collect()
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
/// 10 000, matching old Crane's `GitLogState::reload` walk depth.
pub const MAX_COMMITS: usize = 10_000;

/// Load the full graph for `repo`, with an optional ref scope: `Some("main")`
/// loads the graph from only the commits reachable from that ref (`git log
/// <ref>` — the refs-column branch/tag filter), `None` is the full `--all`
/// walk. Refs always load in full so the pills and the refs column stay
/// complete while the commit list is narrowed. Blocking (subprocess) — call
/// off the UI thread. Returns an empty frame on any error / non-repo.
pub fn load_graph_for(repo: &Path, ref_filter: Option<&str>) -> GraphFrame {
    let commits = load_commits_for(repo, MAX_COMMITS, ref_filter);
    let refs = load_refs(repo);
    let lanes = layout(&commits);
    GraphFrame {
        commits,
        refs,
        lanes,
    }
}

// ── Filters (old view/log.rs filter bar, framework-free) ──────────────────

/// Distinct author names across `commits`, case-insensitively deduped and
/// sorted — the User filter's menu. Built from the LOADED graph rather than a
/// separate `git shortlog`, so the menu always matches what's on screen.
pub fn distinct_authors(commits: &[CommitRecord]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for c in commits {
        if c.author.is_empty() {
            continue;
        }
        if !seen.iter().any(|a| a.eq_ignore_ascii_case(&c.author)) {
            seen.push(c.author.clone());
        }
    }
    seen.sort_by_key(|a| a.to_lowercase());
    seen
}

/// The Date filter's presets. Relative windows rather than a calendar picker:
/// "what landed this week" is the question the log actually gets asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFilter {
    All,
    Last24h,
    Last7Days,
    Last30Days,
}

impl DateFilter {
    pub fn label(self) -> &'static str {
        match self {
            DateFilter::All => "Date",
            DateFilter::Last24h => "Last 24 hours",
            DateFilter::Last7Days => "Last 7 days",
            DateFilter::Last30Days => "Last 30 days",
        }
    }

    /// Every preset, in menu order.
    pub fn all() -> [DateFilter; 4] {
        [
            DateFilter::All,
            DateFilter::Last24h,
            DateFilter::Last7Days,
            DateFilter::Last30Days,
        ]
    }

    /// Oldest Unix timestamp this filter admits, given `now`. `None` = no bound.
    fn since(self, now: i64) -> Option<i64> {
        let day = 86_400;
        match self {
            DateFilter::All => None,
            DateFilter::Last24h => Some(now - day),
            DateFilter::Last7Days => Some(now - 7 * day),
            DateFilter::Last30Days => Some(now - 30 * day),
        }
    }

    fn admits(self, timestamp: i64, now: i64) -> bool {
        self.since(now).is_none_or(|since| timestamp >= since)
    }
}

/// Current wall clock in Unix seconds — the reference point for [`DateFilter`].
/// Separate from the filter so the filter itself stays pure and testable.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Every commit-list filter at once: free text, an exact author, and a date
/// window. Filters AND together — narrowing by user and by date shows the
/// commits matching both, which is what the equivalent JetBrains dropdowns do.
pub fn apply_filters(
    commits: &[CommitRecord],
    needle: &str,
    author: Option<&str>,
    date: DateFilter,
    now: i64,
) -> Vec<CommitRecord> {
    let needle = needle.trim().to_lowercase();
    commits
        .iter()
        .filter(|c| {
            if !needle.is_empty() {
                let hay = format!("{} {} {}", c.subject, c.sha, c.author).to_lowercase();
                if !hay.contains(&needle) {
                    return false;
                }
            }
            // Author match is case-insensitive and whole-name: the menu is
            // built from these same strings, so a substring rule would let
            // "Alice" also select "Alice Smith".
            if let Some(a) = author {
                if !c.author.eq_ignore_ascii_case(a) {
                    return false;
                }
            }
            date.admits(c.timestamp, now)
        })
        .cloned()
        .collect()
}

/// Apply the text filter to a loaded frame, RE-RUNNING lane layout on just
/// the surviving commits — old behavior: lanes reflect what's visible, so
/// filtered-out branches don't linger as passthrough rails. Refs carry over
/// unchanged so ref pills stay categorised. Cheap for the shell to cache
/// keyed on (needle, frame generation).
pub fn filtered_frame(
    frame: &GraphFrame,
    needle: &str,
    author: Option<&str>,
    date: DateFilter,
    now: i64,
) -> GraphFrame {
    let commits = apply_filters(&frame.commits, needle, author, date, now);
    let lanes = layout(&commits);
    GraphFrame {
        commits,
        refs: frame.refs.clone(),
        lanes,
    }
}

// ── Keyboard navigation (old view/log.rs arrow / j / k nav) ────────────────

/// Step the selection one row through `commits` (display order, newest
/// first): `down` moves toward older commits. `None` selection — or a
/// selected SHA that fell out of the (possibly filtered) list — lands on row
/// 0; steps clamp at both ends (old behavior). `None` only on an empty list.
pub fn step_selection(
    commits: &[CommitRecord],
    selected: Option<&str>,
    down: bool,
) -> Option<Sha> {
    if commits.is_empty() {
        return None;
    }
    let cur = selected.and_then(|sha| commits.iter().position(|c| c.sha == sha));
    let next = match cur {
        Some(idx) if down => (idx + 1).min(commits.len() - 1),
        Some(idx) => idx.saturating_sub(1),
        None => 0,
    };
    Some(commits[next].sha.clone())
}

/// Scroll offset (in rows) that keeps `row` inside a viewport of
/// `visible_rows`, moving the current offset as little as possible. The shell
/// writes this back to the shared scroll cell after a keyboard step so the
/// selection never walks off-screen.
pub fn reveal_offset(scroll: f32, row: usize, visible_rows: usize) -> f32 {
    let visible = visible_rows.max(1);
    let row = row as f32;
    if row < scroll.floor() {
        row
    } else if row >= scroll + visible as f32 {
        row - (visible as f32 - 1.0)
    } else {
        scroll
    }
}

// ── Commit detail (`git show`) ────────────────────────────────────────────

/// Line classes the per-file walk distinguishes. Private: the detail model
/// keeps only per-file COUNTS, not the patch text — the actual diff is
/// rendered by a Diff Pane straight out of git's object store, so holding a
/// second copy of every patch line here bought nothing but memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffLineKind {
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

/// The detail for one selected commit: the message block (everything before
/// the first `diff --git`) and a summary row per changed file. The patch text
/// itself is deliberately NOT kept — clicking a file opens a Diff Pane that
/// reads both sides from git directly, so storing a parsed copy of a 70-file
/// patch here would be pure overhead.
#[derive(Clone, Debug, Default)]
pub struct CommitDetail {
    pub header: Vec<String>,
    /// One entry per changed file (JetBrains-style file list).
    pub files: Vec<CommitFileDiff>,
    /// Branches (local + remote-tracking) whose tip contains this commit — the
    /// "In N branches: …" footer under the message.
    pub branches: Vec<String>,
}

/// One changed file's summary within a commit.
#[derive(Clone, Debug)]
pub struct CommitFileDiff {
    /// New-side path (`b/…` of the `diff --git` header; the rename target).
    pub path: String,
    /// Porcelain-style status letter — `A`dded, `D`eleted, `R`enamed, `M`odified.
    /// Read off the file's own header lines, so it needs no second `git` call
    /// (old Crane shelled out to `git show --name-status` for this column).
    /// Drives the row color in the changed-files list.
    pub status: char,
    pub added: usize,
    pub deleted: usize,
}

impl Default for CommitFileDiff {
    fn default() -> Self {
        Self {
            path: String::new(),
            status: 'M',
            added: 0,
            deleted: 0,
        }
    }
}

/// New-side path out of a `diff --git a/<old> b/<new>` header line.
fn diff_git_new_path(line: &str) -> String {
    line.rsplit_once(" b/")
        .map(|(_, b)| b.trim_matches('"').to_string())
        .unwrap_or_else(|| line.to_string())
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
        // No stat flag at all. `--stat=0` (what this used to pass) emits a
        // diffstat block BETWEEN the message and the patch, which lands in
        // `header` and renders as a second, uglier copy of the changed-files
        // list under the message. Plain `git show` emits no diffstat, and
        // `--no-stat` is NOT a valid `git show` argument — passing it makes
        // git exit 128 and the whole detail come back empty.
        .args(["show", "--no-color", sha])
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return CommitDetail::default(),
    };
    let mut detail = parse_detail(&String::from_utf8_lossy(&out.stdout));
    detail.branches = crate::app::git::branches_containing(repo, sha);
    detail
}

/// Split raw `git show` output into the message header (everything before the
/// first `diff --git`) and the classified, per-file patch body. Pure — the
/// subprocess lives in [`load_detail`], so this is directly testable.
pub fn parse_detail(text: &str) -> CommitDetail {
    let mut header = Vec::new();
    let mut files: Vec<CommitFileDiff> = Vec::new();
    let mut in_diff = false;
    for line in text.lines() {
        if !in_diff && line.starts_with("diff --git") {
            in_diff = true;
        }
        if !in_diff {
            header.push(line.to_string());
            continue;
        }
        // Per-file split: each `diff --git` starts a new file section.
        if line.starts_with("diff --git") {
            files.push(CommitFileDiff {
                path: diff_git_new_path(line),
                ..Default::default()
            });
        }
        let Some(f) = files.last_mut() else { continue };
        match classify(line) {
            // `+++ ` / `--- ` classify as FileHeader, so the counts here only
            // ever see real patch lines.
            DiffLineKind::Add => f.added += 1,
            DiffLineKind::Del => f.deleted += 1,
            _ => {}
        }
        // Status letter from the header lines that follow `diff --git`.
        if line.starts_with("new file") {
            f.status = 'A';
        } else if line.starts_with("deleted file") {
            f.status = 'D';
        } else if line.starts_with("rename ") {
            f.status = 'R';
        }
    }
    // Branches are a separate `git branch --contains` call — see `load_detail`,
    // which fills them in. Parsing alone can't know them.
    CommitDetail { header, files, branches: Vec::new() }
}

// ── Changed-files tree ────────────────────────────────────────────────────

/// One rendered row of the commit's changed-files tree.
#[derive(Clone, Debug, PartialEq)]
pub enum FileTreeRow {
    /// A directory node. `key` is its full path (the collapse-state key),
    /// `label` the segment(s) shown — a chain of single-child directories
    /// collapses into one row (`crates/crane_term/src`) the way JetBrains
    /// renders it, instead of three rows of one child each.
    Dir { key: String, label: String, depth: usize, files: usize },
    /// A changed file. `index` points back into [`CommitDetail::files`].
    File { index: usize, label: String, depth: usize },
}

/// Build the nested changed-files tree for `files`, collapsing single-child
/// directory chains and hiding the subtree of any directory whose key is in
/// `collapsed`.
///
/// Directory order follows each directory's FIRST appearance in the patch, so
/// the tree reads in the same order as the diff rather than alphabetically.
pub fn file_tree_rows(
    files: &[CommitFileDiff],
    collapsed: &std::collections::HashSet<String>,
) -> Vec<FileTreeRow> {
    // Insertion-ordered trie over path segments.
    #[derive(Default)]
    struct Node {
        dirs: Vec<(String, Node)>,
        files: Vec<(usize, String)>,
    }
    impl Node {
        fn child(&mut self, seg: &str) -> &mut Node {
            if let Some(i) = self.dirs.iter().position(|(s, _)| s == seg) {
                return &mut self.dirs[i].1;
            }
            self.dirs.push((seg.to_string(), Node::default()));
            &mut self.dirs.last_mut().unwrap().1
        }
        fn count(&self) -> usize {
            self.files.len() + self.dirs.iter().map(|(_, n)| n.count()).sum::<usize>()
        }
    }

    let mut root = Node::default();
    for (i, f) in files.iter().enumerate() {
        let mut segs: Vec<&str> = f.path.split('/').filter(|s| !s.is_empty()).collect();
        let name = segs.pop().unwrap_or("").to_string();
        let mut cur = &mut root;
        for s in segs {
            cur = cur.child(s);
        }
        cur.files.push((i, name));
    }

    fn walk(
        node: &Node,
        prefix: &str,
        depth: usize,
        collapsed: &std::collections::HashSet<String>,
        out: &mut Vec<FileTreeRow>,
    ) {
        for (seg, child) in &node.dirs {
            // Collapse a chain of directories that each hold exactly one
            // directory and no files: `a` → `a/b` → `a/b/c` becomes one row.
            let mut label = seg.clone();
            let mut key = if prefix.is_empty() {
                seg.clone()
            } else {
                format!("{prefix}/{seg}")
            };
            let mut cur = child;
            while cur.files.is_empty() && cur.dirs.len() == 1 {
                let (s, next) = &cur.dirs[0];
                label = format!("{label}/{s}");
                key = format!("{key}/{s}");
                cur = next;
            }
            out.push(FileTreeRow::Dir {
                key: key.clone(),
                label,
                depth,
                files: cur.count(),
            });
            if !collapsed.contains(&key) {
                walk(cur, &key, depth + 1, collapsed, out);
            }
        }
        for (i, name) in &node.files {
            out.push(FileTreeRow::File { index: *i, label: name.clone(), depth });
        }
    }

    let mut out = Vec::new();
    walk(&root, "", 0, collapsed, &mut out);
    out
}

/// The commit MESSAGE out of [`CommitDetail::header`], de-indented.
///
/// `git show`'s header is a contiguous run of `commit …` / `Author:` / `Date:`
/// / `Merge:` lines, then a blank, then the message indented four spaces.
/// Dropping that leading run (rather than filtering blank lines anywhere)
/// is what preserves the message's own paragraph breaks — a filter would run
/// every paragraph together. Trailing blanks are `git show`'s separator before
/// the patch, not message content, so they come off too.
pub fn message_body(header: &[String]) -> Vec<String> {
    let is_meta = |l: &String| {
        l.starts_with("commit ")
            || l.starts_with("Author:")
            || l.starts_with("AuthorDate:")
            || l.starts_with("Commit:")
            || l.starts_with("CommitDate:")
            || l.starts_with("Date:")
            || l.starts_with("Merge:")
    };
    let body: Vec<String> = header
        .iter()
        .skip_while(|l| is_meta(l))
        .skip_while(|l| l.trim().is_empty())
        .map(|l| l.strip_prefix("    ").unwrap_or(l).trim_end().to_string())
        .collect();
    let end = body
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    body[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real repo with one commit that adds, one that modifies + adds, so
    /// `load_detail` can be exercised against actual `git` output.
    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "Tester")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "Tester")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "--initial-branch=main", "."]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "first: add a.txt"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(p.join("b.txt"), "new file\n").unwrap();
        git(&["add", "."]);
        git(&[
            "commit",
            "-m",
            "second: subject line\n\nBody paragraph one.\n\nBody paragraph two.",
        ]);
        dir
    }

    /// End-to-end against a real `git` process, for the same reason as
    /// `load_detail`'s test below: a `--pretty` / `--date` argument git
    /// rejects makes the command exit non-zero and the commit list come back
    /// EMPTY, and no amount of `parse_log_output` testing can see that. The
    /// 10-field format string and `--date=format:` live or die here.
    #[test]
    fn load_commits_reads_all_ten_fields_from_a_real_repo() {
        let repo = temp_repo();
        let commits = load_commits_for(repo.path(), 100, None);
        assert_eq!(commits.len(), 2, "both commits must load");

        let head = &commits[0];
        assert!(head.subject.starts_with("second: subject line"));
        assert_eq!(head.author, "Tester");
        assert_eq!(head.email, "t@example.com", "%ae must land in `email`");
        assert!(head.date.contains('T'), "%aI stays ISO: {}", head.date);
        // `%ad` under `--date=format:%d/%m/%Y, %I:%M %p`.
        assert!(
            head.display_date.contains('/') && head.display_date.contains(':'),
            "display_date must be the formatted local date, got {:?}",
            head.display_date
        );
        assert_ne!(
            head.display_date, head.date,
            "display_date must not just be the ISO string"
        );
        assert!(head.timestamp > 1_000_000_000, "%at must parse: {}", head.timestamp);
        assert!(!head.relative.is_empty(), "%ar must be present");
        // The tip has exactly one parent; the root has none.
        assert_eq!(head.parents.len(), 1);
        assert!(commits[1].parents.is_empty());

        // And those fields feed the filters that depend on them.
        let now = now_unix();
        assert_eq!(
            apply_filters(&commits, "", Some("Tester"), DateFilter::Last24h, now).len(),
            2,
            "freshly created commits must fall inside the 24h window"
        );
        assert_eq!(distinct_authors(&commits), vec!["Tester"]);
    }

    /// End-to-end against a real `git` process. `parse_detail` tests can't
    /// catch a bad ARGUMENT list — this one does: a rejected flag makes git
    /// exit non-zero and the whole detail come back empty, which is exactly
    /// how `--no-stat` (not a valid `git show` argument) shipped an empty
    /// changed-files list.
    #[test]
    fn load_detail_reads_files_and_message_from_a_real_repo() {
        let repo = temp_repo();
        let detail = load_detail(repo.path(), "HEAD");

        // The changed-files list is the thing that silently emptied.
        let mut paths: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "b.txt"], "detail.files must not be empty");

        let a = detail.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(a.status, 'M');
        assert_eq!((a.added, a.deleted), (1, 0));
        let b = detail.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!(b.status, 'A');
        assert_eq!((b.added, b.deleted), (1, 0));

        // Message survives with its paragraph breaks, and no diffstat or
        // `commit`/`Author:`/`Date:` meta leaks into it.
        let body = message_body(&detail.header);
        assert_eq!(
            body,
            vec![
                "second: subject line",
                "",
                "Body paragraph one.",
                "",
                "Body paragraph two.",
            ]
        );
        assert!(
            !body.iter().any(|l| l.contains("a.txt") || l.contains("|")),
            "diffstat leaked into the message: {body:?}"
        );

        // HEAD is on main, so the footer has something to say.
        assert!(
            detail.branches.iter().any(|b| b == "main"),
            "branches: {:?}",
            detail.branches
        );
    }

    /// The root commit has no `^`, and its files are all adds.
    #[test]
    fn load_detail_handles_the_root_commit() {
        let repo = temp_repo();
        let detail = load_detail(repo.path(), "HEAD~1");
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].path, "a.txt");
        assert_eq!(detail.files[0].status, 'A');
        assert_eq!(detail.files[0].added, 2);
    }

    fn dated(sha: &str, author: &str, ts: i64) -> CommitRecord {
        CommitRecord { author: author.to_string(), timestamp: ts, ..cr(sha, &[]) }
    }

    #[test]
    fn distinct_authors_dedupes_case_insensitively_and_sorts() {
        let commits = vec![
            dated("a", "Bob", 0),
            dated("b", "alice", 0),
            dated("c", "Alice", 0),
            dated("d", "", 0),
        ];
        // "Alice" folds into the first-seen "alice"; blanks drop out.
        assert_eq!(distinct_authors(&commits), vec!["alice", "Bob"]);
    }

    #[test]
    fn date_filter_windows_are_inclusive_at_the_boundary() {
        let now = 1_000_000i64;
        let day = 86_400;
        let commits = vec![
            dated("today", "A", now - 60),
            dated("edge", "A", now - day), // exactly 24h old
            dated("week", "A", now - 3 * day),
            dated("old", "A", now - 60 * day),
        ];
        let ids = |d: DateFilter| -> Vec<String> {
            apply_filters(&commits, "", None, d, now).into_iter().map(|c| c.sha).collect()
        };
        assert_eq!(ids(DateFilter::All).len(), 4);
        assert_eq!(ids(DateFilter::Last24h), vec!["today", "edge"]);
        assert_eq!(ids(DateFilter::Last7Days), vec!["today", "edge", "week"]);
        assert_eq!(ids(DateFilter::Last30Days).len(), 3);
    }

    #[test]
    fn filters_and_together_and_author_matches_whole_name() {
        let now = 1_000_000i64;
        let commits = vec![
            CommitRecord { subject: "fix: lane".into(), ..dated("a", "Alice", now) },
            CommitRecord { subject: "fix: refs".into(), ..dated("b", "Alice Smith", now) },
            CommitRecord {
                subject: "fix: lane".into(),
                ..dated("c", "Bob", now - 40 * 86_400)
            },
        ];
        // Author is a WHOLE-name match: "Alice" must not also select
        // "Alice Smith", or the menu entry means something different from
        // what it selects.
        let by_author = apply_filters(&commits, "", Some("Alice"), DateFilter::All, now);
        assert_eq!(by_author.len(), 1);
        assert_eq!(by_author[0].sha, "a");
        // Case-insensitive all the same.
        assert_eq!(
            apply_filters(&commits, "", Some("aLiCe"), DateFilter::All, now).len(),
            1
        );
        // Text AND author AND date.
        assert_eq!(
            apply_filters(&commits, "lane", Some("Alice"), DateFilter::Last24h, now).len(),
            1
        );
        // Bob's "lane" commit is outside the 24h window → nothing.
        assert!(
            apply_filters(&commits, "lane", Some("Bob"), DateFilter::Last24h, now).is_empty()
        );
    }

    fn fd(path: &str) -> CommitFileDiff {
        CommitFileDiff { path: path.to_string(), ..Default::default() }
    }

    #[test]
    fn file_tree_collapses_single_child_directory_chains() {
        let files = vec![
            fd("crates/crane_term/src/term.rs"),
            fd("crates/crane_term/src/grid.rs"),
            fd("README.md"),
        ];
        let rows = file_tree_rows(&files, &std::collections::HashSet::new());
        assert_eq!(
            rows,
            vec![
                // Three directories with one child each collapse to ONE row.
                FileTreeRow::Dir {
                    key: "crates/crane_term/src".into(),
                    label: "crates/crane_term/src".into(),
                    depth: 0,
                    files: 2,
                },
                FileTreeRow::File { index: 0, label: "term.rs".into(), depth: 1 },
                FileTreeRow::File { index: 1, label: "grid.rs".into(), depth: 1 },
                // Root-level files sit at depth 0 with no directory row.
                FileTreeRow::File { index: 2, label: "README.md".into(), depth: 0 },
            ]
        );
    }

    #[test]
    fn file_tree_branches_where_directories_actually_diverge() {
        let files = vec![fd("src/app/a.rs"), fd("src/app/view/b.rs"), fd("src/main.rs")];
        let rows = file_tree_rows(&files, &std::collections::HashSet::new());
        // `src` holds both a directory and a file, so it can't collapse into
        // `src/app`; `src/app/view` has one child and needs no further split.
        assert_eq!(
            rows,
            vec![
                FileTreeRow::Dir { key: "src".into(), label: "src".into(), depth: 0, files: 3 },
                FileTreeRow::Dir {
                    key: "src/app".into(),
                    label: "app".into(),
                    depth: 1,
                    files: 2,
                },
                FileTreeRow::Dir {
                    key: "src/app/view".into(),
                    label: "view".into(),
                    depth: 2,
                    files: 1,
                },
                FileTreeRow::File { index: 1, label: "b.rs".into(), depth: 3 },
                FileTreeRow::File { index: 0, label: "a.rs".into(), depth: 2 },
                FileTreeRow::File { index: 2, label: "main.rs".into(), depth: 1 },
            ]
        );
    }

    #[test]
    fn collapsing_a_directory_hides_its_subtree_only() {
        let files = vec![fd("src/a.rs"), fd("docs/b.md")];
        let mut collapsed = std::collections::HashSet::new();
        collapsed.insert("src".to_string());
        let rows = file_tree_rows(&files, &collapsed);
        assert_eq!(
            rows,
            vec![
                FileTreeRow::Dir { key: "src".into(), label: "src".into(), depth: 0, files: 1 },
                // src/a.rs hidden…
                FileTreeRow::Dir { key: "docs".into(), label: "docs".into(), depth: 0, files: 1 },
                // …but docs is untouched.
                FileTreeRow::File { index: 1, label: "b.md".into(), depth: 1 },
            ]
        );
    }

    #[test]
    fn message_body_drops_meta_and_keeps_paragraph_breaks() {
        let header: Vec<String> = "\
commit deadbeefcafe
Author: Alice <a@example.com>
Date:   Wed Jul 29 23:16:11 2026 +0500

    fix: subject line

    First paragraph explaining the fix.

    Second paragraph after a blank.
"
        .lines()
        .map(String::from)
        .collect();
        let body = message_body(&header);
        assert_eq!(
            body,
            vec![
                "fix: subject line",
                "",
                "First paragraph explaining the fix.",
                "",
                "Second paragraph after a blank.",
            ]
        );
    }

    #[test]
    fn message_body_handles_merge_header_and_trailing_blanks() {
        // A merge commit adds a `Merge:` line, and `git show` leaves a blank
        // line between the message and the patch — neither is message content.
        let header: Vec<String> = vec![
            "commit abc".into(),
            "Merge: 111 222".into(),
            "Author: Bob <b@example.com>".into(),
            "Date:   Wed Jul 29 23:16:11 2026 +0500".into(),
            "".into(),
            "    Merge pull request #13".into(),
            "".into(),
            "".into(),
        ];
        assert_eq!(message_body(&header), vec!["Merge pull request #13"]);
    }

    #[test]
    fn message_body_of_an_empty_header_is_empty() {
        assert!(message_body(&[]).is_empty());
        // Meta lines only (no message at all) must not panic on the rposition.
        assert!(message_body(&["commit abc".to_string(), "".to_string()]).is_empty());
    }

    fn cr(sha: &str, parents: &[&str]) -> CommitRecord {
        CommitRecord {
            sha: sha.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "A".to_string(),
            email: "a@example.com".to_string(),
            display_date: "01/05/2026, 10:00 am".to_string(),
            timestamp: 0,
            date: "2026-05-01T10:00:00+00:00".to_string(),
            relative: "1 day ago".to_string(),
            subject: "S".to_string(),
            refs_decoration: String::new(),
        }
    }

    fn line(sha: &str, parents: &str, subject: &str, refs: &str) -> String {
        // %H %P %an %ae %aI %ad %ar %at %s %D
        format!(
            "{sha}\x1f{parents}\x1fAlice\x1fa@example.com\x1f2026-05-01T10:00:00+00:00\
             \x1f01/05/2026, 10:00 am\x1f1 day ago\x1f1777629600\x1f{subject}\x1f{refs}"
        )
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

    /// The per-file split of a patch: paths, status letters and +/- counts all
    /// come off the raw `git show` body, with no second subprocess.
    #[test]
    fn detail_splits_files_with_status_and_counts() {
        let text = "\
commit deadbeef
Author: Alice <a@example.com>

    fix: three files

diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
-old
+new
+extra
diff --git a/src/b.rs b/src/b.rs
new file mode 100644
--- /dev/null
+++ b/src/b.rs
@@ -0,0 +1 @@
+hello
diff --git a/src/c.rs b/src/c.rs
deleted file mode 100644
--- a/src/c.rs
+++ /dev/null
@@ -1 +0,0 @@
-bye
";
        let detail = parse_detail(text);
        // Everything before the first `diff --git` is the message header.
        assert!(detail.header.iter().any(|l| l.contains("fix: three files")));
        let files = &detail.files;
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[0].status, 'M');
        assert_eq!((files[0].added, files[0].deleted), (2, 1));
        assert_eq!(files[1].status, 'A');
        assert_eq!((files[1].added, files[1].deleted), (1, 0));
        assert_eq!(files[2].status, 'D');
        assert_eq!((files[2].added, files[2].deleted), (0, 1));
    }

    fn sample_refs() -> RefSet {
        RefSet {
            local: vec![
                RefEntry { name: "refs/heads/main".into(), sha: "h1".into() },
                RefEntry { name: "refs/heads/Feat/zeta".into(), sha: "h2".into() },
                RefEntry { name: "refs/heads/dev".into(), sha: "h3".into() },
            ],
            remote: vec![RefEntry {
                name: "refs/remotes/origin/main".into(),
                sha: "h1".into(),
            }],
            tags: vec![],
            head: Some("h1".into()),
        }
    }

    #[test]
    fn ref_groups_strip_sort_and_mark_head() {
        let groups = ref_groups(&sample_refs());
        // TAGS is empty → dropped; LOCAL then REMOTE remain.
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].title, "LOCAL");
        assert_eq!(groups[1].title, "REMOTE");
        // Prefix-stripped + case-insensitive sort: dev, Feat/zeta, main.
        let locals: Vec<&str> = groups[0].items.iter().map(|i| i.display.as_str()).collect();
        assert_eq!(locals, vec!["dev", "Feat/zeta", "main"]);
        // HEAD marker follows the head SHA — on main (local) AND origin/main.
        assert!(groups[0].items.iter().find(|i| i.display == "main").unwrap().is_head);
        assert!(!groups[0].items.iter().find(|i| i.display == "dev").unwrap().is_head);
        assert!(groups[1].items[0].is_head);
    }

    #[test]
    fn ref_groups_empty_set_yields_no_groups() {
        assert!(ref_groups(&RefSet::default()).is_empty());
    }

    fn named(sha: &str, subject: &str, author: &str) -> CommitRecord {
        CommitRecord {
            author: author.to_string(),
            subject: subject.to_string(),
            ..cr(sha, &[])
        }
    }

    #[test]
    fn filter_matches_subject_hash_author_case_insensitive() {
        let commits = vec![
            named("abc123", "fix: lane painter", "Alice"),
            named("def456", "feat: refs column", "Bob"),
            named("789fed", "chore: bump deps", "alice smith"),
        ];
        let text = |n: &str| apply_filters(&commits, n, None, DateFilter::All, 0);
        // Subject, any case.
        assert_eq!(text("LANE").len(), 1);
        // Hash prefix.
        assert_eq!(text("def4")[0].sha, "def456");
        // Author, matching both Alices.
        assert_eq!(text("alice").len(), 2);
        // Empty / whitespace needle keeps everything.
        assert_eq!(text("  ").len(), 3);
        // No match → empty.
        assert!(text("zzz").is_empty());
    }

    #[test]
    fn filtered_frame_relays_lanes_on_survivors() {
        // Fork + merge; filtering to the trunk-only subjects must re-run the
        // lane layout on JUST the survivors so lane rows and commits stay a
        // 1:1 zip (the painter indexes them in lockstep).
        let commits = vec![
            CommitRecord { subject: "trunk m".into(), ..cr("m", &["c2", "b1"]) },
            CommitRecord { subject: "trunk c2".into(), ..cr("c2", &["c1"]) },
            CommitRecord { subject: "branch b1".into(), ..cr("b1", &["c1"]) },
            CommitRecord { subject: "trunk c1".into(), ..cr("c1", &[]) },
        ];
        let frame = GraphFrame {
            refs: RefSet::default(),
            lanes: layout(&commits),
            commits,
        };
        let filtered = filtered_frame(&frame, "trunk", None, DateFilter::All, 0);
        assert_eq!(filtered.commits.len(), 3);
        assert_eq!(filtered.lanes.rows.len(), 3);
        // Every survivor sits on the trunk lane, and each lane row matches
        // its commit by SHA (no index drift from the removed branch commit).
        for (r, c) in filtered.lanes.rows.iter().zip(filtered.commits.iter()) {
            assert_eq!(r.sha, c.sha);
            assert_eq!(r.own_lane, 0, "row {} not on lane 0", r.sha);
        }
    }

    #[test]
    fn step_selection_clamps_and_starts_at_top() {
        let commits = vec![cr("a", &[]), cr("b", &[]), cr("c", &[])];
        // No selection → row 0 regardless of direction.
        assert_eq!(step_selection(&commits, None, true).as_deref(), Some("a"));
        assert_eq!(step_selection(&commits, None, false).as_deref(), Some("a"));
        // Down walks toward older commits, clamping at the end.
        assert_eq!(step_selection(&commits, Some("a"), true).as_deref(), Some("b"));
        assert_eq!(step_selection(&commits, Some("c"), true).as_deref(), Some("c"));
        // Up walks toward newer commits, clamping at the top.
        assert_eq!(step_selection(&commits, Some("b"), false).as_deref(), Some("a"));
        assert_eq!(step_selection(&commits, Some("a"), false).as_deref(), Some("a"));
        // A selection filtered out of the list restarts at row 0.
        assert_eq!(step_selection(&commits, Some("gone"), true).as_deref(), Some("a"));
        // Empty list → no selection.
        assert_eq!(step_selection(&[], Some("a"), true), None);
    }

    #[test]
    fn reveal_offset_scrolls_minimally() {
        // Row already visible → offset unchanged.
        assert_eq!(reveal_offset(10.0, 12, 5), 10.0);
        // Row above the viewport → snap it to the top edge.
        assert_eq!(reveal_offset(10.0, 4, 5), 4.0);
        // Row below the viewport → bottom-align it.
        assert_eq!(reveal_offset(10.0, 20, 5), 16.0);
        // Degenerate viewport clamps to 1 row.
        assert_eq!(reveal_offset(0.0, 3, 0), 3.0);
    }
}
