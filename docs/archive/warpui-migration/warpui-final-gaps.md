All findings verified against the actual code. Every claim checks out: the checkout handlers call only `refresh_panel()` (no `reload_projects()`), `display_name` prefers `terminal_tab_title` with no user-renamed flag, the modal gate hits blanket `StopPropagation`, and settings_card has no zoom buttons. Consolidated below.

---

# Consolidated warpui fix list (7 genuine breaks, deduped)

Dropped nothing as false-positive — all 7 are real breaks in shipped features. Two (#2 and #4) share one root line (`display_name` at shell.rs:4113) and should be fixed together.

## HIGH

**[high] Switch Branch checkout — left-panel row shows stale branch**
Now: `git checkout` succeeds and the status-bar breadcrumb updates, but the left-panel worktree row keeps the OLD branch name indefinitely (until restart). Expected: left row reflects the newly checked-out branch, consistent with the status bar.
Root cause: `CheckoutBranch` (shell.rs:7183) and `CreateBranchCheckout` (shell.rs:7905) call only `refresh_panel()` + `invalidate_editor_diffs()`; `refresh_panel` rebuilds Changes/Files/`self.branch` but never `self.projects`, and `poll_worktrees` skips existing paths without re-reading their branch name.
Fix: in both handlers, before `refresh_panel()`, set `self.projects[pi].worktrees[wi].name = current_branch(root)` (or call `reload_projects()`); or teach `poll_worktrees` step 2a to reconcile the branch name of an existing path.
Target: **shell.rs** (serial)

## MEDIUM

**[medium] Terminal OSC-0/2 title never updates the tab label at idle**
Now: a program emits an OSC-2 title; crane_term stores it and wakes the TerminalView, but only the TerminalView is invalidated (view.rs:200) — the CraneShell view that renders the tab label is a separate entity and is never invalidated, so the tab stays "Terminal N" until the user's next shell action. Expected: tab label reflects the current OSC title promptly.
Root cause: terminal wake (shell.rs:937) pings only the per-terminal repaint rx; no shell observe/subscribe of terminal handles.
Fix: make a title change invalidate CraneShell too — `ctx.observe(&handle, |this,_,vctx| vctx.notify())` in `spawn_terminal`, or route the terminal Wake through a shell-notifying channel like `git_wake`.
Target: **shell.rs:937** (serial)

**[medium] User tab-rename clobbered by live OSC-2 title (git-worktree tabs)**
Now: `commit_rename` stores the new name into `TabMeta.name` (persists), but the git-worktree row prefers the live OSC title — `display_name = if rbuf.is_some() { t.name } else { terminal_tab_title(...).unwrap_or(t.name) }` (shell.rs:4113) — so the rename is overwritten the instant the field closes. Loose-project rows (4007) use `&t.name` directly and are fine. Expected: a renamed tab keeps the chosen name.
Root cause: no user-renamed flag on `TabMeta` (shell.rs:391), so the OSC title always wins.
Fix: add `renamed: bool` to `TabMeta`, set it in `commit_rename`'s Tab arm, and prefer `t.name` over `terminal_tab_title` when renamed. (Fix alongside #1-medium — same line.)
Target: **shell.rs** (serial)

**[medium] Settings zoom shortcuts (Cmd+= / - / 0) swallowed while Settings open**
Now: settings_card advertises "Zoom … (Cmd+= / Cmd+- / Cmd+0)" (shell.rs:2143) but offers no buttons, and the modal key gate falls to blanket `StopPropagation` (shell.rs:6459) for every non-Escape/non-switcher key, so the chords never reach the Cmd-shortcut match. Displayed % stays frozen. Expected: the advertised keystrokes change zoom and update the % live.
Root cause: modal key-swallow gate has no carve-out for the global zoom chords.
Fix: in the gate (shell.rs:6426-6459), before the final `StopPropagation`, if `ks.cmd` and key ∈ {=,+,-,0} dispatch the matching `FontZoom*` (as already done for the switcher's Cmd+`); or add clickable +/- buttons to settings_card.
Target: **shell.rs** (serial)

## LOW

**[low] Gutter deletion wedge painted one row too high (interior deletions)**
Now: `compute_diff` keys all kinds with `idx = line.saturating_sub(1)` (editor_view.rs:446); for a pure deletion git emits `@@ -6 +5,0 @@` where new_start=5 is the line above the gap, so the wedge lands at line 5's top edge instead of between 5 and 6. Added/Modified are correct (new_start is the first changed line). Expected: wedge sits at the surviving line below the gap.
Fix: special-case Deleted in `compute_diff` — key at `line` directly, keep `line-1` for Added/Modified; or push `new_start+1` for the 'D' case in `file_line_diff`.
Target: **editor_view.rs** (parallel)

**[low] Settings "Update available" doesn't surface until an incidental repaint**
Now: `update::spawn_check` (update.rs:37, called at shell.rs:513) publishes the newer version to a Mutex cell but issues no repaint waker; `settings_card` reads `latest_available()` only at render (shell.rs:2178), so if Settings is open (or opened during a quiet frame) when the check lands it shows "Up to date" until an unrelated event repaints. Expected: About refreshes to "Update available: {v}" on its own.
Root cause: the async check has no notify/repaint back-channel to the shell.
Fix: thread a repaint waker into `spawn_check` (same pattern as the PTY wake in `spawn_terminal`, shell.rs:936) and fire it after publishing, or drive via `ctx.spawn`.
Target: **update.rs** (parallel — different file from shell.rs)

**[low] Ln/Col + selection status row doesn't update on mouse-drag selection**
Now: mouse-down fires `FocusPane` (single clicks work), but drag `SelectionExtend` / release `EndSelect` call `ctx.notify()` on the EDITOR vctx only (editor_view.rs:2779-2796) — the shell isn't repainted, so Ln/Col and "(N chars)" don't update until the next shell-level action. Keyboard selection works (routes through SendKeys → shell notify at shell.rs:7981). Expected: mouse drag-select refreshes the status row like Shift+Arrow does.
Root cause: mouse selection actions notify only the editor; warpui doesn't repaint the parent shell on a child notify.
Fix: after routing a mouse-driven caret/selection EditAction, also nudge the shell (dispatch a lightweight shell action / FocusPane, or have the shell observe editor selection changes).
Target: **shell.rs** (serial — pane mouse handler)

---

## Counts

By severity: **HIGH 1 · MEDIUM 3 · LOW 3** (total 7)

By file:
- **shell.rs — 5** (checkout-left-panel HIGH, OSC-2 tab title MED, tab-rename clobber MED, settings-zoom-swallow MED, mouse-drag status-row LOW)
- **editor_view.rs — 1** (gutter deletion wedge LOW)
- **update.rs — 1** (update-available surfacing LOW)

## Parallel vs serial

- **Must be serial (all in shell.rs, one worker):** the 5 shell.rs fixes. Sequence them HIGH→LOW: checkout-left-panel → OSC-2 tab title → tab-rename (do these two together, both edit `display_name`@4113 + `TabMeta`) → settings-zoom → mouse-drag status-row.
- **Safe to fix in parallel (separate files, no overlap with shell.rs or each other):** editor_view.rs gutter wedge; update.rs update-available waker.
- Note: OSC-2 tab title (MED) and tab-rename clobber (MED) both edit the git-worktree `display_name` block and `TabMeta` — batch them in one pass to avoid a merge conflict within shell.rs.
