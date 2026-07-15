# Chrome Polish Sweep — Design

Date: 2026-07-15
Status: approved via visual-companion brainstorm (mockups in `.superpowers/brainstorm/72599-1784105054/content/`)

## Goal

A full chrome sweep of the warpui front-end: compact, flat, Warp-like, consistent
hover language everywhere, plus two functional fixes (live change counters,
smooth notification animation). No behavior changes except those listed.

## Decisions (user-validated)

### 1. Pane header — all panes ("Classic slim", option A)

- Height 26px → **24px**; hairline `divider()` line under every header.
- Close/maximize: 15px glyph + 5px padding → **12px glyph in a 20×20 hit box**
  (4px padding), wrapped in `Hoverable`: hover = `row_hover`-style wash +
  `text_hover` glyph + PointingHand cursor. Buttons remain always visible.
- Focused pane keeps accent title; unfocused keeps muted title (unchanged).

### 2. File pane (and Diff pane) — two-row header

- **Row 1 (24px, pane chrome):** pane icon + title ("Files" / "Diff: <name>"),
  maximize + close on the right. This ✕ closes the **pane**. Click row = focus pane.
- **Row 2 (26px, tab strip):** file tabs move here, full width.
  - Active tab: `surface()` bg + **2px accent underline** at bottom edge + bright text.
  - Inactive: flat, muted text, hover wash.
  - Per-tab ✕ in a 16×16 hover box (closes that tab only); dirty dot stays.
- Terminal/Browser/Markdown/Welcome panes keep the single 24px row.

### 3. Left Panel

- Rows 26px → **24px**, 4px side margins, 4px corner radius on row highlights.
- **Hover wash** on every row: 3.5% white overlay + brightened text.
- **Two-tier selection** (leaf-only "selected", ancestors as context):
  - Selected row: **7% white wash + pure white text and icon**.
  - Ancestor chain (project → workspace → tab of the selection): **2.5% wash +
    slightly brightened text**. No more painting the whole chain as selected.
- **Indent guides**: faint vertical line per depth level.
- Collapsed projects show a muted child-count on the right.
- `PROJECTS` header gains a small ＋ (18×18, hover wash) = Add Project.
- Boxed "Add Project" button becomes a **quiet footer row** (border-top divider,
  hover brightens) — same action.
- **Live +x/−y counters** right-aligned on branch (workspace) rows, green/red,
  watcher-driven (see §8).

### 4. Right Panel

- Changes/Files switcher restyled as **underline tabs** (same language as file
  tabs): active = accent underline + bright text; inactive muted + hover wash;
  hairline divider under the tab row. Loose-project disabled state stays.

### 5. Context menus (all: project rows, folder groups, right-panel rows)

- 8px corner radius, 1px `border()`, **drop shadow**, 5px inset padding.
- Items: 5px corner radius, hover = 7% white wash + white text;
  destructive items hover with a red-tinted wash + `error()` text.
- Right-aligned muted keyboard hints where a shortcut exists.
- Section labels (small caps, muted) + 1px separators between groups.
- Highlight-color picker: **circular 18px swatches**, scale-up on hover,
  white ring on the active one, plus a "none" swatch (hollow).

### 6. Top bar (36px)

Layout: `[traffic-light inset] [left-panel toggle] [breadcrumb capsule] …spacer…
[＋ New Pane ▾] [git log] [right-panel toggle]`

- **Breadcrumb capsule**: `<project> ⎇ <branch>` in a rounded 24px capsule
  (1px border, `sidebar_bg` fill); hover = accent-tinted border + brighter text;
  click opens Switch Branch modal.
- **＋ New Pane dropdown** (styled per §5): Terminal ⌘T, Browser ⌘⇧B, File… ⌘O,
  separator, SPLIT: Split right ⌘D, Split down ⌘⇧D.
- **Removed from top bar**: Terminal pill, Browser pill, theme pill.
- Icon buttons 22×22 with the universal hover wash; all controls vertically
  centered on one 24px line.
- Subtle top-lit vertical gradient on the bar background (topbar_bg lightened
  ~4% at top) for depth.

### 7. Status bar (28px → 26px) — "live repo pulse"

Left → right:
- **Branch cluster**: state dot (green = clean, amber = dirty working tree) +
  `⎇ branch`; click = Switch Branch (existing).
- **+x/−y chip**: live insertions/deletions for the active workspace (same data
  as §3 counters), hidden when clean.
- Spacer.
- **Agent/process activity** (stretch, see Open Items): foreground command of
  the focused terminal + elapsed time, with a green running dot.
- **Ln/Col** + selection info (existing), with hover wash.
- **⚙ gear** → menu (per §5): THEME section listing themes (active marked),
  Keyboard Shortcuts, Settings…. The theme cycle behavior moves here as a
  proper picker instead of a blind cycle button.

### 8. Functional fixes

- **Live change counters**: git-status refresh (feeding Left Panel +x/−y, Right
  Panel Changes, status-bar dot/chip) subscribes to the existing file watcher
  (`file_watcher.rs`) with a debounce (~300–500ms) so counters update as files
  change — not only after manual actions. Watcher must cover each open
  workspace root; git ops keep forcing an immediate refresh.
- **Notification highlight animation**: animate at full frame rate by
  requesting continuous repaints (or timed wakes) for the duration of the
  animation instead of relying on incidental repaints.

### 9. New keyboard shortcuts

- **⌘⇧B** — new Browser pane (split, like ⌘T does for terminal).
- **⌘O** — open File… picker into the Files pane.
- Both listed in the Keyboard Shortcuts modal and the ＋ New Pane menu.

## Implementation notes

- All new colors derive from existing theme tokens or white/black alpha washes —
  no hardcoded hex, works across every theme. New helpers in
  `src/warpui/theme.rs`: `hover_wash()`, `selection_wash()`, `context_wash()`
  (white overlays at 3.5% / 7% / 2.5%), `shadow()`.
- Hover behavior uses the existing `Hoverable` element (`hover_handle` pattern
  already in shell.rs).
- Shared constants: `HEADER_H = 24.0`, `TAB_H = 26.0`, `STATUS_H = 26.0`;
  a single `icon_button` implementation gains hover + hit-box sizing and is
  reused by pane headers, top bar, PROJECTS ＋, and tab closes.
- Menus need a shadow primitive; if warpui `Rect` lacks blur shadows, fake it
  with 2–3 stacked translucent rects (cheap, matches GPU renderer style).
- Icons stay `egui_phosphor`-equivalent glyph set already bundled (icons.rs) —
  no Unicode tofu risk (CLAUDE.md rule).
- Diff pane tab strip only if the Diff pane actually hosts multiple diffs today;
  otherwise it gets the two-row header with a single static title row 1 and no
  row 2 (unified document pane remains deferred per memory).

## Out of scope

- Drag-drop pane rearrange, session restore, wry browser — untouched.
- Renaming Workspace/Worktree structs.
- Unified file+diff document pane (deferred).

## Open items

- Status-bar agent-activity indicator depends on cheaply reading the PTY
  foreground process name. If not cheap, ship the bar without it and revisit.
