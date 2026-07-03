//! Reads the REAL Crane project tree from `~/.crane/session.json` so the
//! warpui shell shows the user's actual projects / worktrees / tabs — proving
//! the existing Crane logic + persistence is consumed unchanged; only the GUI
//! is new. Parsed via serde_json::Value to avoid importing the crane crate's
//! full session schema.

use std::collections::HashMap;

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
    /// Shared-parent folder group. `Some(parent_dir)` when 2+ loaded projects
    /// live directly under the same parent directory — those projects nest
    /// under a collapsible FOLDER header named by the parent's basename.
    /// `None` for projects with no shared-parent sibling (rendered top-level).
    /// Derived at load time by `assign_groups`.
    pub group_path: Option<String>,
}

/// Group projects that share an immediate parent directory. When 2+ projects
/// in the list live directly under the same parent, each of those projects gets
/// `group_path = Some(parent)` so the sidebar renders them under a collapsible
/// FOLDER header. Projects with no shared-parent sibling stay ungrouped.
///
/// Simplification vs. old egui: the original derived groups from a live repo
/// discovery walk (`git::discover_repos`); here we group purely by the loaded
/// projects' immediate parent directory path, which is deterministic and needs
/// no filesystem scan.
fn assign_groups(projects: &mut [ProjectNode]) {
    let parent_of = |path: &str| -> Option<String> {
        std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in projects.iter() {
        if let Some(parent) = parent_of(&p.path) {
            *counts.entry(parent).or_insert(0) += 1;
        }
    }
    for p in projects.iter_mut() {
        if let Some(parent) = parent_of(&p.path) {
            if counts.get(&parent).copied().unwrap_or(0) >= 2 {
                p.group_path = Some(parent);
            }
        }
    }
}

/// Load the project tree from the live Crane session, or empty if missing.
pub fn load_projects() -> Vec<ProjectNode> {
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
                        let p = std::path::Path::new(&wpath);
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
            let is_loose = !std::path::Path::new(&path).join(".git").exists();
            out.push(ProjectNode {
                name,
                path,
                worktrees,
                tint: None,
                is_loose,
                group_path: None,
            });
        }
    }
    out
}

/// Load projects with overlay data from warpui-state.json:
/// - `added`: extra projects appended by the user via "Add Project"
/// - `removed`: paths of session.json projects to exclude
/// - `tints`: per-path tint overrides
pub fn load_projects_extended(
    added: &[crate::warpui::persist::AddedProject],
    removed: &[String],
    tints: &HashMap<String, [u8; 3]>,
) -> Vec<ProjectNode> {
    let mut projects = load_projects();
    projects.retain(|p| !removed.contains(&p.path));
    for p in &mut projects {
        p.tint = tints.get(&p.path).copied();
    }
    for ap in added {
        if !projects.iter().any(|p| p.path == ap.path) {
            let is_loose = !std::path::Path::new(&ap.path).join(".git").exists();
            projects.push(ProjectNode {
                name: ap.name.clone(),
                path: ap.path.clone(),
                worktrees: Vec::new(),
                tint: tints.get(&ap.path).copied(),
                is_loose,
                group_path: None,
            });
        }
    }
    // Derive folder groups on the FINAL list (session + added projects) so a
    // newly added project can join an existing parent-folder group.
    assign_groups(&mut projects);
    projects
}
