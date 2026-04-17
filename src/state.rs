use crate::git::{self, GitStatus};
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub type ProjectId = u64;
pub type WorktreeId = u64;
pub type TabId = u64;

pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub workspace: Workspace,
}

pub struct Worktree {
    pub id: WorktreeId,
    pub name: String,
    pub path: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
    pub expanded: bool,
    pub git_status: Option<GitStatus>,
    pub last_status_refresh: Option<Instant>,
}

pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub worktrees: Vec<Worktree>,
    pub expanded: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RightTab {
    Changes,
    Files,
}

pub struct App {
    pub projects: Vec<Project>,
    pub active: Option<(ProjectId, WorktreeId, TabId)>,
    pub last_worktree: Option<(ProjectId, WorktreeId)>,
    pub show_left: bool,
    pub show_right: bool,
    pub right_tab: RightTab,
    pub commit_message: String,
    pub git_error: Option<String>,
    pub add_project_buf: String,
    pub font_size: f32,
    pub expanded_dirs: HashSet<PathBuf>,
    next_project: ProjectId,
    next_worktree: WorktreeId,
    next_tab: TabId,
}

impl App {
    pub fn new() -> Self {
        Self {
            projects: Vec::new(),
            active: None,
            last_worktree: None,
            show_left: true,
            show_right: true,
            right_tab: RightTab::Changes,
            commit_message: String::new(),
            git_error: None,
            add_project_buf: String::new(),
            font_size: 14.0,
            expanded_dirs: HashSet::new(),
            next_project: 1,
            next_worktree: 1,
            next_tab: 1,
        }
    }

    pub fn ensure_initial(&mut self, ctx: &egui::Context) {
        if self.projects.is_empty() {
            if let Ok(cwd) = std::env::current_dir() {
                let root = git::find_repo_root(&cwd).unwrap_or(cwd);
                self.add_project_from_path(root, ctx);
            }
        }
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

        let infos = git::list_worktrees(&path);
        let infos = if infos.is_empty() {
            vec![git::WorktreeInfo {
                path: path.clone(),
                branch: "(no git)".into(),
            }]
        } else {
            infos
        };

        let mut worktrees = Vec::new();
        let mut first_active: Option<(WorktreeId, TabId)> = None;
        for info in infos {
            let wt_id = self.next_worktree;
            self.next_worktree += 1;
            let tab_id = self.next_tab;
            self.next_tab += 1;
            let mut workspace = Workspace::new(info.path.clone());
            workspace.ensure_initial_terminal(ctx);
            let tab = Tab {
                id: tab_id,
                name: "Terminal".into(),
                workspace,
            };
            if first_active.is_none() {
                first_active = Some((wt_id, tab_id));
            }
            worktrees.push(Worktree {
                id: wt_id,
                name: info.branch,
                path: info.path,
                tabs: vec![tab],
                active_tab: Some(tab_id),
                expanded: true,
                git_status: None,
                last_status_refresh: None,
            });
        }

        self.projects.push(Project {
            id,
            name,
            path,
            worktrees,
            expanded: true,
        });
        if let Some((wt, tab)) = first_active {
            self.active = Some((id, wt, tab));
            self.last_worktree = Some((id, wt));
        }
        Some(id)
    }

    pub fn active_worktree_path(&self) -> Option<&Path> {
        let (pid, wid, _) = self.active?;
        let project = self.projects.iter().find(|p| p.id == pid)?;
        let wt = project.worktrees.iter().find(|w| w.id == wid)?;
        Some(&wt.path)
    }

    pub fn active_workspace(&mut self) -> Option<&mut Workspace> {
        let (pid, wid, tid) = self.active?;
        let project = self.projects.iter_mut().find(|p| p.id == pid)?;
        let worktree = project.worktrees.iter_mut().find(|w| w.id == wid)?;
        let tab = worktree.tabs.iter_mut().find(|t| t.id == tid)?;
        Some(&mut tab.workspace)
    }

    pub fn active_worktree_mut(&mut self) -> Option<&mut Worktree> {
        let (pid, wid, _) = self.active?;
        let project = self.projects.iter_mut().find(|p| p.id == pid)?;
        project.worktrees.iter_mut().find(|w| w.id == wid)
    }

    pub fn set_active(&mut self, pid: ProjectId, wid: WorktreeId, tid: TabId) {
        self.active = Some((pid, wid, tid));
        self.last_worktree = Some((pid, wid));
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.worktrees.iter_mut().find(|w| w.id == wid) {
                w.active_tab = Some(tid);
            }
        }
    }

    pub fn new_tab_in_active_worktree(&mut self, ctx: &egui::Context) {
        self.push_tab(ctx, None, None);
    }

    pub fn new_content_tab(
        &mut self,
        ctx: &egui::Context,
        content: crate::workspace::PaneContent,
        name: String,
    ) {
        self.push_tab(ctx, Some(content), Some(name));
    }

    fn push_tab(
        &mut self,
        ctx: &egui::Context,
        initial_content: Option<crate::workspace::PaneContent>,
        tab_name: Option<String>,
    ) {
        let (pid, wid) = match self.active.map(|(p, w, _)| (p, w)).or(self.last_worktree) {
            Some(a) => a,
            None => return,
        };
        let tab_id = self.next_tab;
        self.next_tab += 1;
        let project = match self.projects.iter_mut().find(|p| p.id == pid) {
            Some(p) => p,
            None => return,
        };
        let wt = match project.worktrees.iter_mut().find(|w| w.id == wid) {
            Some(w) => w,
            None => return,
        };
        let mut workspace = Workspace::new(wt.path.clone());
        let name = match initial_content {
            None => {
                workspace.ensure_initial_terminal(ctx);
                tab_name.unwrap_or_else(|| format!("Tab {}", wt.tabs.len() + 1))
            }
            Some(content) => {
                let default = content.kind_label().to_string();
                workspace.add_pane(content, None);
                tab_name.unwrap_or(default)
            }
        };
        wt.tabs.push(Tab {
            id: tab_id,
            name,
            workspace,
        });
        wt.active_tab = Some(tab_id);
        self.active = Some((pid, wid, tab_id));
        self.last_worktree = Some((pid, wid));
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
        let wt = match project.worktrees.iter_mut().find(|w| w.id == wid) {
            Some(w) => w,
            None => return,
        };
        wt.tabs.retain(|t| t.id != tid);
        let new_tab = wt.tabs.first().map(|t| t.id);
        wt.active_tab = new_tab;
        self.active = new_tab.map(|t| (pid, wid, t));
        self.last_worktree = Some((pid, wid));
    }

    pub fn refresh_active_git_status(&mut self) {
        let now = Instant::now();
        if let Some(wt) = self.active_worktree_mut() {
            let should = wt
                .last_status_refresh
                .map(|t| now.duration_since(t) > Duration::from_millis(1500))
                .unwrap_or(true);
            if should {
                wt.git_status = git::status(&wt.path);
                wt.last_status_refresh = Some(now);
            }
        }
    }

    pub fn breadcrumb(&self) -> String {
        let (pid, wid, tid) = match self.active {
            Some(a) => a,
            None => return String::from("Crane"),
        };
        let project = self.projects.iter().find(|p| p.id == pid);
        let wt = project.and_then(|p| p.worktrees.iter().find(|w| w.id == wid));
        let tab = wt.and_then(|w| w.tabs.iter().find(|t| t.id == tid));
        format!(
            "{}  ›  {}  ›  {}",
            project.map(|p| p.name.as_str()).unwrap_or("?"),
            wt.map(|w| w.name.as_str()).unwrap_or("?"),
            tab.map(|t| t.name.as_str()).unwrap_or("?"),
        )
    }
}
