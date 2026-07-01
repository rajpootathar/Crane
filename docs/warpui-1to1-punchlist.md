# warpui 1:1 Parity Punchlist (consolidated 2026-07-01)

Master gap list from a four-region line-by-line audit of the OLD egui Crane vs the
NEW warpui Crane (`src/warpui/`). Old egui sources: `src/ui/`, `src/views/`,
`src/modals/`, `src/main.rs`, `src/shortcuts.rs`, `src/platform_menu.rs`,
`src/lsp/`, `src/theme.rs`.

Status legend: ✅ done · 🟡 partial · ⛔ missing.

## Editor — DONE this session
click-to-cursor, drag-select, typing/backspace/enter/tab, undo/redo, select-all,
copy/cut/paste, syntax colors (theme-matched), line-number gutter + current-line
(gutter only), scroll-wheel, arrows/word/home-end motion, bracket auto-close,
IBeam cursor, multi-tab editor + persistence.

---

## TIER 1 — Editor essentials (active surface, high frequency)
- [ ] **Find / Replace (Cmd+F / Cmd+H)** — bar, match highlight, next/prev, count, replace one/all. Old: `views/file_find.rs`, `file_view.rs:340-377,554-625`.
- [ ] **Go-to-line (Ctrl+G / Cmd+L)** — modal → jump. Old: `file_view.rs:627-678`.
- [ ] **Current-line background highlight in code area** (not just gutter). Old: `file_view.rs:1684-1710`.
- [ ] **PageUp / PageDown** — scroll by viewport height. `input_key` TODO.
- [ ] **Cmd+←/→ (line boundary), Cmd+↑/↓ (doc start/end)** — needs shell to route Cmd+arrow to editor.
- [ ] **Cut/Copy whole line when selection empty** (Cmd+X/Cmd+C). Old: `file_view.rs:1270-1346,1516-1553`.
- [ ] **Bracket skip-over** — typing `}` over an existing `}` skips. Old: `file_view.rs:1096-1148`.
- [ ] **Toggle line comment (Cmd+/)** — language-aware. Old: `file_view.rs:1249-1263`.
- [ ] **Move line up/down (Alt+↑/↓)**, **Duplicate line (Alt+Shift+↓)**. Old: `file_view.rs:1150-1246`.
- [ ] **Status bar: Ln/Col + selection info + indent/lang**. Old: `views/file_status.rs`, `src/ui/status.rs`.
- [ ] **Dirty indicator in tab** (unsaved dot). Old: `file_view.rs:285-292`.
- [ ] **Right-click context menu**: Copy Path, Reveal in Finder, Save. Old: `file_view.rs:1557-1597`.
- [ ] **Word-wrap toggle** (currently hard InfiniteWidth). Old: `file_view.rs:1353-1357`.
- [ ] **Auto-indent on Enter** (bracket-aware), **Shift+Tab outdent**, **project indent detection**. Old: `file_view.rs:924-1094`.
- [ ] **External change detection + Reload banner**; **trim-trailing/format on save**; **save-error banner**. Old: `views/file_save.rs`.

## TIER 1 — LSP (reuse `src/lsp/`)
- [ ] **Diagnostics squiggles** in editor + **scrollbar markers** + **status pills**. Old: `views/diagnostics_overlay.rs`, `views/file_status.rs`.
- [ ] **Hover type info** + **Cmd-hover underline**. Old: `file_view.rs:1362-1421`.
- [ ] **Goto-definition (F12 / Cmd+click)**. Old: `file_view.rs:1489-1515`.
- [ ] **Autocomplete** (net-new; old had none) — later.

## TIER 1 — Git gutter in editor
- [ ] **Diff markers in gutter** (added/modified/deleted bars) + **hover tooltip** + **scrollbar markers**. Old: `file_view.rs:1650-1900`.

---

## TIER 2 — Doc panes (whole panes unported)
- [ ] **Diff viewer pane** — side-by-side/unified, per-hunk stage, syntect, hunk nav, minimap, async. Old: `views/diff_view.rs` (904 lines). **Needed for git staging.**
- [ ] **Markdown preview pane** — pulldown-cmark render, edit/preview toggle. Old: `views/markdown_view.rs` (223).
- [ ] **Image viewer** — png/jpg/gif/webp decode + pan/zoom. Old: `file_view.rs:830-861`.
- [ ] **PDF viewer** — pdfium, multi-page, zoom, select/copy. Old: `views/pdf_view.rs` (660).
- [ ] **Browser pane** — WKWebView (wry), tabs, URL bar, nav, mem monitor. Old: `views/browser_view.rs` (522). Currently a stub.
- [ ] **Welcome / landing page** — logo, action buttons, shortcut cheat-sheet. Old: `views/welcome_view.rs` (281).

---

## TIER 2 — Right Panel (Changes + Files)
Changes tab:
- [ ] **Push / Pull / Fetch buttons** + spinners + ahead/behind (↑N ↓N). Old: `ui/explorer.rs:257-331`.
- [ ] **Changes grouped tree** by directory; staged/unstaged/untracked groups. Old: `ui/explorer.rs:365-963`.
- [ ] **Click-to-open-diff** (HEAD↔working) instead of open-in-editor. Old: `ui/explorer.rs:568,929`.
- [ ] **Discard/restore** changes; **stage/unstage via context menu**; **copy path**. Old: `ui/explorer.rs:937-959`.
- [ ] **Async git ops** with spinner + **error display**. Old: `ui/explorer.rs:64-73,487-530`.
Files tab:
- [ ] **Git status colors** in tree; hide `.git/target/node_modules`. Old: `ui/explorer.rs:1293-1327`.
- [ ] **New file / new folder / move-to-trash / rename** (inline). Old: `ui/explorer.rs:1484-1759`.
- [ ] **Reveal in Finder / Copy Path / Open Diff** context menu. Old: `ui/explorer.rs:1459-1515`.
- [ ] **Drag-drop files** (move / Alt=copy), external drop. Old: `ui/explorer.rs:1360-1444`.
- [ ] **Search/filter files**; **refresh on file-watch**. Old: `src/file_watcher.rs`.

## TIER 2 — Left Panel (Projects)
- [ ] **Add project** (folder picker) + **Remove project**. Old: `ui/projects.rs:152,500`.
- [ ] **Worktree add / remove / rename** (new_workspace modal). Old: `ui/projects.rs:495,807,852`.
- [ ] **Context menus** (tint palette, rename, reveal, remove, copy path). Old: `ui/projects.rs:325-636`.
- [ ] **Tab rows under worktrees** + tab context menu + attention pulsing. Old: `ui/projects.rs:860-1115`.
- [ ] **Folder groups** (group projects by directory). Old: `ui/projects.rs:268-366`.
- [ ] **Drag-drop reorder** projects/groups. Old: `ui/projects.rs:514-546`.
- [ ] **Per-project tint customization** (currently hardcoded).

---

## TIER 3 — App chrome
- [ ] **Cmd+[ / Cmd+]** focus prev/next pane. Old: `shortcuts.rs:116-117`.
- [ ] **Font zoom Cmd+= / Cmd+- / Cmd+0**. Old: `shortcuts.rs:118-120`.
- [ ] **Cmd+Shift+W** close tab; **Cmd+9** git log binding.
- [ ] **Cmd+O / Cmd+Shift+O** file/folder pickers (rfd). Old: `shortcuts.rs:245-276`.
- [ ] **Breadcrumb** (project/workspace/file trail) in top bar. Old: `ui/top.rs:48-51`.
- [ ] **Menu bar** (About, Check Updates, Settings, Quit, Open, Help). Old: `platform_menu.rs`.
- [ ] **Modals**: Settings, Help/shortcuts, Find-in-files (Cmd+Shift+F), New workspace, Confirm quit, Confirm close terminal, LSP install toast, Update toast. Old: `src/modals/*`.
- [ ] **Theme switching** (20+ themes, persist) + light/dark. Old: `theme.rs`.
- [ ] **Update checker** (startup + menu + toast). Old: `main.rs:149`.
- [ ] **Window icon (crane.png)**, default/min size, persist geometry. Old: `main.rs:61-73`, `startup.rs`.
- [ ] **Terminal/Browser top-bar buttons**; responsive top-bar layout.

---

## Notes
- `src/warpui/file_pane.rs` (old hand-rolled `FileView`) is now **dead** — editor replaced it; remove after parity.
- Terminal pane is ✅ done (`src/warpui/view.rs`).
- LSP backend `src/lsp/` (~1825 lines) is mostly framework-agnostic — reuse for diagnostics/hover/goto/complete.

## Queued (post-theme-subagent)
- [ ] Editor: dragging the pane (splitter/header) must NOT extend text selection. Gate `SelectionExtend` on a `selecting` flag set by an in-editor mouse-down (CursorPlace) and cleared on left_mouse_up. Splitter/header drags never send the editor a mouse-down, so they will not select.
- [ ] Terminal scroll responsiveness: `(delta.y()/10.0).round()` drops small trackpad deltas to 0 → feels un-scrollable. Accumulate fractional delta in a `Cell<f32>` on TerminalView and scroll whole lines as they accrue; verify direction sign. (view.rs on_scroll_wheel)

## User-flagged (left panel — like old Crane)
- [ ] **Add Project** system (folder picker → add to projects tree). Old: `ui/projects.rs:152`.
- [ ] **Project / folder grouping** system (group projects by directory, collapsible groups). Old: `ui/projects.rs:268-366`.
- [ ] **Context menus** in Left Panel (project/workspace/tab: tint palette, rename, reveal, remove, copy path). Old: `ui/projects.rs:325-636,806-856`.
