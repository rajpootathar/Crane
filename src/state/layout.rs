use crate::terminal::Terminal;
use std::collections::HashMap;
use std::path::PathBuf;

pub type PaneId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabKind {
    File,
    Terminal,
    Browser,
}

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
    /// When the find-bar's next/prev jumps to a match, this stores the
    /// target line so the scroll area can reveal it. Cleared after scroll.
    pub find_scroll_to_line: Option<u32>,
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
    /// Cached per-line git change classification. Refreshed from
    /// `git::line_changes()` when the content changes (keyed by
    /// content hash). Used by the gutter and scrollbar to paint
    /// colored change markers.
    pub line_changes: Option<crate::git::FileDiff>,
    /// Content hash at the time `line_changes` was computed. We use this
    /// to avoid re-running `git diff` every frame when nothing changed.
    pub line_changes_key: u64,
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
            find_scroll_to_line: None,
            disk_mtime,
            external_change: false,
            last_cursor_idx: 0,
            line_changes: None,
            line_changes_key: 0,
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

    pub fn take_tab(&mut self, idx: usize) -> Option<FileTab> {
        if idx >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(idx);
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        } else if self.tabs.is_empty() {
            self.active = 0;
        }
        Some(tab)
    }

    pub fn insert_tab(&mut self, idx: usize, tab: FileTab) {
        let idx = idx.min(self.tabs.len());
        self.tabs.insert(idx, tab);
        self.active = idx;
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

    pub fn take_tab(&mut self, idx: usize) -> Option<BrowserTab> {
        if idx >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
        Some(tab)
    }

    pub fn insert_tab(&mut self, idx: usize, mut tab: BrowserTab) {
        let idx = idx.min(self.tabs.len());
        tab.id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.insert(idx, tab);
        self.active = idx;
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut BrowserTab> {
        self.tabs.get_mut(self.active)
    }
}

/// Landing-page pane content. Empty on purpose — the view is stateless,
/// just renders the welcome layout (buttons + shortcut cheat-sheet) and
/// bubbles a `PaneAction` up to replace itself with a Terminal / Browser
/// pane in the same slot, or to open the Right Panel's Files tree.
#[derive(Default)]
pub struct WelcomePane;

/// One tab inside a [`TerminalPane`]. The PTY-bearing [`Terminal`]
/// stays focused on its own concerns (grid, reader thread, child
/// process); UI-only state — the optional user-set display name —
/// rides on the wrapper instead so renaming a tab doesn't leak into
/// the terminal abstraction.
pub struct TerminalTab {
    pub terminal: Terminal,
    /// User-set display name. When `Some(x)`, the tab chip renders
    /// `x` in place of the cwd-basename default; on rename-clear (`x`
    /// trimmed to empty) we drop back to `None` and the cwd label
    /// shows again.
    pub name: Option<String>,
}

impl TerminalTab {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal, name: None }
    }
}

/// Container for one or more terminals sharing a single Pane. Mirrors
/// the `FilesPane` / `BrowserPane` multi-tab pattern: only the active
/// tab renders into the pane body, the inactive ones keep streaming
/// PTY output in the background. The "+" button on the tab strip
/// appends a new tab; closing the last tab is the caller's signal to
/// close the whole Pane.
pub struct TerminalPane {
    pub tabs: Vec<TerminalTab>,
    pub active: usize,
    /// Inline rename buffer. `Some((idx, buf))` while the user is
    /// editing the chip label for tab `idx` via double-click. Enter
    /// commits, Esc cancels. Not persisted.
    pub renaming: Option<(usize, String)>,
}

impl TerminalPane {
    pub fn single(term: Terminal) -> Self {
        Self {
            tabs: vec![TerminalTab::new(term)],
            active: 0,
            renaming: None,
        }
    }

    pub fn active_terminal(&self) -> Option<&Terminal> {
        self.tabs.get(self.active).map(|t| &t.terminal)
    }

    #[allow(dead_code)]
    pub fn active_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.tabs.get_mut(self.active).map(|t| &mut t.terminal)
    }

    pub fn add(&mut self, term: Terminal) {
        self.tabs.push(TerminalTab::new(term));
        self.active = self.tabs.len() - 1;
    }

    pub fn close(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        // Cancel any in-flight rename targeting the closed tab — and
        // shift the rename index down if a tab to its left went away.
        let rid = self.renaming.as_ref().map(|(r, _)| *r);
        if let Some(rid) = rid {
            if rid == idx {
                self.renaming = None;
            } else if rid > idx
                && let Some((r, _)) = self.renaming.as_mut() {
                    *r -= 1;
                }
        }
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
    }

    pub fn take_tab(&mut self, idx: usize) -> Option<TerminalTab> {
        if idx >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(idx);
        let rid = self.renaming.as_ref().map(|(r, _)| *r);
        if let Some(rid) = rid {
            if rid == idx {
                self.renaming = None;
            } else if rid > idx
                && let Some((r, _)) = self.renaming.as_mut() {
                    *r -= 1;
                }
        }
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
        Some(tab)
    }

    pub fn insert_tab(&mut self, idx: usize, tab: TerminalTab) {
        let idx = idx.min(self.tabs.len());
        self.tabs.insert(idx, tab);
        self.active = idx;
    }
}

pub enum PaneContent {
    Terminal(TerminalPane),
    Files(FilesPane),
    Markdown(MarkdownPane),
    Diff(DiffPane),
    Browser(BrowserPane),
    Welcome(WelcomePane),
}

impl PaneContent {
    pub fn kind_label(&self) -> &'static str {
        match self {
            PaneContent::Terminal(_) => "Terminal",
            PaneContent::Files(_) => "Files",
            PaneContent::Markdown(_) => "Markdown",
            PaneContent::Diff(_) => "Diff",
            PaneContent::Browser(_) => "Browser",
            PaneContent::Welcome(_) => "New Tab",
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

    /// Seed an empty layout with the Welcome landing pane. Used for
    /// every new Tab / Workspace / Project so fresh surfaces don't
    /// auto-spawn a shell — the user picks Terminal / Browser / Files
    /// from the landing screen.
    pub fn ensure_initial_welcome(&mut self) {
        if self.root.is_none() {
            self.add_root(PaneContent::Welcome(WelcomePane), "New Tab".into());
        }
    }

    /// In-place swap of the focused pane's content + title. Used by the
    /// Welcome pane buttons to become a Terminal / Browser pane without
    /// reflowing the layout tree.
    pub fn replace_focused_content(&mut self, content: PaneContent, title: String) {
        let Some(id) = self.focus else { return };
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.content = content;
            pane.title = title;
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
            self.add_pane(PaneContent::Terminal(TerminalPane::single(term)), Some(dir));
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
        // If the file is already open in any Files pane, focus that tab.
        let found = self.panes.iter().find_map(|(id, p)| {
            if let PaneContent::Files(files) = &p.content {
                if let Some(idx) = files.tabs.iter().position(|t| t.path == path) {
                    return Some((*id, idx));
                }
            }
            None
        });
        if let Some((pid, tab_idx)) = found {
            if let Some(pane) = self.panes.get_mut(&pid)
                && let PaneContent::Files(files) = &mut pane.content {
                    files.active = tab_idx;
                }
            self.focus = Some(pid);
            return;
        }

        // Not open anywhere — open in the first Files pane (or create one).
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

    /// Extract a tab from a pane and wrap it in a fresh single-tab
    /// PaneContent of the appropriate type.
    pub fn take_tab_as_content(
        &mut self,
        pane_id: PaneId,
        tab_idx: usize,
        kind: TabKind,
    ) -> Option<PaneContent> {
        let pane = self.panes.get_mut(&pane_id)?;
        match kind {
            TabKind::File => {
                let PaneContent::Files(f) = &mut pane.content else {
                    return None;
                };
                let tab = f.take_tab(tab_idx)?;
                let mut fp = FilesPane::empty();
                fp.tabs.push(tab);
                fp.active = 0;
                Some(PaneContent::Files(fp))
            }
            TabKind::Terminal => {
                let PaneContent::Terminal(tp) = &mut pane.content else {
                    return None;
                };
                let tab = tp.take_tab(tab_idx)?;
                let mut new_tp = TerminalPane { tabs: Vec::new(), active: 0, renaming: None };
                new_tp.tabs.push(tab);
                new_tp.active = 0;
                Some(PaneContent::Terminal(new_tp))
            }
            TabKind::Browser => {
                let PaneContent::Browser(bp) = &mut pane.content else {
                    return None;
                };
                let tab = bp.take_tab(tab_idx)?;
                let mut new_bp = BrowserPane { tabs: Vec::new(), active: 0, next_tab_id: 1 };
                new_bp.tabs.push(BrowserTab {
                    id: new_bp.next_tab_id,
                    url: tab.url,
                    input_buf: tab.input_buf,
                    title: tab.title,
                });
                new_bp.next_tab_id += 1;
                new_bp.active = 0;
                Some(PaneContent::Browser(new_bp))
            }
        }
    }

    /// Create a new pane with any content, split adjacent to `neighbor`.
    pub fn add_pane_with_content(
        &mut self,
        content: PaneContent,
        neighbor: PaneId,
        edge: DockEdge,
    ) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        let title = match &content {
            PaneContent::Files(_) => "Files",
            PaneContent::Terminal(_) => "Terminal",
            PaneContent::Browser(_) => "Browser",
            _ => "Pane",
        }.to_string();
        self.panes.insert(id, Pane {
            id,
            title,
            content,
        });
        let root = match self.root.take() {
            Some(r) => r,
            None => Node::Leaf(neighbor),
        };
        self.root = Some(wrap_target(root, neighbor, id, edge));
        self.focus = Some(id);
        id
    }

    /// Move a tab between two same-type panes. If the source pane is left
    /// with zero tabs, remove it from the layout tree.
    pub fn move_tab(
        &mut self,
        src_pane: PaneId,
        tab_idx: usize,
        dst_pane: PaneId,
        insert_idx: usize,
        kind: TabKind,
    ) {
        let tab_content = match kind {
            TabKind::File => {
                let pane = match self.panes.get_mut(&src_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Files(f) = &mut pane.content else { return };
                let tab = match f.take_tab(tab_idx) {
                    Some(t) => t,
                    None => return,
                };
                let pane = match self.panes.get_mut(&dst_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Files(f) = &mut pane.content else { return };
                f.insert_tab(insert_idx, tab);
            }
            TabKind::Terminal => {
                let pane = match self.panes.get_mut(&src_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Terminal(tp) = &mut pane.content else { return };
                let tab = match tp.take_tab(tab_idx) {
                    Some(t) => t,
                    None => return,
                };
                let pane = match self.panes.get_mut(&dst_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Terminal(tp) = &mut pane.content else { return };
                tp.insert_tab(insert_idx, tab);
            }
            TabKind::Browser => {
                let pane = match self.panes.get_mut(&src_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Browser(bp) = &mut pane.content else { return };
                let tab = match bp.take_tab(tab_idx) {
                    Some(t) => t,
                    None => return,
                };
                let pane = match self.panes.get_mut(&dst_pane) {
                    Some(p) => p,
                    None => return,
                };
                let PaneContent::Browser(bp) = &mut pane.content else { return };
                bp.insert_tab(insert_idx, tab);
            }
        };
        let _ = tab_content;

        // Close source pane if it's now empty.
        self.remove_pane_if_empty(src_pane);
        self.focus = Some(dst_pane);
    }

    /// Remove a pane from the layout tree if it has zero tabs (or is
    /// a non-tabbed type — those are never removed here).
    pub fn remove_pane_if_empty(&mut self, pane_id: PaneId) {
        let empty = match self.panes.get(&pane_id) {
            Some(p) => match &p.content {
                PaneContent::Files(f) => f.tabs.is_empty(),
                PaneContent::Terminal(tp) => tp.tabs.is_empty(),
                PaneContent::Browser(bp) => bp.tabs.is_empty(),
                _ => false,
            },
            None => return,
        };
        if !empty {
            return;
        }
        self.panes.remove(&pane_id);
        if let Some(root) = self.root.take() {
            let (new_root, _) = remove_node(root, pane_id);
            self.root = new_root;
        }
        // Transfer focus to the first remaining leaf.
        self.focus = self.root.as_ref().and_then(|r| first_leaf(Some(r)));
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
