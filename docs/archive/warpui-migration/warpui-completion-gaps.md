Verified the load-bearing claims (RemoveProject teardown, any_text_input_focused, OpenExternalFile missing notify, editor colors one-shot, project psel, trim-on-save dormant). Consolidated list below.

# Crane warpui — Consolidated end-to-end fix list

Dropped as false-positive / not-a-live-break: **trim-on-save dirty dot** (editor_view.rs:519-524) — real logic bug but `trim_on_save` is hard-coded `false` (editor_view.rs:643), so it cannot fire today. Latent only; fix opportunistically when the pref is wired, not on this pass.

Kept 16 genuine breaks. `[H]`/`[M]`/`[L]` = severity.

## Group A — shell.rs (SERIAL — all touch the same dispatch/render file)

1. `[H]` **Remove Project — positional-index state not remapped/torn down** — removing a non-last (or the active) project shifts every later project's index but `worktree_tabs/layouts/active_tab/selected/expanded_*` stay keyed to old indices → surviving projects show the wrong tabs, wrong worktree lights up, and a removed active project keeps rendering + leaks its PTY vs. clean teardown + correct tabs — shell.rs:4426-4435 (no teardown/remap) + center() shell.rs:2696 — before/after `reload_projects()` drop the removed project's `(pi,*)` layouts/panes/worktree_tabs, clear/repoint `active_tab/selected/focused`, and remap remaining maps to new indices (or rekey these maps by project path) — **shell.rs**

2. `[M]` **Cmd+O — opened file doesn't appear until an unrelated event** — picker callback runs `open_file` (mutates panes/layouts) but never calls `vctx.notify()`, and `ctx.spawn` does not auto-dirty → the new Editor pane is invisible until the next mouse move/keypress vs. appears immediately like Add Project — shell.rs:4415-4425 (callback lacks notify; AddProject at 4408 has it) — add `vctx.notify();` at end of the OpenExternalFile spawn callback — **shell.rs**

3. `[M]` **Branch picker — remote entry checks out detached HEAD** — clicking `origin/main` dispatches `git checkout origin/main` (full ref, no DWIM) landing on detached HEAD shown as a bare SHA; local+remote also listed with no dedup vs. checkout/create the local tracking branch and dedup against locals — shell.rs:4231 (ShowBranchPicker adds remotes) + 4238 (CheckoutBranch) + git.rs:185 — strip `origin/` and skip names already local in the picker, or detect `<remote>/<name>` in CheckoutBranch and run `git switch --track` / `git checkout <name>` — **shell.rs** (+git.rs)

4. `[M]` **Changes/Files right panel has no ScrollArea** — `right_sidebar` builds an unbounded `Flex::column` with no scroll wrapper, so with many changed files the commit message box + Commit button (and lower Files rows) push below the viewport and become clipped/unclickable vs. content scrolls with Commit box reachable — shell.rs:2004 right_sidebar / 1547 panel() — wrap the row list (and file_rows) in a scroll container with commit_box pinned, `id_salt` per project rule — **shell.rs**

5. `[M]` **Worktree/branch row right-click menu missing** — `worktree_nav_row` only wires `on_left_mouse_down`; no right-click handler/action exists → right-click is swallowed vs. popover with Rename / Reveal / Copy Path / Highlight palette / Remove Worktree — shell.rs:1367 (built at 1770) — add `ShowWorktreeMenu{pi,wi,x,y}` + WorktreeMenu overlay (mirror project row 1693-1704 / menu_popover 906-1016), store in `worktree_menu` Option rendered at 3621-3631 — **shell.rs**

6. `[M]` **Tab row right-click menu missing** — `tab_closeable_row` wires only left-click select + close button; no right-click → no-op vs. popover with Rename / Highlight palette / Close Tab / Close Other Tabs — shell.rs:1481 (built at 1793) — add `ShowTabMenu{key,x,y}` + TabMenu overlay (reuse menu_popover 906), store in `tab_menu` Option rendered at 3621-3631 — **shell.rs**

7. `[H→demoted M] **Remove Project on a grouped child repo is a visible no-op** — RemoveProject pushes the child's own path into `removed_project_paths`, but the reload filter `folders.retain(|f| !removed.contains(&f.path))` keys on the opened *container* folder path, and `expand_folder` re-scans and re-emits the same child → child reappears unchanged — reported `[H]` but net effect is a no-op (nothing corrupted), treat as M — projects.rs:300 (retain on container path) vs shell.rs:4429 (removes by child path) — suppress child repos by their own path during expansion in `child_project_node/expand_folder`, or gate the Remove item on `p.group_path.is_none()` — **projects.rs (+shell.rs)** *(see Group C — the projects.rs half is parallel-safe)*

8. `[L]` **Cmd+B / Cmd+/ dead after opening a file** — `any_text_input_focused()` returns true whenever the active pane is *any* Editor/File pane (not only while typing), so panel toggles silently no-op merely because a file pane is focused vs. old egui gated on real keyboard focus (`ctx.memory().focused()`) — shell.rs:4341-4350 gated by 3376-3388 — narrow `any_text_input_focused()` to true only when genuinely capturing text (commit box / pending-new-entry / editor with active text cursor or find bar) — **shell.rs**

9. `[L]` **Cmd+W destroys all file tabs at once** — CloseFocused always tears down the whole pane even with >1 open file tabs (documented file-tab-first behavior is a TODO) vs. close only the active File Tab, tear down pane on last tab — shell.rs:3918-3922 — if focused pane is files_pane and `file_pane_paths.len() > 1`, route to existing FileTabClose logic; else fall through — **shell.rs**

10. `[L]` **Project row never highlights on select** — `psel = sel == (pi, usize::MAX, usize::MAX)` but `selected` is only ever assigned concrete tab keys (never MAX except the initial `(0,0,MAX)`) → clicking a project row expands/collapses with no selection feedback — shell.rs:1668 vs assignments at 464/507/3409/3436/3898/4318 — set `selected=(pi,MAX,MAX)` in ToggleProject, or derive psel from `active_tab`'s project index (like `w_active`) — **shell.rs**

11. `[L]` **Changes-row Copy Path copies relative path** — dispatches `CopyPathStr(relative)` while the Files-row menu copies the absolute path → inconsistent, pasted value unusable outside repo root vs. absolute path — shell.rs:967 — copy `self.active_cwd.join(path)` — **shell.rs**

12. `[L]` **Branch picker overlay: no scroll / no on-screen clamp** — `menu_popover` renders the branch list at `padding-top = click.y` with no height clamp or scroll, so many branches / a header near the window bottom draw off-screen and can't be reached vs. scrolls and stays on-screen — shell.rs:1065 (via 907 menu_popover) — add max-height + scroll and clamp the (x,y) origin — **shell.rs**

13. `[L]` **Welcome / Diff / Browser no-op when no active Tab** — `split_with_at` does `let tab = self.active_tab?;` and bails when nothing is open → Cmd+Shift+N / OpenDiff / OpenBrowser silently produce nothing on the empty state — shell.rs:3021 — seed a default tab/layout first (like startup 333-498) when active_tab is None, or gate the shortcuts/pills off — **shell.rs**

## Group B — editor_view.rs (PARALLEL)

14. `[M]` **Syntax highlighting smears after any edit / stale on theme change** — `self.colors` is computed once in `new()` and returned every frame from `override_color_map`; the ContentChanged subscription forwards only the edit delta and never recomputes colors → every color range is offset by the edit delta after the first keystroke; theme change keeps old colors until reopen — editor_view.rs:630 (one-shot) + 587-598 (never re-highlights) — in the ContentChanged arm recompute `self.colors = highlight(buffer_text, path)` (debounced), and on theme/font change; store behind interior mutability or cache keyed by buffer version — **editor_view.rs**

15. `[L]` **Page Up/Down scrolls viewport but leaves caret** — PageScroll only moves the render viewport one page; the caret stays put and can go off-screen, then the next arrow jumps the view back vs. caret moves a page and view follows — editor_view.rs:1212-1222 — have PageScroll also issue a page-sized Line CursorMove on the selection — **editor_view.rs**

## Group C — grid_element.rs (PARALLEL)

16. `[M]` **Terminal URL hover-underline not painted at idle** — MouseMoved sets `url_hover` + cursor but never calls `ctx.notify()`; without a repaint the accent underline (drawn in `paint` from `url_hover`) only appears when an unrelated repaint fires, and clearing on leave also doesn't repaint so a stale underline lingers vs. underline appears/erases immediately on hover — grid_element.rs:422-438 — call `ctx.notify()` in both MouseMoved arms whenever `url_hover`'s value actually changes (set and clear) — **grid_element.rs**

## Counts

**By severity (16 kept):** High 1 · Medium 8 · Low 7. (Dropped 1: trim-on-save, dormant.)
Note: input-labeled item #7 (grouped-child Remove) is demoted H→M here since its real effect is a no-op, not corruption.

**By target file:** shell.rs 12 (items 1-6, 8-13) · editor_view.rs 2 (14,15) · grid_element.rs 1 (16) · projects.rs 1 (item 7, shares a shell.rs half).

## Suggested batching

- **Serial batch (one worktree, shell.rs):** items 1-6, 8-13. Item 1 (RemoveProject teardown/remap) first — it is the only correctness/leak High and touches the same maps (active_tab/selected/layouts) that items 6, 10, 13 also read, so landing it first avoids re-merges. Items 5 & 6 (worktree/tab context menus) share the menu_popover/overlay-slot plumbing at 906-1016 / 3621-3631 — do them back-to-back.
- **Parallel batch (independent files, safe alongside the shell.rs work):**
  - editor_view.rs — items 14 + 15 together (one agent).
  - grid_element.rs — item 16 (one agent).
  - projects.rs — the expansion-suppression half of item 7 (one agent); coordinate the tiny shell.rs menu-gating half into the serial batch to avoid a shell.rs conflict.

Do NOT parallelize any two shell.rs items across worktrees — they collide in the dispatch match and the root render stack.
