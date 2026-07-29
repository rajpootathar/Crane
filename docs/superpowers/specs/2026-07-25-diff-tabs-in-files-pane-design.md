# Diff Tabs in the Files Pane — Design

Date: 2026-07-25
Status: approved

## Goal

Clicking a changed file in the Right Panel's **Changes** tree currently spawns a
brand-new **Pane** on every click, so reviewing five files leaves five Panes
fighting for space. Diffs should instead open as **File Tabs inside the Files
Pane**, sharing one tab strip with ordinary file tabs.

A file tab and a diff tab of the *same* file coexist, and several diffs of one
file across different commit ranges coexist:

```
┌─ Files Pane ──────────────────────────────────────────────┐
│ shell.rs │ view.rs │ ⧉ view.rs HEAD–WT │ ⧉ view.rs c3–c7  │
└──────────┴─────────┴───────────────────┴──────────────────┘
   file       file      diff                diff
```

(`⧉` above stands in for `icons::GIT_DIFF` — see "Tab strip label" below. It is
a diagram placeholder only; no Unicode glyph is ever emitted by the UI.)

## Root cause

`open_diff` (`shell.rs:13604`) ends in `self.split_with(PaneContent::Diff(h))` —
an unconditional split, with none of the tab bookkeeping `open_file` performs.
`open_file` by contrast records the tab, sets the active index, then **swaps the
Files Pane's content** to the active document (`self.panes.insert(fp, …)`),
splitting only when no reusable pane exists. `PaneContent::Diff` already exists,
so the pane plumbing is already there — only the tab identity is missing.

## Phasing

| Phase | Scope | Result |
|---|---|---|
| **1 (this spec)** | Diffs become tabs; working-tree diffs only | Fixes the pane-spam complaint; labels read `view.rs ⟷ HEAD–WT` |
| **2 (later)** | `WarpDiffView` learns arbitrary `from`/`to` | `c#x–c#y` tabs; multiple ranges of one file open at once |

`WarpDiffView::new(ctx, repo_root, path)` (`diff_view.rs:555`) takes **no commit
range** today — arbitrary-range diffing is a genuinely new capability, not a
labelling change. Phase 1 builds the `(path, spec)` key up front so Phase 2 is
purely additive.

## Data model

```rust
enum TabKey {
    File(PathBuf),
    Diff { path: PathBuf, spec: DiffSpec },
}

enum DiffSpec {
    WorkingTree,                           // Phase 1: HEAD ↔ working tree
    Commits { from: String, to: String },  // Phase 2
}

impl TabKey {
    /// The on-disk path both variants refer to. Every filesystem-facing
    /// sweep keys off THIS, never off the TabKey itself.
    fn path(&self) -> &Path;
}
```

`file_pane_paths: HashMap<ws, Vec<PathBuf>>` becomes `HashMap<ws, Vec<TabKey>>`.
`file_pane_active` is unchanged (still an index into that list).

**Dedup rule:** `position(|t| t == &key)` — derived `PartialEq` gives exactly the
agreed `(path, from, to)` identity. Re-clicking a changed file focuses its
existing tab; a different range opens a second tab; duplicates never accumulate.

Phase 1 constructs only `DiffSpec::WorkingTree`.

## Components

**`open_diff`** — mirrors `open_file`: build `TabKey::Diff`, do the
entry/dedup/active-index bookkeeping, resolve or create the view handle, then
`reusable_files_pane()` → swap content, else split once at `0.35`. Same shape as
the existing Markdown branch.

**`diff_views: HashMap<TabKey, ViewHandle<WarpDiffView>>`** — new handle cache,
peer of `editor_views` / `markdown_views`. Keyed by the full `TabKey` (not the
path) because two ranges of one file are two distinct live views.

**Tab strip label** — `File` renders `file_name()` as today. `Diff` renders
`icons::GIT_DIFF` + `file_name()` + the spec, where `WorkingTree` → `HEAD–WT`
and `Commits` → short SHAs. The leading icon is what distinguishes a diff tab
from an editable file tab.

`icons::GIT_DIFF` (`src/app/icons.rs:7`) is the required marker — **not** a
Unicode glyph such as `⟷` or `↔`, which the bundled JetBrains Mono and the
default proportional face do not cover and which therefore render as tofu
boxes (project rule: icons always come from the bundled icon font). The Changes
tree already labels these same rows with `icons::GIT_DIFF` at `shell.rs:4596`,
immediately beside the `OpenDiff` dispatch at `:4598`, so the tab inherits an
existing visual convention rather than inventing one.

## The sweeps — the part with teeth

Two existing behaviors key off `PathBuf` and **must** move to `TabKey::path()`,
or deleting a file leaves a live diff tab over a file that no longer exists —
the resurrection bug this codebase already has scar tissue for:

- **`purge_path_everywhere`** (`shell.rs:12217`) matches `starts_with(path)`
  across every Workspace. It must match against `TabKey::path()` so that
  deleting a file (or a directory above it) closes its **diff** tabs too, and
  drops their `diff_views` entries.
- **`FileTabCloseConfirmed`'s `still_open` refcount** (`shell.rs:15508`) decides
  when a cached buffer may be freed. A file tab *and* a diff tab of one path
  both pin that path — the scan must compare `TabKey::path()`, not whole keys.

## Persistence

`SFileTabs { pane, paths: Vec<PathBuf>, active }` gains `tabs: Vec<STabKey>`.
`paths` is retained as a **migration source only**: a state file with `paths` and
no `tabs` restores every entry as `TabKey::File`. Diff tabs restore by
re-deriving the diff from `(path, spec)`; a diff whose commits no longer resolve
is dropped on restore rather than restored broken.

This is a third layer on an established pattern — `file_tabs_by_path` already
supersedes legacy `file_pane_paths` with migration tests (`persist.rs:230`), so
the mechanism and its test shape already exist.

## Testing

Mirrors the delete-path coverage hardened on 2026-07-25:

1. Clicking the same changed file twice **focuses** one tab, never appends.
2. `TabKey` equality discriminates on the spec: `Diff { path, WorkingTree }` and
   `Diff { path, Commits { .. } }` for one path are two distinct keys. Asserted
   as a pure `TabKey` unit test in Phase 1 (no `Commits` tab is constructible
   until Phase 2), which is what lets Phase 2 add ranges without re-deriving
   the dedup rule.
3. A file tab and a diff tab of the same path coexist and are independent.
4. Deleting a file closes its diff tabs **and** its file tab, in every Workspace,
   and drops the `diff_views` entry.
5. Deleting a directory closes diff tabs for files beneath it.
6. A failed delete closes nothing.
7. Legacy state (`paths`, no `tabs`) migrates with no tab lost.

Tests inject the `trash_delete` seam so none of them touch the real system Trash.

## Out of scope

- Arbitrary commit-range diffs (Phase 2).
- Dragging a tab out to split the Layout (tracked separately under Unified
  Document Pane).
- Editing inside a diff tab — diffs stay read-only.
