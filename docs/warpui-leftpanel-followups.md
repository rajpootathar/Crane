# Left-panel follow-ups (user feedback, screenshots)

All three are in `src/warpui/shell.rs` (`left_sidebar` + `project_context_menu`). Do in the
post-parity shell pass (fold #2 into Wave D folder-groups). Old ref: `src/ui/projects.rs`.

## 1. BUG — context menu spans full screen width
The project context menu (Reveal / Copy Path / Initialize Git / color swatches / Default color /
Remove) renders at FULL panel/screen width instead of a compact popover.
**Fix:** wrap `project_context_menu`'s content in a fixed-width `ConstrainedBox` (~220px) so the
menu is a small popover anchored at the click point, not full-bleed. Check the menu-row builders
aren't using `Expanded`/full-width fills.

## 2. FEATURE — folder groups (multi-git parent folder)
Old: a parent folder that contains multiple git repos shows as a FOLDER header, with each child
git repo nested under it as its own project (cube + branches). E.g. `qck-cloud` (folder) →
`qck-cloud` (git), `qck-py-sdk` (git), `qck-js-sdk` (git).
**Fix:** group projects by shared parent directory (old `group_path` in `ui/projects.rs:259-406`).
Render a collapsible FOLDER header per group; nest member projects one indent in. Members keep
their own cube icon + branches + tabs. (This is punchlist P5 "folder groups".)

## 3. BUG — loose (non-git) project shows a "(no git)" branch row
New renders: `OhSugrrr` (folder) → `(no git)` worktree row → `Terminal 4` → `+ New tab`.
Old renders: `OhSugrrr` (folder) → `Terminal` directly under it (NO worktree/branch row).
**Fix:** for `is_loose` projects, do NOT render the worktree/`(no git)` row at all — render the
tabs directly under the project folder at one indent (flatten). The Wave-3a loose handling set the
FOLDER icon but did not suppress the worktree row. Ref old `ui/projects.rs:642-648`.
