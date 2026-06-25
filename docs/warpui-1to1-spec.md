# Crane warpui Port — 1:1 Visual & Behavioral Spec

> The target is a **100% match** of the existing egui Crane, rebuilt on warpui.
> This document is the per-region spec derived from the live egui app: every
> pane, its elements, icons, colors, states, and interactions — plus the
> honest current port status. The code-level gap list (from the chunk-by-chunk
> comparison review) is tracked in `warpui-migration-execution.md`; this doc is
> the *visual/behavioral* source of truth.
>
> Reuse patterns (proven in `crates/warp_term_spike`):
> - **Clickable element** = `Stack[ highlight Rect (bottom) · label · transparent hit Rect (TOP) ]`
>   wrapped in `EventHandler`. The hit Rect MUST be the topmost layer
>   (warpui hit-tests at the child's max z-index).
> - **State change on click** = `ctx.dispatch_typed_action(Action)` →
>   `handle_action(&mut self)` mutates view state → `ctx.notify()` re-renders.
>   External `Rc<RefCell>` mutation alone does NOT re-render.
> - **Icons** = `egui_phosphor::regular::*` glyphs rendered as `Text` (never
>   Unicode — the bundled font lacks those ranges).

Aggregate honest status: **~5–8% of egui Crane by feature.** The skeleton and
interaction plumbing exist; almost none of the IDE behavior does.

---

## 0. Window chrome / titlebar

| | |
|---|---|
| **Elements** | Full-content dark window; macOS traffic lights top-left; "Crane" title; single unified dark bar (no native gray titlebar). |
| **Dimensions** | Titlebar zone 28px; traffic-light inset ~84px on the left. |
| **Port status** | ✅ Done — unified top bar reserves the traffic-light zone. |

---

## 1. Left Panel — PROJECTS  (egui: `src/ui/projects.rs`, ~1400 ln)

The richest, most-used pane. Hierarchy: **Project → Worktree (branch) → Tab**.

### Elements & visuals
- **Header** `PROJECTS` — muted, small, letter-spaced.
- **Project row**
  - **Cube icon** (`CUBE`) tinted per-project (group tint). Observed tints:
    crane=amber, qck-cloud=green, nomli=yellow-green, Dispatr=purple,
    KYC-Kairos=teal, OneVibe=orange, RCubedStudios-site=purple,
    SightOps.AI=blue, Athar.dev=orange, techwire.space=blue,
    indie2dGame=orange, OhSugrrr=orange.
  - **Disclosure chevron** `CARET_DOWN` (expanded) / `CARET_RIGHT` (collapsed),
    animated rotation.
  - Project name in `text` color.
- **Worktree (branch) row** — indent +1
  - **Branch icon** `GIT_BRANCH`.
  - Branch name; **non-default branches render blue** (e.g. `feat/phase1-continue`,
    `main`); the active worktree gets an **amber active-highlight bar**.
  - Its own disclosure chevron (worktrees expand to show tabs).
  - **Change-count badge** (e.g. `+1413`) on the active worktree.
- **Tab row** — indent +2
  - **Terminal/tab icon** `TERMINAL_WINDOW`.
  - Tab name; selected tab has a **darker selection bg + `×` close button**
    (revealed on hover / when active).
- **Footer** — pinned `Add Project…` button with `FOLDER_OPEN` icon.

### States & interactions
- Hover bg on rows; **selected vs. active** are distinct visual states.
- Expand/collapse per project & worktree, **persisted** in session.
- **Right-click context menu** (rename, remove, set tint, new workspace…).
- **`+` workspace / `×` remove** affordances.
- **Drag-drop**: reorder projects, group into folders (group headers + tints).
- Click selects + drives the breadcrumb and active layout.

### Port status — ~5%
Flat clickable text list with selection highlight + breadcrumb. **Missing:**
icons, tints, chevrons/expansion, branch-blue styling, change badges, close/×,
Add-Project button, context menus, drag-drop, group headers.

---

## 2. Top bar — Main Panel header  (egui: `src/ui/top.rs`, height 34px)

- **Left:** left-panel-toggle icon (`SIDEBAR`), then breadcrumb
  `crane / feat/warpui-renderer / Tab 1`.
- **Right (button cluster):** `Terminal` (terminal icon), `Browser` (globe),
  git-branch icon button, right-panel-toggle icon. Hover + active states.
- **Behavior:** the two panel toggles show/hide the left/right sidebars
  (Cmd+B / Cmd+/).

### Port status — ~20%
Breadcrumb only (and it tracks selection ✅). **Missing:** all right-side
buttons, working panel toggles, hover states.

---

## 3. Center — Pane area  (egui: `src/ui/pane_view.rs`, ~620 ln)

- **Pane header bar:** `Terminal · Terminal` title (left);
  **maximize (`ARROWS_OUT`) + close (`X`)** buttons (right).
- **Tab strip:** terminal sub-tabs (`crane ×  crane ×`) + `+` new-tab button;
  active-tab highlight; click-to-switch; close per tab.
- **Content:** the terminal grid; **focus border** (accent) on the active pane;
  subtle border on inactive panes; translucent dim overlay on inactive panes
  (Warp-style, painted last — not a border).
- **Underlying model:** recursive **Layout `Node` tree** (`Node::Leaf` /
  `Node::Split{ dir, ratio }`):
  - Split via **Cmd+D** (horizontal) / **Cmd+Shift+D** (vertical).
  - **Draggable splitters** (resize ratio, clamp 0.05–0.95).
  - **Dock-zone drag-drop** rearrange (5-region: left/right/top/bottom/center).
  - **Cmd+W** close focused pane; **Cmd+[ / Cmd+]** focus prev/next.

### Port status — ~15%
Single hardcoded `terminal | files` split with one draggable bar
(`SplitRow`). **Missing:** tab strip, pane header + buttons, recursive Node
tree, dynamic split, dock zones, focus/dim overlays, pane navigation.

---

## 4. Right Panel — Changes / Files  (egui: `src/ui/explorer.rs`, ~1800 ln)

- **Tab switcher:** `Changes` | `Files` (active one underlined in accent).
- **Files tab (shown):** real FS tree of the active worktree —
  - Folder rows: chevron + `FOLDER` icon, expandable
    (`crates ▾ → crane_term, warp_term_spike ▾ → assets, src, Cargo.toml`;
    `vendor ▾ → pdfium, warp`).
  - File rows: `FILE` icon. One row **selected/highlighted**
    (`warp_term_spike/Cargo.toml`). Click opens the file in the Files pane.
- **Changes tab:** grouped **staged / unstaged**, each file with an
  `M`/`A`/`D`/`U` **status-colored letter**, stage/unstage on click,
  diff-on-click; **commit message box + Commit button**; file move/copy DnD.

### Port status — ~2%
Three hardcoded placeholder rows. **Missing:** Changes/Files switcher, real FS
tree with disclosure + icons, git status colors, stage/commit, open-on-click.

---

## 5. Status bar  (egui: `src/ui/status.rs`, height 28px)

- Left: `GIT_BRANCH` icon + current branch (`feat/warpui-renderer`).
- Also surfaces git-op status / indicators; click opens the **branch picker**.

### Port status — ~10%
Static `main - ready` text. **Missing:** real branch, git status, branch-picker
trigger.

---

## 6. Themes & Modals

- **Themes:** 21 built-in themes (`src/theme.rs`) selectable + live switch; the
  port hardcodes one dark theme as `ColorU` consts. Need all 21 as data + a
  switcher (reuse `Rgb::to_warp()`).
- **Modals:** branch picker, settings, help, new-workspace, quit-confirm — each
  a conditional overlay view (flag/Option on the root view), rendered last
  (z-above), center-anchored, Esc-to-cancel, underlying views non-interactive
  while open. Opened/closed via `dispatch_typed_action`.

### Port status — 0%.

---

## Recommended execution order (to reach 1:1)

1. **Left project tree** — icons + tints, chevrons + expand/collapse, the
   3-level hierarchy, branch-blue, badges, `×`/`+`, Add-Project. *(Most-used;
   exercises every pattern the rest needs.)*
2. **Recursive pane Node tree + tab strip + pane header** — replace the
   hardcoded `SplitRow` with the real `Node` tree; tabs; split/close shortcuts.
3. **Right panel** — Changes/Files switcher; real FS tree; git-status Changes
   with stage/commit.
4. **Top bar buttons + panel toggles**, **status bar** (real branch).
5. **Themes** (all 21 + switcher) and **modals** (branch picker first).
6. **Doc panes** (markdown, diff, browser) and the **file editor** (hardest —
   rope-based, see execution doc).

Each item ships as: build → run → compare against egui original → fix to 1:1 →
commit.
