use crate::terminal::Terminal;
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Absolute path on disk. Kept as `String` so it round-trips
    /// through the session JSON schema unchanged. Migrating to
    /// `PathBuf` would force ~40 callsites in LSP / format / UI to
    /// switch from `&str` to `&Path`; we'd like to, but not today.
    pub path: String,
    pub content: String,
    pub original_content: String,
    pub name: String,
    /// Last content snapshot sent to the LSP server — used to debounce
    /// textDocument/didChange so we only push deltas when the user has
    /// actually edited the file.
    pub last_lsp_content: String,
    /// Timestamp of the most recent didChange we sent, for rate-limiting.
    pub last_lsp_sent_at: Option<std::time::Instant>,
    /// For Markdown files: when true, render the rendered-HTML preview
    /// rather than the source text editor. Toggled by the eye button in
    /// the file path row.
    pub preview_mode: bool,
    /// When a goto-definition lands inside this file (or newly opens it),
    /// we stash the target (line, character) here; the editor applies it
    /// to the TextEdit state on the next render pass.
    pub pending_cursor: Option<(u32, u32)>,
    /// Lazy-loaded GPU texture for image files (.png / .jpg / .gif / .bmp
    /// / .webp / .ico). None until the first render attempts a decode.
    #[allow(dead_code)]
    pub image_texture: Option<egui::TextureHandle>,
    /// Find bar state. None = closed; Some(query) = open and filtered.
    pub find_query: Option<String>,
    /// File mtime at last read (open / reload / save). Used to detect
    /// edits made outside Crane: if the file on disk has a newer mtime
    /// AND its bytes differ from `original_content`, we refuse to save
    /// over it without explicit user confirmation.
    pub disk_mtime: Option<std::time::SystemTime>,
    /// When set, the file on disk has changed out from under us since
    /// we last read it. The UI surfaces a banner with Reload /
    /// Overwrite / Cancel; cleared when the user picks one.
    pub external_change: bool,
    /// Primary-cursor char index captured off the TextEdit's output on
    /// the most recent render. Transient — the status strip reads it
    /// to show `Ln/Col`. Not persisted. Loading the same value via
    /// `TextEdit::load_state(te_id)` from an outer scope is unreliable
    /// when the id path depends on ancestor `push_id` scopes, so we
    /// stash it explicitly here.
    #[allow(dead_code)]
    pub last_cursor_idx: usize,
}

impl FileTab {
    pub fn dirty(&self) -> bool {
        self.content != self.original_content
    }
}

pub struct FilesPane {
    pub tabs: Vec<FileTab>,
    pub active: usize,
    #[allow(dead_code)] // kept for session schema compat
    pub input_buf: String,
    #[allow(dead_code)] // kept for session schema compat
    pub error: Option<String>,
    /// Index of a tab awaiting close confirmation (× or middle-click
    /// on a dirty tab). `None` means no modal open. Not persisted.
    pub pending_close: Option<usize>,
}

impl FilesPane {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            input_buf: String::new(),
            error: None,
            pending_close: None,
        }
    }

    pub fn open(&mut self, path: String, content: String, name: String) {
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.active = idx;
            return;
        }
        let disk_mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();
        self.tabs.push(FileTab {
            path,
            original_content: content.clone(),
            last_lsp_content: content.clone(),
            last_lsp_sent_at: None,
            preview_mode: false,
            pending_cursor: None,
            image_texture: None,
            find_query: None,
            disk_mtime,
            external_change: false,
            last_cursor_idx: 0,
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

pub struct DiffTab {
    pub title: String,
    pub left_path: String,
    pub right_path: String,
    pub left_text: String,
    pub right_text: String,
    pub error: Option<String>,
}

pub struct DiffPane {
    pub tabs: Vec<DiffTab>,
    pub active: usize,
}

impl DiffPane {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
        }
    }

    /// Open a diff tab. If one already exists for the same
    /// `(left_path, right_path)` pair, refresh its contents and focus
    /// it instead of adding a duplicate — so reopening a file's diff
    /// doesn't spawn an endless stack of tabs.
    pub fn open(
        &mut self,
        title: String,
        left_path: String,
        right_path: String,
        left_text: String,
        right_text: String,
    ) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.left_path == left_path && t.right_path == right_path)
        {
            let t = &mut self.tabs[idx];
            t.title = title;
            t.left_text = left_text;
            t.right_text = right_text;
            t.error = None;
            self.active = idx;
            return;
        }
        self.tabs.push(DiffTab {
            title,
            left_path,
            right_path,
            left_text,
            right_text,
            error: None,
        });
        self.active = self.tabs.len() - 1;
    }

    pub fn close(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
    }

    pub fn active_tab(&self) -> Option<&DiffTab> {
        self.tabs.get(self.active)
    }

    #[allow(dead_code)]
    pub fn active_tab_mut(&mut self) -> Option<&mut DiffTab> {
        self.tabs.get_mut(self.active)
    }
}

pub struct BrowserTab {
    /// Pane-scoped id used to key into the native webview host.
    /// Stable for the lifetime of the tab; starts at 1.
    pub id: u32,
    pub url: String,
    pub input_buf: String,
    pub title: String,
}

pub struct BrowserPane {
    pub tabs: Vec<BrowserTab>,
    pub active: usize,
    next_tab_id: u32,
}

impl BrowserPane {
    pub fn new_with(url: String, input_buf: String) -> Self {
        Self {
            tabs: vec![BrowserTab {
                id: 1,
                url,
                input_buf,
                title: String::new(),
            }],
            active: 0,
            next_tab_id: 2,
        }
    }

    pub fn new_tab(&mut self) -> u32 {
        self.new_tab_with(String::new())
    }

    pub fn new_tab_with(&mut self, url: String) -> u32 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let input_buf = if url.is_empty() {
            "https://".into()
        } else {
            url.clone()
        };
        self.tabs.push(BrowserTab {
            id,
            url,
            input_buf,
            title: String::new(),
        });
        self.active = self.tabs.len() - 1;
        id
    }

    pub fn close_tab(&mut self, idx: usize) -> Option<u32> {
        if idx >= self.tabs.len() || self.tabs.len() <= 1 {
            return None;
        }
        let removed = self.tabs.remove(idx).id;
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
        Some(removed)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut BrowserTab> {
        self.tabs.get_mut(self.active)
    }
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
    #[allow(dead_code)] // redundant with HashMap key, kept for session round-trip
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
    /// When Some, the referenced pane is rendered full-size over its
    /// layout. Purely runtime state — never serialized to the session.
    pub maximized: Option<PaneId>,
}

impl Layout {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            root: None,
            panes: HashMap::new(),
            focus: None,
            cwd,
            next_id: 1,
            maximized: None,
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

    pub fn next_pane_id(&self) -> PaneId {
        self.next_id
    }

    pub fn set_next_pane_id(&mut self, id: PaneId) {
        self.next_id = id.max(self.next_id);
    }

    pub fn open_or_focus_diff(
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
                    if let PaneContent::Diff(diff) = &mut pane.content {
                        diff.open(
                            title.clone(),
                            left_path,
                            right_path,
                            left_text,
                            right_text,
                        );
                    }
                    pane.title = title;
                }
                self.focus = Some(pid);
            }
            None => {
                let mut diff = DiffPane::empty();
                diff.open(
                    title.clone(),
                    left_path,
                    right_path,
                    left_text,
                    right_text,
                );
                self.add_pane(PaneContent::Diff(diff), Some(Dir::Horizontal));
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
