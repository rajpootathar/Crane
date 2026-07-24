# Image Viewer Pane — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Open image files in a Pane instead of the editor, which currently either shows binary garbage or refuses them.

**Architecture:** A self-contained warpui `View` mirroring `WarpMarkdownView`, rendering through warpui's shipped `Image` element. Routed by extension beside the existing `.md` branch, cached per path, and persisted per Workspace like markdown panes so a restart does not turn it into a terminal.

**Tech Stack:** Rust edition 2024, warpui (vendored at `vendor/warp`), `cargo test --bin crane`.

## Verified facts — checked against REAL call sites, not struct definitions

| Fact | Evidence |
|---|---|
| `Image::new(source: AssetSource, cache: CacheOption)` then `.finish()` | `vendor/warp/app/src/menu.rs:968` |
| Sizing goes on a **wrapping** `ConstrainedBox`/`Container`, not on `Image` | `menu.rs:968-972` |
| `AssetSource::LocalFile { path: String, content_version: Option<LocalFileContentVersion> }` | `asset_cache.rs:124-131` |
| `LocalFileContentVersion::for_path(path) -> Option<Self>` | `asset_cache.rs:87` |
| **`for_path` does BLOCKING filesystem I/O — "must only be called off the render hot path … never on every frame"** | `asset_cache.rs:82-84` (verbatim) |
| `content_version: None` = path-only caching, no invalidation when the file changes on disk | `asset_cache.rs:126-129` |
| `CacheOption::BySize` = fixed size (icons); `CacheOption::Original` = size varies with window | `image_cache.rs:497-507` |
| Builders: `.contain()`, `.cover()`, `.stretch()`, `.with_corner_radius()`, `.with_opacity()`, `.enable_animation_with_start_time(Instant)`, `.before_load(el)`, `.on_load_failure(el)`, `.on_load_timeout(dur, el)` | `elements/gui/image.rs:80-181` |
| Existing extension list `IMAGE_EXTS = ["png","jpg","jpeg","gif","bmp","webp","ico"]` + `is_image_path_str` | `diff_view.rs:67`, `:142` |

**THE TRAP:** warpui views rebuild their element tree **every frame** (see `markdown_view.rs`'s module docs: "elements are transient; the model persists"). Calling `LocalFileContentVersion::for_path` inside the render path means blocking file I/O at 60fps. **Resolve it once at view construction and store it**; refresh only when the file-watcher reports a change.

## Global Constraints

- Never use Unicode glyph icons — bundled fonts render them as tofu. Only constants that exist in `src/warpui/icons.rs` (there is **no** `EYE`).
- Do NOT modify anything under `vendor/warp/`.
- **Do NOT run `cargo fmt` in any form** — it reformats the entire workspace including vendored code regardless of the path argument. Hand-format.
- Every persisted field carries `#[serde(default)]`. Never break deserialization of an existing `~/.crane/warpui-state.json`.
- Never read/write `~/.crane/` from tests — use the `HomeOverride` guard in `restore_wiring_integration_tests`.
- Never launch GUI windows, and never `osascript`/`kill` a process you did not start.
- Commit messages: conventional prefix, ZERO AI/assistant references, no Co-Authored-By.
- Tests: `make test` (= `cargo test --bin crane`), currently **151**.
- **Every new test must be mutation-verified:** break the code it covers, confirm RED for a genuine behavioral reason, restore, confirm green. Report per test. Several features on this branch shipped with tests that passed while the feature was absent.

---

### Task 1: The image view

**Files:**
- Create: `src/warpui/image_view.rs`
- Modify: `src/warpui/mod.rs` (register the module)
- Test: in `image_view.rs`

**Interfaces:**
- Produces: `WarpImageView::new(ctx, path) -> Self`, `path() -> Option<&Path>`, `title() -> &str`. Task 2 routes to it; Task 3 persists it.

- [ ] **Step 1: Read the precedent first**

Read `src/warpui/markdown_view.rs` end to end — struct shape, `Entity`/`TypedActionView` impls, how it stores `path`/`title`, and the `App::test` layout tests at the bottom. Mirror that structure. Do not invent a different view shape.

- [ ] **Step 2: Write the failing layout test**

Mirror `markdown_view.rs`'s `build_markdown_scene` helper: construct the view through `App::test` → `add_window` → `Presenter::build_scene`, which runs the real layout **and paint** pass headlessly so `Scene::validate_rect` fires. Assert a view over a small image lays out finitely. (This harness is what caught an infinite-height crash on this branch; use it.)

Point it at a tiny fixture image you create under the scratch dir, or a small PNG already in the repo — do NOT depend on a file outside the repo.

- [ ] **Step 3: Implement the view**

```rust
pub struct WarpImageView {
    path: PathBuf,
    title: String,
    /// Content fingerprint, resolved ONCE. `LocalFileContentVersion::for_path`
    /// does blocking filesystem I/O and must never run on the render path.
    content_version: Option<LocalFileContentVersion>,
    /// When this view opened — drives animated GIF playback.
    opened_at: Instant,
}
```

Render:

```rust
Image::new(
    AssetSource::LocalFile {
        path: self.path.to_string_lossy().into_owned(),
        content_version: self.content_version,   // resolved at construction
    },
    CacheOption::Original,   // pane-sized, changes on resize — not a fixed-size icon
)
.contain()
.enable_animation_with_start_time(self.opened_at)
.on_load_failure(/* "Couldn't decode image" text element */)
.finish()
```

Wrap in a `Container`/`Stack` for background and padding, matching how `markdown_view.rs`'s `panel()` does it. Confirm each builder exists in `vendor/warp/crates/warpui_core/src/elements/gui/image.rs` before using it — do not assume from this plan.

Error copy is `"Couldn't decode image"`, matching the pre-warpui build.

- [ ] **Step 4: Verify the hot-path rule**

Confirm by reading your own render function that `LocalFileContentVersion::for_path` is **not** called there. If your view refreshes on file change, that refresh path is where it may be recomputed. State in your report where it is called and how often.

- [ ] **Step 5: Run tests, commit**

`make test` — all 151 plus yours.

---

### Task 2: Route image files to the pane

**Files:**
- Modify: `src/warpui/shell.rs` (`PaneContent` enum, view cache, extension routing, render arm, pane header)

**Interfaces:**
- Consumes Task 1's `WarpImageView`. Produces `PaneContent::Image(ViewHandle<WarpImageView>)` and an `image_views: HashMap<PathBuf, ViewHandle<_>>` cache.

- [ ] **Step 1: Mirror the markdown routing exactly**

Markdown is the working precedent. Find and mirror each site:
- `PaneContent::Markdown(ViewHandle<_>)` variant → add `Image(_)`
- `markdown_views: HashMap<PathBuf, ViewHandle<_>>` → add `image_views`
- the `is_md` extension branch in `open_file` → add an image branch
- the render arm `ChildView::new(h).finish()`
- the pane header `(icons::FILE_TEXT, title)` → pick an existing icon; **there is no `EYE`** — `FILE` or `FILE_TEXT` are safe

Hoist the extension test into a shared helper rather than a third copy — `diff_view.rs:67`/`:142` already has `IMAGE_EXTS` and `is_image_path_str`. Add `svg` to the list (warpui's cache supports SVG).

- [ ] **Step 2: Add a routing test**

Assert an image path routes to `PaneContent::Image` and a `.rs` path still routes to the editor. Extract the extension decision into a pure function if that makes it testable without a full shell — that pattern is already used here (`needs_formatted_text`, `restored_pane_kind`).

- [ ] **Step 3: Mutation-verify, run tests, commit**

---

### Task 3: Persist image panes

Without this, an image pane restores as a **fresh terminal** — the exact bug just fixed for markdown panes. Do not skip.

**Files:**
- Modify: `src/warpui/persist.rs`, `src/warpui/shell.rs`

- [ ] **Step 1: Study how markdown panes persist**

Read `SMarkdown`, the `markdowns` field, the save-side collector in `build_state`, and the `Markdown` arm inside `restored_pane_kind`. Note that `restored_pane_kind` is a **pure, tested function** — extend it rather than adding a parallel decision path.

- [ ] **Step 2: Decide the schema and say why**

Either add an `images` field mirroring `markdowns`, or generalize both into one document-pane record with a kind tag. **Mirroring is lower risk; generalizing is cleaner.** Pick one, justify it in your report, and if you generalize, provide a migration that cannot lose existing `markdowns` entries.

- [ ] **Step 3: Save + restore, with tests**

Add `RestoredPaneKind::Image` to the pure function and cover it. Then an integration test through `new_with_state`: an image pane saved and restored comes back as `PaneContent::Image`, **not** a terminal.

- [ ] **Step 4: Mutation-verify each test, run suite, commit**

## Self-review notes

- **Riskiest thing:** the `for_path` hot-path trap. It is invisible in the type signature and only documented at the definition. Task 1 Step 4 exists solely to force a check.
- **Second riskiest:** skipping Task 3. Every document pane type added without persistence regresses to a terminal on restart.
- **Explicitly out of scope:** the diff-view image block (the user dropped it), markdown inline images, and the edit/preview toggle.
- **Unverifiable here:** that an image visibly renders. No agent can drive a native macOS window; the layout test proves finite layout and no panic, not correct pixels. The user must confirm.
