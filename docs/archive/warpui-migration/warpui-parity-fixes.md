I've verified the key claims. Confirmed: font-zoom arms absent, `SelectAllFocused` has only an `editor_at` branch (no terminal `select_all`), and the Browser pane is a placeholder in warpui (so browser-new-tab is genuinely not-yet-ported, not drift). The report follows.

---

# warpui Parity Drift — Consolidated Fix List

Scope: genuine behavioral drift in **already-ported** features. Not-yet-ported features and deliberate improvements are excluded (listed at bottom).

## HIGH

**[high] Terminal block-cursor color + render model** — Old: translucent `terminal_fg`@130 overlay drawn *over* the glyph (char stays readable). New: opaque `accent`@255 block drawn *under* the glyph with fg inverted to `default_bg` (reverse-video). — Fix: `cursor_color()` returns `current().terminal_fg` with alpha 130; in grid_element paint the cursor rect *after* the glyph pass and drop the `fg = default_bg` inversion at line 297. — **Files: `color.rs` + `grid_element.rs`** *(dedupes the Terminal-rendering and Themes reports of the same bug)*

**[high] Cmd+= / Cmd+- / Cmd+0 font zoom** — Old: +1 (max 40) / -1 (min 8) / reset 14.0. New: no `"="`/`"+"`/`"-"`/`"0"` arms in the cmd match — falls through, does nothing. — Fix: add cmd arms `"="|"+" => ZoomIn`, `"-" => ZoomOut`, `"0" => ZoomReset` adjusting warpui font size (±1 clamp 8..40, reset 14.0). — **File: `shell.rs`** (serial)

**[high] Cmd+W close pane — file-tab-first + running-process confirm** — Old: closes active File Tab first if Files pane has >1 tabs; else stages a confirm modal if the terminal has a live foreground process; only idle panes close immediately. New: `CloseFocused` unconditionally tears down the pane and kills the PTY. — Fix: in `CloseFocused`, close the active file tab first when a Files/Editor pane has >1 tabs; stage a confirm modal when the pane's terminal has a live foreground process. — **File: `shell.rs`** (serial)

**[high] Editor auto-indent on Enter (bracket bump + split-with-closer)** — Old: `auto_indent_context()` bumps one level after an open bracket, and when caret sits between `{`/`}` inserts `\n{body_indent}\n{prev_indent}` so the closer drops to its own line; also dedents. New: only re-inserts the current line's leading whitespace verbatim — no bump, no split, no dedent. — Fix: after `m.enter()`, compute bump/dedent from the line before the caret and whether the next char is a closer; insert an extra indent unit on bump and a second newline + `prev_indent` when the next char is a closer. — **File: `editor_view.rs`** (parallel)

## MEDIUM

**[medium] Terminal selection highlight color/alpha** — Old: `selection` opaque (α255), and `(0,0,0)` serde-default falls back to `accent`@72. New: always applies α180 and has no `(0,0,0)`→accent fallback (custom themes omitting `selection` render a near-black wash). — Fix: when `selection == (0,0,0)` fall back to `accent`@72, else use `selection` opaque; drop the unconditional α180. — **File: `theme.rs`** (parallel)

**[medium] Cmd+A select-all in terminal** — Old: builds a Simple selection (0,0)→(rows-1,cols-1) on the terminal. New: `SelectAllFocused` only has an `editor_at` branch; no `terminal_at`, and `TerminalView` has no `select_all`, so nothing happens in a terminal pane. — Fix: add a `terminal_at(id)` branch calling a new `TerminalView::select_all()` that sets the full-grid selection. — **Files: `shell.rs` (serial) + `view.rs`**

**[medium] Cmd+Shift+F Find-in-Files** — Old: opens the global cross-file Find modal. New: `"f"` arm ignores Shift → dispatches `FindFocused` (in-file bar). — Fix: add `"f" if ks.shift => (open Find-in-Files)` before the plain `"f"` arm. — **File: `shell.rs`** (serial)

**[medium] Cmd+O open file / Cmd+Shift+O add project** — Old: file picker opens a file; folder picker adds a project. New: no `"o"` arm — both do nothing (AddProject only reachable via sidebar). — Fix: add `"o" if ks.shift => AddProject` and plain `"o" => open-external-file` via `rfd::FileDialog::pick_file`. — **File: `shell.rs`** (serial)

**[medium] Cmd+B / Cmd+/ panel toggle focus guard** — Old: toggles only when no widget holds keyboard focus (`!any_focus`). New: `"b"`/`"/"` fire unconditionally, toggling even while typing in an editor/terminal/commit box. — Fix: gate `ToggleLeft`/`ToggleRight` on no text-input pane holding focus. — **File: `shell.rs`** (serial)

**[medium] Untinted project icon color** — Old: untinted project's cube icon uses the single theme `accent`. New: `project_color_for(idx)` returns an 8-entry `project_tint` rainbow by `idx % 8`. — Fix: fall back to `theme::accent()` in `project_color_for`; retire the rainbow stand-in for the leading icon. — **File: `shell.rs`** (serial)

**[medium] Active worktree/tab left-edge accent bar** — Old: active rows paint a 2px accent vertical bar at the left edge (inset 3px) atop the row-active fill. New: only the `row_active()` bg fill; no left accent bar. — Fix: in the selected branch of `nav_row`/`worktree_nav_row`/`tab_closeable_row`, add a 2px accent rect pinned left, inset ~3px vertically. — **File: `shell.rs`** (serial)

**[medium] Editor Tab/Shift+Tab indent unit** — Old: per-file discovered unit via `format::discover(path).indent_unit()` (tabs / 2-space / 4-space). New: hardcoded `IndentUnit::Space(4)` at Buffer construction. — Fix: thread `format::discover(path).indent_unit()` into `Buffer::new`/`from_plain_text` instead of the literal `Space(4)`. — **File: `editor_view.rs`** (parallel)

**[medium] Bracket auto-close skip-over** — Old: typing a closer when the next char is that same closer advances past it (no doubled `))`). New: `InsertChars` only handles openers; typing `)` after an auto-inserted `)` doubles it. — Fix: when the typed char is a closer and the next char equals it, move the caret forward instead of inserting. — **File: `editor_view.rs`** (parallel)

**[medium] Cmd+X on empty selection (whole-line cut)** — Old: cuts the whole current line (incl. newline) to clipboard as one undo step. New: `cut()` does `delete(Backwards, Character)` — deletes one char back and never writes the clipboard. — Fix: on empty selection, select the full line range, write it to clipboard, then delete; fall back to selection-delete only when a selection exists. — **File: `editor_view.rs`** (parallel)

**[medium] Cmd+C on empty selection (whole-line copy)** — Old: copies the whole current line (incl. newline). New: `copy()` reads the (empty) selection → copies nothing. — Fix: when selection is empty, copy the current line text with trailing newline. — **File: `editor_view.rs`** (parallel)

**[medium] Editor Find case sensitivity** — Old: case-sensitive (`matches`/`find`/`rfind` on raw text). New: `find_all()` lowercases both sides → case-insensitive. — Fix: compare raw chars (case-sensitive) to match old, or gate insensitivity behind an explicit toggle. — **File: `editor_view.rs`** (parallel)

**[medium] Cmd+S trim-on-save** — Old: `save_tab` honors `prefs.trim_on_save` before writing. New: `save()` writes the raw buffer only. — Fix: apply trim-trailing-whitespace when the pref is set before writing. — **File: `editor_view.rs`** (parallel) *(formatter pass + LSP `notify_saved` excluded — dependent on not-yet-ported formatter/LSP infra; revisit when those land)*

**[medium] Shift+click extend selection (terminal)** — Old: shift-click over an existing selection calls `sel.update(point, side)` to extend. New: mouse-sel callback carries no modifier; Down always starts a fresh selection. — Fix: thread a `shift: bool` through `mouse_sel_cb`; in Down, when shift + existing selection, call `sel.update(pt, side)` instead of replacing. — **File: `view.rs`** (+ `grid_element.rs` to forward the modifier) (parallel)

**[medium] Block (column) drag selection over TUI separators** — Old: `is_inside_vertical_separators()` promotes the drag to `SelectionType::Block` so dragging a lazygit/k9s column stays rectangular. New: Down always uses `Simple`; drag spans neighboring columns. — Fix: port `is_inside_vertical_separators()` and pick Block vs Simple in the single-click Down branch. — **File: `view.rs`** (parallel)

**[medium] IME / composed multi-codepoint text input** — Old: printable + IME/dead-key/emoji strings arrive via `Event::Text` and are written verbatim to the PTY. New: only the single `Keystroke.key` char is emitted via `keystroke_to_pty_bytes`; `chars`/insert-text ignored, so CJK/emoji composition never reaches the PTY. — Fix: add a text/insert-text route that writes non-empty composed bytes to the PTY (`ctrl.write_input`), mirroring the old `Event::Text` branch. — **Files: `input.rs` + `view.rs` + `shell.rs`** (touches shell — treat serial)

## LOW

**[low] Cmd+9 Git Log — Shift not excluded** — Old: only bare Cmd+9 toggles (`!shift`). New: `"9"` has no shift check, so Cmd+Shift+9 also opens it. — Fix: gate the `"9"` arm on `!ks.shift`. — **File: `shell.rs`** (serial)

**[low] Cmd+Z file-op undo** — Old: Cmd+Z with no editor focus undoes the last Files-Pane move/trash op (`undo_last_file_op`). New: `"z"` only undoes editor text edits. — Fix: add a file-op undo path for the no-editor-focus case. — **File: `shell.rs`** (serial) *(depends on a Files-pane move/trash undo stack existing in warpui; if that infra isn't ported, this is out of scope until it is)*

**[low] Worktree dirty-dot fallback for (0,0) diff** — Old: dirty worktree with no line stats gets a 3px filled add-color dot. New: `worktree_nav_row` renders nothing for `(0,0)`; the dirty indicator is lost. — Fix: render a small success-colored dot when `diff_stat == (0,0)` but the worktree is dirty (ideally detect dirty-without-linestats rather than numstat-only). — **File: `shell.rs`** (serial)

**[low] Active tab leading-icon color** — Old: active tab's `TERMINAL_WINDOW` icon painted in `accent()`. New: `tab_closeable_row` uses `tcol` (text_hover) for both icon and label. — Fix: pass an accent icon color for the active tab's leading glyph, separate from the label color. — **File: `shell.rs`** (serial)

**[low] Project context menu — labels / Remove icon / Init-Git order** — Old: "Reveal in File Manager" (`FOLDER_OPEN`), Remove uses `icons::X`, Initialize-Git placed after Default color. New: "Reveal in Finder", Remove uses `icons::TRASH`, Init-Git placed before the tint palette. — Fix: rename to "Reveal in File Manager", use `icons::X` for Remove, move Init-Git to after Default color. — **File: `shell.rs`** (serial)

**[low] Reveal-in-file-manager path canonicalize** — Old: `std::fs::canonicalize` (fallback to original) before `open`, resolving symlinks/stale `~`. New: passes `p.path` directly — symlinked/non-canonical paths can silently fail. — Fix: canonicalize before spawning `open`/`xdg-open` in `RevealProjectInFinder`. — **File: `shell.rs`** (serial)

**[low] Editor Cmd+F prefill from multi-line selection** — Old: prefills query from any non-empty selection incl. newlines. New: gated on `!sel.contains('\n')`. — Fix: drop the newline guard. — **File: `editor_view.rs`** (parallel)

**[low] Editor Page Up / Page Down caret movement** — Old: native TextEdit PageUp/Down both scrolls *and* moves caret by a page. New: `PageScroll` only scrolls the viewport; caret stays put. — Fix: also move the caret by a viewport-height of lines. — **File: `editor_view.rs`** (parallel)

**[low] Editor Find match-count format** — Old: single total count (`3`). New: `N/M` (or `no match` / `0/M`). — Fix (optional, only if strict parity): render just the total. *Richer N/M is arguably a deliberate improvement — recommend keeping.* — **File: `find_bar_element.rs`** (parallel)

**[low] Consecutive-click detection window** — Old: 500ms. New: tightened to 350ms → double/triple-click harder to trigger. — Fix: change 350 back to 500. — **File: `view.rs`** (parallel)

**[low] Quadruple+ click clears selection** — Old: `2 => word`, `3 => Lines`, `_ => None` (4th click cycles back to cleared). New: `count >= 3 => Lines`, so 4th+ stays a line selection. — Fix: treat `count == 3` as Lines and `count >= 4` (the `_` arm with count>1) as clearing. — **File: `view.rs`** (parallel)

**[low] Scrollbar thumb drag vs click-to-jump** — Old: drag armed only on the thumb rect, incremental via `drag_delta().y`; empty-track click does nothing. New: LeftMouseDown anywhere jump-scrolls to the absolute fraction and ignores thumb height (thumb offset from cursor). — Fix: restrict LeftMouseDown to the thumb rect with incremental delta, or at minimum compute frac against `(track_h - thumb_h)`. — **File: `scrollbar_element.rs`** (parallel)

**[low] Alt/Option key encoding (case + non-letters)** — Old: Alt+letter via `key_letter()` always lowercase a-z; Alt+symbol/digit sends nothing. New: emits `ESC + k.as_bytes()` with case preserved for any char (M-A vs M-a differ in readline/emacs). — Fix (only if strict 1:1): lowercase the char and restrict the Alt+char arm to ASCII letters. *New is more capable — align only if strict parity required.* — **File: `input.rs`** (parallel)

**[low] SGR underline vertical position** — Old: underline via egui font underline metric. New: manual 1px rect at `baseline + 2.0` (fixed offset). — Fix (cosmetic): adjust the `+2.0` offset for pixel parity. — **File: `grid_element.rs`** (parallel)

---

## Excluded — not drift

**Not-yet-ported features (out of scope — no old behavior to match yet):**
- **Cmd+Option+T browser new tab** — Browser pane is a placeholder in warpui (`open_browser` renders a stub FileView); the whole keydown block is `!ks.alt`-gated. Wire when the `wry` browser pane lands.
- **Cmd+S formatter pass + LSP `notify_saved`** — dependent on formatter/LSP infra not yet ported to the warpui editor (trim-on-save *is* portable and is kept above as medium).

**Deliberate improvements — keep new, do NOT revert (flagged by checkers themselves):**
- Cursor visibility honors DECTCEM (new suppresses hidden cursor; old always drew a block).
- Cursor width doubles over CJK/emoji wide cells (new is more correct).
- Goto-line on Cmd+G — intentional alignment to the CLAUDE.md-canonical shortcut (old used Ctrl+G).

---

## Counts

**By severity (actionable drift):** High 4 · Medium 15 · Low 15 = **34 items** (2 low are optional/parity-only: Find N/M format, Alt encoding).

**By target file (serial = shell.rs; rest parallel):**

| Target file | High | Med | Low | Total | Lane |
|---|---|---|---|---|---|
| `shell.rs` | 2 | 6 | 5 | 13 | **serial** |
| `editor_view.rs` | 1 | 5 | 2 | 8 | parallel |
| `view.rs` (terminal) | – | 2 | 2 | 4 | parallel |
| `color.rs` + `grid_element.rs` (cursor) | 1 | – | – | 1 | parallel |
| `grid_element.rs` (underline, standalone) | – | – | 1 | 1 | parallel |
| `theme.rs` | – | 1 | – | 1 | parallel |
| `input.rs` (+view/shell for IME) | – | 1 | 1 | 2 | IME touches shell → serial |
| `scrollbar_element.rs` | – | – | 1 | 1 | parallel |
| `find_bar_element.rs` | – | – | 1 | 1 | parallel |

Notes: Cmd+A select-all and Shift+click/Block selection span `shell.rs`+`view.rs` and `view.rs`+`grid_element.rs` respectively; the IME item spans `input.rs`+`view.rs`+`shell.rs` (so treat as serial with the shell batch). The single cursor-color fix is counted once (dedup of two reports).

Recommended execution: run the **13 `shell.rs` items as one serial batch** (they all edit the cmd-keydown match, row builders, and action handlers in the same file), and fan the **`editor_view.rs`, terminal `view.rs`, `color.rs`/`grid_element.rs`, `theme.rs`, `scrollbar_element.rs`, `find_bar_element.rs`** groups out in parallel.
