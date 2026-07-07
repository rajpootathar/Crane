# warpui — consolidated OPEN parity items (post-verification sweep)

Every item in `warpui-final-parity-audit.md` was re-verified against the current
working tree (feat/warpui-renderer). This file lists only what is still OPEN or
PARTIAL. Everything else in that audit is FIXED — notably: all four data-loss
confirms at the *Tab/worktree/pane* level, trash-delete, attention pulse,
folder-group menu + tints, `-z`/untracked git parsing, repo-scoped op status,
push stdin+summary, clickable terminal paths, copy trim, format-on-save +
LSP did_save, git-log lane graph/rows/pills/detail/auto-reload, FS watcher,
staged auto-updater, OSC toasts (in-app), goto-line clamp, light syntect themes.

## P0 — data-loss / correctness (small, fix first)

- **Dirty File-Tab close has no confirm.** `FileTabClose` (shell.rs ~9030-9054)
  drops unsaved edits silently. Top-level Tab close confirms; the file-chip X
  does not. Add a dirty check + confirm modal.
- **Save failure is silent.** `save()` returns bool, shell discards it
  (shell.rs ~8924-8937); `apply_format_result` write failure ignored
  (editor_view.rs ~804-808). Surface an error banner/toast; never let the user
  believe a failed save succeeded.
- **Deleting a file doesn't close its editor tabs** (shell.rs ~9280) — a stale
  dirty buffer can re-write a deleted path. Close matching File Tabs (and any
  under a deleted directory) after `trash::delete`.
- **Image/binary files open as garbled text** — `open_file` does
  `read_to_string().unwrap_or_default()` (shell.rs ~7205). At minimum detect
  binary and open read-only placeholder; ideally an image viewer branch.
- **Remove-worktree modal lacks the `is_main` warning** (RemoveWtInfo has
  dirty + ahead only, shell.rs ~92-105).
- **Silent auto-reload of clean buffers lost** — external-change banner shows
  even when buffer is clean instead of silently reloading
  (editor_view.rs ~2404-2408).

## Remaining HIGH features (each a real work item)

- **Sidebar drag-drop reorder** (projects/groups/workspaces/tabs) — absent.
- **Files-tree internal drag-drop move** (+ alt-copy, de-dupe naming,
  drop-target highlight, external Finder drop, floating chip) — absent.
- **Diff pane wave 2:** per-line syntax highlight; per-hunk stage/unstage
  gutter; minimap; hunk nav + counter; in-body header w/ rename coloring;
  image diff; proper scroll model; error row.
- **Browser pane** — still a placeholder FileView. WKWebView embed (wry),
  per-pane tabs, URL toolbar + normalization, clipboard selectors, Cmd+Opt+T.
- **PDF pane** — absent entirely (pdfium).
- **Git-Log wave 2:** refs left column w/ click-to-filter; commit context menu
  (checkout/branch/cherry-pick/revert/copy hash); filter bar; fetch-all +
  manual refresh buttons; keyboard nav; column resize; geometry/selection
  persistence; MAX_COMMITS 5k→10k.
- **Non-Latin fallback fonts** (PingFang/Hiragino/Noto CJK/Arabic/Hebrew/
  Devanagari) + bundled JetBrains Mono/Cascadia for terminal — currently
  system Menlo only → tofu risk.
- **OS notification banner** (notify_rust) for OSC 9/777 when unfocused —
  in-app toast exists, OS banner missing.
- **Cmd+` NSEvent interception** — handled in on_keydown only; needs runtime
  verification that macOS doesn't pre-empt it; old code installed an NSEvent
  monitor.
- **Settings rebuild:** sidebar sections (Appearance/Editor/Terminal/LSP/
  Shortcuts/About), font-size slider, theme swatches + open-themes-folder,
  custom mono picker, syntax-theme override UI, editor prefs
  (word-wrap/trim/single-click) persisted, per-server LSP status/install UI.

## MEDIUM

**Editor:** diagnostic pills + click-to-jump in status; path breadcrumb +
Save/Unlock/Preview header; editor right-click menu; gutter diff hover
tooltip; scrollbar diag/git minimap markers; incremental highlight (perf);
read-only affordances (lock icon/unlock); trim-on-save UI + persistence.

**Terminal:** image paste (Cmd+V → temp PNG → bracketed paste);
`has_foreground_process` via pgid not alt-screen; sub-row pixel-smooth wheel
paint; per-pane terminal tab strip; scrollbar hover/alpha states +
hide-when-empty; `xterm-crane` terminfo (DEC 2026 advertise);
TERM_PROGRAM_VERSION; CRANE_VT_TRACE; malloc pressure relief on drop.

**Changes:** multi-line commit message (+ Cmd+Enter submit); multi-line
error pill expand; tri-state checkbox visual (action already correct);
rename `old -> new` display.

**Branch picker:** dirty-tree warning banner; multi-repo filter chips;
collapsible Local/remote sections + counts; has-worktree badges;
primary-action model decision (name-click currently checkout; old was
worktree-first — user chose "Both" in followups doc, already implemented);
bottom-anchored resizable popup w/ persisted size; loading spinner;
inline checkout error; open-grace anti-race.

**Modals:** LSP install-prompt modal + download toast; missing-project
relocate modal (+ restore-time path check); Find-in-Files wave 2 (regex/
case/whole-word/mask/scope/async streaming); New-Workspace location modes
(Project-local/Custom/Browse).

**Keyboard:** tab-switcher commit-on-Cmd-release; Cmd+W closes modals;
Cmd+Backspace delete file; Cmd+Z file-op undo stack; Tab/Shift+Tab swallow
gate verify; help card content refresh (Cmd+`, Shift+Tab, Ctrl+C/D, F2).

**Layout:** inactive-pane dim (needs warpui hit-test-transparent overlay);
Esc restores maximized pane; drop-zone accent border.

**Persistence:** window min-size (warpui API TODO); window position +
maximized; markdown pane restore; per-pane terminal tabs; git-log geometry;
LSP language_configs load/save; editor prefs; commit draft; tree
collapse/expansion states; group collapse; rich project fields; update
Dismissed/RemindAt.

**Chrome:** panel-toggle icons reflect state; tooltips (needs a warpui
tooltip primitive — unlocks ~10 L items); active file path on status bar;
native menu Check-for-Updates + Services/Hide/Window submenu.

## LOW (cosmetic / drift — batch opportunistically)

§1: middle-click tab close; F2/Cmd+R rename chord; reveal symlink
canonicalize + worktree `open -R` drift; menu order/label/glyph drift
(worktree/project/tab menus, "Highlight color" heading); hover-revealed
PLUS/X affordances; FOLDER glyph; project-row active tint; Add-Project
footer style; PROJECTS header drift.
§2: group checkbox indeterminate; active chip underline; toolbar true-disable
+ accent border; loose-chip tooltip; empty-state variants; `MD` glyph
priority; bold branch label.
§3: untracked `U` vs `?`; "File Manager" wording; menu order.
§4: I-beam cursor over terminal.
§5: external-change Overwrite + save-refusal; language+indent status labels;
current-line band; preview-tab dim; diff/lock tab icons; markdown
Preview/Edit toggle; find bar real editable field; diag underline solid +
info color; Cmd+hover goto underline; goto-line glyph.
§6: diff tint constants; markdown path bar/code/heading palette drift;
git-log column resize.
§7: top/status divider lines; breadcrumb/branch label size+color; separator
drift; "- ready" suffix; help glyph; picker title/icon.
§10: drop-zone rounding; pane header `N · kind`; close red hover; maximize
hover/glyph-toggle/tooltip; splitter 5px→4px; clamp 0.1→0.05.
§11: About GitHub/Releases links + manual re-check; theme swatches; native
About panel; legacy history migration; timer autosave for non-action state
(splitter drags).
