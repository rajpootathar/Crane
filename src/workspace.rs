use crate::terminal::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type PaneId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Horizontal,
    Vertical,
}

pub enum Node {
    Leaf(PaneId),
    Split {
        direction: Dir,
        first: Box<Node>,
        second: Box<Node>,
        ratio: f32,
    },
}

pub struct FileTab {
    pub path: String,
    pub content: String,
    pub name: String,
}

pub struct FilesPane {
    pub tabs: Vec<FileTab>,
    pub active: usize,
    pub input_buf: String,
    pub error: Option<String>,
}

impl FilesPane {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            input_buf: String::new(),
            error: None,
        }
    }

    pub fn open(&mut self, path: String, content: String, name: String) {
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.active = idx;
            return;
        }
        self.tabs.push(FileTab { path, content, name });
        self.active = self.tabs.len() - 1;
    }

    pub fn close(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.tabs.remove(idx);
            if self.active >= self.tabs.len() && !self.tabs.is_empty() {
                self.active = self.tabs.len() - 1;
            } else if self.tabs.is_empty() {
                self.active = 0;
            }
        }
    }
}

pub struct MarkdownPane {
    pub path: String,
    pub content: String,
    pub input_buf: String,
    pub error: Option<String>,
}

pub struct DiffPane {
    pub left_path: String,
    pub right_path: String,
    pub left_text: String,
    pub right_text: String,
    pub left_buf: String,
    pub right_buf: String,
    pub error: Option<String>,
}

pub struct BrowserPane {
    pub url: String,
    pub input_buf: String,
}

pub enum PaneContent {
    Terminal(Terminal),
    Files(FilesPane),
    Markdown(MarkdownPane),
    Diff(DiffPane),
    Browser(BrowserPane),
}

impl PaneContent {
    pub fn kind_label(&self) -> &'static str {
        match self {
            PaneContent::Terminal(_) => "Terminal",
            PaneContent::Files(_) => "Files",
            PaneContent::Markdown(_) => "Markdown",
            PaneContent::Diff(_) => "Diff",
            PaneContent::Browser(_) => "Browser",
        }
    }
}

pub struct Pane {
    pub id: PaneId,
    pub title: String,
    pub content: PaneContent,
}

pub struct Workspace {
    pub root: Option<Node>,
    pub panes: HashMap<PaneId, Pane>,
    pub focus: Option<PaneId>,
    pub cwd: PathBuf,
    next_id: PaneId,
}

impl Workspace {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            root: None,
            panes: HashMap::new(),
            focus: None,
            cwd,
            next_id: 1,
        }
    }

    pub fn ensure_initial_terminal(&mut self, ctx: &egui::Context) {
        if self.root.is_none() {
            let cwd = self.cwd.clone();
            if let Ok(term) = Terminal::spawn(ctx.clone(), 80, 24, Some(&cwd)) {
                self.add_root(PaneContent::Terminal(term), "Terminal 1".into());
            }
        }
    }

    fn add_root(&mut self, content: PaneContent, title: String) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        self.panes.insert(
            id,
            Pane {
                id,
                title,
                content,
            },
        );
        self.root = Some(Node::Leaf(id));
        self.focus = Some(id);
        id
    }

    pub fn add_pane(&mut self, content: PaneContent, split: Option<Dir>) {
        let id = self.next_id;
        self.next_id += 1;
        let title = format!("{} {}", content.kind_label(), id);
        self.panes.insert(
            id,
            Pane {
                id,
                title,
                content,
            },
        );
        match (self.root.take(), self.focus, split) {
            (None, _, _) => {
                self.root = Some(Node::Leaf(id));
            }
            (Some(root), Some(focus), Some(dir)) => {
                self.root = Some(split_at(root, focus, id, dir));
            }
            (Some(root), _, _) => {
                if let Some(first) = first_leaf(Some(&root)) {
                    self.root = Some(split_at(root, first, id, Dir::Horizontal));
                } else {
                    self.root = Some(Node::Leaf(id));
                }
            }
        }
        self.focus = Some(id);
    }

    pub fn split_focused_with_terminal(&mut self, ctx: &egui::Context, dir: Dir) {
        let cwd = self.cwd.clone();
        if let Ok(term) = Terminal::spawn(ctx.clone(), 80, 24, Some(&cwd)) {
            self.add_pane(PaneContent::Terminal(term), Some(dir));
        }
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn open_file_in_files_pane(&mut self, path: String, name: String, content: String) {
        let existing = self
            .panes
            .iter()
            .find(|(_, p)| matches!(p.content, PaneContent::Files(_)))
            .map(|(id, _)| *id);
        match existing {
            Some(pid) => {
                if let Some(pane) = self.panes.get_mut(&pid) {
                    if let PaneContent::Files(files) = &mut pane.content {
                        files.open(path, content, name);
                    }
                }
                self.focus = Some(pid);
            }
            None => {
                let mut files = FilesPane::empty();
                files.open(path, content, name);
                self.add_pane(PaneContent::Files(files), Some(Dir::Horizontal));
            }
        }
    }

    pub fn close_focused(&mut self) {
        let focus = match self.focus {
            Some(f) => f,
            None => return,
        };
        self.panes.remove(&focus);
        let root = match self.root.take() {
            Some(r) => r,
            None => return,
        };
        let (new_root, sibling) = remove_node(root, focus);
        self.root = new_root;
        self.focus = sibling.or_else(|| first_leaf(self.root.as_ref()));
    }

    pub fn focus_next(&mut self) {
        let leaves = collect_leaves(self.root.as_ref());
        if leaves.is_empty() {
            self.focus = None;
            return;
        }
        let idx = self
            .focus
            .and_then(|f| leaves.iter().position(|&x| x == f))
            .unwrap_or(0);
        self.focus = Some(leaves[(idx + 1) % leaves.len()]);
    }

    pub fn focus_prev(&mut self) {
        let leaves = collect_leaves(self.root.as_ref());
        if leaves.is_empty() {
            self.focus = None;
            return;
        }
        let idx = self
            .focus
            .and_then(|f| leaves.iter().position(|&x| x == f))
            .unwrap_or(0);
        let next = if idx == 0 { leaves.len() - 1 } else { idx - 1 };
        self.focus = Some(leaves[next]);
    }

    pub fn set_split_ratio(&mut self, path: &[usize], ratio: f32) {
        if let Some(root) = self.root.as_mut() {
            set_ratio(root, path, ratio.clamp(0.05, 0.95));
        }
    }
}

fn split_at(node: Node, target: PaneId, new_pane: PaneId, dir: Dir) -> Node {
    match node {
        Node::Leaf(id) if id == target => Node::Split {
            direction: dir,
            first: Box::new(Node::Leaf(id)),
            second: Box::new(Node::Leaf(new_pane)),
            ratio: 0.5,
        },
        Node::Leaf(_) => node,
        Node::Split {
            direction,
            first,
            second,
            ratio,
        } => Node::Split {
            direction,
            first: Box::new(split_at(*first, target, new_pane, dir)),
            second: Box::new(split_at(*second, target, new_pane, dir)),
            ratio,
        },
    }
}

fn remove_node(node: Node, target: PaneId) -> (Option<Node>, Option<PaneId>) {
    match node {
        Node::Leaf(id) if id == target => (None, None),
        Node::Leaf(_) => (Some(node), None),
        Node::Split {
            direction,
            first,
            second,
            ratio,
        } => {
            if contains(&first, target) {
                let (new_first, _) = remove_node(*first, target);
                match new_first {
                    Some(n) => (
                        Some(Node::Split {
                            direction,
                            first: Box::new(n),
                            second,
                            ratio,
                        }),
                        None,
                    ),
                    None => {
                        let focus = first_leaf(Some(&second));
                        (Some(*second), focus)
                    }
                }
            } else {
                let (new_second, _) = remove_node(*second, target);
                match new_second {
                    Some(n) => (
                        Some(Node::Split {
                            direction,
                            first,
                            second: Box::new(n),
                            ratio,
                        }),
                        None,
                    ),
                    None => {
                        let focus = first_leaf(Some(&first));
                        (Some(*first), focus)
                    }
                }
            }
        }
    }
}

fn contains(node: &Node, id: PaneId) -> bool {
    match node {
        Node::Leaf(n) => *n == id,
        Node::Split { first, second, .. } => contains(first, id) || contains(second, id),
    }
}

fn first_leaf(node: Option<&Node>) -> Option<PaneId> {
    node.and_then(|n| match n {
        Node::Leaf(id) => Some(*id),
        Node::Split { first, .. } => first_leaf(Some(first)),
    })
}

fn collect_leaves(node: Option<&Node>) -> Vec<PaneId> {
    let mut v = Vec::new();
    fn recur(n: &Node, out: &mut Vec<PaneId>) {
        match n {
            Node::Leaf(id) => out.push(*id),
            Node::Split { first, second, .. } => {
                recur(first, out);
                recur(second, out);
            }
        }
    }
    if let Some(n) = node {
        recur(n, &mut v);
    }
    v
}

fn set_ratio(node: &mut Node, path: &[usize], ratio: f32) {
    if path.is_empty() {
        if let Node::Split { ratio: r, .. } = node {
            *r = ratio;
        }
        return;
    }
    if let Node::Split { first, second, .. } = node {
        let (head, tail) = (path[0], &path[1..]);
        if head == 0 {
            set_ratio(first, tail, ratio);
        } else {
            set_ratio(second, tail, ratio);
        }
    }
}
