use crate::git::{self, GitStatus};
use crate::state::layout::Layout;
use crate::update::check::UpdateCheck;
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
    /// Optional per-Tab accent tint applied to the tab's icon and
    /// label in the Left Panel. Matches the Project / Workspace /
    /// folder-group tint affordance so every entity in the sidebar
    /// tree can be colour-coded consistently. `None` → fall back to
    /// the active-tab accent (when active) or the default foreground.
    pub tint: Option<[u8; 3]>,
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
    /// Optional per-Workspace accent tint applied to the branch icon
    /// and label in the Left Panel. Lets users colour-code branches
    /// the same way [`Project::tint`] colour-codes projects. `None`
    /// → fall back to the theme accent.
    pub tint: Option<[u8; 3]>,
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
    /// When the user added a folder that wasn't a git repo but
    /// contained nested repos, Crane creates one Project per discovered
    /// `.git` root and groups them under the original folder. These
    /// fields identify the shared parent so the Left Panel can render a
    /// single collapsible group header above the sibling Projects.
    /// `None` for top-level Projects added directly.
    pub group_path: Option<PathBuf>,
    pub group_name: Option<String>,
    /// True when the folder on disk was missing the last time we
    /// looked. Git / LSP / new-tab / worktree-add all no-op on missing
    /// projects; the user sees a "Project Not Found" modal offering
    /// Relocate / Close.
    pub missing: bool,
    pub workspaces: Vec<Workspace>,
    pub expanded: bool,
    /// Most recently active workspace within this project. Restored on
    /// next launch so re-opening a project lands on where you left it.
    pub last_active_workspace: Option<WorkspaceId>,
    /// Remembered "new workspace" modal preferences, so the modal
    /// preloads the same mode + custom path you used last time for
    /// this project (instead of resetting to Global + ~/.crane-worktrees/<name>).
    pub preferred_location_mode: Option<LocationMode>,
    pub preferred_custom_path: Option<String>,
    /// Optional per-Project accent tint applied to the cube icon in the
    /// Left Panel. Lets users visually distinguish projects at a glance
    /// without renaming. `None` → fall back to the theme accent.
    pub tint: Option<[u8; 3]>,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocationMode {
    Global,
    ProjectLocal,
    Custom,
}

impl LocationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationMode::Global => "Global",
            LocationMode::ProjectLocal => "ProjectLocal",
            LocationMode::Custom => "Custom",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "Global" => Some(Self::Global),
            "ProjectLocal" => Some(Self::ProjectLocal),
            "Custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Distinguishes "create file" from "create folder" in the
/// [`NewEntryModal`]. Two-variant enum (instead of a bool) so call
/// sites read clearly at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewEntryKind {
    File,
    Folder,
}

/// One reversible Files-Pane operation. Pushed onto
/// [`App::file_op_history`] after every successful move / trash so
/// Cmd+Z can pop and undo. Bounded stack — see field docs there.
#[derive(Debug, Clone)]
pub enum FileOp {
    /// User dragged `from` onto a folder; we did `fs::rename` to
    /// `to`. Undo: rename back. Refuses if `from` now exists (would
    /// overwrite a new file at the original location).
    Move { from: PathBuf, to: PathBuf },
    /// User trashed `path`. Undo on Linux/Windows uses the `trash`
    /// crate's restore API. macOS has no programmatic restore — the
    /// undo there silently no-ops (Finder's "Put Back" still works).
    Trash { path: PathBuf },
}

/// Bound on undo history. 64 ops covers a typical refactor session
/// without pinning unbounded path memory.
pub const FILE_OP_HISTORY_CAP: usize = 64;

/// Async git operation kinds the Changes pane can dispatch. Drives
/// per-button spinner state + post-completion result message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitOpKind {
    Commit,
    /// Reserved for a future keyboard shortcut (e.g. ⌘⇧↩) — the
    /// caret dropdown that previously surfaced this got removed
    /// when push/pull/fetch moved to the top toolbar. The dispatch
    /// path still handles it so wiring a shortcut is a one-liner.
    #[allow(dead_code)]
    CommitAndPush,
    Push,
    Pull,
    Fetch,
}

impl GitOpKind {
    pub fn label(self) -> &'static str {
        match self {
            GitOpKind::Commit => "Commit",
            GitOpKind::CommitAndPush => "Commit & Push",
            GitOpKind::Push => "Push",
            GitOpKind::Pull => "Pull",
            GitOpKind::Fetch => "Fetch",
        }
    }
}

/// State of the most recent (or in-flight) async git op. Worker
/// thread owns this via `Arc<Mutex<…>>` and the render loop polls.
#[derive(Clone, Debug)]
pub enum GitOpStatus {
    /// No op has been run yet, or the last result was dismissed.
    Idle,
    /// An op is in flight — the UI shows a spinner on the matching
    /// button and disables the others.
    Running(GitOpKind),
    /// Last op succeeded; carries a short result message ("Pulled
    /// 3 commits", "Already up to date", "Pushed to origin/main")
    /// for the bottom pill. Auto-cleared when the user starts the
    /// next op.
    Done { kind: GitOpKind, message: String },
    /// Last op failed; carries the stderr-derived error text.
    Failed { kind: GitOpKind, error: String },
}

impl Default for GitOpStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// State for the JetBrains-style inline "new entry" editor in the
/// Files Pane: when the user picks New File / New Folder from a
/// right-click menu, an extra row appears in the tree at the parent
/// dir with a focused TextEdit. Enter creates, Escape cancels, focus
/// loss with empty name cancels. `parent` is always a directory
/// (the row's parent dir if right-clicked on a file, the dir itself
/// if right-clicked on a folder).
pub struct PendingNewEntry {
    pub parent: PathBuf,
    pub kind: NewEntryKind,
    pub name: String,
    pub error: Option<String>,
    /// First-frame focus latch — prevents the TextEdit from stealing
    /// focus on every subsequent frame, which would block clicks on
    /// other rows from cancelling the pending entry.
    pub focused_once: bool,
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

/// One in-flight goto-definition request. `dispatched_at` lets us drop
/// requests that never come back (slow LSP, crashed server) so the
/// list doesn't leak.
pub struct PendingGoto {
    pub server: crate::lsp::ServerKey,
    pub request_id: i64,
    pub dispatched_at: Instant,
}

/// All state that only matters while the bottom-anchored branch picker
/// is open (or caching recent picker state across opens). Previously
/// lived as 11 flat `branch_picker_*` fields on `App`; grouping them
/// keeps `App::new` readable and makes it obvious what is scoped to
/// this one subsystem.
pub struct BranchPickerState {
    pub open: bool,
    pub query: String,
    pub collapsed: std::collections::HashSet<String>,
    pub width: f32,
    pub height: f32,
    pub opened_at: Option<Instant>,
    pub error: Option<String>,
    pub loading: bool,
    pub rx: Option<std::sync::mpsc::Receiver<Vec<(PathBuf, Vec<String>, Vec<String>)>>>,
    /// Per-repo branch data loaded when the picker opens:
    /// repo_root → (local branches, remote branches in `remote/branch` form).
    pub repos: Vec<(PathBuf, Vec<String>, Vec<String>)>,
    /// None = "All repos" aggregate view; Some(root) = filter to one repo.
    pub filter: Option<PathBuf>,
}

impl Default for BranchPickerState {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            collapsed: std::collections::HashSet::new(),
            width: 420.0,
            height: 360.0,
            opened_at: None,
            error: None,
            loading: false,
            rx: None,
            repos: Vec::new(),
            filter: None,
        }
    }
}

pub struct TabSwitcherState {
    /// Flattened list of every tab, pre-sorted by MRU (front = most
    /// recent). Frozen for the lifetime of the overlay so rapid cycling
    /// doesn't reshuffle under the user's fingers.
    pub entries: Vec<(ProjectId, WorkspaceId, TabId)>,
    /// Index into `entries` currently highlighted. Wraps.
    pub highlight: usize,
    /// True on the frame the overlay opened — suppresses a stray
    /// release-commit when the user only tapped Cmd+~ once. (egui
    /// reports modifiers per-frame; without this, the same-frame
    /// check would treat "Cmd+~ down" and its immediate "Cmd still
    /// held" as a cycle+commit.)
    pub cmd_was_held: bool,
}

pub struct PendingRemoveWorktree {
    pub project_id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub label: String,
    pub path: PathBuf,
    pub unpushed_commits: usize,
    pub modified_files: usize,
    pub has_upstream: bool,
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
    pub pending_new_entry: Option<PendingNewEntry>,
    /// Most recently single-clicked path in the Files Pane. Drives
    /// the row highlight + the Cmd+Delete keyboard shortcut. Cleared
    /// when the file is deleted/moved, or when focus moves to a
    /// non-tree widget.
    pub selected_file: Option<PathBuf>,
    /// LIFO stack of reversible Files-Pane operations. Driven by
    /// Cmd+Z. Bounded so a long session doesn't pin large pathbuf
    /// allocations forever. Reversal: Move → fs::rename back; Trash
    /// → restore via the `trash` crate (Linux/Windows only — macOS
    /// `trash` crate doesn't expose a programmatic restore API, so
    /// the entry is dropped from the stack as a no-op there).
    pub file_op_history: std::collections::VecDeque<FileOp>,
    /// Shared with the git-op worker thread. Reads from the render
    /// loop drive spinner state + result-pill rendering; writes from
    /// the worker mark Running → Done/Failed atomically.
    pub git_op_status: std::sync::Arc<parking_lot::Mutex<GitOpStatus>>,
    pub update_check: UpdateCheck,
    pub updater: crate::update::apply::Updater,
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
    /// Global opt-out for the LSP install prompt. Set by "Never ask for
    /// any language" in the install modal; persisted in settings.json.
    pub lsp_install_prompts_disabled: bool,
    pub branch_picker: BranchPickerState,
    /// (project, workspace, tab, edit buffer) of the tab currently in
    /// inline rename mode. Set on double-click; committed on Enter /
    /// focus-lost, cancelled on Esc.
    pub renaming_tab: Option<(ProjectId, WorkspaceId, TabId, String)>,
    /// Parallel slot for workspace-level rename. Commits into
    /// `Workspace::display_name`, not `name` — the canonical folder /
    /// branch label is preserved, the custom alias just decorates.
    pub renaming_workspace: Option<(ProjectId, WorkspaceId, String)>,
    /// Queue of projects whose root folder was missing at session
    /// restore. The `missing_project` modal dequeues one at a time.
    pub missing_project_modals: Vec<ProjectId>,
    /// In-flight goto-definition requests. Each tick we poll these for
    /// results and land at most one successful jump. A 5 s watchdog
    /// drops any request that never comes back so we don't leak ids.
    pub pending_gotos: Vec<PendingGoto>,
    pub repo_branch_cache: std::collections::HashMap<PathBuf, (String, Instant)>,
    /// Pending "Remove Worktree" awaiting user confirmation because the
    /// worktree has unpushed commits or modified files. `None` while no
    /// modal is open.
    pub pending_remove_worktree: Option<PendingRemoveWorktree>,
    /// Pending "Close workspace tab" awaiting user confirmation.
    /// Populated by the × button or middle-click on a tab row in the
    /// projects pane — closing a tab drops its terminal / pane
    /// contents, so we always confirm first.
    pub pending_close_tab: Option<(ProjectId, WorkspaceId, TabId)>,
    /// MRU (most-recently-used) list of tabs across all projects /
    /// workspaces, front = most recent. Updated when `active` changes.
    /// Drives the Cmd+~ tab switcher — just like alt+tab, the first
    /// tap after opening lands you on the previously-focused tab.
    pub tab_mru: Vec<(ProjectId, WorkspaceId, TabId)>,
    /// Active tab-switcher overlay state. `None` = closed.
    pub tab_switcher: Option<TabSwitcherState>,
    /// Per-group folder-header tints, keyed by a group's `group_path`.
    /// Lets users colour-code folder groups the same way
    /// [`Project::tint`] and [`Workspace::tint`] colour-code projects
    /// and branches. Missing entry → fall back to the `muted()`
    /// folder colour.
    pub group_tints: std::collections::HashMap<PathBuf, [u8; 3]>,
    /// Folder-group collapse state, keyed by `group_path`. Absent →
    /// expanded (the default). Clicking a folder header toggles
    /// membership; collapsed groups hide all their Sub-projects in
    /// the Left Panel tree walk.
    pub group_collapsed: std::collections::HashSet<PathBuf>,
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
            pending_new_entry: None,
            selected_file: None,
            file_op_history: std::collections::VecDeque::new(),
            git_op_status: std::sync::Arc::new(parking_lot::Mutex::new(GitOpStatus::Idle)),
            update_check: UpdateCheck::new(Default::default()),
            updater: crate::update::apply::Updater::new(),
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
            lsp_install_prompts_disabled: false,
            branch_picker: BranchPickerState::default(),
            renaming_tab: None,
            renaming_workspace: None,
            missing_project_modals: Vec::new(),
            pending_gotos: Vec::new(),
            repo_branch_cache: std::collections::HashMap::new(),
            pending_remove_worktree: None,
            pending_close_tab: None,
            tab_mru: Vec::new(),
            tab_switcher: None,
            group_tints: std::collections::HashMap::new(),
            group_collapsed: std::collections::HashSet::new(),
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

    /// Maintain the tab MRU: whenever `active` is Some and isn't
    /// already at MRU front, push it to front (dedup). Called once
    /// per frame from the main render loop so every code path that
    /// mutates `active` is covered without plumbing a setter through
    /// dozens of call sites.
    pub fn sync_tab_mru(&mut self) {
        let Some(cur) = self.active else { return };
        if self.tab_mru.first() == Some(&cur) {
            return;
        }
        self.tab_mru.retain(|e| e != &cur);
        self.tab_mru.insert(0, cur);
        // Cap — unlikely to exceed, but keep the list bounded.
        if self.tab_mru.len() > 256 {
            self.tab_mru.truncate(256);
        }
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

        // Auto-discover nested git repos under `path`. The discovery
        // rules differ based on whether `path` itself is a repo:
        //
        // * `path` is NOT a repo: promote every discovered `.git` root
        //   as a Sub-project. Matches the monorepo-of-clones case.
        // * `path` IS a repo: only promote nested `.git` roots that are
        //   gitignored AND not submodules — those are user-cloned
        //   siblings the parent doesn't track. Submodules share history
        //   with the parent so we keep them invisible here.
        let path_is_repo = path.join(".git").exists();
        let all_roots = git::discover_repos(&path, 5);
        let nested: Vec<_> = all_roots
            .iter()
            .filter(|r| r.as_path() != path.as_path())
            .cloned()
            .collect();

        let siblings: Vec<std::path::PathBuf> = if path_is_repo {
            nested
                .into_iter()
                .filter(|nr| {
                    git::is_path_ignored(&path, nr) && !git::is_submodule(&path, nr)
                })
                .collect()
        } else {
            nested
        };

        if !siblings.is_empty() {
            let group_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("group")
                .to_string();
            let mut first_id: Option<ProjectId> = None;
            // Include the parent itself as a Sub-project sibling when
            // it's a real repo, so its own branches / Commit UI still
            // work alongside the ignored children.
            if path_is_repo {
                let id = self.add_single_project(
                    path.clone(),
                    Some(path.clone()),
                    Some(group_name.clone()),
                    ctx,
                );
                first_id = id;
            }
            for root in siblings {
                let id = self.add_single_project(
                    root,
                    Some(path.clone()),
                    Some(group_name.clone()),
                    ctx,
                );
                if first_id.is_none() {
                    first_id = id;
                }
            }
            return first_id;
        }

        self.add_single_project(path, None, None, ctx)
    }

    fn add_single_project(
        &mut self,
        path: PathBuf,
        group_path: Option<PathBuf>,
        group_name: Option<String>,
        _ctx: &egui::Context,
    ) -> Option<ProjectId> {
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
            layout.ensure_initial_welcome();
            let tab = Tab {
                id: tab_id,
                name: "Terminal".into(),
                layout,
                tint: None,
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
                tint: None,
            });
        }

        self.projects.push(Project {
            id,
            name,
            path,
            group_path,
            group_name,
            missing: false,
            workspaces,
            expanded: true,
            last_active_workspace: first_active.map(|(wt, _)| wt),
            preferred_location_mode: None,
            preferred_custom_path: None,
            tint: None,
        });
        if let Some((wt, tab)) = first_active
            && self.active.is_none()
        {
            self.active = Some((id, wt, tab));
            self.last_workspace = Some((id, wt));
        }
        Some(id)
    }

    /// Re-probe disk state for every existing Project and pick up any
    /// repo / worktree / sibling-clone that was added outside Crane
    /// between sessions. Three cases handled:
    ///
    /// 1. Project has a new `git worktree add` branch-checkout that
    ///    isn't in its persisted Workspaces list → appended.
    /// 2. A nested `.git` root appeared under an existing Project's
    ///    path (user cloned into the monorepo parent, or `git init`'d
    ///    a subdir) → added as a sibling Sub-project under the same
    ///    group. A standalone Project whose path now has nested clones
    ///    gets promoted to a group on-the-fly using the folder name as
    ///    the group label.
    /// 3. Project path was `(no git)` and is now a repo → the
    ///    `git worktree list` call picks up the real branch(es) and
    ///    appends them as additional Workspaces; the placeholder
    ///    "(no git)" Workspace is left alone so any terminals/tabs the
    ///    user opened there keep working.
    ///
    /// Missing projects (path gone) are skipped — the existing
    /// missing-project modal flow already handles them.
    pub fn reindex_git_state(&mut self, ctx: &egui::Context) {
        use std::collections::HashSet;

        let existing_project_paths: HashSet<PathBuf> =
            self.projects.iter().map(|p| p.path.clone()).collect();

        // Dedup scan roots: group parents (for already-grouped Projects)
        // and standalone Project paths (for fresh clones under a
        // not-yet-grouped folder).
        let mut scan_roots: Vec<(PathBuf, Option<String>)> = Vec::new();
        for p in &self.projects {
            if p.missing || !p.path.exists() {
                continue;
            }
            let (root, name) = match &p.group_path {
                Some(gp) => (gp.clone(), p.group_name.clone()),
                None => (p.path.clone(), None),
            };
            if !scan_roots.iter().any(|(r, _)| r == &root) {
                scan_roots.push((root, name));
            }
        }

        let mut new_sub_projects: Vec<(PathBuf, PathBuf, String)> = Vec::new();
        // Standalone Projects that need to be promoted into a group
        // header — happens when we discover new nested clones under
        // a previously standalone Project's path. Keyed by the root
        // path so the existing Project and the new siblings all
        // end up sharing the same `group_path`.
        let mut promote_to_group: std::collections::HashMap<PathBuf, String> =
            std::collections::HashMap::new();
        for (root, group_name) in &scan_roots {
            let path_is_repo = root.join(".git").exists();
            let discovered = crate::git::discover_repos(root, 5);
            let mut siblings: Vec<PathBuf> = Vec::new();
            for repo_path in discovered {
                if &repo_path == root {
                    continue;
                }
                if existing_project_paths.contains(&repo_path) {
                    continue;
                }
                // Same filter as add_project_from_path: when the root
                // is itself a repo, only promote clones the parent
                // doesn't track (gitignored, non-submodule).
                if path_is_repo
                    && (!crate::git::is_path_ignored(root, &repo_path)
                        || crate::git::is_submodule(root, &repo_path))
                {
                    continue;
                }
                siblings.push(repo_path);
            }
            if siblings.is_empty() {
                continue;
            }
            let effective_name = group_name.clone().unwrap_or_else(|| {
                root.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("group")
                    .to_string()
            });
            // Standalone Project → needs to join the group we're
            // about to create so it renders under one header with
            // the new siblings.
            if group_name.is_none() {
                promote_to_group.insert(root.clone(), effective_name.clone());
            }
            for repo_path in siblings {
                new_sub_projects.push((repo_path, root.clone(), effective_name.clone()));
            }
        }

        // Refresh worktrees for each existing non-missing project —
        // catches `git worktree add` done outside Crane, and the
        // "(no git) → now git" case where `list_workspaces` suddenly
        // returns real branches.
        let refresh_targets: Vec<(ProjectId, PathBuf)> = self
            .projects
            .iter()
            .filter(|p| !p.missing && p.path.exists())
            .map(|p| (p.id, p.path.clone()))
            .collect();

        for (pid, path) in refresh_targets {
            let new_infos = crate::git::list_workspaces(&path);
            if new_infos.is_empty() {
                continue;
            }
            let existing_wt_paths: HashSet<PathBuf> = self
                .projects
                .iter()
                .find(|p| p.id == pid)
                .map(|p| p.workspaces.iter().map(|w| w.path.clone()).collect())
                .unwrap_or_default();
            for info in new_infos {
                if existing_wt_paths.contains(&info.path) {
                    continue;
                }
                let wt_id = self.next_workspace;
                self.next_workspace += 1;
                let tab_id = self.next_tab;
                self.next_tab += 1;
                let mut layout = Layout::new(info.path.clone());
                layout.ensure_initial_welcome();
                let tab = Tab {
                    id: tab_id,
                    name: "Terminal".into(),
                    layout,
                    tint: None,
                };
                let new_workspace = Workspace {
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
                    tint: None,
                };
                if let Some(project) = self.projects.iter_mut().find(|p| p.id == pid) {
                    project.workspaces.push(new_workspace);
                }
            }
        }

        // Catch the "already-grouped previously, but the parent Project
        // at the group root itself is still standalone" case — i.e. an
        // earlier reindex added sibling Sub-projects under a group_path
        // without promoting the original Project, so the Left Panel
        // renders two headers for the same directory (one cube + one
        // folder). For each existing group's group_path, if there's a
        // standalone Project whose own path matches that group_path,
        // promote it so they collapse under one header.
        let existing_group_paths: std::collections::HashMap<PathBuf, String> = self
            .projects
            .iter()
            .filter_map(|p| {
                let gp = p.group_path.as_ref()?;
                let name = p.group_name.clone().unwrap_or_else(|| {
                    gp.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("group")
                        .to_string()
                });
                Some((gp.clone(), name))
            })
            .collect();
        for p in &self.projects {
            if p.missing || !p.path.exists() || p.group_path.is_some() {
                continue;
            }
            if let Some(name) = existing_group_paths.get(&p.path) {
                promote_to_group
                    .entry(p.path.clone())
                    .or_insert_with(|| name.clone());
            }
        }

        // Promote previously-standalone Projects into the group we're
        // about to create — must run BEFORE add_single_project so the
        // existing Project and the new siblings share one group_path,
        // rendering under a single folder header in the Left Panel.
        for (root, name) in &promote_to_group {
            if let Some(project) = self
                .projects
                .iter_mut()
                .find(|p| p.path == *root && p.group_path.is_none())
            {
                project.group_path = Some(root.clone());
                project.group_name = Some(name.clone());
            }
        }

        // Apply new sub-projects last — add_single_project mutates
        // self.projects which would invalidate the scan snapshot.
        for (path, group_root, group_name) in new_sub_projects {
            self.add_single_project(path, Some(group_root), Some(group_name), ctx);
        }

        // Cluster each group's members contiguously, anchored at the
        // first-occurrence slot. Without this pass, a group's existing
        // siblings (restored from the last session) and newly-appended
        // ones (from `add_single_project` above) can be interleaved
        // with unrelated standalone Projects — the Left Panel then
        // re-renders a folder header every time `group_path` flips
        // between adjacent rows, producing the duplicate headers the
        // reindex was supposed to collapse.
        let n = self.projects.len();
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut placed_groups: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        for i in 0..n {
            match &self.projects[i].group_path {
                None => order.push(i),
                Some(gp) => {
                    if placed_groups.contains(gp) {
                        continue;
                    }
                    let gp = gp.clone();
                    placed_groups.insert(gp.clone());
                    for j in 0..n {
                        if self.projects[j].group_path.as_ref() == Some(&gp) {
                            order.push(j);
                        }
                    }
                }
            }
        }
        if order.len() == n {
            let mut slots: Vec<Option<Project>> =
                std::mem::take(&mut self.projects).into_iter().map(Some).collect();
            let mut reordered: Vec<Project> = Vec::with_capacity(n);
            for idx in order {
                if let Some(p) = slots[idx].take() {
                    reordered.push(p);
                }
            }
            self.projects = reordered;
        }

        // Demote any group whose live member count has dropped to ≤1.
        // Happens when a sibling repo was deleted on disk outside Crane
        // (the `Remove` UI path already flattens via `remove_project`,
        // but nothing rebalances on session restore). A folder wrapping
        // a single Project is indistinguishable from a standalone, so
        // the survivor — plus any still-missing siblings — get their
        // group bindings cleared and render flush-left.
        self.rebalance_groups();
    }

    /// Collapse any `group_path` that no longer has at least two live
    /// members (`!missing && path.exists()`). Clears `group_path` /
    /// `group_name` on every remaining member (including missing ones
    /// that previously belonged to the group) and drops the matching
    /// `group_tints` / `group_collapsed` entries.
    fn rebalance_groups(&mut self) {
        use std::collections::HashMap;
        let mut live_counts: HashMap<PathBuf, usize> = HashMap::new();
        let mut total_counts: HashMap<PathBuf, usize> = HashMap::new();
        for p in &self.projects {
            if let Some(gp) = &p.group_path {
                *total_counts.entry(gp.clone()).or_insert(0) += 1;
                if !p.missing && p.path.exists() {
                    *live_counts.entry(gp.clone()).or_insert(0) += 1;
                }
            }
        }
        let to_demote: Vec<PathBuf> = total_counts
            .into_iter()
            .filter_map(|(gp, _total)| {
                let live = live_counts.get(&gp).copied().unwrap_or(0);
                if live <= 1 { Some(gp) } else { None }
            })
            .collect();
        if to_demote.is_empty() {
            return;
        }
        for gp in &to_demote {
            for p in self.projects.iter_mut() {
                if p.group_path.as_ref() == Some(gp) {
                    p.group_path = None;
                    p.group_name = None;
                }
            }
            self.group_tints.remove(gp);
            self.group_collapsed.remove(gp);
        }
    }

    /// Remove every Project whose `group_path` matches `group`. Used
    /// by the Left Panel's folder-header context menu to unload an
    /// entire group (all sibling Sub-projects) in one action.
    pub fn remove_group(&mut self, group: &std::path::Path) {
        let ids: Vec<ProjectId> = self
            .projects
            .iter()
            .filter(|p| p.group_path.as_deref() == Some(group))
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.remove_project(id);
        }
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
        use crate::state::layout::PaneContent;
        let layout = self.active_layout_ref()?;
        if let Some(id) = layout.focus
            && let Some(p) = layout.panes.get(&id)
            && let PaneContent::Files(files) = &p.content
            && let Some(t) = files.tabs.get(files.active)
        {
            return Some(t.path.clone());
        }
        for p in layout.panes.values() {
            if let PaneContent::Files(files) = &p.content
                && let Some(t) = files.tabs.get(files.active)
            {
                return Some(t.path.clone());
            }
        }
        None
    }

    /// After a file save, update every open Diff pane whose right side
    /// points at `path` so the shown diff reflects the new working-tree
    /// text. Left side (HEAD) is re-read too in case the user committed
    /// between opens. `new_text` is the freshly-saved buffer so we avoid
    /// an extra disk read when the caller already has it.
    pub fn refresh_diff_panes_for_path(&mut self, path: &str, new_text: &str) {
        use crate::state::layout::PaneContent;
        for project in &mut self.projects {
            for workspace in &mut project.workspaces {
                for tab in &mut workspace.tabs {
                    for (_, pane) in tab.layout.panes.iter_mut() {
                        let PaneContent::Diff(diff) = &mut pane.content else {
                            continue;
                        };
                        for dt in diff.tabs.iter_mut() {
                            if dt.right_path != path {
                                continue;
                            }
                            dt.right_text = new_text.to_string();
                            // Re-read HEAD version — cheap (`git show`)
                            // and keeps the left side correct after a
                            // commit lands while the diff is open.
                            if let Some(left) = dt
                                .left_path
                                .strip_prefix("HEAD:")
                                .and_then(|rel| {
                                    crate::git::find_git_root(std::path::Path::new(path))
                                        .map(|root| (root, rel.to_string()))
                                })
                            {
                                let (root, rel) = left;
                                dt.left_text =
                                    crate::git::head_content(&root, &rel);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn active_workspace_path(&self) -> Option<&Path> {
        let (pid, wid, _) = self.active?;
        let project = self.projects.iter().find(|p| p.id == pid)?;
        let wt = project.workspaces.iter().find(|w| w.id == wid)?;
        Some(&wt.path)
    }

    /// Drop any open File Tab whose path matches `path`. Called after
    /// a Files-Pane delete or move so the editor doesn't keep
    /// pointing at a non-existent file. Walks every project →
    /// workspace → tab → pane the same way `refresh_diff_panes_for_path`
    /// does — paths can be opened in multiple Layouts at once and we
    /// want to clean them all.
    pub fn close_file_tabs_for_path(&mut self, path: &Path) {
        use crate::state::layout::PaneContent;
        let path_str = path.to_string_lossy().to_string();
        for project in &mut self.projects {
            for workspace in &mut project.workspaces {
                for tab in &mut workspace.tabs {
                    for (_, pane) in tab.layout.panes.iter_mut() {
                        let PaneContent::Files(files) = &mut pane.content else {
                            continue;
                        };
                        // Iterate from the back so we can swap_remove
                        // without shifting later indices.
                        let mut i = files.tabs.len();
                        while i > 0 {
                            i -= 1;
                            if files.tabs[i].path == path_str {
                                files.tabs.remove(i);
                                if files.active >= files.tabs.len()
                                    && !files.tabs.is_empty()
                                {
                                    files.active = files.tabs.len() - 1;
                                } else if files.tabs.is_empty() {
                                    files.active = 0;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Update any open File Tab whose path matches `src` to point at
    /// `dst` after a Files-Pane move/rename. The buffer text and
    /// dirty state are preserved; only the path + display name
    /// change.
    pub fn rename_file_tabs_for_path(&mut self, src: &Path, dst: &Path) {
        use crate::state::layout::PaneContent;
        let src_str = src.to_string_lossy().to_string();
        let dst_str = dst.to_string_lossy().to_string();
        let dst_name = dst
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&dst_str)
            .to_string();
        for project in &mut self.projects {
            for workspace in &mut project.workspaces {
                for tab in &mut workspace.tabs {
                    for (_, pane) in tab.layout.panes.iter_mut() {
                        let PaneContent::Files(files) = &mut pane.content else {
                            continue;
                        };
                        for ft in files.tabs.iter_mut() {
                            if ft.path == src_str {
                                ft.path = dst_str.clone();
                                ft.name = dst_name.clone();
                            }
                        }
                    }
                }
            }
        }
    }

    /// Kick off a git operation in a background thread. Sets status
    /// to `Running(kind)` immediately so the spinner appears the
    /// next frame, runs the op (network-bound for push/pull/fetch),
    /// then writes the result back as `Done` or `Failed`. Repaint is
    /// requested at completion so the UI doesn't have to poll. The
    /// `commit_message` arg is only consulted for Commit.
    ///
    /// Pre-checks short-circuit before spawning a thread when the op
    /// would be a guaranteed no-op (Push with `ahead == 0`, Pull
    /// with `behind == 0`) — the result lands as `Done` with a
    /// plain-language explanation so the user sees "Nothing to
    /// push" instead of getting an opaque "Everything up-to-date"
    /// from git.
    pub fn dispatch_git_op(
        &self,
        kind: GitOpKind,
        repo: std::path::PathBuf,
        ctx: egui::Context,
        commit_message: Option<String>,
    ) {
        {
            let mut guard = self.git_op_status.lock();
            if matches!(*guard, GitOpStatus::Running(_)) {
                return;
            }
            *guard = GitOpStatus::Running(kind);
        }

        // Short-circuit pre-checks. Cheap (single git rev-list).
        // Without these, the user clicks Push and waits 800ms only
        // to be told "Everything up-to-date" — confusing because it
        // looks like nothing happened. Tell them upfront instead.
        if matches!(kind, GitOpKind::Push | GitOpKind::Pull) {
            if let Some(ab) = crate::git::ahead_behind(&repo) {
                let mut guard = self.git_op_status.lock();
                if kind == GitOpKind::Push && ab.ahead == 0 {
                    *guard = GitOpStatus::Done {
                        kind,
                        message: if ab.behind > 0 {
                            format!("Nothing to push (behind {} — pull first)", ab.behind)
                        } else {
                            "Nothing to push (up to date)".into()
                        },
                    };
                    ctx.request_repaint();
                    return;
                }
                if kind == GitOpKind::Pull && ab.behind == 0 {
                    *guard = GitOpStatus::Done {
                        kind,
                        message: "Already up to date".into(),
                    };
                    ctx.request_repaint();
                    return;
                }
            }
        }

        let status = self.git_op_status.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let result: Result<String, String> = match kind {
                GitOpKind::Commit => match commit_message.as_deref() {
                    Some(msg) if !msg.trim().is_empty() => {
                        crate::git::commit(&repo, msg).map(|()| "Committed".to_string())
                    }
                    _ => Err("No commit message".into()),
                },
                GitOpKind::CommitAndPush => match commit_message.as_deref() {
                    Some(msg) if !msg.trim().is_empty() => crate::git::commit(&repo, msg)
                        .map_err(|e| e)
                        .and_then(|()| crate::git::push(&repo))
                        .map(|s| format!("Committed and pushed — {s}")),
                    _ => Err("No commit message".into()),
                },
                GitOpKind::Push => crate::git::push(&repo),
                GitOpKind::Pull => crate::git::pull(&repo),
                GitOpKind::Fetch => crate::git::fetch(&repo),
            };
            let mut guard = status.lock();
            *guard = match result {
                Ok(message) => GitOpStatus::Done { kind, message },
                Err(error) => GitOpStatus::Failed { kind, error },
            };
            drop(guard);
            ctx2.request_repaint();
        });
    }

    /// Pop the last reversible Files-Pane op and undo it. Returns
    /// true if anything was undone, false if the stack was empty or
    /// the op couldn't be reversed (e.g. trash undo on macOS, or
    /// the original location is now occupied). Errors surface via
    /// `git_error` so the user sees a soft warning in the bottom bar.
    pub fn undo_last_file_op(&mut self) -> bool {
        let Some(op) = self.file_op_history.pop_back() else {
            return false;
        };
        match op {
            FileOp::Move { from, to } => {
                if from.exists() {
                    self.git_error = Some(format!(
                        "Undo: `{}` is occupied — refusing to overwrite",
                        from.display()
                    ));
                    return false;
                }
                if let Err(e) = std::fs::rename(&to, &from) {
                    self.git_error = Some(format!("Undo move: {e}"));
                    return false;
                }
                if self.selected_file.as_deref() == Some(&to) {
                    self.selected_file = Some(from.clone());
                }
                self.rename_file_tabs_for_path(&to, &from);
                if let Some(parent) = from.parent() {
                    self.expanded_dirs.insert(parent.to_path_buf());
                }
                true
            }
            FileOp::Trash { path } => {
                // macOS: `trash::os_limited` isn't compiled in, so
                // there's no programmatic restore. Surface a hint
                // pointing the user at Finder's "Put Back" so they
                // know what to do.
                #[cfg(target_os = "macos")]
                {
                    let _ = path;
                    self.git_error = Some(
                        "Undo trash: open Finder → Trash → right-click → Put Back".into(),
                    );
                    false
                }
                #[cfg(not(target_os = "macos"))]
                {
                    use trash::os_limited;
                    // Match by path — trash items have an
                    // `original_parent` we can compare against.
                    let parent = match path.parent() {
                        Some(p) => p.to_path_buf(),
                        None => {
                            self.git_error = Some("Undo trash: no parent dir".into());
                            return false;
                        }
                    };
                    let name = match path.file_name() {
                        Some(n) => n.to_os_string(),
                        None => {
                            self.git_error = Some("Undo trash: no file name".into());
                            return false;
                        }
                    };
                    let items = match os_limited::list() {
                        Ok(items) => items,
                        Err(e) => {
                            self.git_error = Some(format!("Undo trash: list: {e}"));
                            return false;
                        }
                    };
                    let target = items.into_iter().find(|it| {
                        it.original_parent == parent && it.name == name
                    });
                    match target {
                        Some(item) => match os_limited::restore_all([item]) {
                            Ok(()) => {
                                if let Some(parent) = path.parent() {
                                    self.expanded_dirs.insert(parent.to_path_buf());
                                }
                                true
                            }
                            Err(e) => {
                                self.git_error = Some(format!("Undo trash: {e}"));
                                false
                            }
                        },
                        None => {
                            self.git_error = Some(
                                "Undo trash: not found in trash (already restored or emptied?)".into(),
                            );
                            false
                        }
                    }
                }
            }
        }
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
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            p.last_active_workspace = Some(wid);
            if let Some(w) = p.workspaces.iter_mut().find(|w| w.id == wid) {
                w.active_tab = Some(tid);
            }
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
            if let crate::state::layout::PaneContent::Files(files) = &mut pane.content {
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
        _ctx: &egui::Context,
        initial_content: Option<crate::state::layout::PaneContent>,
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
                layout.ensure_initial_welcome();
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
            tint: None,
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
        // Skip polls on projects whose root folder is gone. Spares us
        // a pile of spurious "fatal: not a git repository" subprocess
        // errors per tick while the user hasn't relocated yet.
        if let Some((pid, _, _)) = self.active
            && let Some(p) = self.projects.iter().find(|p| p.id == pid)
            && p.missing
        {
            return;
        }
        let wt = match self.active_workspace_mut() {
            Some(w) => w,
            None => return,
        };

        if let Some(rx) = wt.git_rx.as_ref()
            && let Ok(status) = rx.try_recv() {
                // Pull the branch name forward from the freshly-polled
                // git status: branches renamed outside Crane (e.g.
                // `git branch -m`) now show up on the next tick instead
                // of being frozen at worktree-creation time. Canonical
                // `name` updates; `display_name` alias stays intact.
                if let Some(s) = status.as_ref()
                    && !s.branch.is_empty()
                    && s.branch != wt.name
                {
                    wt.name = s.branch.clone();
                }
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
        // Seed the modal from this project's remembered preferences so
        // the second + N-th worktree for a project defaults to the
        // mode + path the user picked the first time.
        let default_custom = format!("{home}/.crane-worktrees/{safe}");
        let custom_path = project
            .preferred_custom_path
            .clone()
            .unwrap_or(default_custom);
        let mode = project
            .preferred_location_mode
            .unwrap_or(LocationMode::Global);
        self.new_workspace_modal = Some(NewWorkspaceModal {
            project_id: pid,
            branch: String::new(),
            custom_path,
            mode,
            create_new_branch: true,
            branch_locked: false,
            error: None,
        });
    }

    pub fn create_workspace_from_modal(&mut self, _ctx: &egui::Context) {
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
        let picked_mode = modal.mode;
        let picked_custom = modal.custom_path.clone();
        match git::workspace_add(&project_path, &wt_path, &branch, modal.create_new_branch) {
            Ok(()) => {
                let _ = project_name;
                let project = match self.projects.iter_mut().find(|p| p.id == modal.project_id) {
                    Some(p) => p,
                    None => return,
                };
                // Remember the choice for the next "+ new worktree" on
                // this project. Only capture Custom path when the user
                // actually chose Custom mode.
                project.preferred_location_mode = Some(picked_mode);
                if picked_mode == LocationMode::Custom {
                    project.preferred_custom_path = Some(picked_custom);
                }
                let wt_id = self.next_workspace;
                self.next_workspace += 1;
                let tab_id = self.next_tab;
                self.next_tab += 1;
                let mut layout = Layout::new(wt_path.clone());
                layout.ensure_initial_welcome();
                let tab = Tab {
                    id: tab_id,
                    name: "Terminal".into(),
                    layout,
                    tint: None,
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
                    tint: None,
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
        // Remember the removed project's group so we can rebalance the
        // group afterwards (promote a lone survivor out of the folder,
        // drop group-keyed state when the group empties).
        let removed_group: Option<PathBuf> = self
            .projects
            .iter()
            .find(|p| p.id == pid)
            .and_then(|p| p.group_path.clone());
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
        if let Some(gp) = removed_group {
            let remaining: Vec<ProjectId> = self
                .projects
                .iter()
                .filter(|p| p.group_path.as_ref() == Some(&gp))
                .map(|p| p.id)
                .collect();
            match remaining.len() {
                0 => {
                    // Last member gone — drop the group's tint and
                    // collapse state so re-adding the same folder
                    // doesn't inherit stale UI preferences.
                    self.group_tints.remove(&gp);
                    self.group_collapsed.remove(&gp);
                }
                1 => {
                    // A folder group containing a single project is
                    // indistinguishable from a standalone project, so
                    // flatten it — the survivor renders at the top
                    // level rather than under a one-child folder header.
                    if let Some(p) = self
                        .projects
                        .iter_mut()
                        .find(|p| p.id == remaining[0])
                    {
                        p.group_path = None;
                        p.group_name = None;
                    }
                    self.group_tints.remove(&gp);
                    self.group_collapsed.remove(&gp);
                }
                _ => {}
            }
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
