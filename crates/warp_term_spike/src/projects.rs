//! Reads the REAL Crane project tree from `~/.crane/session.json` so the
//! warpui shell shows the user's actual projects / worktrees / tabs — proving
//! the existing Crane logic + persistence is consumed unchanged; only the GUI
//! is new. Parsed via serde_json::Value to avoid importing the crane crate's
//! full session schema.

pub struct WorktreeNode {
    pub name: String,
    pub tabs: Vec<String>,
}

pub struct ProjectNode {
    pub name: String,
    pub worktrees: Vec<WorktreeNode>,
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
                    worktrees.push(WorktreeNode { name: wname, tabs });
                }
            }
            out.push(ProjectNode { name, worktrees });
        }
    }
    out
}
