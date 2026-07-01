# warpCrane — 1:1 Port Master Punchlist

Consolidated from 6 domain audits (old egui `src/` vs new warpui `src/warpui/`), 2026-07-01.
Status legend: ✅ done · 🟡 partial · ❌ missing · 🔵 in-progress.

**Execution rule:** items touching `src/warpui/shell.rs` must land **serially** (parallel
subagents clobber it — learned the hard way). Non-`shell.rs` work (crate `crane_term`,
`grid_element.rs`, `editor_view.rs`, new pane modules) can run in parallel.

---

## Shared infrastructure (spike these first — they unblock whole waves)

- **INFRA-1 · warpui texture-upload API** — blocks Image viewer, PDF, Welcome logo, real Browser.
  Confirm warpui's equivalent of `egui::Context::load_texture`. Spike alongside Welcome logo.
- **INFRA-2 · async git-op system** — `std::thread::spawn` + `Arc<Mutex<OpStatus>>` + `ctx.notify()`
  wake (mirror the terminal reader). Blocks push/pull/fetch + commit spinner/error feedback.
- **INFRA-3 · modal/overlay framework** — generalize the `Dismiss`-overlay used by the project
  context menu into a reusable blocking modal (backdrop dim + click-absorb). Blocks all confirm
  dialogs + new-workspace + find-in-files + settings.
- **INFRA-4 · shared font-size** — `Rc<Cell<f32>>` threaded shell→terminal→editor. Blocks Cmd+=/-/0.
- **INFRA-5 · nav_row extensions** — trailing hover buttons, double-click rename, right-click,
  middle-click, drag-drop on left-tree rows. Blocks tab close-×, rename, reorder, workspace menus.
- **INFRA-6 · scroll containers** — wrap Left Panel column + Right Panel (Changes+Files) in a
  warpui scroll element. They overflow today with long lists.

---

## P0 — Safety & correctness (do first)

- ❌ **Quit confirm** — Cmd+Q / window-× kills running terminals silently. Intercept close request
  + modal. (old `main.rs:393`, `modals/confirm_quit.rs`) [INFRA-3]
- ❌ **Close-running-terminal guard** — Cmd+W on a pane with a foreground process should ask first.
  (`main.rs:218`, `shortcuts.rs:159`) [INFRA-3]
- ❌ **Session `.bak` + atomic write** — `warpui-state.json` writes directly; add `.tmp`+rename+`.bak`
  (old `main.rs:297`).
- 🟡 **Scroll wrappers** — Left Panel + Right Panel lists overflow. [INFRA-6]
- ❌ **File-tree noise filter** — skip `.git`/`target`/`node_modules` in `file_tree::walk()`.

## P1 — Daily-driver regressions (keyboard + core edit + terminal legibility)

Keyboard (chrome audit):
- ❌ **Cmd+[ / Cmd+]** focus prev/next pane · ❌ **Cmd+Shift+W** close tab (action exists, bind key)
- ❌ **Cmd+=/-/0** font zoom [INFRA-4] · ❌ **Cmd+9** git-log toggle (action exists, bind key)

Editor (editor audit Tier 1):
- 🟡 **Tab indent / Shift+Tab outdent** — call Warp `m.indent(false/true)` (`model.rs:311`) instead
  of literal spaces.
- 🟡 **Auto-indent on Enter** — `IndentBehavior::Ignore` → `CopyFromPreviousLine` + `{`-bump.
- ❌ **Page Up/Down** in editor · ❌ **Ln/Col status row** · ❌ **Trim-on-save + save-error banner**

Terminal SGR rendering (terminal audit Tier 1 — visible in every git/diff/man output):
- ❌ **Bold** (`Properties{weight:BOLD}`) · ❌ **Italic** · ❌ **Underline** (`Flags::UNDERLINE`)
  · ❌ **Dim** · ❌ **Hidden** · ❌ **Strikethrough** — all in `grid_element.rs` render loop.
- 🟡 **Cmd+K 2-regime clear** — bare-shell path corrupts TUIs; port `has_foreground_process()` +
  `\x1b[3J` branch (old `view.rs:1569`).
- ❌ **Shift+click extend selection** · ❌ **Cmd+A select-all in terminal**

## P2 — Context-menu wave (infra exists: `Dismiss` overlay + `menu_item()`/`menu_separator()`)

1. ✅ **Project row** menu (reveal/copy-path/tint/remove) — done; add "Initialize Git" for loose.
2. ❌ **Workspace/branch row** — Rename, Reveal, Copy Path, Highlight, Remove Worktree [INFRA-5]
3. ❌ **Tab row** — Rename, Highlight, Default color [INFRA-5]
4. ❌ **Changes row** (right) — Stage, Unstage, Open Diff, Open as File, Copy Path
5. ❌ **Files-tree row** (right) — Open, New File/Folder, Reveal, Copy Path, Move to Trash
6. ❌ **Terminal body** — Copy, Paste, Select All, Clear (new for both) — `on_right_mouse_down` in
   `grid_element.rs`
7. ❌ **Folder-group row** — Highlight, Remove group (blocked on folder groups, P5)

## P3 — Right Panel git (31 missing)

- ❌ **Directory-grouped Changes tree** — port `build_tree()`+`render_change_node()` (old
  `explorer.rs:752`). Changes tab unusable for real repos without it.
- ❌ **Stage-all / Unstage-all** (folder checkbox) · ❌ **Commit disabled-state + error surface**
  · 🟡 **"Commit to <branch> (N)"** label
- ❌ **Async Push / Pull / Fetch** + spinner/Done/Failed pill [INFRA-2]; port `pull()`/`fetch()` to
  `warpui/git.rs` (old `git.rs:607/632`); `ahead_behind()` (old `git.rs:674`) in Changes header.
- ❌ **Files-tree git-status colors** · ❌ **dir-cache** (replace per-frame `read_dir`)
  · ❌ **New File/Folder inline** · ❌ **Delete (confirm)** · ❌ **skip nested-repo paths**
- ❌ **Loose-project guard** (disable Changes tab for non-git roots)
- ❌ **Branch picker** (click branch label → fuzzy popup, local+remote)

## P4 — Document panes (only Terminal + Editor exist today)

- ❌ **Welcome pane** — lowest effort (~280 lines); `PaneContent::Welcome` + logo [INFRA-1] + session.
  Unblocks "new tab = landing" instead of always-terminal.
- ❌ **Markdown pane** — pulldown-cmark render loop (~170 lines) → warpui Flex children; `.md` routing.
- ❌ **Image viewer** — decode + texture [INFRA-1] (spike with Welcome logo).
- ❌ **PDF pane** — `PdfTabState` ports verbatim; needs [INFRA-1] + scroll model.
- ❌ **Browser pane** — `src/browser/` is framework-agnostic; port tab strip + URL bar + `report_pane`
  rect via `RectProbe`. [INFRA-1 for placeholder→real]
- ❌ **Diff pane** (highest complexity) — (a) read-only unified diff (`similar`+syntect+add/del color);
  (b) per-hunk stage/unstage (`git::stage_hunk`) + minimap. Wire Changes-row click → OpenDiff.
- 🟡 **Pane focus border** — 2px accent border element (currently bg-color only).

## P5 — Left Panel richness (44 features audited; project system ✅ landed)

- ❌ **Tab close-× in row** + middle-click close [INFRA-5] · 🟡 **Workspace active highlight**
  · 🟡 **Project tint on name label** (pass tint to nav_row text)
- ❌ **New workspace** modal — `git worktree add` [INFRA-3] · ❌ **Workspace rename** (dbl-click + menu)
  · ❌ **Workspace remove** (dirty-check confirm) · ❌ **Workspace diff-stat badge `+N -M`**
  (`git diff --numstat` per worktree, background refresh)
- ❌ **Loose-project detection** (`.git` check → FOLDER icon, "New tab" vs "New worktree", Init Git)
- ❌ **Tab rename** (dbl-click + F2/Cmd+R + menu) · ❌ **Tab tint** · ❌ **Attention/activity dots**
  (`attention_since` + pulse repaint + bubble-up to collapsed ancestors)
- ❌ **Folder groups** (group header/collapse/tint/menu) · ❌ **Drag-drop reorder** (project/workspace/
  tab + drop-line indicator) [INFRA-5]

## P6 — LSP (none wired into warpui today)

- ❌ **Wire `LspClient`** through shell — `track`+`didOpen`/`didChange`/`didSave` from file actions.
- ❌ **Diagnostic squiggles** (inject into `text_decorations`) + scrollbar markers + severity pills.
- ❌ **Goto-definition** (F12 / Cmd+click, reuse old async dispatch) · ❌ **Format-on-save**
  · ❌ **Hover** (delayed tooltip; lowest LSP priority).

## Also-missing (chrome / editor completeness, slot by value)

- ❌ Settings modal (Appearance/Editor/About) — theme cycle-pill is a stopgap · ❌ Help/Shortcuts modal
- ❌ Find-in-files (Cmd+Shift+F) · ❌ Tab switcher (Cmd+`) · ❌ macOS platform menu · ❌ Update checker
- ❌ Editor: Find/Replace (Cmd+F/H via Warp `Searcher`) + Goto-line · ❌ Comment toggle (Cmd+/)
  · ❌ Alt+Up/Down move-line + duplicate · ❌ gutter git-diff bars · ❌ word-wrap toggle
  · ❌ external-change reload banner · ❌ preview/read-only tabs · ❌ unsaved-close confirm
- ❌ Terminal: clickable URLs/paths (`scan_urls`/`scan_paths`) · ❌ desktop notifications (OSC 9/777)
  · ❌ cursor shape/blink · ❌ OSC-2 title → tab · ❌ bell → audio · ❌ SGR mouse-click forwarding
  · ❌ block/rect selection · ❌ macOS image paste · ❌ OSC-8 hyperlinks · ❌ find-in-terminal

## Explicitly out of scope for v1 (per CLAUDE.md)

Vim mode · plugin system · multi-user · full LSP autocomplete/signature/rename · multi-cursor ·
code folding · minimap · whitespace render · Windows/Linux polish.
