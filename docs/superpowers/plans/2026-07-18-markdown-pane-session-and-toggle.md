# Markdown Pane Session + Edit/Preview Toggle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Persist markdown panes across restarts (today they silently become terminals), and add an edit/preview toggle so `.md` files can be edited at all.

**Architecture:** `shell.rs` routes `.md` to a read-only `PaneContent::Markdown` and returns before the editor path, so markdown is uneditable. Its restore chain has no `Markdown` arm, so restored markdown panes fall through to a fresh terminal. Both are fixed in the same pane-routing code: the view gains its path, persistence gains a markdown entry carrying path + mode, and a header button swaps `PaneContent::Markdown` ↔ `PaneContent::Editor` for the same path.

**Tech Stack:** Rust edition 2024, warpui (vendored, `vendor/warp`), serde_json state at `~/.crane/warpui-state.json`, `cargo test --bin crane`.

## Verified facts (checked against the code — do not re-derive)

| Fact | Location |
|---|---|
| Restore chain: Editor → Browser → Terminal → **fresh Terminal fallback**; no Markdown arm | `shell.rs:1351-1385` |
| Save collection pattern to mirror (browsers) | `shell.rs:1897-1907` |
| `markdown_views: HashMap<PathBuf, ViewHandle<WarpMarkdownView>>` | `shell.rs:295` |
| `PaneContent::Markdown(ViewHandle<_>)` variant | `shell.rs:909` |
| Pane render `ChildView::new(h).finish()` | `shell.rs:10236` |
| Pane header: `(icons::FILE_TEXT, h.as_ref(app).title())` | `shell.rs:10642` |
| `.md` routing (`is_md`), returns before editor | `shell.rs:10794` |
| `WarpMarkdownView::new(ctx, path)` / `from_source(ctx, title, text)` / `title()` | `markdown_view.rs:613 / 639 / 650` |
| **`WarpMarkdownView` does NOT store its path** — only `title` | `markdown_view.rs` struct |
| persist fields use `#[serde(default)]` throughout — adding a field is backward-compatible | `persist.rs:77+` |
| Available icons: `PENCIL_SIMPLE`, `FILE_TEXT`, `CODE`. **There is no `EYE`.** | `icons.rs:41 / 14 / 34` |

## Global Constraints

- **Never use Unicode glyph icons** (`▲ ▼ ✕ • ▎ 👁`). Bundled fonts don't cover them; they render as tofu. Use `crate::warpui::icons::*` only, and only constants that exist in `icons.rs`.
- Do NOT modify anything under `vendor/warp/` (upstream submodule).
- **Do NOT run `cargo fmt` in any form** — it reformats the entire workspace including vendored code regardless of path argument. Hand-format.
- Commit messages: conventional prefix, ZERO AI/assistant/Claude references, no Co-Authored-By lines. Strict project policy.
- `~/.crane/warpui-state.json` is the user's real session. Any new persisted field MUST carry `#[serde(default)]` so existing state files still load. Never write a migration that can drop panes.
- Icon buttons need `min_size` ≥ 22×22 — a single-glyph button can collapse to an invisible hitbox.
- Tests: `make test` (= `cargo test --bin crane`). Currently 117.

---

### Task 1: Persist markdown panes across restart

Today a markdown pane is not saved, and on restore falls through to `PaneContent::Terminal` — the user loses the document and gets a shell.

**Files:**
- Modify: `src/warpui/markdown_view.rs` (struct + `new` + new accessor)
- Modify: `src/warpui/persist.rs` (new `SMarkdown` + state field)
- Modify: `src/warpui/shell.rs` (save collection ~`:1897`, restore arm ~`:1351-1385`)
- Test: `src/warpui/markdown_view.rs` and/or `persist.rs` test modules

**Interfaces:**
- Produces: `WarpMarkdownView::path() -> Option<&Path>`; `persist::SMarkdown { path: PathBuf, editing: bool }`; `WarpuiState.markdowns: Vec<(PaneId, SMarkdown)>`. Task 2 consumes `SMarkdown.editing`.

- [ ] **Step 1: Give the view its path**

In `markdown_view.rs`, add to the struct and set it in both constructors:

```rust
pub struct WarpMarkdownView {
    // ... existing fields ...
    /// Source file, when this view was opened from one. `None` for
    /// `from_source` (in-memory) documents, which cannot be persisted.
    path: Option<PathBuf>,
}
```

`new(ctx, path)` sets `path: Some(path)`; `from_source(...)` sets `path: None`. Add:

```rust
/// Source file this view renders, if any. `None` for in-memory documents.
pub fn path(&self) -> Option<&std::path::Path> {
    self.path.as_deref()
}
```

- [ ] **Step 2: Write the failing persistence round-trip test**

Add to `persist.rs`'s test module (match its existing style; read it first):

```rust
#[test]
fn markdown_panes_survive_a_state_round_trip() {
    let mut st = WarpuiState::default();
    st.markdowns = vec![(
        7,
        SMarkdown { path: std::path::PathBuf::from("/tmp/doc.md"), editing: false },
    )];
    let json = serde_json::to_string(&st).expect("serialize");
    let back: WarpuiState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.markdowns.len(), 1, "markdown panes must survive a round trip");
    assert_eq!(back.markdowns[0].1.path, std::path::PathBuf::from("/tmp/doc.md"));
}

#[test]
fn state_without_markdowns_still_loads() {
    // Backward compatibility: an existing ~/.crane/warpui-state.json predates
    // this field and must still deserialize rather than wiping the session.
    let legacy = r#"{}"#;
    let st: WarpuiState = serde_json::from_str(legacy).expect("legacy state must load");
    assert!(st.markdowns.is_empty());
}
```

Use the real `PaneId` type for the `7` above — check what `PaneId` actually is before writing this.

- [ ] **Step 3: Run it and confirm it fails**

Run: `cargo test --bin crane markdown_panes_survive -- --nocapture`
Expected: FAIL — no `SMarkdown` type, no `markdowns` field.

- [ ] **Step 4: Add the persisted type**

In `persist.rs`, beside `SBrowser`:

```rust
/// Persisted Markdown Pane: the file it renders and whether it was left in
/// edit mode. Restored as a Markdown (or Editor) pane rather than a terminal.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SMarkdown {
    #[serde(default)]
    pub path: std::path::PathBuf,
    /// True = the pane was showing the editor, false = the rendered preview.
    #[serde(default)]
    pub editing: bool,
}
```

Match the derive list and serde attributes actually used by `SBrowser` — read it and mirror it exactly.

Add to the state struct beside `browsers`:

```rust
/// Per Markdown pane: the file + mode, keyed by pane id, so the restore
/// loop rebuilds a Markdown pane (not a terminal) at that leaf.
#[serde(default)]
pub markdowns: Vec<(PaneId, SMarkdown)>,
```

- [ ] **Step 5: Run tests to confirm they pass**

Run: `cargo test --bin crane markdown_panes_survive state_without_markdowns -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Save markdown panes**

In `shell.rs`, beside the `browsers` collection (~`:1897`), mirror the pattern:

```rust
let markdowns: Vec<(PaneId, crate::warpui::persist::SMarkdown)> = self
    .panes
    .iter()
    .filter_map(|(id, pc)| match pc {
        PaneContent::Markdown(h) => h.as_ref(app).path().map(|p| {
            (
                *id,
                crate::warpui::persist::SMarkdown {
                    path: p.to_path_buf(),
                    editing: false,
                },
            )
        }),
        _ => None,
    })
    .collect();
```

Thread `markdowns` into the constructed state alongside `browsers`. Follow how `browsers` reaches the saved struct — do not guess.

- [ ] **Step 7: Restore markdown panes**

In the restore chain (`shell.rs:1351-1385`), add an arm **before** the final terminal fallback, mirroring the browser arm:

```rust
} else if let Some(sm) = restored_markdowns.get(&pid) {
    // Rebuild the Markdown pane on its saved file. Without this the pane
    // fell through to a fresh terminal and the document was lost.
    let p = sm.path.clone();
    let h = ctx.add_typed_action_view(move |ctx| {
        crate::warpui::markdown_view::WarpMarkdownView::new(ctx, p)
    });
    panes.insert(pid, PaneContent::Markdown(h));
}
```

Build `restored_markdowns` from the loaded state the same way `restored_browsers` is built — find that code and mirror it. Also insert the handle into `self.markdown_views` keyed by path, so a later open of the same file reuses this view instead of building a second one.

**Ignore `sm.editing` in this task** — Task 2 wires it.

- [ ] **Step 8: Verify the round trip in the real app**

Run: `cargo run`. Open a markdown file. Quit with Cmd+Q. Relaunch.
Expected: the markdown pane returns showing the document — **not a terminal**.

If you cannot drive the GUI, say so explicitly and mark DONE_WITH_CONCERNS. Do not claim a verified restore you did not observe.

- [ ] **Step 9: Full suite and commit**

Run: `make test` — all 117 plus your new tests must pass.

```bash
git add src/warpui/markdown_view.rs src/warpui/persist.rs src/warpui/shell.rs
git commit -m "fix(warpui): restore markdown panes instead of replacing them with terminals

A markdown pane was never persisted, so the restore chain fell through to
its terminal fallback and the open document was silently replaced by a
shell. The view now carries its source path, state gains a defaulted
markdowns entry, and the restore loop rebuilds the pane on its file."
```

---

### Task 2: Edit/preview toggle

`.md` currently routes read-only and returns before the editor path, so markdown cannot be edited at all.

**Files:**
- Modify: `src/warpui/shell.rs` (routing ~`:10794`, pane header ~`:10642`, a new action)
- Modify: `src/warpui/persist.rs` (write real `editing` values)
- Test: `src/warpui/shell.rs` test module if one exists; otherwise cover the mode-persistence logic where testable

**Interfaces:**
- Consumes: `SMarkdown.editing` and `WarpMarkdownView::path()` from Task 1.

- [ ] **Step 1: Track per-path mode**

Add to the shell struct, beside `markdown_views`:

```rust
/// Markdown files the user switched into edit mode. Absent = rendered
/// preview (the default). Keyed by path so the mode follows the file
/// across pane moves and restarts.
md_edit_mode: std::collections::HashSet<PathBuf>,
```

Initialize it empty alongside the other maps, and on restore populate it from every `SMarkdown` whose `editing` is true.

- [ ] **Step 2: Route by mode**

At the `is_md` branch (`shell.rs:10794`), open the **editor** instead of the markdown view when the path is in `md_edit_mode`. Concretely: keep the existing markdown path for preview mode, and when in edit mode fall through to the editor path below rather than returning early.

**Default stays preview** — this is a deliberate deviation from old Crane (which was editor-first). It preserves current behavior; the toggle is what resolves the complaint.

- [ ] **Step 3: Add the toggle action and header button**

Add a `ToggleMarkdownMode(PaneId)` variant to the shell action enum, following how a neighbouring pane action is declared and dispatched.

Handling it: look up the pane's path (from the `Markdown` handle's `path()`, or from `editor_views` when already editing), flip membership in `md_edit_mode`, then swap `self.panes` for that id between `PaneContent::Markdown` and `PaneContent::Editor`, reusing the cached handle from `markdown_views` / `editor_views` and building one only if absent.

**Switching to preview must re-parse from the editor's live buffer**, not from disk, so unsaved edits appear. `WarpMarkdownView::from_source(ctx, title, text)` (`markdown_view.rs:639`) exists for exactly this and is currently unwired. Note `from_source` sets `path: None`, which would break Task 1's persistence — so when using it for a real file, set the path as well (add a constructor or a setter; do not leave a real file's view pathless).

In the pane header (`shell.rs:10642`), add a button dispatching the action for markdown/editor panes on a `.md` file:
- showing `icons::PENCIL_SIMPLE` when in preview (click → edit)
- showing `icons::FILE_TEXT` when editing (click → preview)

There is **no `EYE` icon** — do not reference one. Give the button `min_size` ≥ 22×22.

- [ ] **Step 4: Persist the mode**

In the save collection from Task 1 Step 6, set `editing` from `md_edit_mode.contains(path)` rather than the hardcoded `false`. Also emit an `SMarkdown` for `.md` files currently open as **Editor** panes — otherwise switching to edit mode and quitting loses the pane, reintroducing Task 1's bug through the back door.

In the restore arm from Task 1 Step 7, build a `PaneContent::Editor` when `sm.editing` is true and a `PaneContent::Markdown` otherwise.

- [ ] **Step 5: Verify in the real app**

Run: `cargo run`.
1. Open a `.md` file → renders as preview.
2. Click the toggle → editor appears, file is editable.
3. Type an edit without saving, toggle back → **the preview shows the unsaved edit**.
4. Toggle to edit, Cmd+Q, relaunch → pane returns **in edit mode**.
5. Toggle to preview, Cmd+Q, relaunch → pane returns **in preview**.

If you cannot drive the GUI, say so explicitly and mark DONE_WITH_CONCERNS rather than claiming observed behavior.

- [ ] **Step 6: Full suite and commit**

Run: `make test`.

```bash
git add -A
git commit -m "feat(warpui): edit/preview toggle for markdown panes

Markdown files routed read-only with no path back to the editor, so a
.md file could not be edited at all. A header button now swaps the pane
between the rendered preview and the editor, preview re-parses from the
editor's live buffer so unsaved edits show, and the mode persists with
the pane."
```

## Self-review notes

- **Coverage:** session persistence → Task 1; edit/preview toggle → Task 2; mode persistence spans both (written in T1 Step 6 as `false`, made real in T2 Step 4).
- **Ordering hazard:** Task 2 Step 4 must update Task 1's save collection, not add a second one. Two collections writing the same field would race.
- **Known risk:** `from_source` sets `path: None`; using it for a real file without setting the path silently breaks persistence. Called out inline in T2 Step 3.
- **Not covered:** no test exercises the GUI toggle itself — both verification steps are manual. The layout-test harness (`App::test` → `Presenter::build_scene`, see `markdown_view.rs` tests) can catch layout panics in a rebuilt pane but not click behavior.
