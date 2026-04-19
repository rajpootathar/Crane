use crate::state::{
    App, Project, RightTab, Tab, Workspace, WorkspaceId, TabId, ProjectId,
};
use crate::update_check::{PromptState, UpdateCheck};
use crate::layout::{
    self, BrowserPane, DiffPane, Dir, FileTab, FilesPane, Layout, MarkdownPane, Node, Pane,
    PaneContent, PaneId,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub projects: Vec<SProject>,
    pub active: Option<(ProjectId, WorkspaceId, TabId)>,
    pub last_workspace: Option<(ProjectId, WorkspaceId)>,
    pub show_left: bool,
    pub show_right: bool,
    pub right_tab: String,
    pub font_size: f32,
    pub collapsed_change_dirs: Vec<String>,
    pub expanded_dirs: Vec<PathBuf>,
    pub commit_message: String,
    pub next_project: ProjectId,
    pub next_workspace: WorkspaceId,
    pub next_tab: TabId,
    #[serde(default)]
    pub update_prompts: std::collections::HashMap<String, PromptState>,
    #[serde(default = "default_theme_name")]
    pub selected_theme: String,
    #[serde(default)]
    pub custom_mono_font: Option<String>,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    #[serde(default)]
    pub syntax_theme_override: Option<String>,
    #[serde(default = "default_left_w")]
    pub left_panel_w: f32,
    #[serde(default = "default_right_w")]
    pub right_panel_w: f32,
    #[serde(default)]
    pub language_configs: crate::lsp::LanguageConfigs,
}

fn default_left_w() -> f32 {
    240.0
}
fn default_right_w() -> f32 {
    300.0
}

fn default_ui_scale() -> f32 {
    1.0
}

fn default_theme_name() -> String {
    "crane-dark".into()
}

#[derive(Serialize, Deserialize)]
pub struct SProject {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub expanded: bool,
    pub workspaces: Vec<SWorkspace>,
    #[serde(default)]
    pub last_active_workspace: Option<WorkspaceId>,
    #[serde(default)]
    pub preferred_location_mode: Option<String>,
    #[serde(default)]
    pub preferred_custom_path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
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
    PathBuf::from(format!("{home}/.crane/session.json"))
}

pub fn load() -> Option<Session> {
    let bytes = std::fs::read(session_file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

impl Session {
    pub fn from_app(app: &App) -> Self {
        let mut projects = Vec::with_capacity(app.projects.len());
        for p in &app.projects {
            let mut workspaces = Vec::with_capacity(p.workspaces.len());
            for w in &p.workspaces {
                let mut tabs = Vec::with_capacity(w.tabs.len());
                for t in &w.tabs {
                    tabs.push(STab::from_tab(t));
                }
                workspaces.push(SWorkspace {
                    id: w.id,
                    name: w.name.clone(),
                    display_name: w.display_name.clone(),
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
                workspaces,
                last_active_workspace: p.last_active_workspace,
                preferred_location_mode: p.preferred_location_mode.map(|m| m.as_str().to_string()),
                preferred_custom_path: p.preferred_custom_path.clone(),
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
            last_workspace: app.last_workspace,
            show_left: app.show_left,
            show_right: app.show_right,
            right_tab,
            font_size: app.font_size,
            collapsed_change_dirs: app.collapsed_change_dirs.iter().cloned().collect(),
            expanded_dirs: app.expanded_dirs.iter().cloned().collect(),
            commit_message: app.commit_message.clone(),
            next_project: app.next_project_id(),
            next_workspace: app.next_workspace_id(),
            next_tab: app.next_tab_id(),
            update_prompts: app.update_check.prompts.clone(),
            selected_theme: app.selected_theme.clone(),
            custom_mono_font: app.custom_mono_font.clone(),
            ui_scale: app.ui_scale,
            syntax_theme_override: app.syntax_theme_override.clone(),
            left_panel_w: app.left_panel_w,
            right_panel_w: app.right_panel_w,
            language_configs: app.language_configs.clone(),
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
            let mut workspaces = Vec::with_capacity(sp.workspaces.len());
            for sw in sp.workspaces {
                let mut tabs = Vec::with_capacity(sw.tabs.len());
                for st in sw.tabs {
                    tabs.push(st.into_tab(ctx, &sw.path));
                }
                workspaces.push(Workspace {
                    id: sw.id,
                    name: sw.name,
                    display_name: sw.display_name,
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
                workspaces,
                last_active_workspace: sp.last_active_workspace,
                preferred_location_mode: sp
                    .preferred_location_mode
                    .as_deref()
                    .and_then(crate::state::LocationMode::parse),
                preferred_custom_path: sp.preferred_custom_path,
            });
        }

        app.active = self.active;
        app.last_workspace = self.last_workspace;
        app.set_id_counters(self.next_project, self.next_workspace, self.next_tab);
        app.update_check = UpdateCheck::new(self.update_prompts);
        app.selected_theme = self.selected_theme;
        app.custom_mono_font = self.custom_mono_font;
        app.ui_scale = self.ui_scale.clamp(0.75, 1.5);
        app.syntax_theme_override = self.syntax_theme_override;
        app.left_panel_w = self.left_panel_w.clamp(180.0, 600.0);
        app.right_panel_w = self.right_panel_w.clamp(200.0, 700.0);
        app.language_configs = self.language_configs;
        app
    }
}

impl STab {
    fn from_tab(t: &Tab) -> Self {
        // Diff panes are transient by design — never persist them. Prune their
        // IDs from both the pane map and the layout tree so we never restore
        // with an empty Diff pane hanging around.
        let diff_ids: Vec<PaneId> = t
            .layout
            .panes
            .iter()
            .filter(|(_, p)| matches!(p.content, PaneContent::Diff(_)))
            .map(|(id, _)| *id)
            .collect();

        let panes: Vec<SPane> = t
            .layout
            .panes
            .iter()
            .filter(|(id, _)| !diff_ids.contains(id))
            .map(|(id, p)| SPane::from_pane(*id, p))
            .collect();

        let mut pruned_root = t.layout.root.clone();
        for id in &diff_ids {
            if let Some(root) = pruned_root.take() {
                let (new_root, _) = layout::prune_leaf(root, *id);
                pruned_root = new_root;
            }
        }
        let pruned_focus = t.layout.focus.filter(|f| !diff_ids.contains(f));

        STab {
            id: t.id,
            name: t.name.clone(),
            layout: pruned_root.as_ref().map(SNode::from_node),
            focus: pruned_focus,
            next_pane_id: t.layout.next_pane_id(),
            panes,
        }
    }

    fn into_tab(self, ctx: &egui::Context, cwd: &Path) -> Tab {
        let mut layout = Layout::new(cwd.to_path_buf());
        for sp in self.panes {
            let (id, pane) = sp.into_pane(ctx, cwd);
            layout.panes.insert(id, pane);
        }
        layout.root = self.layout.map(|n| n.into_node());
        layout.focus = self.focus;
        layout.set_next_pane_id(self.next_pane_id);
        Tab {
            id: self.id,
            name: self.name,
            layout,
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
            SPaneContent::Terminal { cwd: saved_cwd, history_b64 } => {
                let spawn_cwd = if saved_cwd.as_os_str().is_empty() {
                    cwd
                } else {
                    saved_cwd.as_path()
                };
                // Replay the saved scrollback if we have it — decoding
                // failures fall back to a fresh terminal rather than
                // blocking the whole session restore.
                let history = base64_decode(&history_b64).unwrap_or_default();
                let spawned = if history.is_empty() {
                    crate::terminal::Terminal::spawn(ctx.clone(), 80, 24, Some(spawn_cwd))
                } else {
                    crate::terminal::Terminal::spawn_with_history(
                        ctx.clone(),
                        80,
                        24,
                        Some(spawn_cwd),
                        &history,
                    )
                };
                match spawned {
                    Ok(t) => PaneContent::Terminal(t),
                    Err(_) => PaneContent::Files(FilesPane::empty()),
                }
            }
            SPaneContent::Files { files, active } => {
                let tabs: Vec<FileTab> = files
                    .into_iter()
                    .map(|sf| {
                        let content = std::fs::read_to_string(&sf.path).unwrap_or_default();
                        let disk_mtime = std::fs::metadata(&sf.path)
                            .and_then(|m| m.modified())
                            .ok();
                        FileTab {
                            path: sf.path,
                            name: sf.name,
                            original_content: content.clone(),
                            last_lsp_content: content.clone(),
                            last_lsp_sent_at: None,
                            preview_mode: false,
                            pending_cursor: None,
                            image_texture: None,
                            find_query: None,
                            disk_mtime,
                            external_change: false,
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
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
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
