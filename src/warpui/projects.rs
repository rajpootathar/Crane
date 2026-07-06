//! Reads the REAL Crane project tree from `~/.crane/session.json` so the
//! warpui shell shows the user's actual projects / worktrees / tabs — proving
//! the existing Crane logic + persistence is consumed unchanged; only the GUI
//! is new. Parsed via serde_json::Value to avoid importing the crane crate's
//! full session schema.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Per-repo git-status cache.
//
// `load_projects_extended` is re-run on every AddProject / InitGitProject /
// reload. Naively it re-shells `git rev-parse` (branch) + `git diff --numstat`
// + `git status --porcelain` for EVERY project and worktree in the whole tree —
// so adding one project forks git dozens of times on the UI thread (a freeze).
//
// The git content of the OTHER, unchanged projects does not move between two
// reloads triggered seconds apart, so we memoize each helper's result per repo
// path with a short TTL. A reload then re-forks git only for genuinely new
// paths (cache miss) and reuses recent results for everything else. This never
// forks MORE than the uncached path did on a cold start (each field is computed
// at most once per TTL window), so first-frame startup cost is unchanged; only
// repeat reloads get cheaper. Staleness is bounded by `GIT_CACHE_TTL` and is
// no worse than the pre-existing behaviour, where sidebar diff/dirty badges are
// already only refreshed on structural events (worktree add/remove) and
// reloads — never on plain file edits.
// ---------------------------------------------------------------------------

/// Max age of a cached git result before it is recomputed on next access.
const GIT_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Default, Clone)]
struct RepoGit {
    branch: Option<(Instant, String)>,
    diff: Option<(Instant, (u32, u32))>,
    dirty: Option<(Instant, bool)>,
}

fn git_cache() -> &'static Mutex<HashMap<String, RepoGit>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RepoGit>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fresh<T>(slot: &Option<(Instant, T)>) -> Option<&T> {
    slot.as_ref()
        .filter(|(t, _)| t.elapsed() < GIT_CACHE_TTL)
        .map(|(_, v)| v)
}

/// Cached `git::current_branch` keyed by repo path (see module note).
fn cached_current_branch(path: &Path) -> String {
    let key = path.to_string_lossy().to_string();
    if let Some(v) = git_cache()
        .lock()
        .unwrap()
        .get(&key)
        .and_then(|e| fresh(&e.branch).cloned())
    {
        return v;
    }
    let v = crate::warpui::git::current_branch(path);
    git_cache().lock().unwrap().entry(key).or_default().branch =
        Some((Instant::now(), v.clone()));
    v
}

/// Cached `git::diff_numstat` keyed by repo path (see module note).
fn cached_diff_numstat(path: &Path) -> (u32, u32) {
    let key = path.to_string_lossy().to_string();
    if let Some(v) = git_cache()
        .lock()
        .unwrap()
        .get(&key)
        .and_then(|e| fresh(&e.diff).copied())
    {
        return v;
    }
    let v = crate::warpui::git::diff_numstat(path);
    git_cache().lock().unwrap().entry(key).or_default().diff = Some((Instant::now(), v));
    v
}

/// Cached `git::is_dirty` keyed by repo path (see module note).
fn cached_is_dirty(path: &Path) -> bool {
    let key = path.to_string_lossy().to_string();
    if let Some(v) = git_cache()
        .lock()
        .unwrap()
        .get(&key)
        .and_then(|e| fresh(&e.dirty).copied())
    {
        return v;
    }
    let v = crate::warpui::git::is_dirty(path);
    git_cache().lock().unwrap().entry(key).or_default().dirty = Some((Instant::now(), v));
    v
}

/// Drop the cached git status for `path`, forcing a fresh shell-out on next
/// access. Call after an operation that mutates a repo's working tree / HEAD
/// (commit, checkout, stage) so the sidebar badge refreshes immediately instead
/// of waiting out the TTL. Exposed for callers in the shell.
pub fn invalidate_git_cache(path: &str) {
    git_cache().lock().unwrap().remove(path);
}

/// The three git fields a worktree/repo node needs, computed off the UI thread
/// by the shell's keyed async scan (`CraneShellView::spawn_git_scan`) and
/// applied back into the matching [`WorktreeNode`] by path.
///
/// Shallow project loading (`load_projects_shallow` / `load_one_shallow`)
/// builds the whole tree STRUCTURE with these fields empty/zeroed so the
/// sidebar appears instantly; the scan then fills them a moment later.
#[derive(Clone, Default)]
pub struct RepoGitInfo {
    /// Current branch (or short SHA when detached); empty on error / non-repo.
    pub branch: String,
    /// `git diff --numstat HEAD` totals: (added_lines, deleted_lines).
    pub diff_stat: (u32, u32),
    /// Whether the working tree has ANY uncommitted change (incl. untracked).
    pub dirty: bool,
}

/// Compute all three git fields for one repo/worktree path with a single set
/// of shell-outs. MUST be called off the UI thread (from the shell's
/// background scan future) — it forks `git` three times.
pub fn scan_repo_git(path: &Path) -> RepoGitInfo {
    RepoGitInfo {
        branch: crate::warpui::git::current_branch(path),
        diff_stat: crate::warpui::git::diff_numstat(path),
        dirty: crate::warpui::git::is_dirty(path),
    }
}

pub struct WorktreeNode {
    pub name: String,
    pub path: String,
    pub tabs: Vec<String>,
    /// Cached `git diff --numstat HEAD` totals: (added_lines, deleted_lines).
    /// Computed once at load/reload time — never per frame.
    pub diff_stat: (u32, u32),
    /// Whether the working tree has ANY uncommitted change (incl. untracked).
    /// Lets the branch row paint a "dirty dot" when `diff_stat` is (0, 0) but
    /// the tree is still dirty (e.g. only untracked files). Computed at load.
    pub dirty: bool,
}

pub struct ProjectNode {
    pub name: String,
    pub path: String,
    pub worktrees: Vec<WorktreeNode>,
    /// Per-project accent tint set by the user (overrides the palette default).
    pub tint: Option<[u8; 3]>,
    /// True when the project folder has no `.git` entry (directory or file).
    /// Computed once at load time. A loose project shows a FOLDER icon, hides
    /// branch/worktree rows, and offers "Initialize Git" in its context menu.
    pub is_loose: bool,
    /// Set ONLY on a git project that was discovered INSIDE a container folder
    /// the user opened — i.e. the user opened a non-git folder whose immediate
    /// child directories are themselves git repos (e.g. opening `qck-platform`
    /// surfaces `qck-cloud` / `qck-py-sdk` / `qck-js-sdk`). The value is the
    /// CONTAINER folder's OWN path. The sidebar renders a collapsible FOLDER
    /// header (label = the container's basename, collapse keyed by this path via
    /// `ToggleGroup`/`collapsed_groups`) once per contiguous run of children and
    /// nests each child project one indent below it.
    ///
    /// `None` for a folder the user opened directly (a git project or a loose
    /// folder) — those render top-level. Grouping is INTRINSIC to a single
    /// opened container folder; it is NEVER inferred from a shared parent
    /// directory of separately-opened projects.
    pub group_path: Option<String>,
}

/// One folder the user explicitly opened (from session.json or "Add Project"),
/// before container-expansion. Carries any worktrees session.json recorded for
/// it (used verbatim when the folder is itself a git repo / loose folder).
struct OpenedFolder {
    name: String,
    path: String,
    worktrees: Vec<WorktreeNode>,
}

/// Names of child directories that are never scanned when detecting whether a
/// non-git container folder holds git repos (build output / dependency dirs).
fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | "target" | "dist" | "build" | ".next" | "vendor"
            | ".venv" | "venv" | ".cache" | ".turbo" | ".cargo"
    )
}

/// Immediate child directories of `dir` that are themselves git repos (contain
/// a `.git` entry). Only the FIRST level is scanned — grouping is intrinsic to
/// the one opened container folder, so we never recurse into a parent the user
/// didn't open. Sorted for stable ordering across reloads.
fn immediate_child_repos(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with('.') || skip_dir(n) {
            continue;
        }
        let path = entry.path();
        if path.join(".git").exists() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Basename of a path as a `String`, falling back to `fallback`.
fn basename(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Public basename helper for callers outside this module (e.g. the shell's
/// worktree-poll tick naming a detached worktree by its directory).
pub fn basename_of(path: &Path) -> String {
    basename(path, "(worktree)")
}

/// Build a git-project `ProjectNode` for a discovered CHILD repo (a git repo
/// found directly inside an opened container folder). It gets a single
/// synthesized worktree = its own checkout (current branch), so the sidebar
/// renders a real branch row + tabs + diff badges under it.
///
/// When `with_git` is false the git fields are left empty/zeroed (branch = "",
/// diff = (0,0), dirty = false) and NO `git` subprocess is forked — the shallow
/// load path used to paint the tree instantly, with an async scan filling the
/// fields afterwards.
fn child_project_node(child: &Path, container_path: &str, with_git: bool) -> ProjectNode {
    let cpath = child.to_string_lossy().to_string();
    let cname = basename(child, "(repo)");
    let branch = if with_git { cached_current_branch(child) } else { String::new() };
    let wname = if branch.is_empty() { cname.clone() } else { branch };
    let (diff_stat, dirty) = if with_git {
        (cached_diff_numstat(child), cached_is_dirty(child))
    } else {
        ((0, 0), false)
    };
    ProjectNode {
        name: cname,
        path: cpath.clone(),
        worktrees: vec![WorktreeNode {
            name: wname,
            path: cpath,
            tabs: Vec::new(),
            diff_stat,
            dirty,
        }],
        tint: None,
        is_loose: false,
        group_path: Some(container_path.to_string()),
    }
}

/// Expand ONE opened folder into the flat `ProjectNode`s it contributes:
/// - git repo            → itself, top-level (`group_path = None`).
/// - non-git CONTAINER   → one child ProjectNode per immediate git-repo child,
///                         each carrying `group_path = Some(container path)`.
/// - non-git loose folder → itself as a loose folder (`is_loose = true`).
/// Build a default single worktree for a freshly-opened folder that has no
/// session worktrees yet (e.g. one just added via the folder picker). Without
/// this the project renders no worktree/branch row, so no tab or terminal can
/// appear under it. For a git repo the row shows the current branch; for a loose
/// folder the branch is empty so it falls back to the folder name.
///
/// `with_git` false ⇒ no `git` subprocess: branch is empty (so the row falls
/// back to `folder_name`), diff is (0,0) and dirty is false. The async scan
/// refills these once the shallow tree is painted.
fn default_worktree(path: &Path, folder_name: &str, with_git: bool) -> WorktreeNode {
    let branch = if with_git { cached_current_branch(path) } else { String::new() };
    let wname = if branch.is_empty() {
        folder_name.to_string()
    } else {
        branch
    };
    let (diff_stat, dirty) = if with_git {
        (cached_diff_numstat(path), cached_is_dirty(path))
    } else {
        ((0, 0), false)
    };
    WorktreeNode {
        name: wname,
        path: path.to_string_lossy().to_string(),
        tabs: Vec::new(),
        diff_stat,
        dirty,
    }
}

/// Expand one opened folder. `removed` is the raw removal set keyed by the path
/// the user acted on. A top-level opened folder is filtered out before this is
/// called (by `folders.retain`), but a container's discovered CHILD repos are
/// keyed by their OWN path, not the container path — so "Remove Project" on a
/// grouped child lands a child path in `removed` that the container filter can
/// never see. We therefore re-check each child's own path here and suppress a
/// removed child so container expansion does not re-emit it.
fn expand_folder(
    opened: OpenedFolder,
    removed: &[String],
    with_git: bool,
    out: &mut Vec<ProjectNode>,
) {
    let path = Path::new(&opened.path);
    let is_git = path.join(".git").exists();
    if is_git {
        let worktrees = if opened.worktrees.is_empty() {
            vec![default_worktree(path, &opened.name, with_git)]
        } else {
            opened.worktrees
        };
        out.push(ProjectNode {
            name: opened.name,
            path: opened.path,
            worktrees,
            tint: None,
            is_loose: false,
            group_path: None,
        });
        return;
    }
    // Non-git folder: is it a CONTAINER (immediate children are git repos)?
    let children = if opened.path.is_empty() {
        Vec::new()
    } else {
        immediate_child_repos(path)
    };
    if !children.is_empty() {
        for child in &children {
            // Suppress a child repo the user explicitly removed. "Remove Project"
            // on a grouped child records the child's OWN path in `removed`; the
            // top-level `folders.retain` keys on the container path and can't
            // filter it, so it must be dropped here or it re-appears on reload.
            // If every child is removed the container contributes nothing (it
            // never falls through to render as a loose folder).
            let cpath = child.to_string_lossy().to_string();
            if removed.contains(&cpath) {
                continue;
            }
            out.push(child_project_node(child, &opened.path, with_git));
        }
        return;
    }
    // Loose folder (non-git, no git children): tabs render directly under it,
    // but it still needs a worktree to hold those tabs.
    let worktrees = if opened.worktrees.is_empty() {
        vec![default_worktree(path, &opened.name, with_git)]
    } else {
        opened.worktrees
    };
    out.push(ProjectNode {
        name: opened.name,
        path: opened.path,
        worktrees,
        tint: None,
        is_loose: true,
        group_path: None,
    });
}

/// Parse the opened folders recorded in `~/.crane/session.json` (unexpanded).
fn session_folders(with_git: bool) -> Vec<OpenedFolder> {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = format!("{home}/.crane/session.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    if let Some(projects) = v.get("projects").and_then(|x| x.as_array()) {
        for p in projects {
            let name = p
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("(unnamed)")
                .to_string();
            let path = p
                .get("path")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let mut worktrees = Vec::new();
            if let Some(ws) = p.get("workspaces").and_then(|x| x.as_array()) {
                for w in ws {
                    let wname = w
                        .get("display_name")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| w.get("name").and_then(|x| x.as_str()))
                        .unwrap_or("(branch)")
                        .to_string();
                    let wpath = w
                        .get("path")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut tabs = Vec::new();
                    if let Some(ts) = w.get("tabs").and_then(|x| x.as_array()) {
                        for t in ts {
                            tabs.push(
                                t.get("name")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("Tab")
                                    .to_string(),
                            );
                        }
                    }
                    let (diff_stat, dirty) = if with_git && !wpath.is_empty() {
                        let p = Path::new(&wpath);
                        (cached_diff_numstat(p), cached_is_dirty(p))
                    } else {
                        ((0, 0), false)
                    };
                    worktrees.push(WorktreeNode {
                        name: wname,
                        path: wpath,
                        tabs,
                        diff_stat,
                        dirty,
                    });
                }
            }
            out.push(OpenedFolder {
                name,
                path,
                worktrees,
            });
        }
    }
    out
}

/// Load projects with overlay data from warpui-state.json:
/// - `added`: extra projects appended by the user via "Add Project"
/// - `removed`: paths of opened folders to exclude
/// - `tints`: per-path tint overrides
///
/// The overlay is applied to the OPENED-folder list (keyed by the folder path
/// the user actually opened) BEFORE container-expansion, so removing a
/// container folder drops all of its discovered child repos atomically and
/// re-adding one via "Add Project" is deduped by the opened path.
/// The original synchronous-with-git load (branch/diff/dirty computed inline via
/// the 10s-TTL cache). SUPERSEDED by `load_projects_shallow` + the shell's async
/// `spawn_git_scan`, which never forks `git` on the UI thread. Retained as the
/// reference implementation of the fully-populated tree (and to keep the TTL
/// cache path compiled); not on any hot path.
#[allow(dead_code)]
pub fn load_projects_extended(
    added: &[crate::warpui::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    load_projects_core(added, removed, tints, true)
}

/// SHALLOW project load: builds the FULL tree STRUCTURE (project names/paths,
/// worktrees, loose/git flags, container grouping) using ONLY cheap fs checks
/// (`.git` presence + a single `read_dir` per opened container) and forks ZERO
/// `git` subprocesses. Worktree nodes come back with an empty branch (rows show
/// the folder name), `diff_stat = (0,0)` and `dirty = false`.
///
/// This is what the shell calls on startup / reload so the sidebar paints
/// instantly; branch labels and diff/dirty badges are then filled in a moment
/// later by `CraneShellView::spawn_git_scan` (which runs the git shell-outs off
/// the UI thread and applies the results back into these nodes by path).
pub fn load_projects_shallow(
    added: &[crate::warpui::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    load_projects_core(added, removed, tints, false)
}

/// Shallow-expand ONE user-added folder into the flat `ProjectNode`(s) it
/// contributes (git repo → itself; non-git container → one node per git-repo
/// child; loose folder → itself). Forks ZERO `git`. Used by "Add Project" to
/// APPEND the picked folder to `self.projects` in place — no whole-tree reload,
/// so existing projects' already-filled badges and (project-index-keyed) state
/// are untouched. The caller then runs a targeted `spawn_git_scan` for the new
/// paths.
pub fn load_one_shallow(
    added: &crate::warpui::persist::AddedProject,
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    let mut projects = Vec::new();
    expand_folder(
        OpenedFolder {
            name: added.name.clone(),
            path: added.path.clone(),
            worktrees: Vec::new(),
        },
        removed,
        false,
        &mut projects,
    );
    for p in &mut projects {
        p.tint = tints.get(&p.path).copied();
    }
    projects
}

fn load_projects_core(
    added: &[crate::warpui::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
    with_git: bool,
) -> Vec<ProjectNode> {
    // 1. Gather opened folders (session + user-added), minus removed, deduped
    //    by the opened path.
    let mut folders: Vec<OpenedFolder> = session_folders(with_git);
    folders.retain(|f| !removed.contains(&f.path));
    for ap in added {
        if !folders.iter().any(|f| f.path == ap.path) {
            folders.push(OpenedFolder {
                name: ap.name.clone(),
                path: ap.path.clone(),
                worktrees: Vec::new(),
            });
        }
    }
    // 2. Expand each opened folder into its flat ProjectNode(s).
    let mut projects = Vec::new();
    for folder in folders {
        expand_folder(folder, removed, with_git, &mut projects);
    }
    // 3. Apply per-path tint overrides (git/loose keyed by opened path, child
    //    repos keyed by their own path).
    for p in &mut projects {
        p.tint = tints.get(&p.path).copied();
    }
    projects
}

/// Collect the repo/worktree paths under `projects` that warrant a git scan
/// (branch + diff + dirty). Loose (non-git) projects are skipped — they have no
/// HEAD, so scanning them would fork `git` only to get empty results. Returns
/// de-duplicated, non-empty checkout paths.
pub fn scan_paths(projects: &[ProjectNode]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in projects {
        if p.is_loose {
            continue;
        }
        for w in &p.worktrees {
            if w.path.is_empty() {
                continue;
            }
            let pb = PathBuf::from(&w.path);
            if !out.contains(&pb) {
                out.push(pb);
            }
        }
    }
    out
}
