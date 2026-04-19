use crate::git::{self, GitStatus};
use crate::layout::Layout;
use crate::update_check::UpdateCheck;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn shellexpand_home(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    s.to_string()
}

pub type ProjectId = u64;
pub type WorkspaceId = u64;
pub type TabId = u64;

pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: Layout,
}

pub struct Workspace {
    pub id: WorkspaceId,
    /// Canonical name — the branch / worktree folder. Never mutated by
    /// the UI; Crane only changes it if git itself renames the branch.
    pub name: String,
    /// Optional user-set display alias. When `Some(x)`, the UI renders
    /// "x (name)" so the original folder / branch stays visible.
    pub display_name: Option<String>,
    pub path: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
    pub expanded: bool,
    pub git_status: Option<GitStatus>,
    pub last_status_refresh: Option<Instant>,
    pub git_rx: Option<std::sync::mpsc::Receiver<Option<GitStatus>>>,
}

impl Workspace {
    /// Display form for UI rows: `alias (name)` when aliased, else name.
    pub fn label(&self) -> String {
        match &self.display_name {
            Some(alias) if !alias.trim().is_empty() => format!("{alias} ({})", self.name),
            _ => self.name.clone(),
        }
    }
}

pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub workspaces: Vec<Workspace>,
    pub expanded: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RightTab {
    Changes,
    Files,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Appearance,
    Editor,
    Terminal,
    LanguageServers,
    Shortcuts,
    About,
}

impl SettingsSection {
    pub const ALL: &'static [SettingsSection] = &[
        SettingsSection::Appearance,
        SettingsSection::Editor,
        SettingsSection::Terminal,
        SettingsSection::LanguageServers,
        SettingsSection::Shortcuts,
        SettingsSection::About,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Editor => "Editor",
            SettingsSection::Terminal => "Terminal",
            SettingsSection::LanguageServers => "Language Servers",
            SettingsSection::Shortcuts => "Keyboard Shortcuts",
            SettingsSection::About => "About",
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            SettingsSection::Appearance => i::PAINT_BRUSH,
            SettingsSection::Editor => i::CODE,
            SettingsSection::Terminal => i::TERMINAL_WINDOW,
            SettingsSection::LanguageServers => i::LIGHTNING,
            SettingsSection::Shortcuts => i::KEYBOARD,
            SettingsSection::About => i::INFO,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocationMode {
    Global,
    ProjectLocal,
    Custom,
}

pub struct NewWorkspaceModal {
    pub project_id: ProjectId,
    pub branch: String,
    pub custom_path: String,
    pub mode: LocationMode,
    pub create_new_branch: bool,
    /// True when the modal is opened from the branch picker with an
    /// existing branch: the branch name is known and the "Create new
    /// branch" choice is fixed. We hide the checkbox and lock the
    /// branch text field to avoid letting the user flip into a state
    /// that git would reject (e.g. `-b` on a branch that already exists).
    pub branch_locked: bool,
    pub error: Option<String>,
}

impl NewWorkspaceModal {
    pub fn resolved_parent(&self, project_path: &Path, project_name: &str) -> PathBuf {
        match self.mode {
            LocationMode::Global => {
                let home = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(format!("{home}/.crane-worktrees/{project_name}"))
            }
            LocationMode::ProjectLocal => project_path.join(".crane-worktrees"),
            LocationMode::Custom => PathBuf::from(shellexpand_home(&self.custom_path)),
        }
    }
}

pub struct App {
    pub projects: Vec<Project>,
    pub active: Option<(ProjectId, WorkspaceId, TabId)>,
    pub last_workspace: Option<(ProjectId, WorkspaceId)>,
    pub show_left: bool,
    pub show_right: bool,
    pub show_help: bool,
    pub right_tab: RightTab,
    pub commit_message: String,
    pub git_error: Option<String>,
    pub font_size: f32,
    pub expanded_dirs: HashSet<PathBuf>,
    pub collapsed_change_dirs: HashSet<String>,
    pub new_workspace_modal: Option<NewWorkspaceModal>,
    pub update_check: UpdateCheck,
    pub updater: crate::updater::Updater,
    pub selected_theme: String,
    pub show_settings: bool,
    pub settings_section: SettingsSection,
    pub custom_mono_font: Option<String>,
    pub ui_scale: f32,
    pub syntax_theme_override: Option<String>,
    pub left_panel_w: f32,
    pub right_panel_w: f32,
    pub editor_word_wrap: bool,
    pub editor_trim_on_save: bool,
    pub lsp: crate::lsp::LspManager,
    pub language_configs: crate::lsp::LanguageConfigs,
    pub branch_picker_open: bool,
    pub branch_picker_query: String,
    pub branch_picker_collapsed: std::collections::HashSet<String>,
    pub branch_picker_width: f32,
    pub branch_picker_height: f32,
    pub branch_picker_opened_at: Option<Instant>,
    pub branch_picker_error: Option<String>,
    /// (project, workspace, tab, edit buffer) of the tab currently in
    /// inline rename mode. Set on double-click; committed on Enter /
    /// focus-lost, cancelled on Esc.
    pub renaming_tab: Option<(ProjectId, WorkspaceId, TabId, String)>,
    /// Parallel slot for workspace-level rename. Commits into
    /// `Workspace::display_name`, not `name` — the canonical folder /
    /// branch label is preserved, the custom alias just decorates.
    pub renaming_workspace: Option<(ProjectId, WorkspaceId, String)>,
    pub branch_picker_loading: bool,
    pub branch_picker_rx:
        Option<std::sync::mpsc::Receiver<Vec<(PathBuf, Vec<String>, Vec<String>)>>>,
    /// Per-repo branch data loaded when the picker opens:
    /// repo_root → (local branches, remote branches in `remote/branch` form).
    pub branch_picker_repos: Vec<(PathBuf, Vec<String>, Vec<String>)>,
    /// None = "All repos" aggregate view; Some(root) = filter to one repo.
    pub branch_picker_filter: Option<PathBuf>,
    pub repo_branch_cache: std::collections::HashMap<PathBuf, (String, Instant)>,
    next_project: ProjectId,
    next_workspace: WorkspaceId,
    next_tab: TabId,
}

impl App {
    pub fn new() -> Self {
        Self {
            projects: Vec::new(),
            active: None,
            last_workspace: None,
            show_left: true,
            show_right: true,
            show_help: false,
            right_tab: RightTab::Changes,
            commit_message: String::new(),
            git_error: None,
            font_size: 14.0,
            expanded_dirs: HashSet::new(),
            collapsed_change_dirs: HashSet::new(),
            new_workspace_modal: None,
            update_check: UpdateCheck::new(Default::default()),
            updater: crate::updater::Updater::new(),
            selected_theme: "crane-dark".to_string(),
            show_settings: false,
            settings_section: SettingsSection::Appearance,
            custom_mono_font: None,
            ui_scale: 1.0,
            syntax_theme_override: None,
            left_panel_w: 240.0,
            editor_word_wrap: false,
            editor_trim_on_save: false,
            right_panel_w: 300.0,
            lsp: crate::lsp::LspManager::new(),
            language_configs: crate::lsp::LanguageConfigs::default(),
            branch_picker_open: false,
            branch_picker_query: String::new(),
            branch_picker_collapsed: std::collections::HashSet::new(),
            branch_picker_width: 420.0,
            branch_picker_height: 360.0,
            branch_picker_opened_at: None,
            branch_picker_error: None,
            renaming_tab: None,
            renaming_workspace: None,
            branch_picker_loading: false,
            branch_picker_rx: None,
            branch_picker_repos: Vec::new(),
            branch_picker_filter: None,
            repo_branch_cache: std::collections::HashMap::new(),
            next_project: 1,
            next_workspace: 1,
            next_tab: 1,
        }
    }

    pub fn ensure_initial(&mut self, _ctx: &egui::Context) {
        // Intentionally empty. First launch shows an empty state — the user
        // picks a project via "Add Project…" in the Left Panel footer.
        // Subsequent launches restore via session::load().
    }

    pub fn next_project_id(&self) -> ProjectId {
        self.next_project
    }
    pub fn next_workspace_id(&self) -> WorkspaceId {
        self.next_workspace
    }
    pub fn next_tab_id(&self) -> TabId {
        self.next_tab
    }
    pub fn set_id_counters(&mut self, p: ProjectId, w: WorkspaceId, t: TabId) {
        self.next_project = p.max(self.next_project);
        self.next_workspace = w.max(self.next_workspace);
        self.next_tab = t.max(self.next_tab);
    }

    pub fn add_project_from_path(&mut self, path: PathBuf, ctx: &egui::Context) -> Option<ProjectId> {
        if !path.is_dir() {
            return None;
        }
        let id = self.next_project;
        self.next_project += 1;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();

        let infos = git::list_workspaces(&path);
        let infos = if infos.is_empty() {
            vec![git::WorkspaceInfo {
                path: path.clone(),
                branch: "(no git)".into(),
            }]
        } else {
            infos
        };

        let mut workspaces = Vec::new();
        let mut first_active: Option<(WorkspaceId, TabId)> = None;
        for info in infos {
            let wt_id = self.next_workspace;
            self.next_workspace += 1;
            let tab_id = self.next_tab;
            self.next_tab += 1;
            let mut layout = Layout::new(info.path.clone());
            layout.ensure_initial_terminal(ctx);
            let tab = Tab {
                id: tab_id,
                name: "Terminal".into(),
                layout,
            };
            if first_active.is_none() {
                first_active = Some((wt_id, tab_id));
            }
            workspaces.push(Workspace {
                id: wt_id,
                name: info.branch,
                display_name: None,
                path: info.path,
                tabs: vec![tab],
                active_tab: Some(tab_id),
                expanded: true,
                git_status: None,
                last_status_refresh: None,
                git_rx: None,
            });
        }

        self.projects.push(Project {
            id,
            name,
            path,
            workspaces,
            expanded: true,
        });
        if let Some((wt, tab)) = first_active {
            self.active = Some((id, wt, tab));
            self.last_workspace = Some((id, wt));
        }
        Some(id)
    }

    /// Nearest `.git` root for the active file's path, falling back to
    /// the active Workspace path if no file is open (or no nested repo
    /// is found). This is what branch picker / commit tree / branch
    /// label bind to, so nested submodules "just work".
    pub fn active_repo_root(&self) -> Option<PathBuf> {
        if let Some(path) = self.active_file_path_str()
            && let Some(root) = crate::git::find_git_root(Path::new(&path))
        {
            return Some(root);
        }
        self.active_workspace_path().map(|p| p.to_path_buf())
    }

    /// Branch for the repo containing the active file. Cached 2s to
    /// avoid spawning git-subprocesses every frame. Falls back to the
    /// cached Workspace status when the active repo == Workspace root.
    pub fn active_repo_branch(&mut self) -> Option<String> {
        let root = self.active_repo_root()?;
        if let Some(ws) = self.active_workspace_path()
            && ws == root.as_path()
        {
            let (pid, wid, _) = self.active?;
            let project = self.projects.iter().find(|p| p.id == pid)?;
            let wt = project.workspaces.iter().find(|w| w.id == wid)?;
            if let Some(s) = &wt.git_status
                && !s.branch.is_empty()
            {
                return Some(s.branch.clone());
            }
        }
        if let Some((b, t)) = self.repo_branch_cache.get(&root)
            && t.elapsed().as_secs() < 2
        {
            return Some(b.clone());
        }
        let b = crate::git::current_branch(&root)?;
        self.repo_branch_cache
            .insert(root, (b.clone(), Instant::now()));
        Some(b)
    }

    fn active_file_path_str(&self) -> Option<String> {
        use crate::layout::PaneContent;
        let layout = self.active_layout_ref()?;
        if let Some(id) = layout.focus
            && let Some(p) = layout.panes.get(&id)
            && let PaneContent::Files(files) = &p.content
            && let Some(t) = files.tabs.get(files.active)
        {
            return Some(t.path.clone());
        }
        for (_, p) in &layout.panes {
            if let PaneContent::Files(files) = &p.content
                && let Some(t) = files.tabs.get(files.active)
            {
                return Some(t.path.clone());
            }
        }
        None
    }

    pub fn active_workspace_path(&self) -> Option<&Path> {
        let (pid, wid, _) = self.active?;
        let project = self.projects.iter().find(|p| p.id == pid)?;
        let wt = project.workspaces.iter().find(|w| w.id == wid)?;
        Some(&wt.path)
    }

    pub fn active_layout_ref(&self) -> Option<&Layout> {
        let (pid, wid, tid) = self.active?;
        let project = self.projects.iter().find(|p| p.id == pid)?;
        let workspace = project.workspaces.iter().find(|w| w.id == wid)?;
        let tab = workspace.tabs.iter().find(|t| t.id == tid)?;
        Some(&tab.layout)
    }

    pub fn active_layout(&mut self) -> Option<&mut Layout> {
        let (pid, wid, tid) = self.active?;
        let project = self.projects.iter_mut().find(|p| p.id == pid)?;
        let workspace = project.workspaces.iter_mut().find(|w| w.id == wid)?;
        let tab = workspace.tabs.iter_mut().find(|t| t.id == tid)?;
        Some(&mut tab.layout)
    }

    pub fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        let (pid, wid, _) = self.active?;
        let project = self.projects.iter_mut().find(|p| p.id == pid)?;
        project.workspaces.iter_mut().find(|w| w.id == wid)
    }

    pub fn set_active(&mut self, pid: ProjectId, wid: WorkspaceId, tid: TabId) {
        self.active = Some((pid, wid, tid));
        self.last_workspace = Some((pid, wid));
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid)
            && let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
                w.active_tab = Some(tid);
            }
    }

    pub fn new_tab_in_active_workspace(&mut self, ctx: &egui::Context) {
        self.push_tab(ctx, None, None);
    }

    /// Open a file in the active Workspace's Files Pane and notify the LSP
    /// manager. Called from the Right Panel / file picker / Files browser.
    pub fn open_file_into_active_layout(
        &mut self,
        ctx: &egui::Context,
        path: String,
        name: String,
        content: String,
    ) {
        if let Some(layout) = self.active_layout() {
            layout.open_file_in_files_pane(path.clone(), name, content.clone());
        }
        let cfg_snapshot = self.language_configs.clone();
        self.lsp
            .did_open(ctx, std::path::Path::new(&path), &content, &cfg_snapshot);
    }

    /// Per-frame sync, scoped to the ACTIVE layout only. Was iterating every
    /// project × workspace × tab × pane × file on every frame, which made
    /// the whole app (including the terminal) crawl on large sessions.
    /// Also debounces `textDocument/didChange` to 300 ms idle so a burst of
    /// keystrokes doesn't flood the LSP server.
    pub fn sync_lsp_changes(&mut self, ctx: &egui::Context) {
        self.lsp.tick(ctx);
        let debounce = std::time::Duration::from_millis(300);
        let now = std::time::Instant::now();
        let Some((pid, wid, tid)) = self.active else {
            return;
        };
        let Some(project) = self.projects.iter_mut().find(|p| p.id == pid) else {
            return;
        };
        let Some(ws) = project.workspaces.iter_mut().find(|w| w.id == wid) else {
            return;
        };
        let Some(tab) = ws.tabs.iter_mut().find(|t| t.id == tid) else {
            return;
        };
        let configs_snapshot = self.language_configs.clone();
        for (_, pane) in tab.layout.panes.iter_mut() {
            if let crate::layout::PaneContent::Files(files) = &mut pane.content {
                for ft in files.tabs.iter_mut() {
                    let path = std::path::Path::new(&ft.path);
                    if !self.lsp.is_tracked(path) {
                        self.lsp.did_open(ctx, path, &ft.content, &configs_snapshot);
                        ft.last_lsp_content = ft.content.clone();
                        ft.last_lsp_sent_at = Some(now);
                    } else if ft.content != ft.last_lsp_content {
                        let quiet_enough = ft
                            .last_lsp_sent_at
                            .map(|t| now.duration_since(t) >= debounce)
                            .unwrap_or(true);
                        if quiet_enough {
                            self.lsp.did_change(path, &ft.content);
                            ft.last_lsp_content = ft.content.clone();
                            ft.last_lsp_sent_at = Some(now);
                        }
                    }
                }
            }
        }
    }

    fn push_tab(
        &mut self,
        ctx: &egui::Context,
        initial_content: Option<crate::layout::PaneContent>,
        tab_name: Option<String>,
    ) {
        let (pid, wid) = match self.active.map(|(p, w, _)| (p, w)).or(self.last_workspace) {
            Some(a) => a,
            None => return,
        };
        let tab_id = self.next_tab;
        self.next_tab += 1;
        let project = match self.projects.iter_mut().find(|p| p.id == pid) {
            Some(p) => p,
            None => return,
        };
        let wt = match project.workspaces.iter_mut().find(|w| w.id == wid) {
            Some(w) => w,
            None => return,
        };
        let mut layout = Layout::new(wt.path.clone());
        let name = match initial_content {
            None => {
                layout.ensure_initial_terminal(ctx);
                tab_name.unwrap_or_else(|| format!("Tab {}", wt.tabs.len() + 1))
            }
            Some(content) => {
                let default = content.kind_label().to_string();
                layout.add_pane(content, None);
                tab_name.unwrap_or(default)
            }
        };
        wt.tabs.push(Tab {
            id: tab_id,
            name,
            layout,
        });
        wt.active_tab = Some(tab_id);
        self.active = Some((pid, wid, tab_id));
        self.last_workspace = Some((pid, wid));
    }

    pub fn close_active_tab(&mut self) {
        let (pid, wid, tid) = match self.active {
            Some(a) => a,
            None => return,
        };
        let project = match self.projects.iter_mut().find(|p| p.id == pid) {
            Some(p) => p,
            None => return,
        };
        let wt = match project.workspaces.iter_mut().find(|w| w.id == wid) {
            Some(w) => w,
            None => return,
        };
        wt.tabs.retain(|t| t.id != tid);
        let new_tab = wt.tabs.first().map(|t| t.id);
        wt.active_tab = new_tab;
        self.active = new_tab.map(|t| (pid, wid, t));
        self.last_workspace = Some((pid, wid));
    }

    pub fn refresh_active_git_status(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let wt = match self.active_workspace_mut() {
            Some(w) => w,
            None => return,
        };

        if let Some(rx) = wt.git_rx.as_ref()
            && let Ok(status) = rx.try_recv() {
                wt.git_status = status;
                wt.last_status_refresh = Some(now);
                wt.git_rx = None;
            }

        if wt.git_rx.is_some() {
            return;
        }
        let due = wt
            .last_status_refresh
            .map(|t| now.duration_since(t) > Duration::from_millis(2000))
            .unwrap_or(true);
        if !due {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let path = wt.path.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let status = git::status(&path);
            let _ = tx.send(status);
            ctx.request_repaint();
        });
        wt.git_rx = Some(rx);
    }

    pub fn open_new_workspace_modal(&mut self, pid: ProjectId) {
        let project = match self.projects.iter().find(|p| p.id == pid) {
            Some(p) => p,
            None => return,
        };
        let home = std::env::var("HOME").unwrap_or_default();
        // Sanitize the project name for use as a path segment: drop any
        // character that could break out of ~/.crane-worktrees (leading
        // dots, slashes, backslashes). Prevents a project folder named
        // "../escape" from producing a traversal path.
        let safe: String = project
            .name
            .chars()
            .map(|c| match c {
                '/' | '\\' | '\0' => '_',
                c => c,
            })
            .collect();
        let safe = safe.trim_start_matches('.').trim_start_matches('_');
        let safe = if safe.is_empty() { "project" } else { safe };
        self.new_workspace_modal = Some(NewWorkspaceModal {
            project_id: pid,
            branch: String::new(),
            custom_path: format!("{home}/.crane-worktrees/{safe}"),
            mode: LocationMode::Global,
            create_new_branch: true,
            branch_locked: false,
            error: None,
        });
    }

    pub fn create_workspace_from_modal(&mut self, ctx: &egui::Context) {
        let modal = match self.new_workspace_modal.take() {
            Some(m) => m,
            None => return,
        };
        let branch = modal.branch.trim().to_string();
        if branch.is_empty() {
            self.new_workspace_modal = Some(NewWorkspaceModal {
                error: Some("Branch name is required".into()),
                ..modal
            });
            return;
        }
        let (project_path, project_name) = match self.projects.iter().find(|p| p.id == modal.project_id) {
            Some(p) => (p.path.clone(), p.name.clone()),
            None => return,
        };
        let parent = modal.resolved_parent(&project_path, &project_name);
        let wt_path = parent.join(&branch);
        let _ = std::fs::create_dir_all(&parent);
        match git::workspace_add(&project_path, &wt_path, &branch, modal.create_new_branch) {
            Ok(()) => {
                let _ = project_name;
                let project = match self.projects.iter_mut().find(|p| p.id == modal.project_id) {
                    Some(p) => p,
                    None => return,
                };
                let wt_id = self.next_workspace;
                self.next_workspace += 1;
                let tab_id = self.next_tab;
                self.next_tab += 1;
                let mut layout = Layout::new(wt_path.clone());
                layout.ensure_initial_terminal(ctx);
                let tab = Tab {
                    id: tab_id,
                    name: "Terminal".into(),
                    layout,
                };
                project.workspaces.push(Workspace {
                    id: wt_id,
                    name: branch,
                    display_name: None,
                    path: wt_path,
                    tabs: vec![tab],
                    active_tab: Some(tab_id),
                    expanded: true,
                    git_status: None,
                    last_status_refresh: None,
                    git_rx: None,
                });
                self.active = Some((modal.project_id, wt_id, tab_id));
                self.last_workspace = Some((modal.project_id, wt_id));
            }
            Err(e) => {
                self.new_workspace_modal = Some(NewWorkspaceModal {
                    error: Some(e),
                    ..modal
                });
            }
        }
    }

    pub fn remove_project(&mut self, pid: ProjectId) {
        self.projects.retain(|p| p.id != pid);
        if let Some((p, _, _)) = self.active
            && p == pid {
                self.active = self
                    .projects
                    .first()
                    .and_then(|p| p.workspaces.first().map(|w| (p.id, w)))
                    .and_then(|(pid, w)| w.active_tab.map(|t| (pid, w.id, t)));
            }
        if let Some((p, _)) = self.last_workspace
            && p == pid {
                self.last_workspace = self
                    .active
                    .map(|(pid, wid, _)| (pid, wid))
                    .or_else(|| {
                        self.projects
                            .first()
                            .and_then(|p| p.workspaces.first().map(|w| (p.id, w.id)))
                    });
            }
    }

    pub fn breadcrumb(&self) -> String {
        let (pid, wid, tid) = match self.active {
            Some(a) => a,
            None => return String::from("Crane"),
        };
        let project = self.projects.iter().find(|p| p.id == pid);
        let wt = project.and_then(|p| p.workspaces.iter().find(|w| w.id == wid));
        let tab = wt.and_then(|w| w.tabs.iter().find(|t| t.id == tid));
        // Separator is ASCII '/' — U+203A is tofu in JetBrains Mono
        // (per CLAUDE.md). If we want a caret glyph later, source it
        // from egui_phosphor::regular::CARET_RIGHT.
        format!(
            "{} / {} / {}",
            project.map(|p| p.name.as_str()).unwrap_or("?"),
            wt.map(|w| w.name.as_str()).unwrap_or("?"),
            tab.map(|t| t.name.as_str()).unwrap_or("?"),
        )
    }
}
