# Left-panel follow-ups (user feedback, screenshots)

All three are in `src/warpui/shell.rs` (`left_sidebar` + `project_context_menu`). Do in the
post-parity shell pass (fold #2 into Wave D folder-groups). Old ref: `src/ui/projects.rs`.

## 1. BUG — context menu spans full screen width
The project context menu (Reveal / Copy Path / Initialize Git / color swatches / Default color /
Remove) renders at FULL panel/screen width instead of a compact popover.
**Fix:** wrap `project_context_menu`'s content in a fixed-width `ConstrainedBox` (~220px) so the
menu is a small popover anchored at the click point, not full-bleed. Check the menu-row builders
aren't using `Expanded`/full-width fills.

## 2. FEATURE — folder groups (REDONE — first attempt was WRONG)

**WRONG (what a71e94e shipped, must be reverted):** grouped projects by their shared *filesystem
parent directory* (`assign_groups` by `Path::parent()`). This invents fake groups like `ideaProjects`
/ `OneVibe` from projects the user opened *separately*, and inspects a parent folder the user never
opened. User feedback: "wrong approach … why checking a folder that is not opened."

**CORRECT model (matches how the user opens projects):** grouping is INTRINSIC to a single opened
folder, not inferred across projects.
- A project = a folder the user explicitly opened (an entry in the projects list). Never group two
  separately-opened projects together just because they share a parent dir.
- For each opened project folder:
  - If the folder is itself a git repo → git project (cube + branches). (as today)
  - If the folder is NOT a git repo but its IMMEDIATE children contain git repos → render the opened
    folder as a collapsible FOLDER header, and nest each child git repo under it as its own project
    (cube + branches + tabs). Discover children via scanning immediate subdirs for `.git`
    (old egui used `git::discover_repos(path)`). E.g. opening `qck-platform` → folder header with
    `qck-cloud`, `qck-py-sdk`, `qck-js-sdk` nested.
  - If the folder is not git and has no git children → loose folder (tabs directly, per Fix #3).
- REMOVE `assign_groups`/`group_path` parent-directory inference entirely. Remove the
  `ideaProjects`/`OneVibe` synthetic groups. The `ToggleGroup`/`collapsed_groups` collapse mechanism
  can be reused, but keyed by the opened folder's own path, not an inferred parent.

Ref: old `ui/projects.rs` group rendering + `git::discover_repos`.

## 3. BUG — loose (non-git) project shows a "(no git)" branch row
New renders: `OhSugrrr` (folder) → `(no git)` worktree row → `Terminal 4` → `+ New tab`.
Old renders: `OhSugrrr` (folder) → `Terminal` directly under it (NO worktree/branch row).
**Fix:** for `is_loose` projects, do NOT render the worktree/`(no git)` row at all — render the
tabs directly under the project folder at one indent (flatten). The Wave-3a loose handling set the
FOLDER icon but did not suppress the worktree row. Ref old `ui/projects.rs:642-648`.
