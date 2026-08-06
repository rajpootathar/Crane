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
    let v = crate::app::git::current_branch(path);
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
    let v = crate::app::git::diff_numstat(path);
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
    let v = crate::app::git::is_dirty(path);
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
        branch: crate::app::git::current_branch(path),
        diff_stat: crate::app::git::diff_numstat(path),
        dirty: crate::app::git::is_dirty(path),
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

/// Expand one opened folder into the flat `ProjectNode`s it contributes,
/// applying old-Crane's multi-git grouping rules (see
/// `docs/specs/2026-07-09-multi-git-grouping-and-instant-branch-design.md`):
/// - Discover nested git repos with `discover_repos(path, 5)` (recurses into
///   repos, skips build/dep dirs + symlinks).
/// - Parent IS a repo  → group the parent (as the first member, full git
///   wiring) plus any gitignored, non-submodule nested clones (submodules
///   share history with the parent and stay hidden — e.g. `vendor/warp`).
/// - Parent is NOT a repo → group every nested repo under a folder header.
/// - No nested repos → a git repo renders as itself (top-level); a non-git
///   folder renders as a loose folder.
///
/// `removed` is the raw removal set keyed by the path the user acted on. A
/// top-level opened folder is filtered out before this is called (by
/// `folders.retain`), but a container's discovered CHILD repos are keyed by
/// their OWN path — so "Remove Project" on a grouped child lands a child path
/// in `removed` that the container filter can never see. We re-check each
/// child's own path here and suppress a removed child so container expansion
/// does not re-emit it.
fn expand_folder(
    opened: OpenedFolder,
    removed: &[String],
    with_git: bool,
    out: &mut Vec<ProjectNode>,
) {
    let path = Path::new(&opened.path);
    let is_git = path.join(".git").exists();
    // The opened folder's own path: doubles as the group key AND the parent's
    // own `path` field when it becomes a group member.
    let container = opened.path.clone();

    // Nested repos under the opened folder (the folder itself is excluded).
    let nested: Vec<PathBuf> = if opened.path.is_empty() {
        Vec::new()
    } else {
        crate::app::git::discover_repos(path, 5)
            .into_iter()
            .filter(|p| p.as_path() != path)
            .collect()
    };

    // Filter siblings per old-Crane rules.
    let siblings: Vec<PathBuf> = if is_git {
        // Parent is a repo: only gitignored, non-submodule nested clones.
        // Resolve the submodule set and the ignored subset ONCE (one `git`
        // fork each) rather than shelling out per candidate.
        let submods = crate::app::git::submodule_paths(path);
        let ignored = crate::app::git::ignored_paths(path, &nested);
        nested
            .into_iter()
            .filter(|p| {
                ignored.contains(p)
                    && !submods.contains(&p.canonicalize().unwrap_or_else(|_| p.clone()))
            })
            .collect()
    } else {
        // Parent is not a repo: every nested repo.
        nested
    };

    // Drop children the user explicitly removed BEFORE deciding what this folder
    // contributes. "Remove Project" on a grouped child records the CHILD's own
    // path in `removed`; the top-level `folders.retain` keys on the container
    // path and can never see it, so it has to be filtered here.
    let live: Vec<PathBuf> = siblings
        .iter()
        .filter(|p| !removed.contains(&p.to_string_lossy().to_string()))
        .cloned()
        .collect();

    if !live.is_empty() {
        // GROUP under a collapsible folder header (label = opened folder
        // basename), keyed by the container's own path via `group_path`.
        if is_git {
            // The parent is itself a repo: add it FIRST as a real group member
            // (full git wiring — its branch / Commit UI work) so the user can
            // operate on the parent alongside the nested clones, exactly as old
            // Crane did. 1:1 with old `add_project_from_path`.
            let worktrees = if opened.worktrees.is_empty() {
                vec![default_worktree(path, &opened.name, with_git)]
            } else {
                opened.worktrees
            };
            out.push(ProjectNode {
                name: opened.name,
                path: container.clone(),
                worktrees,
                tint: None,
                is_loose: false,
                group_path: Some(container.clone()),
            });
        } else {
            // Non-git CONTAINER: emit the container ITSELF as a LOOSE group
            // member (first, above its repos) so everything in it that is NOT a
            // repo — stray files, plain subfolders — stays reachable. It carries
            // a worktree, so Tabs / terminals / the Files pane can be rooted at
            // the folder. Without this the container vanished entirely the
            // moment it contained one git repo, and its loose content had no
            // node to hang off.
            let worktrees = if opened.worktrees.is_empty() {
                vec![default_worktree(path, &opened.name, with_git)]
            } else {
                opened.worktrees
            };
            out.push(ProjectNode {
                name: opened.name,
                path: container.clone(),
                worktrees,
                tint: None,
                is_loose: true,
                group_path: Some(container.clone()),
            });
        }
        for child in &live {
            out.push(child_project_node(child, &container, with_git));
        }
        return;
    }

    // No SURVIVING nested repos. A git repo still renders as itself, top-level.
    if is_git {
        let worktrees = if opened.worktrees.is_empty() {
            vec![default_worktree(path, &opened.name, with_git)]
        } else {
            opened.worktrees
        };
        out.push(ProjectNode {
            name: opened.name,
            path: container.clone(),
            worktrees,
            tint: None,
            is_loose: false,
            group_path: None,
        });
        return;
    }

    // Non-git container whose every discovered repo the user removed: contribute
    // NOTHING. Falling through would materialise the bare folder as a top-level
    // project row nobody asked for — the user removed what was in it, so the
    // container goes with them.
    if !siblings.is_empty() {
        return;
    }

    // Loose folder (non-git, no nested repos): tabs render directly under it,
    // but it still needs a worktree to hold those tabs.
    let worktrees = if opened.worktrees.is_empty() {
        vec![default_worktree(path, &opened.name, with_git)]
    } else {
        opened.worktrees
    };
    out.push(ProjectNode {
        name: opened.name,
        path: container.clone(),
        worktrees,
        tint: None,
        is_loose: true,
        group_path: None,
    });
}

/// Drop `path` — and the repos DISCOVERED under it — from the removal set, so a
/// folder the user re-picks in "Add Project" comes back whole.
///
/// An exact-match retain is not enough: "Remove Project" on a repo discovered
/// inside a container records the CHILD's own path (see `expand_folder`), which
/// no container-keyed filter can ever clear. Re-adding the container then
/// re-emitted it with every previously-removed repo still suppressed — i.e. the
/// folder came back permanently empty.
///
/// But a blanket prefix clear over-reaches. A path nested under `path` may be a
/// folder the user opened INDEPENDENTLY (its own session.json / "Add Project"
/// entry) and removed as a top-level project in its own right — e.g. removing
/// `OneVibe/Android` and `OneVibe/Backend`, then later adding their parent
/// `OneVibe`. Those keep their own identity, so re-adding an ancestor must not
/// resurrect them; only re-picking that exact path may. `opened` carries every
/// independently-opened path for exactly this test.
pub fn unsuppress_tree(
    removed: &mut Vec<String>,
    path: &str,
    opened: &std::collections::HashSet<String>,
) {
    let prefix = format!("{path}/");
    removed.retain(|r| {
        // The folder the user just picked always comes back.
        if r == path {
            return false;
        }
        // Unrelated entry (incl. a mere name-prefix sibling like `foo-old`).
        if !r.starts_with(&prefix) {
            return true;
        }
        // Descendant: restore it only if it exists SOLELY as a discovered child
        // of this container. An independently-opened one stays removed.
        opened.contains(r)
    });
}

/// Paths of every folder recorded as independently opened in `session.json`.
/// Combined with `added_projects`, this is the set that `unsuppress_tree` must
/// not resurrect when an ancestor folder is re-added.
pub fn session_folder_paths() -> Vec<String> {
    session_folders(false)
        .into_iter()
        .map(|f| f.path)
        .filter(|p| !p.is_empty())
        .collect()
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
    added: &[crate::app::persist::AddedProject],
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
    added: &[crate::app::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    load_projects_core(added, removed, tints, false)
}

/// Shallow-expand ONE user-added folder into the flat `ProjectNode`(s) it
/// contributes (git repo → itself; container/parent-repo → one node per grouped
/// repo; loose folder → itself). The grouping pass in `expand_folder` forks
/// `git` twice per git-container (one `submodule status`, one `check-ignore`) to
/// decide membership, but skips the per-worktree branch/diff/dirty git calls
/// (those still fill in later via a targeted `spawn_git_scan`). Used by "Add
/// Project" to APPEND the picked folder to `self.projects` in place — no
/// whole-tree reload, so existing projects' already-filled badges and
/// (project-index-keyed) state are untouched.
pub fn load_one_shallow(
    added: &crate::app::persist::AddedProject,
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
    added: &[crate::app::persist::AddedProject],
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
    // 2b. Fold nested repos that were BOTH discovered as a parent-repo group
    //     child AND opened independently (see `fold_grouped_duplicates`).
    fold_grouped_duplicates(&mut projects);
    // 3. Apply per-path tint overrides (git/loose keyed by opened path, child
    //    repos keyed by their own path).
    for p in &mut projects {
        p.tint = tints.get(&p.path).copied();
    }
    projects
}

/// Resolve the collision between parent-repo grouping and independently-opened
/// nested repos. A gitignored nested clone (e.g. `qck-cloud/qck-py-sdk`) that the
/// user ALSO opened on its own is emitted twice: once as a FRESH group child by
/// its parent repo's `expand_folder` (a default worktree, no tabs) and once as a
/// STANDALONE node carrying the real session worktrees/tabs. Left as-is the
/// sidebar shows a duplicate row AND — because the standalone sits between the
/// group's children — splits the parent's FOLDER header into two (the header is
/// drawn once per CONTIGUOUS run of a `group_path`).
///
/// Fix: keep the standalone (it owns the real state), ADOPT the group by copying
/// the child's `group_path` onto it, and drop the fresh child. A genuinely
/// not-separately-opened sibling (no standalone twin) keeps its fresh child node,
/// so single-container grouping is unaffected.
fn fold_grouped_duplicates(projects: &mut Vec<ProjectNode>) {
    // Paths that have an independently-opened (top-level) node.
    let standalone_paths: std::collections::HashSet<String> = projects
        .iter()
        .filter(|p| p.group_path.is_none())
        .map(|p| p.path.clone())
        .collect();
    // Fresh group-CHILD nodes: group_path set AND not the group PARENT (whose
    // own path equals its group_path). Map child path -> its container group.
    let child_group: HashMap<String, String> = projects
        .iter()
        .filter(|p| p.group_path.as_deref().is_some_and(|g| g != p.path))
        .map(|p| (p.path.clone(), p.group_path.clone().unwrap()))
        .collect();
    if child_group.is_empty() {
        return;
    }
    // Drop the fresh child duplicate wherever a standalone twin exists.
    projects.retain(|p| {
        let is_fresh_child = p.group_path.as_deref().is_some_and(|g| g != p.path);
        !(is_fresh_child && standalone_paths.contains(&p.path))
    });
    // Fold each surviving standalone twin into its parent's group.
    for p in projects.iter_mut() {
        if p.group_path.is_none() {
            if let Some(g) = child_group.get(&p.path) {
                p.group_path = Some(g.clone());
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, path: &str, group: Option<&str>, wt: &str) -> ProjectNode {
        ProjectNode {
            name: name.to_string(),
            path: path.to_string(),
            worktrees: vec![WorktreeNode {
                name: wt.to_string(),
                path: path.to_string(),
                tabs: Vec::new(),
                diff_stat: (0, 0),
                dirty: false,
            }],
            tint: None,
            is_loose: false,
            group_path: group.map(str::to_string),
        }
    }

    // The qck-cloud bug: `qck-cloud` is a repo whose gitignored nested clones
    // `qck-py-sdk` / `qck-js-sdk` are ALSO opened independently. `expand_folder`
    // emits each nested clone twice — a fresh group child AND a standalone node
    // with the real worktree. Fold must collapse each pair to ONE node that
    // keeps the standalone's worktree and adopts the parent's group.
    #[test]
    fn fold_collapses_opened_nested_clone_into_its_group() {
        let g = "/p/qck-cloud";
        let mut projects = vec![
            node("qck-cloud", g, Some(g), "fix/qr"), // group parent (path == group)
            node("qck-py-sdk", "/p/qck-cloud/qck-py-sdk", Some(g), "qck-py-sdk"), // fresh child
            node("qck-py-sdk", "/p/qck-cloud/qck-py-sdk", None, "main"), // standalone (real state)
            node("qck-js-sdk", "/p/qck-cloud/qck-js-sdk", Some(g), "qck-js-sdk"), // fresh child
            node("qck-js-sdk", "/p/qck-cloud/qck-js-sdk", None, "main"), // standalone (real state)
        ];
        fold_grouped_duplicates(&mut projects);

        // One node per path — no duplicates.
        assert_eq!(projects.len(), 3, "each nested clone collapses to one node");
        // Every surviving node carries the group; the two clones adopted it.
        for p in &projects {
            assert_eq!(p.group_path.as_deref(), Some(g), "node {} lost its group", p.name);
        }
        // The SURVIVOR is the standalone (its worktree is the real branch, not
        // the basename default the fresh child carried).
        let py = projects.iter().find(|p| p.path.ends_with("qck-py-sdk")).unwrap();
        assert_eq!(py.worktrees[0].name, "main", "kept the fresh child, lost real state");
    }

    /// A non-git CONTAINER holding one git repo plus loose (non-repo) content
    /// must emit the container itself as a loose group member — otherwise the
    /// loose files have no node and are unreachable.
    #[test]
    fn container_with_loose_content_emits_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-folder");
        std::fs::create_dir_all(root.join("repo-a/.git")).unwrap();
        std::fs::create_dir_all(root.join("plain-subdir")).unwrap();
        std::fs::write(root.join("notes.md"), "loose").unwrap();

        let mut out = Vec::new();
        expand_folder(
            OpenedFolder {
                name: "my-folder".into(),
                path: root.to_string_lossy().into(),
                worktrees: Vec::new(),
            },
            &[],
            false,
            &mut out,
        );

        let group = root.to_string_lossy().to_string();
        assert_eq!(out.len(), 2, "container + its one repo");
        assert!(out[0].is_loose, "container must render as a loose folder");
        assert_eq!(out[0].path, group);
        assert_eq!(out[0].group_path.as_deref(), Some(group.as_str()));
        assert_eq!(
            out[0].worktrees.len(),
            1,
            "loose container needs a worktree to hold Tabs / terminals"
        );
        assert!(!out[1].is_loose);
        assert!(out[1].path.ends_with("repo-a"));
        assert_eq!(out[1].group_path.as_deref(), Some(group.as_str()));
    }

    /// Re-adding a container must restore the repos inside it. `expand_folder`
    /// suppresses any child listed in `removed`, and "Remove Project" on a child
    /// records the CHILD's path — so the un-remove has to clear the whole
    /// subtree, not just the exact folder the user picked.
    #[test]
    fn unsuppress_tree_restores_removed_children() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-folder");
        std::fs::create_dir_all(root.join("repo-a/.git")).unwrap();
        std::fs::create_dir_all(root.join("repo-b/.git")).unwrap();
        let group = root.to_string_lossy().to_string();
        let opened = || OpenedFolder {
            name: "my-folder".into(),
            path: group.clone(),
            worktrees: Vec::new(),
        };

        // User removed the whole group: the container plus both child repos,
        // each keyed by its own path.
        let mut removed = vec![
            group.clone(),
            format!("{group}/repo-a"),
            format!("{group}/repo-b"),
        ];

        // Exact-match clearing only (the old behaviour) brings the folder back
        // with both repos still suppressed.
        let mut exact = removed.clone();
        exact.retain(|r| r != &group);
        let mut out = Vec::new();
        expand_folder(opened(), &exact, false, &mut out);
        assert!(out.is_empty(), "every child suppressed => folder contributes nothing");

        // Subtree clearing restores the container AND both repos.
        unsuppress_tree(&mut removed, &group, &Default::default());
        assert!(removed.is_empty(), "subtree entries must all be cleared");
        let mut out = Vec::new();
        expand_folder(opened(), &removed, false, &mut out);
        assert_eq!(out.len(), 3, "container + both repos come back");
    }

    /// A non-git container whose every discovered repo the user removed must
    /// contribute NOTHING. Materialising the bare folder as a top-level project
    /// row resurrects something the user never opened again.
    #[test]
    fn container_with_all_children_removed_contributes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("OneVibe");
        std::fs::create_dir_all(root.join("Android/.git")).unwrap();
        std::fs::create_dir_all(root.join("Backend/.git")).unwrap();
        std::fs::write(root.join("README.md"), "loose").unwrap();
        let group = root.to_string_lossy().to_string();

        let removed = vec![format!("{group}/Android"), format!("{group}/Backend")];
        let mut out = Vec::new();
        expand_folder(
            OpenedFolder {
                name: "OneVibe".into(),
                path: group.clone(),
                worktrees: Vec::new(),
            },
            &removed,
            false,
            &mut out,
        );
        assert!(
            out.is_empty(),
            "container must not reappear as a bare row; got {:?}",
            out.iter().map(|p| p.path.clone()).collect::<Vec<_>>()
        );
    }

    /// Re-adding an ancestor must not resurrect a nested folder the user opened
    /// INDEPENDENTLY and removed as a project in its own right.
    #[test]
    fn unsuppress_tree_leaves_independently_opened_descendants_removed() {
        let opened: std::collections::HashSet<String> =
            ["/p/OneVibe/Android".to_string()].into_iter().collect();
        let mut removed = vec![
            "/p/OneVibe".to_string(),
            "/p/OneVibe/Android".to_string(), // opened on its own => stays removed
            "/p/OneVibe/Backend".to_string(), // discovered child only => restored
        ];
        unsuppress_tree(&mut removed, "/p/OneVibe", &opened);
        assert_eq!(removed, vec!["/p/OneVibe/Android".to_string()]);

        // Re-picking that exact path is the way back.
        unsuppress_tree(&mut removed, "/p/OneVibe/Android", &opened);
        assert!(removed.is_empty(), "the exact path always comes back");
    }

    /// The subtree clear must not touch a sibling folder that merely shares a
    /// name prefix (`/p/proj` vs `/p/proj-old`).
    #[test]
    fn unsuppress_tree_respects_path_boundaries() {
        let mut removed = vec![
            "/p/proj".to_string(),
            "/p/proj/child".to_string(),
            "/p/proj-old".to_string(),
        ];
        unsuppress_tree(&mut removed, "/p/proj", &Default::default());
        assert_eq!(removed, vec!["/p/proj-old".to_string()]);
    }

    // A discovered sibling that was NOT opened on its own (e.g. a gitignored
    // clone the user never added) must keep its fresh child node.
    #[test]
    fn fold_preserves_unopened_group_child() {
        let g = "/p/parent";
        let mut projects = vec![
            node("parent", g, Some(g), "main"),
            node("clone", "/p/parent/clone", Some(g), "clone"), // no standalone twin
        ];
        fold_grouped_duplicates(&mut projects);
        assert_eq!(projects.len(), 2, "unopened sibling must survive");
        assert_eq!(projects[1].group_path.as_deref(), Some(g));
    }
}
