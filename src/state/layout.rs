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
    /// When true, the go-to-line modal is shown. Toggled by Ctrl+G.
    pub goto_line_active: bool,
    /// Buffer for the go-to-line text input.
    pub goto_line_input: String,
    /// Replace bar state. None = hidden; Some(text) = shown.
    pub replace_query: String,
    /// When true, the replace row is visible below the find bar.
    pub show_replace: bool,
    /// Selection info set each frame by the editor: (selected_chars, selected_lines).
    /// Displayed in the status strip.
    pub selection_info: Option<(usize, usize)>,
    /// Last save error, if any. Cleared on successful save. Displayed as a
    /// toast/banner in the editor.
    pub save_error: Option<String>,
    /// Preview tabs are opened by single-click and auto-replaced by the
    /// next single-click. They promote to permanent on first edit.
    pub preview: bool,
}

impl FileTab {
    pub fn dirty(&self) -> bool {
        self.content != self.original_content
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffMode {
    Unified,
    SideBySide,
}

pub struct DiffTabData {
    pub title: String,
    pub left_path: String,
    pub right_path: String,
    pub left_text: String,
    pub right_text: String,
    pub error: Option<String>,
    pub image_texture: Option<egui::TextureHandle>,
    pub diff_mode: DiffMode,
    /// Absolute path to the git repo root. Used by the diff view to
    /// call `git apply --cached` for per-hunk staging.
    pub repo_path: Option<String>,
    /// Set to true by the diff view when a hunk is staged. The main loop
    /// reads this flag and triggers a diff content refresh.
    pub pending_hunk_stage: bool,
    /// Horizontal scroll offsets for side-by-side mode (left/right halves).
    pub sbs_h_scroll_left: f32,
    pub sbs_h_scroll_right: f32,
}

impl DiffTabData {
    /// Reload the left-side content from git (staged or HEAD) using
    /// `right_path` to resolve the repo root. Shared by both
    /// save-triggered and hunk-stage diff refreshes.
    pub fn reload_left_text(&mut self) {
        let right = std::path::Path::new(&self.right_path);
        if let Some((root, rel)) = self
            .left_path
            .strip_prefix("staged:")
            .and_then(|rel| {
                crate::git::find_git_root(right).map(|root| (root, rel.to_string()))
            })
        {
            self.left_text = crate::git::staged_content(&root, &rel)
                .unwrap_or_else(|| crate::git::head_content(&root, &rel));
        } else if let Some((root, rel)) = self
            .left_path
            .strip_prefix("HEAD:")
            .and_then(|rel| {
                crate::git::find_git_root(right).map(|root| (root, rel.to_string()))
            })
        {
            self.left_text = crate::git::head_content(&root, &rel);
        }
    }
}

pub enum TabKind {
    File(FileTab),
    Diff(DiffTabData),
}

impl TabKind {
    pub fn name(&self) -> &str {
        match self {
            TabKind::File(ft) => &ft.name,
            TabKind::Diff(dt) => &dt.title,
        }
    }

    pub fn is_dirty(&self) -> bool {
        match self {
            TabKind::File(ft) => ft.dirty(),
            TabKind::Diff(_) => false,
        }
    }

    pub fn as_file_mut(&mut self) -> Option<&mut FileTab> {
        match self {
            TabKind::File(ft) => Some(ft),
            TabKind::Diff(_) => None,
        }
    }

    pub fn as_file(&self) -> Option<&FileTab> {
        match self {
            TabKind::File(ft) => Some(ft),
            TabKind::Diff(_) => None,
        }
    }

    pub fn as_diff_mut(&mut self) -> Option<&mut DiffTabData> {
        match self {
            TabKind::File(_) => None,
            TabKind::Diff(dt) => Some(dt),
        }
    }
}

pub struct FilesPane {
    pub tabs: Vec<TabKind>,
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

    pub fn open(&mut self, path: String, content: String, name: String, preview: bool) {
        if let Some(idx) = self.tabs.iter().position(|t| matches!(t, TabKind::File(ft) if ft.path == path)) {
            self.active = idx;
            // Promote preview tab to permanent on re-open
            if let TabKind::File(ft) = &mut self.tabs[idx] {
                ft.preview = false;
            }
            return;
        }
        let disk_mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();
        self.tabs.push(TabKind::File(FileTab {
            path,
            original_content: content.clone(),
            last_lsp_content: content.clone(),
            last_lsp_sent_at: None,
            preview_mode: false,
            preview,
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
            content,
            name,
        }));
        self.active = self.tabs.len() - 1;
    }

    /// Open a diff tab. If one already exists for the same
    /// `(left_path, right_path)` pair, refresh its contents and focus
    /// it instead of adding a duplicate.
    pub fn open_diff(
        &mut self,
        title: String,
        left_path: String,
        right_path: String,
        left_text: String,
        right_text: String,
        repo_path: Option<String>,
    ) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| matches!(t, TabKind::Diff(dt) if dt.left_path == left_path && dt.right_path == right_path))
        {
            let t = self.tabs[idx].as_diff_mut().unwrap();
            t.title = title;
            t.left_text = left_text;
            t.right_text = right_text;
            t.error = None;
            t.image_texture = None;
            t.repo_path = repo_path;
            self.active = idx;
            return;
        }
        self.tabs.push(TabKind::Diff(DiffTabData {
            title,
            left_path,
            right_path,
            left_text,
            right_text,
            error: None,
            image_texture: None,
            diff_mode: DiffMode::Unified,
            repo_path,
            pending_hunk_stage: false,
            sbs_h_scroll_left: 0.0,
            sbs_h_scroll_right: 0.0,
        }));
        self.active = self.tabs.len() - 1;
    }

    pub fn close(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.tabs.remove(idx);
            if self.active >= self.tabs.len() && !self.tabs.is_empty() {
                self.active = self.tabs.len() - 1;
            } else if self.tabs.is_empty() {
                self.active = 0;
            } else if self.active > idx {
                self.active -= 1;
            }
        }
    }

    /// Remove any preview tab. Called before opening a new preview.
    pub fn close_preview_tab(&mut self) {
        self.tabs.retain(|t| {
            !matches!(t, TabKind::File(ft) if ft.preview)
        });
        if self.active >= self.tabs.len() && !self.tabs.is_empty() {
            self.active = self.tabs.len() - 1;
        }
    }
}

pub struct MarkdownPane {
    pub path: String,
    pub content: String,
    pub input_buf: String,
    pub error: Option<String>,
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
}

pub enum PaneContent {
    Terminal(TerminalPane),
    Files(FilesPane),
    Markdown(MarkdownPane),
    Browser(BrowserPane),
    Welcome(WelcomePane),
}

impl PaneContent {
    pub fn kind_label(&self) -> &'static str {
        match self {
            PaneContent::Terminal(_) => "Terminal",
            PaneContent::Files(_) => "Files",
            PaneContent::Markdown(_) => "Markdown",
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

    pub fn open_diff_in_files_pane(
        &mut self,
        left_path: String,
        right_path: String,
        left_text: String,
        right_text: String,
        title: String,
        repo_path: Option<String>,
    ) {
        let existing = self
            .panes
            .iter()
            .find(|(_, p)| matches!(p.content, PaneContent::Files(_)))
            .map(|(id, _)| *id);
        match existing {
            Some(pid) => {
                if let Some(pane) = self.panes.get_mut(&pid)
                    && let PaneContent::Files(files) = &mut pane.content
                {
                    files.open_diff(title.clone(), left_path, right_path, left_text, right_text, repo_path.clone());
                }
                self.focus = Some(pid);
            }
            None => {
                let mut files = FilesPane::empty();
                files.open_diff(title.clone(), left_path, right_path, left_text, right_text, repo_path.clone());
                self.add_pane(PaneContent::Files(files), Some(Dir::Horizontal));
                if let Some(focus) = self.focus
                    && let Some(pane) = self.panes.get_mut(&focus)
                {
                    pane.title = title;
                }
            }
        }
    }

    pub fn open_file_in_files_pane(&mut self, path: String, name: String, content: String, preview: bool) {
        let existing = self
            .panes
            .iter()
            .find(|(_, p)| matches!(p.content, PaneContent::Files(_)))
            .map(|(id, _)| *id);
        match existing {
            Some(pid) => {
                if let Some(pane) = self.panes.get_mut(&pid)
                    && let PaneContent::Files(files) = &mut pane.content {
                        // Close any existing preview tab before opening a new preview
                        if preview {
                            files.close_preview_tab();
                        }
                        files.open(path, content, name, preview);
                    }
                self.focus = Some(pid);
            }
            None => {
                let mut files = FilesPane::empty();
                files.open(path, content, name, preview);
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
