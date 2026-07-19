# PDF Viewer Pane — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Open PDFs in a Pane, rendered by pdfium — restoring what the pre-warpui build had.

**Architecture:** A warpui `View` mirroring `WarpImageView`. pdfium renders a page to an encoded PNG in memory; the bytes are registered with warpui's asset cache under a stable id and drawn with the same `Image` element the image viewer uses. Routing and persistence mirror the Image pane exactly.

**Tech Stack:** Rust edition 2024, warpui (vendored), `pdfium-render` bound at runtime to a vendored `libpdfium.dylib`, `cargo test --bin crane`.

## Verified facts — checked against the code, not assumed

| Fact | Evidence |
|---|---|
| Vendored dylibs present: arm64 (7.0MB) + x86_64 (7.4MB), pinned `chromium/7763` | `vendor/pdfium/`, `.pinned-tag` |
| `make vendor-pdfium` fetches them; **idempotent**, so it no-ops when present | `Makefile:87`, `scripts/vendor-pdfium.sh` |
| `bundle` copies host-arch dylib to `Contents/Frameworks`, sets `install_name_tool -id @rpath/libpdfium.dylib` + `-add_rpath @executable_path/../Frameworks` | `Makefile:90-100` |
| `bundle-universal` `lipo`-fuses both arches into one fat dylib | `Makefile:130,140-147` |
| Old binding: `Pdfium::bind_to_library(path)` → `Pdfium::new(bindings)`, falling back to `Pdfium::bind_to_system_library()` | `d578f1a:src/views/pdf_view.rs:130-143` |
| Old helpers `candidate_pdfium_paths()`, `host_arch()` search bundle + vendor dirs | same file, `:152`, `:169` |
| Old API use: `doc.pages()`, `pages.get(idx as i32)`, `page.text()` | same file, `:100`, `:442-445` |
| Old dep was `pdfium-render = "0.9"`, ABI-matched to `chromium/7763` | `d578f1a:Cargo.toml:94` |
| **Runtime-generated bitmaps go in via `AssetSource::Raw { id: String }`** | `asset_cache.rs:132-133` |
| `insert_raw_asset_bytes<T: Asset>(id: String, bytes: &[u8], ctx: &mut ModelContext<Self>)` | `asset_cache.rs:470` |
| It calls `T::try_from_bytes(bytes)` — bytes must be an **encoded image (PNG)**, not raw RGBA | `asset_cache.rs:481` |
| Real precedent for `Raw`: warp's own terminal grid renderer | `app/src/terminal/grid_renderer.rs:1835` |
| Old constants: `ZOOM_PRESETS = [0.50,0.75,1.00,1.25,1.50,2.00,3.00,4.00]`, `DEFAULT_ZOOM = 1.00`, `PAGE_GAP = 12.0`, `TEXTURE_KEEP_RADIUS = 5` | `d578f1a:src/views/pdf_view.rs:25-29` |

**Design consequence:** `insert_raw_asset_bytes` needs a `&mut ModelContext`, so registration must happen inside an asset-cache model update — it cannot be called from an arbitrary render path. Plan for registration to happen on an action (open, page change, zoom change), never per frame.

## Global Constraints

- Never use Unicode glyph icons — bundled fonts render them as tofu. Only constants that exist in `src/warpui/icons.rs` (there is **no** `EYE`).
- Do NOT modify anything under `vendor/warp/`.
- **Do NOT run `cargo fmt` in any form** — it reformats the entire workspace including vendored code regardless of the path argument. Hand-format.
- Every persisted field carries `#[serde(default)]`. Never break deserialization of an existing `~/.crane/warpui-state.json`; never read or write it from tests (use the `HomeOverride` guard).
- Never launch GUI windows; never `osascript`/`kill` a process you did not start.
- Commit messages: conventional prefix, ZERO AI/assistant references, no Co-Authored-By.
- `make test` (= `cargo test --bin crane`), currently **174**.
- **Mutation-verify every new test:** break the code it covers, confirm RED for a genuine behavioral reason, restore, confirm green. Report per test. Multiple features on this branch shipped with tests that stayed green while the feature was absent.

---

### Task 1: pdfium binding + page rendering

**Files:** create `src/warpui/pdf_view.rs`; modify `src/warpui/mod.rs`, `Cargo.toml`.

- [ ] **Step 1: Add the dependency, matching the pinned ABI**

Re-add `pdfium-render`. The vendored dylib is `chromium/7763` and the old build paired it with `0.9`. **Check what `0.9.x` resolves to today and whether its documented pdfium ABI still matches `7763`.** If a newer minor has diverged, pin the exact version that matches and say so in your report — an ABI mismatch is a runtime crash, not a compile error.

- [ ] **Step 2: Port the binding logic**

Port `get_pdfium()`, `candidate_pdfium_paths()` and `host_arch()` from `d578f1a:src/views/pdf_view.rs:130-178` (`git show` it). Search order must cover the bundled `Contents/Frameworks` path **and** the repo's `vendor/pdfium/<arch>/` so `cargo run` works during development, falling back to the system library.

**A missing or unloadable dylib must degrade to an error panel, never panic** — this is user-facing on any machine where vendoring was skipped.

- [ ] **Step 3: Render a page to PNG bytes**

pdfium renders to a bitmap; encode it to PNG in memory (the `image` crate is already an indirect dependency — confirm before adding it directly). Key the asset id on `(path, page_index, zoom)` so a zoom change is a deliberate cache miss.

- [ ] **Step 4: Register and draw**

```rust
// inside an asset-cache model update — NOT on the render path
cache.insert_raw_asset_bytes::<ImageType>(id.clone(), &png_bytes, ctx);
// then, in render:
Image::new(AssetSource::Raw { id }, CacheOption::Original).contain().finish()
```

Confirm the concrete `T` for `insert_raw_asset_bytes::<T>` by reading how `grid_renderer.rs:1835` does it — do not guess the type parameter.

- [ ] **Step 5: Test via the headless harness**

Use the `App::test` → `add_window` → `Presenter::build_scene` pattern from `image_view.rs`. Assert a view over a small PDF lays out finitely, and that a **missing/corrupt PDF and a missing dylib** both lay out finitely with the error panel rather than panicking.

Create a minimal PDF fixture under the scratch dir. Do not depend on files outside the repo.

**Note:** `Image::paint` swallows a failed rect by painting nothing, so a blank pane passes layout tests. State plainly in your report that "renders finitely" is not "renders visibly."

---

### Task 2: Toolbar, navigation, zoom

Port from the old view: page navigation, `ZOOM_PRESETS` stepping, `PAGE_GAP`, `TEXTURE_KEEP_RADIUS = 5` (evict page assets more than 5 pages from the viewport), an **Open Externally** button, and the error panel for encrypted/corrupt files.

Use only icons that exist in `icons.rs`. Keep the zoom presets and default identical to the old build.

---

### Task 3: Routing + persistence

**Mirror the Image pane exactly** — it is the freshest precedent and already survived review. Same four routing sites, and extend the pure `restored_pane_kind` rather than forking it.

**Persistence is required, not a follow-up.** A document pane without it restores as a terminal.

**Check the four known defect shapes** that the markdown and image panes each hit:
1. **Dead restore arm** — a document pane persists into BOTH its own record and the files-pane record; does the ladder reach the PDF arm for the shape `open_file` actually writes?
2. **Missing files-pane bookkeeping** → duplicate pane.
3. **Migration gate** testing a resolved map instead of a raw persisted field.
4. **`editor_paths_for_restore` not excluding the new type** → an empty editor buffer over a binary file that Cmd+S truncates to zero bytes. *A PDF is binary; this one destroys data.* Note the restore pre-pass now has a UTF-8 guard, which should cover it — **verify that rather than assuming**.

---

### Task 4 (optional, decide before starting): text selection

The old build had drag text-selection with Cmd+C, via `screen_to_pdf_pts` → `char_hit_test` → `handle_selection` → `paint_selection_for_page` → `selection_text`, backed by `page.text()`.

This is the largest single piece and is independent of viewing. **Ship Tasks 1-3 first and treat this as a separate decision** — a PDF viewer that displays and navigates is useful without selection; a half-built selection model is not.

Out of scope regardless (inherited from the 2026-05-03 spec): in-document find, cross-page selection, thumbnails, outline/TOC, annotations, form filling, print.

## Self-review notes

- **Riskiest:** the pdfium ABI pin. A mismatch between the crate version and `chromium/7763` fails at runtime, not compile time, and only on a machine that actually loads the dylib.
- **Second riskiest:** skipping Task 3's persistence, or missing defect shape 4 — a PDF is binary, so an empty editor buffer over it is data destruction.
- **Build system needs no work.** Vendoring, `@rpath` bundling and universal `lipo` all survived the warpui migration and are verified present.
- **Must be verified by a human:** that pages visibly render. The layout harness cannot prove pixels, and a blank pane passes every test.
