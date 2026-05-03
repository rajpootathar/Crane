# PDF viewer design

**Status**: design — implementation pending.
**Scope**: B-tier viewer (view + page nav + zoom + text select/copy + Open Externally).
**Out of scope for v1**: in-document find (Cmd+F), cross-page selection, thumbnails, outline/TOC, annotations, form filling, print.

## Why

Crane is positioned as an Agent-native ADE. PDFs land in the Files
Pane tree (specs, RFCs, vendor datasheets) and right now there is no
in-app viewer — clicking a `.pdf` falls through the syntect path and
renders garbage. Users have to bounce out to Preview.app, which
breaks the pane-centric workflow Crane is built around.

The bar is not "be Preview." The bar is: open the file, read it,
copy a quote, move on. If a PDF is encrypted, malformed, or
otherwise outside our renderer's comfort zone, hand it to the system
viewer and stay out of the way.

## Architecture

Renders as a **file tab inside the Files Pane**, mirroring the
markdown path at `src/views/file_view.rs:852` (`markdown_view::render_md`).
Click a `.pdf` in the file tree → opens as a file tab → `file_view`
detects the extension and dispatches to `pdf_view::render_pdf` instead
of the syntect/text path.

```
src/views/pdf_view.rs   ← new module
crates: pdfium-render    ← new dep
make vendor-pdfium       ← new target
make release             ← copies libpdfium.dylib into Crane.app/Contents/Frameworks
```

## Components

### `PdfTabState` (per open PDF, owned by FilesPane)

| field           | type                                              | purpose                                           |
|-----------------|---------------------------------------------------|---------------------------------------------------|
| `path`          | `PathBuf`                                         | source file (also for Open Externally)            |
| `doc`           | `Option<pdfium_render::PdfDocument>`              | None when load failed                             |
| `page_count`    | `usize`                                           |                                                   |
| `current_page`  | `usize`                                           | 0-indexed; tracks viewport top                    |
| `zoom`          | `ZoomMode` (Fixed(f32) / FitWidth / FitPage)      | presets: 50/75/100/125/150/200/300/400 + fit modes |
| `texture_cache` | `HashMap<(page, zoom_bucket), egui::TextureHandle>` | LRU-evicted to ±5 pages of viewport             |
| `selection`     | `Option<Selection { page, char_start, char_end }>` | single-page only in v1                          |
| `text_extract`  | `HashMap<usize, PdfPageText>`                     | lazy per-page char-rect cache for hit-testing     |
| `error`         | `Option<String>`                                  | encryption / corruption / dylib-missing message   |

### `pdf_view::render_pdf(ui, state, ctx)`

Two regions:

1. **Toolbar** (top, fixed height): page-prev, page-input + total, page-next · zoom dropdown · spacer · `Open Externally` button (right-aligned via `Layout::right_to_left`). Uses `egui_phosphor` icons (`CARET_LEFT`, `CARET_RIGHT`, `MAGNIFYING_GLASS_PLUS`, `MAGNIFYING_GLASS_MINUS`, `ARROW_SQUARE_OUT`).
2. **Document** (vertical scroll): pages stacked top-to-bottom with a small gap, each page = an `egui::Image` from a cached texture. Drag-to-select draws semi-transparent overlay rects over each character's bounding box in the active selection range.

## Data flow

```
.pdf click in FileTree
  → FilesPane opens FileTab with PdfTabState (Pdfium::load_pdf_from_file)
  → render_pdf(ui) each frame:
       toolbar
       for visible page idx:
            cache.entry((idx, zoom_bucket)).or_insert_with(|| render_page_to_texture(doc, idx, zoom))
            ui.add(Image::from_texture(...))
            if selection.page == idx: paint highlight rects
       drag in document → update selection (text_hit_test against text_extract)
       Cmd+C → copy selected substring to clipboard
       Open Externally → Command::new("open").arg(&state.path).spawn() (macOS)
                       → xdg-open / start equivalent on Linux/Windows
```

Texture eviction runs each frame after the viewport-page mapping:
any cached `(page, _)` outside `current_page ± 5` is dropped. Zoom
buckets keep two: the "live" zoom and (if mid-transition) the prior
one — once the user settles, the prior bucket evicts.

## Error handling

- **Encrypted PDF / open failure** → `error = Some(...)`, render only the toolbar with `Open Externally` enabled and a centered explanatory message. No render attempts.
- **`libpdfium.dylib` missing at runtime** → catch the bind error on first `Pdfium::default()` call (lazy, lazily-initialized once per process), set the same error path for every `.pdf` opened thereafter. Don't crash the app.
- **Per-page render failure** → render that page as a placeholder rect with the error inline; sibling pages continue. Cache the failure so we don't retry every frame.

## Keyboard

Scoped to the PDF tab when focused. The Files Pane's existing
focus model already gates input — we add bindings inside the
`render_pdf` block, behind `if response.has_focus()`.

| key                    | action                            |
|------------------------|-----------------------------------|
| `PageUp` / `PageDown`  | prev / next page                  |
| `Home` / `End`         | first / last page                 |
| `Cmd+=` / `Cmd+-`      | zoom in / out (preset stops)      |
| `Cmd+0`                | zoom 100%                         |
| `Cmd+C`                | copy selection                    |
| `Cmd+A`                | select all on current page (v1)   |

The global `Cmd+=` / `Cmd+-` / `Cmd+0` font-size shortcuts keep
working in all other contexts — we intercept the event only when a
PDF tab is focused. (Same gating as the existing terminal-pane key
routing in `terminal/view.rs`.)

## Distribution

### `vendor/pdfium/` layout

```
vendor/pdfium/
├── arm64/
│   └── libpdfium.dylib   (~10 MB)
└── x86_64/
    └── libpdfium.dylib   (~10 MB)
```

Both pinned to a tagged release of
[`bblanchon/pdfium-binaries`](https://github.com/bblanchon/pdfium-binaries),
checksum-verified. Not committed to the repo (binaries) — `make
vendor-pdfium` downloads them on demand. `vendor/pdfium/` is added
to `.gitignore`.

### Makefile changes

```make
vendor-pdfium:
        # Downloads libpdfium.dylib for arm64 + x86_64, verifies sha256.
        ./scripts/vendor-pdfium.sh

bundle: icns install-cargo-bundle vendor-pdfium
        cargo bundle --release
        # Copy host-arch dylib into the bundle.
        mkdir -p "$(APP)/Contents/Frameworks"
        cp "vendor/pdfium/$(ARCH)/libpdfium.dylib" \
                "$(APP)/Contents/Frameworks/libpdfium.dylib"
        install_name_tool -id @rpath/libpdfium.dylib \
                "$(APP)/Contents/Frameworks/libpdfium.dylib"
        install_name_tool -add_rpath @executable_path/../Frameworks \
                "$(APP)/Contents/MacOS/$(BIN_NAME)" 2>/dev/null || true
        # ... existing ad-hoc sign / status echo ...
```

`bundle-universal` does the same plus a `lipo -create` of the two
arch-specific dylibs into a single fat dylib before copying.

The existing `_sign_bundle` macro already finds `*.dylib` under
`Contents` and signs each — no change required for the signing
target. `entitlements.plist` may need
`com.apple.security.cs.disable-library-validation` if the dylib
fails the hardened runtime check (we'll only add it if signing
flags it; default is to try without).

### Runtime dylib resolution

`pdfium-render` calls `Pdfium::bind_to_library(path)`. We try, in
order:

1. `@executable_path/../Frameworks/libpdfium.dylib` (bundled — production)
2. `vendor/pdfium/<host-arch>/libpdfium.dylib` (relative to CWD — `cargo run`)
3. System default lookup (`Pdfium::bind_to_system_library()`) (fallback)

If all three fail, set `error` and let Open Externally take over.

## Testing

Unit tests in `pdf_view`:

- `page_at_y(scroll_y, page_heights) -> usize` — viewport-to-page mapping
- `text_hit_test(char_rects, point) -> Option<char_idx>` — selection start hit-test
- `clamp_zoom_to_preset(current, direction) -> f32` — preset stop math

Manual smoke matrix:

| PDF kind                                | expected                                              |
|-----------------------------------------|-------------------------------------------------------|
| 1-page, plain text                      | renders, drag-select works, Cmd+C copies              |
| 50+ pages, mixed graphics               | smooth scroll, no memory blowup, eviction kicks in    |
| Encrypted (Acrobat password)            | error UI, Open Externally launches Preview            |
| Truncated header (corrupt)              | error UI, no crash                                    |
| Renamed `.txt` to `.pdf`                | error UI, no crash                                    |
| `libpdfium.dylib` removed from bundle   | error UI on every open, no crash                      |
| Universal DMG on Intel + Apple Silicon  | both archs render; check via `lipo -info` on dylib    |

No automated rendering snapshot tests in v1 — gold-image diffing
across architectures is expensive and brittle, and the rendering is
pdfium's job, not ours.

## Risks and mitigations

- **Pdfium dylib weight (~10 MB per arch, ~20 MB universal)**. Acceptable; the rest of Crane is already in the same ballpark. Document in release notes.
- **Hardened runtime + dylib-validation**. If notarization fails on the bundled dylib, add `com.apple.security.cs.disable-library-validation` to entitlements as a targeted fix. Don't pre-emptively weaken signing.
- **`pdfium-render` API instability**. Pin a tagged version; no `*` ranges. Major bumps get a manual review.
- **Selection across multi-column or rotated pages**. v1 handles single-flow text only. Multi-column/rotated PDFs may produce surprising selection behavior — acceptable for v1; the Open Externally button is the escape hatch.
- **Memory**. Texture cache bounded to ±5 pages × 2 zoom buckets ≈ 10 page textures. At 200% zoom on a Letter page (~1700 × 2200 RGBA) that's ~15 MB/texture × 10 ≈ 150 MB worst case. Acceptable for a focused doc-reading session; document the budget.

## Pending work after v1

(Not blocking this spec — intentional v2 list.)

- In-document find (Cmd+F), reusing the Files Pane find-bar pattern.
- Cross-page selection.
- Thumbnails sidebar.
- Outline / TOC navigation panel.
- Linux/Windows dylib bundling — same `vendor-pdfium.sh` script extends to `libpdfium.so` / `pdfium.dll`. Out of scope for the macOS-first v1.
