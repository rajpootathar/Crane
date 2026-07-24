# Per-Terminal Status Bar — Design

Date: 2026-07-24
Status: approved

## Goal

Warp-style status bar pinned to the bottom of every **Terminal Pane** (per-Pane,
not per-Tab: each split terminal reflects its own shell). Shows where that shell
is and the repo state at a glance, using data Crane already maintains — zero new
git shell-outs.

## Content (right-aligned chip row)

| Chip | Source | Notes |
|---|---|---|
| 📁 folder name | terminal's `live_cwd` (OSC-driven, already tracked per controller) | updates instantly on `cd` |
| ⎇ branch | worktree node matched by path-prefix against `self.projects` (branch kept fresh by existing async git scans / `poll_worktrees`) | hidden when cwd is not inside a known repo |
| `N • +A −D` | `N` = changed-file count from the existing `changes` set when the cwd matches the active repo, else omitted; `+A −D` = the worktree node's `diff_stat` | hidden when clean |

No PR chip in v1 (needs `gh` polling; deferred).

## Layout / rendering

- One ~24px row inside the Terminal Pane, under the grid+scrollbar row. The
  grid's existing `desired`-size two-frame resize dance absorbs the lost row.
- Built in the terminal pane composition (`view.rs` render → `Flex::column`
  wrapping the current grid+scrollbar row plus the bar row).
- Chips reuse Left Panel chip styling: `egui_phosphor`-equivalent warpui icons
  (`FOLDER`, `GIT_BRANCH`), theme colors, pointing-hand hover. Icon-free text
  fallback is not needed — icons come from the bundled icon font.
- Pane too narrow (< ~300 px): drop chips right-to-left (diff first, then
  branch) so the folder chip survives longest.
- Dimmed (unfocused) panes dim the bar with the same alpha rule as the grid.

## Interactions (v1)

- Folder chip click → existing "reveal in Files pane" action for the cwd.
- Branch chip click → existing Switch Branch popup (`OpenSwitchBranch`).
- Diff chip: display-only in v1.

## Data flow

Shell continues to own git state. The bar is pure presentation:
`live_cwd` → longest-prefix match over project/worktree paths → read that
node's `branch`/`diff_stat`/`dirty`. No timers, no new subprocesses, no new
locks; recomputed per frame from already-cached state (cheap string compares
over a handful of paths).

## Out of scope

- PR chip (`gh` integration)
- Left-side feature buttons (explorer toggle, input mode, etc.)
- Per-Tab aggregate bar

## Testing

- Unit: path-prefix worktree matcher (new pure fn) — nested worktrees pick the
  longest match; non-repo cwd yields None.
- Manual: `cd` between repos updates chips; split panes show independent bars;
  narrow-pane chip dropping; branch switch reflects after the existing scan.
