//! warpui-frontend state persistence. During egui↔warpui coexistence this
//! writes to a SEPARATE `~/.crane/warpui-state.json` so it can never corrupt
//! the egui app's rich `session.json`. Restores panels, the tab list per
//! worktree, the active tab, expand state, and each tab's split layout
//! (terminals are respawned in the worktree cwd).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
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
    /// Active tab as (project_idx, worktree_idx, tab_id).
    #[serde(default)]
    pub active_tab: Option<(usize, usize, usize)>,
    #[serde(default)]
    pub expanded_projects: Vec<usize>,
    #[serde(default)]
    pub expanded_worktrees: Vec<(usize, usize)>,
    /// Per (project_idx, worktree_idx): the tabs in that worktree.
    #[serde(default)]
    pub worktree_tabs: Vec<((usize, usize), Vec<STab>)>,
    #[serde(default)]
    pub next_tab_id: usize,
    #[serde(default)]
    pub next_pane_id: PaneId,
    /// The File pane's leaf id (so it's restored as a File pane, not a terminal).
    #[serde(default)]
    pub files_pane: Option<PaneId>,
    /// Files open in the File pane, restored as tabs.
    #[serde(default)]
    pub file_pane_paths: Vec<PathBuf>,
    /// The active file tab index within `file_pane_paths`.
    #[serde(default)]
    pub file_pane_active: usize,
    /// Per terminal pane: cwd + ANSI scrollback snapshot, keyed by pane id.
    #[serde(default)]
    pub terminals: Vec<(PaneId, STerminal)>,
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
    write_bytes_async(path, bytes);
}

/// Off-thread crash-safe write of already-serialized bytes. Owns its inputs so
/// the closure is `'static` for the spawned thread.
fn write_bytes_async(path: std::path::PathBuf, bytes: Vec<u8>) {
    std::thread::spawn(move || {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            // Best-effort backup of the current good state before we replace it,
            // so a crash between here and the rename still leaves a recoverable
            // copy.
            if path.exists() {
                let bak = path.with_extension("json.bak");
                let _ = std::fs::copy(&path, &bak);
            }
            let _ = std::fs::rename(&tmp, &path);
        }
    });
}

/// Helper to rebuild HashMap fields from the flat Vecs.
pub fn worktree_tabs_map(state: &WarpuiState) -> HashMap<(usize, usize), Vec<STab>> {
    state.worktree_tabs.iter().cloned().collect()
}

pub fn expanded_sets(
    state: &WarpuiState,
) -> (HashSet<usize>, HashSet<(usize, usize)>) {
    (
        state.expanded_projects.iter().copied().collect(),
        state.expanded_worktrees.iter().copied().collect(),
    )
}
