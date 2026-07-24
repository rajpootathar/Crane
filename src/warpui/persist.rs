//! warpui-frontend state persistence. During egui↔warpui coexistence this
//! writes to a SEPARATE `~/.crane/warpui-state.json` so it can never corrupt
//! the egui app's rich `session.json`. Restores panels, the tab list per
//! worktree, the active tab, expand state, and each tab's split layout
//! (terminals are respawned in the worktree cwd).

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::warpui::layout::{Dir, Node, PaneId};

/// Serializable mirror of `layout::Node` (drops the live Rc<Cell> handles).
#[derive(Serialize, Deserialize, Clone)]
pub enum SNode {
    Leaf(PaneId),
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<SNode>,
        second: Box<SNode>,
    },
}

impl SNode {
    pub fn from_node(n: &Node) -> SNode {
        match n {
            Node::Leaf(id) => SNode::Leaf(*id),
            Node::Split {
                dir, ratio, first, second, ..
            } => SNode::Split {
                vertical: matches!(dir, Dir::Vertical),
                ratio: ratio.get(),
                first: Box::new(SNode::from_node(first)),
                second: Box::new(SNode::from_node(second)),
            },
        }
    }

    pub fn to_node(&self) -> Node {
        match self {
            SNode::Leaf(id) => Node::Leaf(*id),
            SNode::Split {
                vertical, ratio, first, second,
            } => Node::Split {
                dir: if *vertical { Dir::Vertical } else { Dir::Horizontal },
                ratio: Rc::new(Cell::new(*ratio)),
                dragging: Rc::new(Cell::new(false)),
                first: Box::new(first.to_node()),
                second: Box::new(second.to_node()),
            },
        }
    }

    /// Collect every leaf pane id in this tree.
    pub fn leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            SNode::Leaf(id) => out.push(*id),
            SNode::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct STab {
    pub id: usize,
    pub name: String,
    pub layout: SNode,
    /// The focused pane within this tab at the time of save (None if unknown or
    /// the focused pane was not a leaf of this tab's layout).
    #[serde(default)]
    pub focus: Option<PaneId>,
    /// True if the user explicitly renamed this tab (pins the name against the
    /// terminal's live OSC title across restarts).
    #[serde(default)]
    pub renamed: bool,
}

/// Persisted terminal state (old-Crane parity): spawn cwd + an ANSI snapshot of
/// the scrollback + grid, replayed on restore.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct STerminal {
    pub cwd: PathBuf,
    #[serde(default)]
    pub history: String,
}

/// Persisted Browser Pane: its tabs as (url, title) + the active tab index.
/// Page state (scroll, forms, history) is native WKWebView state and does not
/// survive a relaunch — matching old Crane, which restored URLs only.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SBrowser {
    pub tabs: Vec<(String, String)>,
    #[serde(default)]
    pub active: usize,
}

/// Persisted Markdown Pane: the file it renders and whether it was left in
/// edit mode. Restored as a Markdown (or Editor) pane rather than a terminal.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SMarkdown {
    #[serde(default)]
    pub path: PathBuf,
    /// True = the pane was showing the editor, false = the rendered preview.
    #[serde(default)]
    pub editing: bool,
}

/// Persisted Image Pane: the image file it renders. Restored as an Image (or
/// Editor) pane rather than a terminal.
///
/// A separate record from `SMarkdown` rather than one generalised
/// document-pane struct with a kind tag: `SMarkdown` carries an `editing`
/// flag that has no image analogue, and unifying them would require migrating
/// the user's live `~/.crane/warpui-state.json` — real risk of dropping
/// already-persisted `markdowns` entries — to deduplicate a two-field struct.
/// Adding a field is purely additive: older state files parse (serde default),
/// and an older binary reading a newer file simply ignores this one.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SImage {
    #[serde(default)]
    pub path: PathBuf,
}

/// Persisted PDF Pane: the PDF file it renders. Exact peer of `SImage` —
/// restored as a PDF (or, for a corrupt state file, Editor) pane rather than a
/// terminal. Kept as its own record for the same additive-migration reasons as
/// `SImage` (see its doc comment).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SPdf {
    #[serde(default)]
    pub path: PathBuf,
}

/// One Workspace's File Tabs: the pane that IS its Files Pane, that pane's
/// open File Tab paths, and which of them was active.
///
/// `pane` is part of the record rather than a separate map because a saved
/// leaf only restores as a document pane (Editor / Markdown) when restore can
/// recognise it as that Workspace's Files Pane — see `restored_pane_kind` in
/// `shell.rs`. The old flat schema carried exactly one such id
/// (`WarpuiState::files_pane`), which is precisely why only one Workspace's
/// File Tabs could ever come back.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SFileTabs {
    /// The Files Pane's leaf id in this Workspace (None if it had none).
    #[serde(default)]
    pub pane: Option<PaneId>,
    /// Files open as File Tabs in that pane, in tab order.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// The active File Tab index within `paths`.
    #[serde(default)]
    pub active: usize,
}

/// A project added via the warpui "Add Project" flow (not sourced from session.json).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AddedProject {
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct WarpuiState {
    #[serde(default)]
    pub show_left: bool,
    #[serde(default)]
    pub show_right: bool,
    #[serde(default)]
    pub files_tab: bool,
    /// Active tab as (project_idx, worktree_idx, tab_id). LEGACY — indices
    /// shift when projects/worktrees are added, removed or reordered between
    /// runs. Kept written + read as a fallback; `active_tab_path` wins.
    #[serde(default)]
    pub active_tab: Option<(usize, usize, usize)>,
    #[serde(default)]
    pub expanded_projects: Vec<usize>,
    #[serde(default)]
    pub expanded_worktrees: Vec<(usize, usize)>,
    /// Per (project_idx, worktree_idx): the tabs in that worktree. LEGACY —
    /// see `worktree_tabs_by_path`, which restore prefers.
    #[serde(default)]
    pub worktree_tabs: Vec<((usize, usize), Vec<STab>)>,
    /// Per worktree checkout PATH: the tabs in that worktree. Paths are stable
    /// across reloads (indices shift), so tab lists + terminal histories land
    /// back in the RIGHT worktree even after projects are added/removed/reordered.
    #[serde(default)]
    pub worktree_tabs_by_path: Vec<(String, Vec<STab>)>,
    /// Active tab as (worktree checkout path, tab_id) — the stable-key twin of
    /// `active_tab`.
    #[serde(default)]
    pub active_tab_path: Option<(String, usize)>,
    /// Expanded sidebar projects keyed by project path (stable-key twin of
    /// `expanded_projects`).
    #[serde(default)]
    pub expanded_project_paths: Vec<String>,
    /// Expanded sidebar worktrees keyed by worktree checkout path.
    #[serde(default)]
    pub expanded_worktree_paths: Vec<String>,
    #[serde(default)]
    pub next_tab_id: usize,
    #[serde(default)]
    pub next_pane_id: PaneId,
    /// The File pane's leaf id (so it's restored as a File pane, not a terminal).
    /// LEGACY — one flat record for the whole session, so only ONE Workspace's
    /// File Tabs survived a restart. See `file_tabs_by_path`, which restore
    /// prefers. Still written (from the selected Workspace) so an older binary
    /// reading this file finds something sane, and still READ as the migration
    /// source for state files that predate `file_tabs_by_path`.
    #[serde(default)]
    pub files_pane: Option<PaneId>,
    /// Files open in the File pane, restored as tabs. LEGACY — see `files_pane`.
    #[serde(default)]
    pub file_pane_paths: Vec<PathBuf>,
    /// The active file tab index within `file_pane_paths`. LEGACY — see
    /// `files_pane`.
    #[serde(default)]
    pub file_pane_active: usize,
    /// Per worktree checkout PATH: that Workspace's File Tabs. Path-keyed for
    /// the same reason as `worktree_tabs_by_path` — indices shift when projects
    /// are added, removed or reordered between runs; checkout paths do not, so
    /// each Workspace's File Tabs land back in the RIGHT Workspace.
    #[serde(default)]
    pub file_tabs_by_path: Vec<(String, SFileTabs)>,
    /// Per terminal pane: cwd + ANSI scrollback snapshot, keyed by pane id.
    #[serde(default)]
    pub terminals: Vec<(PaneId, STerminal)>,
    /// Per Browser pane: its tabs' URLs + titles, keyed by pane id, so the
    /// restore loop rebuilds a Browser (not a terminal) at that leaf.
    #[serde(default)]
    pub browsers: Vec<(PaneId, SBrowser)>,
    /// Per Markdown pane: the file + mode, keyed by pane id, so the restore
    /// loop rebuilds a Markdown pane (not a terminal) at that leaf.
    #[serde(default)]
    pub markdowns: Vec<(PaneId, SMarkdown)>,
    /// Per Image pane: the file it renders, keyed by pane id, so the restore
    /// loop rebuilds an Image pane (not a terminal) at that leaf. Exact peer
    /// of `markdowns` — see `SImage` for why it is a separate field.
    #[serde(default)]
    pub images: Vec<(PaneId, SImage)>,
    /// Per PDF pane: the file it renders, keyed by pane id, so the restore loop
    /// rebuilds a PDF pane (not a terminal) at that leaf. Exact peer of
    /// `images` — see `SPdf`.
    #[serde(default)]
    pub pdfs: Vec<(PaneId, SPdf)>,
    /// Projects the user added via "Add Project" (not from session.json).
    #[serde(default)]
    pub added_projects: Vec<AddedProject>,
    /// Paths of session.json projects the user explicitly removed.
    #[serde(default)]
    pub removed_project_paths: Vec<String>,
    /// Per-project tint overrides keyed by project path.
    #[serde(default)]
    pub project_tints: Vec<(String, [u8; 3])>,
    /// Per-worktree display-name overrides keyed by the worktree's checkout PATH
    /// (paths are stable across reloads; indices shift).
    #[serde(default)]
    pub worktree_names: Vec<(String, String)>,
    /// Per-worktree tint overrides keyed by the worktree's checkout PATH.
    #[serde(default)]
    pub worktree_tints: Vec<(String, [u8; 3])>,
    /// Per-tab tint overrides keyed by (worktree_path, tab_id) — stable across
    /// reloads even though (project_idx, worktree_idx) shift.
    #[serde(default)]
    pub tab_tints: Vec<((String, usize), [u8; 3])>,
    /// Per folder-group tint overrides keyed by the container folder's own path
    /// (`ProjectNode::group_path`). Painted on the collapsible FOLDER header's
    /// icon + label. Stable across reloads (the container path never shifts).
    #[serde(default)]
    pub group_tints: Vec<(String, [u8; 3])>,
    /// Last saved window width in logical pixels (0.0 = unset / use default).
    #[serde(default)]
    pub window_w: f32,
    /// Last saved window height in logical pixels (0.0 = unset / use default).
    #[serde(default)]
    pub window_h: f32,
    /// Name of the active colour theme, persisted so it is restored on next launch.
    #[serde(default)]
    pub theme_name: String,
    /// App-wide zoom level (Cmd+= / Cmd+- / Cmd+0), 1.0 = 100%. 0.0 = unset.
    #[serde(default)]
    pub zoom_level: f32,
    /// Editor Language Server (LSP) opt-in. OFF by default — the agent CLI is
    /// the code-intelligence layer, so we never spawn rust-analyzer et al.
    /// unless the user explicitly enables this in Settings.
    #[serde(default)]
    pub lsp_enabled: bool,
    /// Editor format-on-save. ON by default — matches the old egui build, which
    /// ran the buffer through rustfmt / prettier / ruff / gofmt before every
    /// write. A formatter error (missing binary, non-zero exit) never mutates
    /// the file; the original buffer is written unchanged.
    #[serde(default = "default_true")]
    pub format_on_save: bool,
    /// Terminal base font size in points (0.0 = unset → default 14).
    #[serde(default)]
    pub terminal_font: f32,
    /// Editor base font size in points (0.0 = unset → default 13).
    #[serde(default)]
    pub editor_font: f32,
    /// Editor soft word-wrap default for newly opened files (Cmd+Opt+W still
    /// toggles per-editor at runtime).
    #[serde(default)]
    pub word_wrap: bool,
    /// Strip trailing whitespace on save (old `prefs.trim_on_save`).
    #[serde(default)]
    pub trim_on_save: bool,
    /// Syntect theme override; "" = auto (pair with the UI theme's
    /// `syntax_theme`).
    #[serde(default)]
    pub syntax_override: String,
    /// Sidebar drag-drop ordering: (project path, worktree paths in order),
    /// in project display order. Applied after the project load so freshly
    /// discovered projects/worktrees (absent here) append at the end.
    #[serde(default)]
    pub sidebar_order: Vec<(String, Vec<String>)>,
    /// Per-version update-prompt decisions (old check.rs `PromptState`):
    /// value is `"dismissed"` (Skip this version) or `"remind:<epoch-secs>"`
    /// (Remind in 7 days). Keyed by the release version string.
    #[serde(default)]
    pub update_prompts: Vec<(String, String)>,
}

/// Serde default for `format_on_save`: ON, so existing state files that predate
/// the field (and the `Default` fallback path) still format on save.
fn default_true() -> bool {
    true
}

fn state_file() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".crane").join("warpui-state.json"))
}

/// Try to parse a state file at `path`, returning None if missing or corrupt.
fn load_from(path: &std::path::Path) -> Option<WarpuiState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Load persisted warpui state, or None if absent/corrupt. Falls back to the
/// `.bak` safety copy (written by `save()` before each atomic replace) when the
/// primary file is missing or fails to parse, so a crash mid-write can't lose
/// the whole session.
pub fn load() -> Option<WarpuiState> {
    let path = state_file()?;
    if let Some(state) = load_from(&path) {
        return Some(state);
    }
    // Primary missing/corrupt — try the pre-write backup.
    let bak = path.with_extension("json.bak");
    load_from(&bak)
}

/// Write state atomically (tmp → rename) so a crash mid-write can't truncate it.
/// Before the rename, the previous good file (if any) is copied to `<path>.bak`
/// as a best-effort safety net that `load()` can fall back to.
///
/// The `serde_json` serialize runs on the CALLING (UI) thread — the state graph
/// is borrowed and can't cross the thread boundary cheaply — but the three
/// blocking filesystem ops (tmp write, `.bak` copy, rename) are handed to a
/// short-lived `std::thread::spawn` so the UI never stalls on disk IO. This
/// mirrors OG Crane's `maybe_save` (serialize on the render thread, spawn the
/// atomic write). No async runtime is involved (project rule).
pub fn save(state: &WarpuiState) {
    let Some(path) = state_file() else { return };
    let Ok(bytes) = serde_json::to_vec_pretty(state) else {
        return;
    };
    std::thread::spawn(move || write_bytes(&path, &bytes));
}

/// Synchronous variant of `save` for the app-terminate path: the process may
/// exit before a spawned writer thread finishes, so the final save (which
/// carries the freshest terminal snapshots) must complete on the calling
/// thread before termination is approved.
pub fn save_sync(state: &WarpuiState) {
    let Some(path) = state_file() else { return };
    let Ok(bytes) = serde_json::to_vec_pretty(state) else {
        return;
    };
    write_bytes(&path, &bytes);
}

/// Crash-safe write of already-serialized bytes (tmp → rename). The tmp name
/// is unique per write so a concurrent async save and the terminate-path sync
/// save can never interleave into the same tmp file.
fn write_bytes(path: &std::path::Path, bytes: &[u8]) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp{n}"));
    if std::fs::write(&tmp, bytes).is_ok() {
        // Best-effort backup of the current good state before we replace it,
        // so a crash between here and the rename still leaves a recoverable
        // copy.
        if path.exists() {
            let bak = path.with_extension("json.bak");
            let _ = std::fs::copy(path, &bak);
        }
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path-keyed session fields must survive a serialize → deserialize
    /// round trip (they carry tab lists + the active tab across restarts).
    #[test]
    fn path_keyed_fields_round_trip() {
        let mut st = WarpuiState::default();
        st.worktree_tabs_by_path = vec![(
            "/tmp/wt".into(),
            vec![STab {
                id: 3,
                name: "build".into(),
                layout: SNode::Leaf(7),
                focus: Some(7),
                renamed: true,
            }],
        )];
        st.active_tab_path = Some(("/tmp/wt".into(), 3));
        st.expanded_project_paths = vec!["/tmp/proj".into()];
        st.expanded_worktree_paths = vec!["/tmp/wt".into()];
        let bytes = serde_json::to_vec(&st).unwrap();
        let back: WarpuiState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.worktree_tabs_by_path.len(), 1);
        assert_eq!(back.worktree_tabs_by_path[0].0, "/tmp/wt");
        assert_eq!(back.worktree_tabs_by_path[0].1[0].id, 3);
        assert_eq!(back.worktree_tabs_by_path[0].1[0].focus, Some(7));
        assert_eq!(back.active_tab_path, Some(("/tmp/wt".into(), 3)));
        assert_eq!(back.expanded_project_paths, vec!["/tmp/proj".to_string()]);
        assert_eq!(back.expanded_worktree_paths, vec!["/tmp/wt".to_string()]);
    }

    /// A state file written BEFORE the path-keyed fields existed must still
    /// parse, with the new fields defaulting to empty (restore then falls back
    /// to the legacy index-keyed fields).
    #[test]
    fn legacy_state_file_still_parses() {
        let legacy = r#"{
            "show_left": true,
            "active_tab": [1, 0, 4],
            "worktree_tabs": [[[1, 0], [{"id": 4, "name": "t", "layout": {"Leaf": 9}}]]]
        }"#;
        let st: WarpuiState = serde_json::from_str(legacy).unwrap();
        assert_eq!(st.active_tab, Some((1, 0, 4)));
        assert_eq!(st.worktree_tabs.len(), 1);
        assert!(st.worktree_tabs_by_path.is_empty());
        assert!(st.active_tab_path.is_none());
        assert!(st.expanded_project_paths.is_empty());
    }

    /// Every Workspace's File Tabs must survive a serialize → deserialize
    /// round trip, each keyed by its own worktree checkout PATH. This is the
    /// field that makes "Workspace B's open files come back in B" possible at
    /// all — the legacy flat trio could only ever carry ONE Workspace's.
    #[test]
    fn file_tabs_by_path_round_trips_every_workspace() {
        let mut st = WarpuiState::default();
        st.file_tabs_by_path = vec![
            (
                "/tmp/wt-a".into(),
                SFileTabs {
                    pane: Some(4),
                    paths: vec![PathBuf::from("/tmp/wt-a/lib.rs"), PathBuf::from("/tmp/wt-a/x.md")],
                    active: 1,
                },
            ),
            (
                "/tmp/wt-b".into(),
                SFileTabs {
                    pane: Some(9),
                    paths: vec![PathBuf::from("/tmp/wt-b/main.rs")],
                    active: 0,
                },
            ),
        ];
        let bytes = serde_json::to_vec(&st).unwrap();
        let back: WarpuiState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.file_tabs_by_path.len(), 2, "both Workspaces' File Tabs must survive");
        assert_eq!(back.file_tabs_by_path[0].0, "/tmp/wt-a");
        assert_eq!(back.file_tabs_by_path[0].1.pane, Some(4));
        assert_eq!(
            back.file_tabs_by_path[0].1.paths,
            vec![PathBuf::from("/tmp/wt-a/lib.rs"), PathBuf::from("/tmp/wt-a/x.md")],
            "tab order and contents must round trip exactly"
        );
        assert_eq!(back.file_tabs_by_path[0].1.active, 1, "the active File Tab index must survive");
        assert_eq!(back.file_tabs_by_path[1].0, "/tmp/wt-b");
        assert_eq!(back.file_tabs_by_path[1].1.pane, Some(9));
        assert_eq!(back.file_tabs_by_path[1].1.paths, vec![PathBuf::from("/tmp/wt-b/main.rs")]);
    }

    /// The user's real `~/.crane/warpui-state.json` predates `file_tabs_by_path`
    /// and carries the legacy FLAT Files-Pane trio. It must still parse, with
    /// the flat fields intact (restore migrates them) and the new field empty.
    #[test]
    fn legacy_flat_file_pane_fields_still_parse() {
        let legacy = r#"{
            "files_pane": 12,
            "file_pane_paths": ["/tmp/wt-a/lib.rs", "/tmp/wt-a/x.md"],
            "file_pane_active": 1
        }"#;
        let st: WarpuiState = serde_json::from_str(legacy).expect("legacy state must load");
        assert_eq!(st.files_pane, Some(12), "the legacy Files Pane id is the migration source");
        assert_eq!(st.file_pane_paths.len(), 2, "the legacy open files must not be dropped");
        assert_eq!(st.file_pane_active, 1);
        assert!(st.file_tabs_by_path.is_empty(), "the path-keyed field defaults to empty");
    }

    /// A Markdown pane's saved file + mode must survive a serialize →
    /// deserialize round trip, keyed by pane id, the same as `browsers`.
    #[test]
    fn markdown_panes_survive_a_state_round_trip() {
        let mut st = WarpuiState::default();
        st.markdowns = vec![(
            7,
            SMarkdown { path: std::path::PathBuf::from("/tmp/doc.md"), editing: false },
        )];
        let json = serde_json::to_string(&st).expect("serialize");
        let back: WarpuiState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.markdowns.len(), 1, "markdown panes must survive a round trip");
        assert_eq!(back.markdowns[0].1.path, std::path::PathBuf::from("/tmp/doc.md"));
    }

    /// Backward compatibility: an existing ~/.crane/warpui-state.json predates
    /// this field and must still deserialize rather than wiping the session.
    #[test]
    fn state_without_markdowns_still_loads() {
        let legacy = r#"{}"#;
        let st: WarpuiState = serde_json::from_str(legacy).expect("legacy state must load");
        assert!(st.markdowns.is_empty());
    }

    /// An Image pane's saved file must survive a serialize → deserialize round
    /// trip, keyed by pane id, exactly like `markdowns` / `browsers`. Without
    /// this field the pane restores as a fresh terminal.
    #[test]
    fn image_panes_survive_a_state_round_trip() {
        let mut st = WarpuiState::default();
        st.images = vec![(7, SImage { path: PathBuf::from("/tmp/logo.png") })];
        let json = serde_json::to_string(&st).expect("serialize");
        let back: WarpuiState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.images.len(), 1, "image panes must survive a round trip");
        assert_eq!(back.images[0].0, 7, "the pane id keys the restore lookup");
        assert_eq!(back.images[0].1.path, PathBuf::from("/tmp/logo.png"));
    }

    /// A PDF pane's saved file must survive a round trip, keyed by pane id —
    /// peer of `image_panes_survive_a_state_round_trip`. Without it the pane
    /// restores as a fresh terminal.
    #[test]
    fn pdf_panes_survive_a_state_round_trip() {
        let mut st = WarpuiState::default();
        st.pdfs = vec![(9, SPdf { path: PathBuf::from("/tmp/report.pdf") })];
        let json = serde_json::to_string(&st).expect("serialize");
        let back: WarpuiState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pdfs.len(), 1, "pdf panes must survive a round trip");
        assert_eq!(back.pdfs[0].0, 9, "the pane id keys the restore lookup");
        assert_eq!(back.pdfs[0].1.path, PathBuf::from("/tmp/report.pdf"));
    }

    /// Compatibility: a state file written before `pdfs` existed must still
    /// load, with `pdfs` defaulting to empty (never a parse error on the user's
    /// live `~/.crane/warpui-state.json`).
    #[test]
    fn a_state_file_predating_pdfs_still_loads() {
        let legacy = r#"{"images":[]}"#;
        let st: WarpuiState = serde_json::from_str(legacy).expect("pre-pdfs state must load");
        assert!(st.pdfs.is_empty(), "the new field defaults to empty");
    }

    /// THE compatibility guarantee for the user's live session file: a state
    /// file written before `images` existed must still load, with `images`
    /// defaulting to empty AND every previously-persisted `markdowns` entry
    /// intact. Adding a document-pane field must never cost the user the
    /// document panes they already had.
    #[test]
    fn a_state_file_predating_images_still_loads_and_keeps_its_markdowns() {
        let legacy = r#"{
            "show_left": true,
            "markdowns": [[3, {"path": "/tmp/doc.md", "editing": false}]],
            "files_pane": 3,
            "file_pane_paths": ["/tmp/doc.md"]
        }"#;
        let st: WarpuiState = serde_json::from_str(legacy).expect("legacy state must load");
        assert!(st.images.is_empty(), "the new field defaults to empty");
        assert_eq!(st.markdowns.len(), 1, "existing markdown panes must NOT be dropped");
        assert_eq!(st.markdowns[0].1.path, PathBuf::from("/tmp/doc.md"));
        assert_eq!(st.files_pane, Some(3), "the rest of the legacy state must survive too");
    }
}
