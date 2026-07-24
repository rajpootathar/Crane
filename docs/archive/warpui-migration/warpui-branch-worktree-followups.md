# Branch / worktree / context-menu follow-ups (user feedback)

All shell.rs / projects.rs. Do in the next shell.rs wave AFTER the LSP wave finishes.
Old egui reference: src/ui/projects.rs, src/modals/new_workspace.rs, src/main.rs
(poll_dead_worktrees / poll_loose_git_init), src/git.rs (worktree add/list/remove).

## 1. Branch-switch model — CHOSEN: "Both (checkout + optional worktree)"
A "Switch Branch" modal (use the Modal framework): searchable list of local + remote
branches. Picking a branch = `git checkout <branch>` in the CURRENT workspace folder
(DWIM remote → tracking, dedup local/remote — the Wave-B checkout already does this).
PLUS a per-row "+ worktree" affordance that opens the branch's existing workspace or
runs `git worktree add` to create one. Plus "Create new branch…" (git checkout -b) and
"Create & checkout new branch". Trigger: click the branch in the breadcrumb/status bar
(old Crane's status-bar branch click) AND/OR a shortcut. The current right-panel Changes-
header picker can stay or be replaced by this.

## 2. Create-worktree flow (MISSING) — "no way to create a worktree like old crane"
Old Crane: a project's "+ New workspace" affordance → new_workspace modal → input a
branch name + target path → `git worktree add <path> <branch>` (default under
~/.crane-worktrees/<project>/<branch>) → the new workspace appears in the tree.
Implement: a "New Workspace" modal (branch name field + optional new-branch checkbox),
`git::add_worktree(project, branch, path)` in warpui/git.rs, then reload/insert the new
worktree into the project's worktrees. Add a "+" affordance on the project row (git
projects) and/or in the branch-switch modal (#1). Ref old src/modals/new_workspace.rs +
src/ui/projects.rs new_workspace_for_project.

## 3. Live worktree auto-detection (MISSING) — "no whole-worktree auto detection live mechanism"
Old Crane polled the filesystem/git so the tree stays in sync when worktrees are
created/removed OUTSIDE the app (poll_dead_worktrees + reindex, poll_loose_git_init).
Implement a lightweight periodic refresh (a std::thread or a repaint-timer tick, debounced
~1-2s) that re-runs `git worktree list` per project + rechecks `.git` presence, and
updates the tree (add new worktrees, mark/remove dead ones, flip loose→git on init)
WITHOUT the user having to reload. Must be cheap — cache and only refresh when mtime or
`git worktree list` output changes. Ref old src/main.rs poll_dead_worktrees /
poll_loose_git_init.

## 4. Context-menu items have NO hover animation
The context-menu rows (menu_item / menu_popover / the tint swatch row) don't highlight on
hover. Add a hover state: wrap each menu row in a Hoverable (warpui MouseStateHandle, as
the Welcome buttons use) so it paints a `theme::row_hover()` background + pointer cursor on
hover, matching old egui's menu hover. Applies to project/worktree/tab/changes/file menus.

## 5. PERF BUG — project context menu "color change" is SLOW
Root cause: `SetProjectTint` → `reload_projects()` → `load_projects_extended` →
`expand_folder`, which re-shells `git` (`current_branch`, `diff_numstat`, `is_dirty`) for
EVERY project + worktree on the machine on every single tint click. That's dozens of git
subprocess spawns per color change.
Fix: a tint change must NOT trigger a full git re-scan. Update the tint IN PLACE —
`self.projects[idx].tint = Some(rgb)` (and `project_tints` map + persist) + `ctx.notify()`
— with NO `reload_projects()`. (Same for worktree/tab tint.) More broadly: cache the git
per-project data (branch/diff/dirty) so `reload_projects` after add/remove is cheap, and
only recompute git for a project when its path/HEAD/mtime changed (or on the #3 poll tick),
not on every reload. Priority: HIGH (user-visible lag).
