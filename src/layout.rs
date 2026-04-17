use crate::terminal::Terminal;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type PaneId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Horizontal,
    Vertical,
}

#[derive(Clone)]
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
    pub original_content: String,
    pub name: String,
}

impl FileTab {
    pub fn dirty(&self) -> bool {
        self.content != self.original_content
    }
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
        self.tabs.push(FileTab {
            path,
            original_content: content.clone(),
            content,
            name,
        });
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

pub struct Layout {
    pub root: Option<Node>,
    pub panes: HashMap<PaneId, Pane>,
    pub focus: Option<PaneId>,
    pub cwd: PathBuf,
    next_id: PaneId,
}

impl Layout {
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

    pub fn next_pane_id(&self) -> PaneId {
        self.next_id
    }

    pub fn set_next_pane_id(&mut self, id: PaneId) {
        self.next_id = id.max(self.next_id);
    }

    pub fn open_or_replace_diff(
        &mut self,
        left_path: String,
        right_path: String,
        left_text: String,
        right_text: String,
        title: String,
    ) {
        let existing = self
            .panes
            .iter()
            .find(|(_, p)| matches!(p.content, PaneContent::Diff(_)))
            .map(|(id, _)| *id);
        match existing {
            Some(pid) => {
                if let Some(pane) = self.panes.get_mut(&pid) {
                    pane.title = title;
                    if let PaneContent::Diff(diff) = &mut pane.content {
                        diff.left_path = left_path;
                        diff.right_path = right_path;
                        diff.left_text = left_text;
                        diff.right_text = right_text;
                        diff.error = None;
                    }
                }
                self.focus = Some(pid);
            }
            None => {
                self.add_pane(
                    PaneContent::Diff(DiffPane {
                        left_path,
                        right_path,
                        left_text,
                        right_text,
                        left_buf: String::new(),
                        right_buf: String::new(),
                        error: None,
                    }),
                    Some(Dir::Horizontal),
                );
                if let Some(focus) = self.focus
                    && let Some(pane) = self.panes.get_mut(&focus) {
                        pane.title = title;
                    }
            }
        }
    }

    pub fn open_file_in_files_pane(&mut self, path: String, name: String, content: String) {
        let existing = self
            .panes
            .iter()
            .find(|(_, p)| matches!(p.content, PaneContent::Files(_)))
            .map(|(id, _)| *id);
        match existing {
            Some(pid) => {
                if let Some(pane) = self.panes.get_mut(&pid)
                    && let PaneContent::Files(files) = &mut pane.content {
                        files.open(path, content, name);
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

    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) {
        if a == b {
            return;
        }
        if let Some(root) = self.root.as_mut() {
            swap_leaves(root, a, b);
        }
    }

    /// Remove `src` from its current position and re-insert it adjacent to
    /// `target` on the given edge. No-op if src == target.
    pub fn dock_pane(&mut self, src: PaneId, target: PaneId, edge: DockEdge) {
        if src == target {
            return;
        }
        let root = match self.root.take() {
            Some(r) => r,
            None => return,
        };
        let (root_without_src, _) = remove_node(root, src);
        let Some(root_without_src) = root_without_src else {
            self.root = None;
            return;
        };
        self.root = Some(wrap_target(root_without_src, target, src, edge));
        self.focus = Some(src);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockEdge {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

fn wrap_target(node: Node, target: PaneId, src: PaneId, edge: DockEdge) -> Node {
    match node {
        Node::Leaf(id) if id == target => {
            let (direction, src_first) = match edge {
                DockEdge::Left => (Dir::Horizontal, true),
                DockEdge::Right => (Dir::Horizontal, false),
                DockEdge::Top => (Dir::Vertical, true),
                DockEdge::Bottom => (Dir::Vertical, false),
                DockEdge::Center => return Node::Leaf(id),
            };
            let (first, second) = if src_first {
                (Node::Leaf(src), Node::Leaf(target))
            } else {
                (Node::Leaf(target), Node::Leaf(src))
            };
            Node::Split {
                direction,
                first: Box::new(first),
                second: Box::new(second),
                ratio: 0.5,
            }
        }
        Node::Leaf(_) => node,
        Node::Split {
            direction,
            first,
            second,
            ratio,
        } => Node::Split {
            direction,
            first: Box::new(wrap_target(*first, target, src, edge)),
            second: Box::new(wrap_target(*second, target, src, edge)),
            ratio,
        },
    }
}

fn swap_leaves(node: &mut Node, a: PaneId, b: PaneId) {
    match node {
        Node::Leaf(id) => {
            if *id == a {
                *id = b;
            } else if *id == b {
                *id = a;
            }
        }
        Node::Split { first, second, .. } => {
            swap_leaves(first, a, b);
            swap_leaves(second, a, b);
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

pub fn prune_leaf(node: Node, target: PaneId) -> (Option<Node>, Option<PaneId>) {
    remove_node(node, target)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: PaneId) -> Node {
        Node::Leaf(id)
    }

    fn split(dir: Dir, first: Node, second: Node) -> Node {
        Node::Split {
            direction: dir,
            first: Box::new(first),
            second: Box::new(second),
            ratio: 0.5,
        }
    }

    #[test]
    fn split_at_on_leaf_wraps_into_split() {
        let tree = leaf(1);
        let out = split_at(tree, 1, 2, Dir::Horizontal);
        match out {
            Node::Split { direction, first, second, ratio } => {
                assert_eq!(direction, Dir::Horizontal);
                assert!(matches!(*first, Node::Leaf(1)));
                assert!(matches!(*second, Node::Leaf(2)));
                assert_eq!(ratio, 0.5);
            }
            _ => panic!("expected Split"),
        }
    }

    #[test]
    fn split_at_missing_target_noop() {
        let tree = split(Dir::Horizontal, leaf(1), leaf(2));
        let out = split_at(tree, 99, 3, Dir::Vertical);
        assert_eq!(collect_leaves(Some(&out)), vec![1, 2]);
    }

    #[test]
    fn remove_leaf_promotes_sibling() {
        let tree = split(Dir::Horizontal, leaf(1), leaf(2));
        let (new_tree, focus) = remove_node(tree, 1);
        assert!(matches!(new_tree, Some(Node::Leaf(2))));
        assert_eq!(focus, Some(2));
    }

    #[test]
    fn remove_last_leaf_returns_none() {
        let tree = leaf(1);
        let (new_tree, focus) = remove_node(tree, 1);
        assert!(new_tree.is_none());
        assert_eq!(focus, None);
    }

    #[test]
    fn first_leaf_finds_leftmost() {
        let tree = split(Dir::Horizontal, split(Dir::Vertical, leaf(1), leaf(2)), leaf(3));
        assert_eq!(first_leaf(Some(&tree)), Some(1));
    }

    #[test]
    fn collect_leaves_in_order() {
        let tree = split(Dir::Horizontal, split(Dir::Vertical, leaf(1), leaf(2)), leaf(3));
        assert_eq!(collect_leaves(Some(&tree)), vec![1, 2, 3]);
    }

    #[test]
    fn contains_walks_tree() {
        let tree = split(Dir::Horizontal, leaf(1), split(Dir::Vertical, leaf(2), leaf(3)));
        assert!(contains(&tree, 1));
        assert!(contains(&tree, 2));
        assert!(contains(&tree, 3));
        assert!(!contains(&tree, 99));
    }

    #[test]
    fn set_ratio_updates_target() {
        let mut tree = split(Dir::Horizontal, leaf(1), split(Dir::Vertical, leaf(2), leaf(3)));
        set_ratio(&mut tree, &[1], 0.75);
        match &tree {
            Node::Split { second, .. } => {
                if let Node::Split { ratio, .. } = second.as_ref() {
                    assert!((ratio - 0.75).abs() < f32::EPSILON);
                } else {
                    panic!("expected inner split");
                }
            }
            _ => panic!("expected outer split"),
        }
    }
}
