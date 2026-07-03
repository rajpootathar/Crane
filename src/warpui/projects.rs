//! Reads the REAL Crane project tree from `~/.crane/session.json` so the
//! warpui shell shows the user's actual projects / worktrees / tabs — proving
//! the existing Crane logic + persistence is consumed unchanged; only the GUI
//! is new. Parsed via serde_json::Value to avoid importing the crane crate's
//! full session schema.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Build a git-project `ProjectNode` for a discovered CHILD repo (a git repo
/// found directly inside an opened container folder). It gets a single
/// synthesized worktree = its own checkout (current branch), so the sidebar
/// renders a real branch row + tabs + diff badges under it.
fn child_project_node(child: &Path, container_path: &str) -> ProjectNode {
    let cpath = child.to_string_lossy().to_string();
    let cname = basename(child, "(repo)");
    let branch = crate::warpui::git::current_branch(child);
    let wname = if branch.is_empty() { cname.clone() } else { branch };
    let diff_stat = crate::warpui::git::diff_numstat(child);
    let dirty = crate::warpui::git::is_dirty(child);
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
fn expand_folder(opened: OpenedFolder, out: &mut Vec<ProjectNode>) {
    let path = Path::new(&opened.path);
    let is_git = path.join(".git").exists();
    if is_git {
        out.push(ProjectNode {
            name: opened.name,
            path: opened.path,
            worktrees: opened.worktrees,
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
            out.push(child_project_node(child, &opened.path));
        }
        return;
    }
    // Loose folder (non-git, no git children): tabs render directly under it.
    out.push(ProjectNode {
        name: opened.name,
        path: opened.path,
        worktrees: opened.worktrees,
        tint: None,
        is_loose: true,
        group_path: None,
    });
}

/// Parse the opened folders recorded in `~/.crane/session.json` (unexpanded).
fn session_folders() -> Vec<OpenedFolder> {
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
                    let (diff_stat, dirty) = if !wpath.is_empty() {
                        let p = Path::new(&wpath);
                        (
                            crate::warpui::git::diff_numstat(p),
                            crate::warpui::git::is_dirty(p),
                        )
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
pub fn load_projects_extended(
    added: &[crate::warpui::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    // 1. Gather opened folders (session + user-added), minus removed, deduped
    //    by the opened path.
    let mut folders: Vec<OpenedFolder> = session_folders();
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
        expand_folder(folder, &mut projects);
    }
    // 3. Apply per-path tint overrides (git/loose keyed by opened path, child
    //    repos keyed by their own path).
    for p in &mut projects {
        p.tint = tints.get(&p.path).copied();
    }
    projects
}
