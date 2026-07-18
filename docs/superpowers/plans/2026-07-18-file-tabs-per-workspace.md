# File Tabs Per Workspace — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Scope the Files Pane and its File Tabs to a Workspace, so opening a file in one project no longer shows tabs from every other project.

**Architecture:** `files_pane`, `file_pane_paths` and `file_pane_active` are flat global shell state, so all open files share one list regardless of Workspace. They become keyed by Workspace at runtime — the same `(project_idx, worktree_idx)` key `worktree_tabs` already uses — and keyed by **worktree checkout path** when persisted, mirroring the existing `worktree_tabs_by_path` stable-key pattern. `editor_views` stays global: reusing an editor handle across Workspaces is correct and desirable; only *tab visibility* is scoped.

**Tech Stack:** Rust edition 2024, warpui (vendored), serde_json state at `~/.crane/warpui-state.json`, `cargo test --bin crane`.

## Verified facts (checked against the code — do not re-derive)

| Fact | Location |
|---|---|
| `selected: (project_idx, worktree_idx, tab_idx)` — the current-workspace key is `(selected.0, selected.1)` | `shell.rs:454` |
| `worktree_tabs: HashMap<(usize, usize), Vec<TabMeta>>` — the runtime keying precedent | `shell.rs:471` |
| `files_pane: Option<PaneId>` — **global** | `shell.rs:282` |
| `file_pane_paths: Vec<PathBuf>` — **global, flat** | `shell.rs:285` |
| `file_pane_active: usize` — **global** | `shell.rs:287` |
| `editor_views: HashMap<PathBuf, ViewHandle<WarpEditorView>>` — global, and should STAY global | `shell.rs:291` |
| ~78 usage sites of the three globals in `shell.rs` | grep |
| Persisted `worktree_tabs` (index-keyed) is marked **LEGACY**; restore prefers `worktree_tabs_by_path` | `persist.rs:139-147` |
| Reason, verbatim: *"Paths are stable across reloads (indices shift), so tab lists … land back in the RIGHT worktree even after projects are added/removed/reordered."* | `persist.rs:143-145` |
| `path_keyed_fields_round_trip` shows the house test style for this | `persist.rs:363` |

## Global Constraints

- **Persist by PATH, not index.** Index-keyed persistence is documented legacy in this file. A new index-keyed field would reintroduce the exact bug `worktree_tabs_by_path` exists to fix.
- `~/.crane/warpui-state.json` is the user's **real, live session**. Every new field carries `#[serde(default)]`. Deserializing an existing file must keep working, and **must not lose their currently open files** — the migration path is mandatory, not optional.
- Do NOT modify anything under `vendor/warp/` (upstream submodule).
- **Do NOT run `cargo fmt` in any form** — it reformats the whole workspace including vendored code regardless of the path argument. Hand-format.
- Never use Unicode glyph icons — bundled fonts render them as tofu. Only constants that exist in `icons.rs`.
- Commit messages: conventional prefix, ZERO AI/assistant/Claude references, no Co-Authored-By lines.
- **Never launch, quit, kill or `osascript`-address any application.** Never signal a process you did not start. Verify headlessly.
- Tests: `make test` (= `cargo test --bin crane`).

---

### Task 1: Scope the runtime state per Workspace

**Files:**
- Modify: `src/warpui/shell.rs` (the three fields + their ~78 usage sites)
- Test: `src/warpui/shell.rs` test module (extract pure helpers if needed)

**Interfaces:**
- Produces: `fn ws_key(&self) -> (usize, usize)` returning `(self.selected.0, self.selected.1)`; the three fields rekeyed as maps. Task 2 persists them.

- [ ] **Step 1: Write the failing test**

The visible bug is that tabs from Workspace A appear in Workspace B. Make that testable by extracting the lookup rather than testing the whole shell:

```rust
#[test]
fn file_tabs_are_scoped_to_their_workspace() {
    let mut tabs: std::collections::HashMap<(usize, usize), Vec<std::path::PathBuf>> =
        std::collections::HashMap::new();
    tabs.entry((0, 0)).or_default().push("/a/one.md".into());
    tabs.entry((1, 0)).or_default().push("/b/two.md".into());

    assert_eq!(
        tabs.get(&(0, 0)).map(Vec::len),
        Some(1),
        "workspace (0,0) sees only its own tab"
    );
    assert_eq!(
        tabs.get(&(1, 0)).map(|v| v[0].clone()),
        Some(std::path::PathBuf::from("/b/two.md")),
        "workspace (1,0) must not see workspace (0,0)'s tabs"
    );
}
```

Replace this with a test against the real accessor once Step 2 defines it — the point is to pin *scoping*, not to test `HashMap`. If you can reach the shell's real lookup from a test, do that instead and delete this placeholder.

- [ ] **Step 2: Rekey the three fields**

```rust
/// The Files Pane leaf, per Workspace. Global before — one project's pane
/// was reused for every other project's files.
files_pane: HashMap<(usize, usize), PaneId>,
/// Open File Tabs, per Workspace.
file_pane_paths: HashMap<(usize, usize), Vec<PathBuf>>,
/// Active File Tab index within this Workspace's `file_pane_paths`.
file_pane_active: HashMap<(usize, usize), usize>,
```

Add the accessor used everywhere:

```rust
/// Current Workspace key — `(project_idx, worktree_idx)`, matching
/// `worktree_tabs`. `selected.2` (the Tab index) is deliberately not part
/// of the key: File Tabs belong to the Workspace, not to one Tab.
fn ws_key(&self) -> (usize, usize) {
    (self.selected.0, self.selected.1)
}
```

Leave `editor_views` global.

- [ ] **Step 3: Migrate every usage site**

Work through all ~78 sites (`grep -n 'file_pane_paths\|file_pane_active\|files_pane' src/warpui/shell.rs`). Most become `self.file_pane_paths.entry(self.ws_key()).or_default()` or `.get(&self.ws_key())`. Let the compiler drive it — the type change makes every site an error until handled.

**Take special care at these**, which do cross-workspace or lifecycle work:
- `:2689`, `:2774` — path rewrites across tabs (a rename/move); must update the right Workspace's list, and possibly several
- `:2994-2998` — pane-close teardown clearing all three; must clear only that Workspace's entry
- `:4200`, `:10452-10453`, `:10590-10591` — tab-strip rendering; must read the current Workspace
- `:10378`, `:10661` — `files_pane == Some(id)` identity checks; become a lookup for the pane's own Workspace, **not** necessarily the selected one (a pane can render while another Workspace is selected)

That last point is the main hazard: several sites ask "is pane `id` the files pane?" — which must resolve against **the Workspace that pane belongs to**, not `ws_key()`. If a reverse lookup is needed (pane id → workspace), add a small helper rather than assuming the selected Workspace.

- [ ] **Step 4: Run tests**

Run: `make test`
Expected: PASS. Fix any breakage from the rekeying before continuing.

- [ ] **Step 5: Commit**

```bash
git add src/warpui/shell.rs
git commit -m "refactor(warpui): scope Files Pane state to its Workspace

files_pane, file_pane_paths and file_pane_active were flat globals, so
every Workspace shared one File Tab list and opening a file in a new
project showed tabs from every other project. All three are now keyed by
(project_idx, worktree_idx), matching worktree_tabs. editor_views stays
global so an editor handle is still reused across Workspaces."
```

---

### Task 2: Persist per Workspace, keyed by path

**Files:**
- Modify: `src/warpui/persist.rs` (new path-keyed field + keep legacy for migration)
- Modify: `src/warpui/shell.rs` (save + restore)
- Test: `src/warpui/persist.rs` test module

**Interfaces:**
- Consumes: Task 1's keyed fields and `ws_key`.
- Produces: `file_tabs_by_path: Vec<(String, SFileTabs)>` where `SFileTabs { paths: Vec<PathBuf>, active: usize }`.

- [ ] **Step 1: Write the failing tests**

Mirror `path_keyed_fields_round_trip` (`persist.rs:363`) for style:

```rust
#[test]
fn file_tabs_round_trip_keyed_by_path() {
    let mut st = WarpuiState::default();
    st.file_tabs_by_path = vec![(
        "/Users/me/proj".to_string(),
        SFileTabs { paths: vec!["/Users/me/proj/a.md".into()], active: 0 },
    )];
    let back: WarpuiState = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
    assert_eq!(back.file_tabs_by_path.len(), 1);
    assert_eq!(back.file_tabs_by_path[0].1.paths.len(), 1);
}

#[test]
fn legacy_flat_file_tabs_migrate_and_are_not_lost() {
    // A real pre-upgrade state file has a FLAT file_pane_paths and no
    // file_tabs_by_path. Those open files must survive the upgrade.
    let legacy = r#"{"file_pane_paths":["/Users/me/proj/a.md"],"file_pane_active":0,"files_pane":141}"#;
    let st: WarpuiState = serde_json::from_str(legacy).expect("legacy state must load");
    assert_eq!(st.file_pane_paths.len(), 1, "legacy flat list must still parse");
    assert!(st.file_tabs_by_path.is_empty(), "new field absent in legacy files");
}
```

- [ ] **Step 2: Run them, confirm they fail**

Run: `cargo test --bin crane file_tabs -- --nocapture`
Expected: FAIL — no `SFileTabs`, no `file_tabs_by_path`.

- [ ] **Step 3: Add the persisted type**

In `persist.rs`, beside the other path-keyed fields:

```rust
/// One Workspace's File Tabs.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SFileTabs {
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub active: usize,
}
```

```rust
/// Per worktree checkout PATH: that Workspace's File Tabs. Path-keyed for
/// the same reason as `worktree_tabs_by_path` — indices shift when projects
/// are added, removed or reordered; paths do not.
#[serde(default)]
pub file_tabs_by_path: Vec<(String, SFileTabs)>,
```

**Keep the legacy `file_pane_paths` / `file_pane_active` / `files_pane` fields.** They are the migration source and must not be deleted.

- [ ] **Step 4: Save**

In `build_state` (`shell.rs:~2019`), write `file_tabs_by_path` by mapping each Workspace key to its checkout path. Find how `worktree_tabs_by_path` is built on the save side and mirror it exactly — do not invent a second path-resolution scheme.

Keep writing the legacy flat fields too, populated from the **currently selected** Workspace, so downgrading to an older binary still finds something sane.

- [ ] **Step 5: Restore, with migration**

In the restore path, prefer `file_tabs_by_path` when non-empty (mirroring how `effective_tabs` at `shell.rs:1300-1309` prefers `worktree_tabs_by_path`).

**Migration — the safety-critical part:** when `file_tabs_by_path` is empty but the legacy flat `file_pane_paths` is not, assign those paths to the **restored default/first Workspace** so the user's open files are not lost on upgrade. Add a comment saying so.

- [ ] **Step 6: Verify no data loss against a realistic legacy blob**

Write a test using a legacy state shape containing `file_pane_paths`, `files_pane`, `terminals` and `browsers` together, asserting every one survives restore alongside an empty `file_tabs_by_path`. A migration that drops panes is worse than the bug being fixed.

- [ ] **Step 7: Run the suite and commit**

Run: `make test`

```bash
git add src/warpui/persist.rs src/warpui/shell.rs
git commit -m "feat(warpui): persist File Tabs per Workspace, keyed by checkout path

File Tabs were persisted as one flat list, so they reappeared in whichever
Workspace happened to restore first. They are now stored per worktree
checkout path, matching worktree_tabs_by_path — indices shift when projects
are reordered, paths do not. A state file written before this change
migrates its flat list into the default Workspace rather than losing it."
```

## Self-review notes

- **Riskiest step:** Task 1 Step 3. ~78 sites, and the `files_pane == Some(id)` identity checks must resolve against the pane's OWN Workspace, not the selected one. Getting that wrong makes panes appear to belong to whichever Workspace is on screen.
- **Data-loss risk:** Task 2 Step 5's migration. If it is skipped or wrong, upgrading silently empties the user's open files. Step 6 exists specifically to prove it doesn't.
- **Deliberately unscoped:** `editor_views` stays global (a handle cache, not visibility state).
- **Interaction:** the markdown restore arm added earlier sets `restored_files_pane` / `restored_file_paths`; those become per-Workspace here. Task 1 must update that arm too, not just the editor arm.
