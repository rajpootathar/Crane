use crate::state::{
    App, Project, RightTab, Tab, Worktree, WorktreeId, TabId, ProjectId,
};
use crate::workspace::{
    BrowserPane, DiffPane, Dir, FileTab, FilesPane, MarkdownPane, Node, Pane, PaneContent,
    PaneId, Workspace,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub projects: Vec<SProject>,
    pub active: Option<(ProjectId, WorktreeId, TabId)>,
    pub last_worktree: Option<(ProjectId, WorktreeId)>,
    pub show_left: bool,
    pub show_right: bool,
    pub right_tab: String,
    pub font_size: f32,
    pub collapsed_change_dirs: Vec<String>,
    pub expanded_dirs: Vec<PathBuf>,
    pub commit_message: String,
    pub next_project: ProjectId,
    pub next_worktree: WorktreeId,
    pub next_tab: TabId,
}

#[derive(Serialize, Deserialize)]
pub struct SProject {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub expanded: bool,
    pub worktrees: Vec<SWorktree>,
}

#[derive(Serialize, Deserialize)]
pub struct SWorktree {
    pub id: WorktreeId,
    pub name: String,
    pub path: PathBuf,
    pub expanded: bool,
    pub active_tab: Option<TabId>,
    pub tabs: Vec<STab>,
}

#[derive(Serialize, Deserialize)]
pub struct STab {
    pub id: TabId,
    pub name: String,
    pub layout: Option<SNode>,
    pub focus: Option<PaneId>,
    pub next_pane_id: PaneId,
    pub panes: Vec<SPane>,
}

#[derive(Serialize, Deserialize)]
pub enum SNode {
    Leaf(PaneId),
    Split {
        direction: String,
        first: Box<SNode>,
        second: Box<SNode>,
        ratio: f32,
    },
}

#[derive(Serialize, Deserialize)]
pub struct SPane {
    pub id: PaneId,
    pub title: String,
    pub content: SPaneContent,
}

#[derive(Serialize, Deserialize)]
pub enum SPaneContent {
    Terminal {
        cwd: PathBuf,
        history_b64: String,
    },
    Files {
        files: Vec<SFile>,
        active: usize,
    },
    Markdown {
        path: String,
    },
    Diff {
        left_path: String,
        right_path: String,
    },
    Browser {
        url: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct SFile {
    pub path: String,
    pub name: String,
}

pub fn session_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.config/crane/session.json"))
}

pub fn save(app: &App) -> std::io::Result<()> {
    let s = Session::from_app(app);
    let bytes = serde_json::to_vec_pretty(&s)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let path = session_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load() -> Option<Session> {
    let bytes = std::fs::read(session_file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

impl Session {
    pub fn from_app(app: &App) -> Self {
        let mut projects = Vec::with_capacity(app.projects.len());
        for p in &app.projects {
            let mut worktrees = Vec::with_capacity(p.worktrees.len());
            for w in &p.worktrees {
                let mut tabs = Vec::with_capacity(w.tabs.len());
                for t in &w.tabs {
                    tabs.push(STab::from_tab(t));
                }
                worktrees.push(SWorktree {
                    id: w.id,
                    name: w.name.clone(),
                    path: w.path.clone(),
                    expanded: w.expanded,
                    active_tab: w.active_tab,
                    tabs,
                });
            }
            projects.push(SProject {
                id: p.id,
                name: p.name.clone(),
                path: p.path.clone(),
                expanded: p.expanded,
                worktrees,
            });
        }

        let right_tab = match app.right_tab {
            RightTab::Changes => "changes",
            RightTab::Files => "files",
        }
        .to_string();

        Session {
            version: 1,
            projects,
            active: app.active,
            last_worktree: app.last_worktree,
            show_left: app.show_left,
            show_right: app.show_right,
            right_tab,
            font_size: app.font_size,
            collapsed_change_dirs: app.collapsed_change_dirs.iter().cloned().collect(),
            expanded_dirs: app.expanded_dirs.iter().cloned().collect(),
            commit_message: app.commit_message.clone(),
            next_project: app.next_project_id(),
            next_worktree: app.next_worktree_id(),
            next_tab: app.next_tab_id(),
        }
    }

    pub fn restore(self, ctx: &egui::Context) -> App {
        let mut app = App::new();
        app.show_left = self.show_left;
        app.show_right = self.show_right;
        app.right_tab = match self.right_tab.as_str() {
            "files" => RightTab::Files,
            _ => RightTab::Changes,
        };
        app.font_size = self.font_size;
        app.commit_message = self.commit_message;
        app.collapsed_change_dirs = self.collapsed_change_dirs.into_iter().collect();
        app.expanded_dirs = self.expanded_dirs.into_iter().collect();

        for sp in self.projects {
            let mut worktrees = Vec::with_capacity(sp.worktrees.len());
            for sw in sp.worktrees {
                let mut tabs = Vec::with_capacity(sw.tabs.len());
                for st in sw.tabs {
                    tabs.push(st.into_tab(ctx, &sw.path));
                }
                worktrees.push(Worktree {
                    id: sw.id,
                    name: sw.name,
                    path: sw.path,
                    expanded: sw.expanded,
                    active_tab: sw.active_tab,
                    tabs,
                    git_status: None,
                    last_status_refresh: None,
                    git_rx: None,
                });
            }
            app.projects.push(Project {
                id: sp.id,
                name: sp.name,
                path: sp.path,
                expanded: sp.expanded,
                worktrees,
            });
        }

        app.active = self.active;
        app.last_worktree = self.last_worktree;
        app.set_id_counters(self.next_project, self.next_worktree, self.next_tab);
        app
    }
}

impl STab {
    fn from_tab(t: &Tab) -> Self {
        let panes: Vec<SPane> = t
            .workspace
            .panes
            .iter()
            .map(|(id, p)| SPane::from_pane(*id, p))
            .collect();
        STab {
            id: t.id,
            name: t.name.clone(),
            layout: t.workspace.root.as_ref().map(SNode::from_node),
            focus: t.workspace.focus,
            next_pane_id: t.workspace.next_pane_id(),
            panes,
        }
    }

    fn into_tab(self, ctx: &egui::Context, cwd: &Path) -> Tab {
        let mut workspace = Workspace::new(cwd.to_path_buf());
        for sp in self.panes {
            let (id, pane) = sp.into_pane(ctx, cwd);
            workspace.panes.insert(id, pane);
        }
        workspace.root = self.layout.map(|n| n.into_node());
        workspace.focus = self.focus;
        workspace.set_next_pane_id(self.next_pane_id);
        Tab {
            id: self.id,
            name: self.name,
            workspace,
        }
    }
}

impl SNode {
    fn from_node(n: &Node) -> Self {
        match n {
            Node::Leaf(id) => SNode::Leaf(*id),
            Node::Split {
                direction,
                first,
                second,
                ratio,
            } => SNode::Split {
                direction: match direction {
                    Dir::Horizontal => "h".into(),
                    Dir::Vertical => "v".into(),
                },
                first: Box::new(SNode::from_node(first)),
                second: Box::new(SNode::from_node(second)),
                ratio: *ratio,
            },
        }
    }

    fn into_node(self) -> Node {
        match self {
            SNode::Leaf(id) => Node::Leaf(id),
            SNode::Split {
                direction,
                first,
                second,
                ratio,
            } => Node::Split {
                direction: if direction == "v" {
                    Dir::Vertical
                } else {
                    Dir::Horizontal
                },
                first: Box::new(first.into_node()),
                second: Box::new(second.into_node()),
                ratio,
            },
        }
    }
}

impl SPane {
    fn from_pane(id: PaneId, p: &Pane) -> Self {
        let content = match &p.content {
            PaneContent::Terminal(t) => {
                let bytes = t.history_snapshot();
                SPaneContent::Terminal {
                    cwd: t.cwd.clone(),
                    history_b64: base64_encode(&bytes),
                }
            }
            PaneContent::Files(f) => SPaneContent::Files {
                files: f
                    .tabs
                    .iter()
                    .map(|ft| SFile {
                        path: ft.path.clone(),
                        name: ft.name.clone(),
                    })
                    .collect(),
                active: f.active,
            },
            PaneContent::Markdown(m) => SPaneContent::Markdown {
                path: m.path.clone(),
            },
            PaneContent::Diff(d) => SPaneContent::Diff {
                left_path: d.left_path.clone(),
                right_path: d.right_path.clone(),
            },
            PaneContent::Browser(b) => SPaneContent::Browser {
                url: b.url.clone(),
            },
        };
        SPane {
            id,
            title: p.title.clone(),
            content,
        }
    }

    fn into_pane(self, ctx: &egui::Context, cwd: &Path) -> (PaneId, Pane) {
        let content = match self.content {
            SPaneContent::Terminal { cwd: saved_cwd, history_b64: _ } => {
                let spawn_cwd = if saved_cwd.as_os_str().is_empty() {
                    cwd
                } else {
                    saved_cwd.as_path()
                };
                match crate::terminal::Terminal::spawn(
                    ctx.clone(),
                    80,
                    24,
                    Some(spawn_cwd),
                ) {
                    Ok(t) => PaneContent::Terminal(t),
                    Err(_) => PaneContent::Files(FilesPane::empty()),
                }
            }
            SPaneContent::Files { files, active } => {
                let tabs: Vec<FileTab> = files
                    .into_iter()
                    .map(|sf| {
                        let content = std::fs::read_to_string(&sf.path).unwrap_or_default();
                        FileTab {
                            path: sf.path,
                            name: sf.name,
                            original_content: content.clone(),
                            content,
                        }
                    })
                    .collect();
                let len = tabs.len();
                PaneContent::Files(FilesPane {
                    tabs,
                    active: if len == 0 { 0 } else { active.min(len - 1) },
                    input_buf: String::new(),
                    error: None,
                })
            }
            SPaneContent::Markdown { path } => {
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                PaneContent::Markdown(MarkdownPane {
                    path,
                    content,
                    input_buf: String::new(),
                    error: None,
                })
            }
            SPaneContent::Diff {
                left_path,
                right_path,
            } => {
                let right_text = std::fs::read_to_string(&right_path).unwrap_or_default();
                let left_text = if let Some(rel) = left_path.strip_prefix("HEAD:") {
                    crate::git::head_content(cwd, rel)
                } else {
                    std::fs::read_to_string(&left_path).unwrap_or_default()
                };
                PaneContent::Diff(DiffPane {
                    left_path,
                    right_path,
                    left_text,
                    right_text,
                    left_buf: String::new(),
                    right_buf: String::new(),
                    error: None,
                })
            }
            SPaneContent::Browser { url } => PaneContent::Browser(BrowserPane {
                url: url.clone(),
                input_buf: url,
            }),
        };
        (
            self.id,
            Pane {
                id: self.id,
                title: self.title,
                content,
            },
        )
    }
}

// --- Tiny base64 helpers (stdlib-only — no extra dep) ---

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | input[i + 2] as u32;
        out.push(B64[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64[((n >> 6) & 0x3f) as usize] as char);
        out.push(B64[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(B64[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64[((n >> 12) & 0x3f) as usize] as char);
        out.push_str("==");
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(B64[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut lookup = [255u8; 256];
    for (i, &b) in B64.iter().enumerate() {
        lookup[b as usize] = i as u8;
    }
    let bytes: Vec<u8> = input.bytes().filter(|b| *b != b'=' && !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4 + 3);
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let a = lookup[bytes[i] as usize];
        let b = lookup[bytes[i + 1] as usize];
        let c = lookup[bytes[i + 2] as usize];
        let d = lookup[bytes[i + 3] as usize];
        if a == 255 || b == 255 || c == 255 || d == 255 {
            return None;
        }
        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | d as u32;
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
        i += 4;
    }
    match bytes.len() - i {
        0 => {}
        2 => {
            let a = lookup[bytes[i] as usize];
            let b = lookup[bytes[i + 1] as usize];
            if a == 255 || b == 255 {
                return None;
            }
            let n = ((a as u32) << 18) | ((b as u32) << 12);
            out.push((n >> 16) as u8);
        }
        3 => {
            let a = lookup[bytes[i] as usize];
            let b = lookup[bytes[i + 1] as usize];
            let c = lookup[bytes[i + 2] as usize];
            if a == 255 || b == 255 || c == 255 {
                return None;
            }
            let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6);
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => return None,
    }
    Some(out)
}

pub const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);
