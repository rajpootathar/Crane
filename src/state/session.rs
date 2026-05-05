use crate::state::{
    App, Project, RightTab, Tab, Workspace, WorkspaceId, TabId, ProjectId,
};
use crate::update::check::{PromptState, UpdateCheck};
use crate::state::layout::{
    BrowserPane, Dir, FileTab, FilesPane, Layout, MarkdownPane, Node, Pane,
    PaneContent, PaneId, TabKind,
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
    /// Persisted folder-group tints, keyed by the group's parent path.
    /// Serialized as a Vec<(PathBuf, [u8; 3])> to survive JSON's lack
    /// of non-string map keys. Missing → empty map on restore.
    #[serde(default)]
    pub group_tints: Vec<(PathBuf, [u8; 3])>,
    /// Persisted folder-group collapse state. Only collapsed groups
    /// are stored (default is expanded, so an empty list restores
    /// with everything open).
    #[serde(default)]
    pub group_collapsed: Vec<PathBuf>,
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
    #[serde(default)]
    pub group_path: Option<PathBuf>,
    #[serde(default)]
    pub group_name: Option<String>,
    #[serde(default)]
    pub tint: Option<[u8; 3]>,
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
    #[serde(default)]
    pub tint: Option<[u8; 3]>,
}

#[derive(Serialize, Deserialize)]
pub struct STab {
    pub id: TabId,
    pub name: String,
    pub layout: Option<SNode>,
    pub focus: Option<PaneId>,
    pub next_pane_id: PaneId,
    pub panes: Vec<SPane>,
    #[serde(default)]
    pub tint: Option<[u8; 3]>,
    #[serde(default)]
    pub git_log_visible: bool,
    #[serde(default)]
    pub git_log_state: Option<SGitLogState>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct SGitLogState {
    #[serde(default = "default_git_log_height")]
    pub height: f32,
    #[serde(default = "default_git_log_col_refs")]
    pub col_refs_width: f32,
    #[serde(default = "default_git_log_col_details")]
    pub col_details_width: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub selected_commit: Option<String>,
    #[serde(default)]
    pub selected_file: Option<String>,
}

fn default_git_log_height() -> f32 { 320.0 }
fn default_git_log_col_refs() -> f32 { 220.0 }
fn default_git_log_col_details() -> f32 { 360.0 }

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
        /// Legacy single-tab fields — populated for old session files.
        /// Newer writes use `tabs` + `active` below and leave these
        /// empty. Restore reads the legacy fields only when `tabs` is
        /// missing/empty.
        #[serde(default, skip_serializing_if = "path_is_empty")]
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        history_text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        history_b64: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tabs: Vec<STerminalTab>,
        #[serde(default)]
        active: usize,
    },
    Files {
        files: Vec<SFile>,
        active: usize,
    },
    Markdown {
        path: String,
    },
    Browser {
        /// Legacy single-URL field — populated for old session files.
        /// Newer writes use `tabs` + `active` below.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        url: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tabs: Vec<SBrowserTab>,
        #[serde(default)]
        active: usize,
    },
    /// Landing-page pane. Stateless — just restores an empty Welcome
    /// surface so the user lands back on the same screen they closed.
    Welcome,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SBrowserTab {
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct STerminalTab {
    pub cwd: PathBuf,
    #[serde(default)]
    pub history_text: String,
    /// User-set display name for the tab chip. Empty string means
    /// "no override" — restore lands `None` in that case so the cwd
    /// basename shows again.
    #[serde(default)]
    pub name: String,
}

fn path_is_empty(p: &PathBuf) -> bool {
    p.as_os_str().is_empty()
}

#[derive(Serialize, Deserialize)]
pub struct SFile {
    pub path: String,
    pub name: String,
}

pub fn session_file() -> PathBuf {
    crate::util::home_dir()
        .map(|h| h.join(".crane").join("session.json"))
        .unwrap_or_default()
}

pub fn load() -> Option<Session> {
    let path = session_file();
    // Try the live file first, then the last-known-good .bak (written
    // by maybe_save before each atomic replace). Without the fallback,
    // a corrupt or partial session.json silently wipes every project
    // the user has added.
    let candidates = [path.clone(), path.with_extension("json.bak")];
    for candidate in candidates {
        let Ok(bytes) = std::fs::read(&candidate) else {
            continue;
        };
        if let Ok(session) = serde_json::from_slice::<Session>(&bytes) {
            return Some(session);
        }
        eprintln!(
            "[session] failed to parse {} — trying fallback",
            candidate.display()
        );
    }
    None
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
                    tint: w.tint,
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
                group_path: p.group_path.clone(),
                group_name: p.group_name.clone(),
                tint: p.tint,
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
            group_tints: app.group_tints.iter().map(|(p, c)| (p.clone(), *c)).collect(),
            group_collapsed: app.group_collapsed.iter().cloned().collect(),
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
                    tint: sw.tint,
                });
            }
            // Stat the folder on restore. Missing projects still load
            // (so they keep their ids + expanded state) but all git /
            // LSP / terminal / worktree actions skip them until the
            // user relocates or removes via the modal.
            let missing = !sp.path.exists();
            app.projects.push(Project {
                id: sp.id,
                name: sp.name,
                path: sp.path,
                missing,
                expanded: sp.expanded,
                workspaces,
                last_active_workspace: sp.last_active_workspace,
                preferred_location_mode: sp
                    .preferred_location_mode
                    .as_deref()
                    .and_then(crate::state::LocationMode::parse),
                preferred_custom_path: sp.preferred_custom_path,
                group_path: sp.group_path,
                group_name: sp.group_name,
                tint: sp.tint,
            });
            if missing {
                app.missing_project_modals.push(sp.id);
            }
        }

        // Sanitize cursors: a saved (pid, wid, tid) might reference a
        // project/workspace/tab that no longer exists (e.g., the user
        // ran `git worktree remove` outside Crane, or we pruned a
        // missing project above). Dangling ids used to surface as
        // blank panels / panics on first interaction; now we drop
        // them cleanly.
        if let Some((pid, wid, tid)) = self.active
            && !app.projects.iter().any(|p| {
                p.id == pid && p.workspaces.iter().any(|w| {
                    w.id == wid && w.tabs.iter().any(|t| t.id == tid)
                })
            })
        {
            app.active = None;
        } else {
            app.active = self.active;
        }
        if let Some((pid, wid)) = self.last_workspace
            && !app
                .projects
                .iter()
                .any(|p| p.id == pid && p.workspaces.iter().any(|w| w.id == wid))
        {
            app.last_workspace = None;
        } else {
            app.last_workspace = self.last_workspace;
        }
        app.set_id_counters(self.next_project, self.next_workspace, self.next_tab);
        app.update_check = UpdateCheck::new(self.update_prompts);
        app.selected_theme = self.selected_theme;
        app.custom_mono_font = self.custom_mono_font;
        app.ui_scale = self.ui_scale.clamp(0.75, 1.5);
        app.syntax_theme_override = self.syntax_theme_override;
        app.left_panel_w = self.left_panel_w.clamp(180.0, 600.0);
        app.right_panel_w = self.right_panel_w.clamp(200.0, 700.0);
        app.language_configs = self.language_configs;
        app.group_tints = self.group_tints.into_iter().collect();
        app.group_collapsed = self.group_collapsed.into_iter().collect();
        // Re-probe disk for repos / worktrees / sub-clones added outside
        // Crane since the last session — picks up newly-cloned siblings,
        // `git worktree add` branches, and `git init`-after-the-fact
        // directories. See `App::reindex_git_state`.
        app.reindex_git_state(ctx);
        app
    }
}

impl STab {
    fn from_tab(t: &Tab) -> Self {
        // Diff tabs are transient — never persist them. They live inside
        // FilesPanes as TabKind::Diff entries. We prune them from the
        // serialized tab list and adjust active index accordingly.
        let panes: Vec<SPane> = t
            .layout
            .panes
            .iter()
            .map(|(id, p)| SPane::from_pane(*id, p))
            .collect();

        STab {
            id: t.id,
            name: t.name.clone(),
            layout: t.layout.root.as_ref().map(SNode::from_node),
            focus: t.layout.focus,
            next_pane_id: t.layout.next_pane_id(),
            panes,
            tint: t.tint,
            git_log_visible: t.git_log_visible,
            git_log_state: t.git_log_state.as_ref().map(|s| SGitLogState {
                height: s.height,
                col_refs_width: s.col_refs_width,
                col_details_width: s.col_details_width,
                maximized: s.maximized,
                selected_commit: s.selected_commit.clone(),
                selected_file: s.selected_file.as_ref().map(|p| p.to_string_lossy().to_string()),
            }),
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
            tint: self.tint,
            git_log_visible: self.git_log_visible,
            git_log_state: self.git_log_state.map(|s| crate::git_log::GitLogState {
                height: s.height,
                col_refs_width: s.col_refs_width,
                col_details_width: s.col_details_width,
                maximized: s.maximized,
                selected_commit: s.selected_commit,
                selected_file: s.selected_file.map(std::path::PathBuf::from),
                last_poll: std::time::Instant::now(),
                frame: None,
                generation: 0,
                worker_rx: None,
                filter: crate::git_log::state::FilterState::default(),
                watcher: None,
                fetch_in_flight: std::sync::Arc::new(
                    std::sync::atomic::AtomicBool::new(false),
                ),
                watched_repo: None,
            }),
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
            PaneContent::Terminal(tp) => {
                // Rendered-grid ANSI snapshot instead of the raw PTY
                // byte log — replaying raw bytes does not survive
                // width changes (shell prompts use absolute cursor-
                // positioning escapes baked against the original
                // width). The ANSI form preserves every cell's
                // colors and SGR flags so a restored session looks
                // visually identical to the saved one. Older sessions
                // saved as plain text still load — `history_text` is
                // fed through the parser either way, plain text just
                // produces default-styled cells.
                let tabs: Vec<STerminalTab> = tp
                    .tabs
                    .iter()
                    .map(|t| STerminalTab {
                        cwd: t.terminal.cwd.clone(),
                        history_text: t.terminal.snapshot_ansi(),
                        name: t.name.clone().unwrap_or_default(),
                    })
                    .collect();
                SPaneContent::Terminal {
                    cwd: PathBuf::new(),
                    history_text: String::new(),
                    history_b64: String::new(),
                    tabs,
                    active: tp.active,
                }
            }
            PaneContent::Files(f) => {
                // Only persist file tabs — diff tabs are ephemeral.
                // Adjust active index to account for removed diff tabs.
                let mut adjusted_active = f.active;
                let files: Vec<SFile> = f
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, tk)| {
                        if let TabKind::File(ft) = tk {
                            Some(SFile {
                                path: ft.path.clone(),
                                name: ft.name.clone(),
                            })
                        } else {
                            if i < adjusted_active {
                                adjusted_active = adjusted_active.saturating_sub(1);
                            }
                            None
                        }
                    })
                    .collect();
                SPaneContent::Files {
                    files,
                    active: adjusted_active,
                }
            }
            PaneContent::Markdown(m) => SPaneContent::Markdown {
                path: m.path.clone(),
            },
            PaneContent::Browser(b) => SPaneContent::Browser {
                url: String::new(),
                tabs: b
                    .tabs
                    .iter()
                    .map(|t| SBrowserTab { url: t.url.clone() })
                    .collect(),
                active: b.active,
            },
            PaneContent::Welcome(_) => SPaneContent::Welcome,
        };
        SPane {
            id,
            title: p.title.clone(),
            content,
        }
    }

    fn into_pane(self, ctx: &egui::Context, cwd: &Path) -> (PaneId, Pane) {
        let content = match self.content {
            SPaneContent::Terminal {
                cwd: saved_cwd,
                history_text,
                history_b64,
                tabs,
                active,
            } => {
                // New format (tabs vec) wins. Fall back to the legacy
                // single-tab fields when `tabs` is empty — keeps old
                // session.json files restoring without surprise.
                let saved_tabs: Vec<STerminalTab> = if !tabs.is_empty() {
                    tabs
                } else {
                    let legacy_text = if !history_text.is_empty() {
                        history_text.clone()
                    } else if !history_b64.is_empty() {
                        let raw = base64_decode(&history_b64).unwrap_or_default();
                        String::from_utf8_lossy(&strip_ansi(&raw)).into_owned()
                    } else {
                        String::new()
                    };
                    vec![STerminalTab {
                        cwd: saved_cwd.clone(),
                        history_text: legacy_text,
                        name: String::new(),
                    }]
                };

                let spawned: Vec<crate::state::layout::TerminalTab> = saved_tabs
                    .into_iter()
                    .filter_map(|st| {
                        let spawn_cwd: &Path = if st.cwd.as_os_str().is_empty() {
                            cwd
                        } else {
                            st.cwd.as_path()
                        };
                        let result = if st.history_text.is_empty() {
                            crate::terminal::Terminal::spawn(
                                ctx.clone(),
                                80,
                                24,
                                Some(spawn_cwd),
                            )
                        } else {
                            crate::terminal::Terminal::spawn_with_text_history(
                                ctx.clone(),
                                80,
                                24,
                                Some(spawn_cwd),
                                &st.history_text,
                            )
                        };
                        result.ok().map(|term| crate::state::layout::TerminalTab {
                            terminal: term,
                            name: if st.name.trim().is_empty() {
                                None
                            } else {
                                Some(st.name)
                            },
                        })
                    })
                    .collect();
                if spawned.is_empty() {
                    PaneContent::Files(FilesPane::empty())
                } else {
                    let active_idx = active.min(spawned.len().saturating_sub(1));
                    PaneContent::Terminal(crate::state::layout::TerminalPane {
                        tabs: spawned,
                        active: active_idx,
                        renaming: None,
                    })
                }
            }
            SPaneContent::Files { files, active } => {
                let tabs: Vec<TabKind> = files
                    .into_iter()
                    .map(|sf| {
                        let content = std::fs::read_to_string(&sf.path).unwrap_or_default();
                        let disk_mtime = std::fs::metadata(&sf.path)
                            .and_then(|m| m.modified())
                            .ok();
                        TabKind::File(FileTab {
                            path: sf.path,
                            name: sf.name,
                            original_content: content.clone(),
                            last_lsp_content: content.clone(),
                            last_lsp_sent_at: None,
                            preview_mode: false,
                            pending_cursor: None,
                            image_texture: None,
                            find_query: None,
                            find_scroll_to_line: None,
                            disk_mtime,
                            external_change: false,
                            last_cursor_idx: 0,
                            line_changes: None,
                            line_changes_key: 0,
                            goto_line_active: false,
                            goto_line_input: String::new(),
                            replace_query: String::new(),
                            show_replace: false,
                            selection_info: None,
                            save_error: None,
                            preview: false,
                            content,
                            read_only: false,
                            pdf_state: None,
                        })
                    })
                    .collect();
                let len = tabs.len();
                PaneContent::Files(FilesPane {
                    tabs,
                    active: if len == 0 { 0 } else { active.min(len - 1) },
                    input_buf: String::new(),
                    error: None,
                    pending_close: None,
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
            SPaneContent::Browser { url, tabs, active } => {
                if tabs.is_empty() {
                    PaneContent::Browser(BrowserPane::new_with(
                        url.clone(),
                        url,
                    ))
                } else {
                    let mut bp = BrowserPane::new_with(
                        tabs[0].url.clone(),
                        tabs[0].url.clone(),
                    );
                    for extra in tabs.iter().skip(1) {
                        bp.new_tab_with(extra.url.clone());
                    }
                    bp.active = active.min(bp.tabs.len().saturating_sub(1));
                    PaneContent::Browser(bp)
                }
            }
            SPaneContent::Welcome => {
                PaneContent::Welcome(crate::state::layout::WelcomePane)
            }
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

#[allow(dead_code)] // paired with base64_decode; retained for legacy session read path
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

/// Remove ANSI escape sequences from `input`. Preserves text + newlines
/// + tabs. Strips CSI (`ESC [ … letter`), OSC (`ESC ] … BEL or ESC \`),
/// simple ESC sequences (`ESC letter`), single-char `\x07` bell, and
/// `\r` carriage returns (which would overwrite previous content on
/// replay).
fn strip_ansi(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        if b == 0x1b && i + 1 < input.len() {
            let next = input[i + 1];
            match next {
                b'[' => {
                    // CSI: ESC [ params* intermediates* final-byte
                    i += 2;
                    while i < input.len() {
                        let c = input[i];
                        i += 1;
                        if c.is_ascii_alphabetic() || c == b'~' {
                            break;
                        }
                    }
                }
                b']' => {
                    // OSC: ESC ] … (BEL | ESC \\)
                    i += 2;
                    while i < input.len() {
                        let c = input[i];
                        if c == 0x07 {
                            i += 1;
                            break;
                        }
                        if c == 0x1b && input.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                b'P' | b'X' | b'^' | b'_' => {
                    // DCS / SOS / PM / APC: until ESC \\
                    i += 2;
                    while i < input.len() {
                        if input[i] == 0x1b && input.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Two-byte ESC sequence (ESC letter).
                    i += 2;
                }
            }
            continue;
        }
        // Drop only the bell (0x07) — CR must stay or lines collapse
        // together. LF alone moves the cursor down but keeps the
        // column, so stripping CR from "\r\n" endings piled every line
        // at an increasing indent.
        if b == 0x07 {
            i += 1;
            continue;
        }
        out.push(b);
        i += 1;
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
