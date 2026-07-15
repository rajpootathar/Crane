# Chrome Polish Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the approved chrome-polish design (`docs/superpowers/specs/2026-07-15-chrome-polish-design.md`): compact 24px pane headers with hover-lit 20×20 buttons, a two-row File-pane header, two-tier Left-Panel selection, underline tabs, premium context menus, a decluttered top bar (＋ New Pane dropdown, theme → gear menu), a live-pulse status bar, watcher-driven change counters for all repos, and full-FPS notification animation.

**Architecture:** All changes live in the warpui front-end (`src/warpui/`), almost entirely `shell.rs` + `theme.rs`. New visual language = white-alpha "washes" layered via existing `Hoverable`/`Container` elements; menus use `Container::with_corner_radius` + `with_drop_shadow` (already available). No new crates, no async runtime, no behavior changes except those in the spec.

**Tech Stack:** Rust edition 2024, in-house warpui (vendor/warp), `Hoverable` / `Container` / `Flex` / `ConstrainedBox` / `Stack` / `EventHandler` elements. Build with `cargo build`, tests with `make test`.

## Global Constraints

- Icons: only glyphs from `src/warpui/icons.rs` — never raw Unicode (tofu risk).
- All colors via `src/warpui/theme.rs` tokens or the new wash helpers — no hardcoded hex/rgb in shell.rs.
- Commit messages: conventional commits, zero AI references, no Co-Authored-By.
- `cargo build` must be warning-free for touched code; `make test` must pass before each commit.
- Run the app for visual checks with `CRANE_WARP=1 cargo run` (warpui front-end).
- shell.rs line anchors below are as of commit `988003c`; re-locate with the quoted `rg` patterns if drifted.

---

### Task 1: Wash tokens + shared hover icon button

**Files:**
- Modify: `src/warpui/theme.rs` (append)
- Modify: `src/warpui/shell.rs:6999-7010` (`fn icon_button`)

**Interfaces:**
- Produces: `theme::hover_wash() -> ColorU` (white @ 3.5% ≈ a9), `theme::selection_wash() -> ColorU` (white @ 7% ≈ a18), `theme::context_wash() -> ColorU` (white @ 2.5% ≈ a6), `theme::menu_shadow() -> ColorU` (black @ 50%).
- Produces: `fn icon_button(&self, key: &str, glyph: &'static str, action: CraneShellAction) -> Box<dyn Element>` — NOTE the new `key` param (hover-state key); every existing caller must pass a unique key.

- [ ] **Step 1: Add wash helpers to theme.rs**

Append to `src/warpui/theme.rs`:

```rust
/// White-alpha overlay washes — the app-wide hover/selection language.
/// Alphas are on-white overlays so they read identically on every theme.
pub fn hover_wash() -> ColorU     { ColorU { r: 255, g: 255, b: 255, a: 9 }  }
pub fn selection_wash() -> ColorU { ColorU { r: 255, g: 255, b: 255, a: 18 } }
pub fn context_wash() -> ColorU   { ColorU { r: 255, g: 255, b: 255, a: 6 }  }
/// Destructive menu-item hover: error() at ~15% alpha.
pub fn danger_wash() -> ColorU {
    let e = crate::theme::current().error;
    ColorU { r: e.r, g: e.g, b: e.b, a: 38 }
}
pub fn menu_shadow() -> ColorU    { ColorU { r: 0, g: 0, b: 0, a: 128 } }
```

Also change the header-height constant block (bottom of theme.rs):

```rust
pub const TOPBAR_H: f32 = 36.0;
pub const STATUS_H: f32 = 26.0;
pub const HEADER_H: f32 = 24.0;
pub const TAB_H: f32    = 26.0;
```

(`TOPBAR_H` 34→36 per spec §6; `STATUS_H` 28→26 per §7. Grep for uses of both constants — layout math in `main`/`shell` that assumes 34/28 must keep using the constant, not a literal.)

- [ ] **Step 2: Rework `icon_button` with hover + fixed hit box**

Replace `src/warpui/shell.rs` `fn icon_button` (anchor: `rg -n "fn icon_button" src/warpui/shell.rs`):

```rust
    /// A 20×20 hover-lit icon button (12px glyph). `key` must be unique per
    /// on-screen instance — it keys the persistent hover state.
    fn icon_button(&self, key: &str, glyph: &'static str, action: CraneShellAction) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("ibtn:{key}"));
        let icon_font = self.icon_font;
        Hoverable::new(state, move |ms| {
            let (bg, fg) = if ms.is_hovered() {
                (theme::selection_wash(), theme::text_hover())
            } else {
                (ColorU::new(0, 0, 0, 0), theme::text_muted())
            };
            ConstrainedBox::new(
                Container::new(
                    Align::new(
                        Text::new(glyph.to_string(), icon_font, 12.0)
                            .with_color(fg)
                            .finish(),
                    )
                    .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish(),
            )
            .with_width(20.0)
            .with_height(20.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
    }
```

If warpui has no `Align` element (check `rg -n "pub struct Align" vendor/warp/crates/warpui_core/src`), center with padding instead: `.with_padding_left(4.0).with_padding_top(4.0)` inside the 20×20 box.

- [ ] **Step 3: Fix all `icon_button` callers**

`rg -n "self.icon_button\(" src/warpui/shell.rs` — each call gains a key string naming the spot, e.g. in `pane_header`: `self.icon_button(&format!("pane-max:{id}"), icons::ARROWS_OUT, …)`, `…("pane-close:{id}")…`; in `top_bar`: `"tb-left"`, `"tb-gitlog"`, `"tb-right"`. Buttons keyed per pane id MUST include the id (multiple panes on screen).

- [ ] **Step 4: Build + test**

Run: `cargo build 2>&1 | tail -5` — expect `Finished`. Run: `make test` — expect existing tests pass.

- [ ] **Step 5: Visual check + commit**

`CRANE_WARP=1 cargo run` — pane-header ✕/⛶ are visibly smaller, hover shows a rounded wash + pointing hand.

```bash
git add src/warpui/theme.rs src/warpui/shell.rs
git commit -m "feat(warpui): hover-lit compact icon buttons + wash tokens"
```

### Task 2: Slim pane header (all panes) + hairline divider

**Files:**
- Modify: `src/warpui/shell.rs:9179-9311` (`fn pane_header`)

**Interfaces:**
- Consumes: Task 1 `icon_button(key, glyph, action)`, `theme::HEADER_H`.
- Produces: pane header rendering at `theme::HEADER_H` (24px) with a 1px `theme::divider()` line at its bottom; Task 3 restructures the File-pane branch inside this same function.

- [ ] **Step 1: Shrink header + add divider**

In `fn pane_header` (anchor: `rg -n "fn pane_header" src/warpui/shell.rs`):
- Delete `const H: f32 = 26.0;`, use `theme::HEADER_H`.
- Non-file title: `Text::new(label, self.ui_font, 11.0)` keeps size 11; adjust `with_padding_top(6.0)` → `4.0` (24px row centering).
- Buttons row gets right padding + vertical centering:

```rust
        let buttons = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(self.icon_button(&format!("pane-max:{id}"), icons::ARROWS_OUT, CraneShellAction::ToggleMaximize(id)))
                .with_child(Self::spacer(2.0))
                .with_child(self.icon_button(&format!("pane-close:{id}"), icons::X, CraneShellAction::ClosePane(id)))
                .finish(),
        )
        .with_padding_right(4.0)
        .with_padding_top(2.0)
        .finish();
```

- Wrap the final row so a divider paints under it:

```rust
        ConstrainedBox::new(
            Flex::column()
                .with_child(
                    Expanded::new(
                        1.0,
                        Stack::new()
                            .with_child(Rect::new().with_background_color(bg).finish())
                            .with_child(row)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
                        .with_height(1.0)
                        .finish(),
                )
                .finish(),
        )
        .with_height(theme::HEADER_H)
        .finish()
```

(24px total includes the 1px divider — the fill area is 23px; fine.)

- [ ] **Step 2: Build, visual check, commit**

`cargo build` then `CRANE_WARP=1 cargo run`: headers slimmer, hairline separates header from grid, terminal content not clipped (grid height derives from pane rect minus header — verify no constant `26.0` remains: `rg -n "26\.0" src/warpui/shell.rs` and fix any header-math hit).

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): 24px pane headers with bottom hairline"
```

### Task 3: File pane two-row header (pane chrome + tab strip)

**Files:**
- Modify: `src/warpui/shell.rs` (`fn pane_header` file-pane branch; the caller that stacks header above content — anchor `rg -n "pane_header\(" src/warpui/shell.rs`)

**Interfaces:**
- Consumes: Task 1 washes/buttons, Task 2 structure.
- Produces: `fn file_tab_strip(&self, app: &AppContext) -> Box<dyn Element>` (26px, full width); `fn pane_header` for the file pane returns 24px chrome row only. The pane's total chrome height for the File pane = `theme::HEADER_H + theme::TAB_H`; whoever computes the content rect must use that (find it via the `pane_header` call site).

- [ ] **Step 1: Extract the tab strip into `file_tab_strip`**

Move the `is_file_pane` strip-building block out of `pane_header` into a new method. Restyle each tab:

```rust
    /// The File pane's tab strip — second header row. Active tab: surface bg +
    /// 2px accent underline. Inactive: flat, hover wash. Per-tab ✕ closes the tab.
    fn file_tab_strip(&self, app: &AppContext) -> Box<dyn Element> {
        let mut strip = Flex::row();
        for (i, path) in self.file_pane_paths.iter().enumerate() {
            let active = i == self.file_pane_active;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let dirty = self.editor_views.get(path)
                .map(|h| h.as_ref(app).is_dirty(app)).unwrap_or(false);
            let state = self.hover_handle(&format!("ftab:{i}"));
            let ui_font = self.ui_font;
            let icon_font = self.icon_font;
            let label_color = if active { theme::text() } else { theme::text_muted() };
            let name_cl = name.clone();
            let chip = Hoverable::new(state, move |ms| {
                let bg = if active { theme::surface() }
                    else if ms.is_hovered() { theme::hover_wash() }
                    else { ColorU::new(0, 0, 0, 0) };
                let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
                if dirty {
                    row = row.with_child(
                        Container::new(Text::new(icons::CIRCLE.to_string(), icon_font, 8.0)
                            .with_color(theme::accent()).finish())
                        .with_padding_right(5.0).finish());
                }
                row = row.with_child(Text::new(name_cl.clone(), ui_font, 11.0)
                    .with_color(label_color).finish());
                // Underline: 2px accent for the active tab, transparent filler otherwise.
                let underline = ConstrainedBox::new(
                    Rect::new().with_background_color(
                        if active { theme::accent() } else { ColorU::new(0, 0, 0, 0) }).finish())
                    .with_height(2.0).finish();
                Container::new(
                    Flex::column()
                        .with_child(Expanded::new(1.0,
                            Container::new(row).with_padding_left(12.0).with_padding_right(4.0)
                                .with_padding_top(6.0).finish()).finish())
                        .with_child(underline)
                        .finish(),
                ).with_background_color(bg).finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FileTabSelect(i));
            })
            .finish();
            // Per-tab close — 16×16 hover box.
            let xstate = self.hover_handle(&format!("ftabx:{i}"));
            let icon_font2 = self.icon_font;
            let close = Hoverable::new(xstate, move |ms| {
                let (bg, fg) = if ms.is_hovered() { (theme::selection_wash(), theme::text_hover()) }
                    else { (ColorU::new(0, 0, 0, 0), theme::text_muted()) };
                ConstrainedBox::new(
                    Container::new(Text::new(icons::X.to_string(), icon_font2, 10.0)
                        .with_color(fg).finish())
                    .with_background_color(bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .with_padding_left(3.0).with_padding_top(3.0)
                    .finish(),
                ).with_width(16.0).with_height(16.0).finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(CraneShellAction::FileTabClose(i));
            })
            .finish();
            strip = strip.with_child(
                Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(chip)
                    .with_child(Container::new(close).with_padding_right(6.0).finish())
                    .finish());
        }
        ConstrainedBox::new(
            Flex::column()
                .with_child(Expanded::new(1.0,
                    Stack::new()
                        .with_child(Rect::new().with_background_color(theme::topbar_bg()).finish())
                        .with_child(strip.finish())
                        .finish()).finish())
                .with_child(ConstrainedBox::new(
                    Rect::new().with_background_color(theme::divider()).finish())
                    .with_height(1.0).finish())
                .finish(),
        ).with_height(theme::TAB_H).finish()
    }
```

- [ ] **Step 2: File pane's `pane_header` row 1 becomes plain pane chrome**

In `pane_header`, replace the `is_file_pane` title branch with the standard title path using `(icons::FILE, "Files".to_string())` — the strip no longer renders here. The row-1 ✕ keeps `CraneShellAction::ClosePane(id)` (this is the "separate pane closure").

- [ ] **Step 3: Stack the strip under the header at the pane assembly site**

Find where `pane_header(id, app)` is composed above the pane body (`rg -n "pane_header" src/warpui/shell.rs`). For the file pane insert the strip between header and body:

```rust
        let mut col = Flex::column().with_child(self.pane_header(id, app));
        if self.files_pane == Some(id) {
            col = col.with_child(self.file_tab_strip(app));
        }
        col = col.with_child(Expanded::new(1.0, body).finish());
```

(Adapt to the actual local structure — the key invariant: strip renders only for the File pane, between header and content, and content height shrinks accordingly since Flex handles it.)

- [ ] **Step 4: Build, visual check, commit**

`cargo build`; run: File pane shows "Files" header w/ ⛶ ✕ on row 1, tabs with underline + own ✕ on row 2; pane-✕ closes the whole pane, tab-✕ closes one tab; dirty dot renders; other panes unchanged.

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): file pane gets own header row with tab strip beneath"
```

### Task 4: Left Panel — two-tier selection, hover, 24px rows, footer Add Project

**Files:**
- Modify: `src/warpui/shell.rs` — left-tree row builders (anchor: `rg -n "fn tree_row|fn left_" src/warpui/shell.rs` and the project/worktree/tab/terminal row builders around `shell.rs:7100-7900`), the Add Project button, and the `PROJECTS` header.

**Interfaces:**
- Consumes: Task 1 washes.
- Produces: `fn row_shell(&self, key: &str, tier: RowTier, content_fn: impl Fn() -> Box<dyn Element> + 'static, action: CraneShellAction) -> Box<dyn Element>` plus `enum RowTier { Plain, Ancestor, Selected }`.

- [ ] **Step 1: Add `RowTier` + `row_shell` helper**

```rust
/// Visual tier of a Left-Panel row. Selected = the one active leaf;
/// Ancestor = its project/workspace/tab chain (context, not selection).
#[derive(Clone, Copy, PartialEq)]
enum RowTier { Plain, Ancestor, Selected }
```

```rust
    /// Shared Left-Panel row chrome: 24px tall, 4px side margins, rounded
    /// hover/selection washes, two-tier selection language.
    fn row_shell(&self, key: &str, tier: RowTier, content_fn: impl Fn() -> Box<dyn Element> + 'static, action: CraneShellAction) -> Box<dyn Element> {
        let state = self.hover_handle(&format!("lrow:{key}"));
        Box::new(
            Hoverable::new(state, move |ms| {
                let bg = match (tier, ms.is_hovered()) {
                    (RowTier::Selected, _) => theme::selection_wash(),
                    (_, true) => theme::hover_wash(),
                    (RowTier::Ancestor, false) => theme::context_wash(),
                    (RowTier::Plain, false) => ColorU::new(0, 0, 0, 0),
                };
                Container::new(
                    ConstrainedBox::new(content_fn())  // see note below
                        .with_height(24.0)
                        .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_margin_left(4.0)
                .with_margin_right(4.0)
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(action.clone());
            })
            .finish(),
        )
    }
```

Note: `Hoverable::new` closures re-run per frame, so `content` must be buildable per call — make the parameter `content_fn: impl Fn() -> Box<dyn Element> + 'static` (matches the existing Hoverable closure idiom used at shell.rs:2782). If `Container` lacks `with_margin_*` (check `rg -n "with_margin" vendor/warp/crates/warpui_core/src/elements/gui/container.rs`), wrap in an outer `Container` with `with_padding_left(4.0)/with_padding_right(4.0)` instead.

- [ ] **Step 2: Apply tiers in the tree walk**

In the left-tree build (project → worktree → tab → terminal rows): compute the selection chain from the existing active/selected state (`self.selected` for project index; active workspace/tab identify the chain; the focused terminal row is `RowTier::Selected`). Route every row's chrome through `row_shell`, passing existing row internals (chevron + icon + label + badges) as the content closure. **Text colors per tier:** Selected → `theme::text_hover()` for label AND icon; Ancestor → `theme::text()`; Plain → `theme::text_muted()`. Remove the old solid `row_active()` background usage for these rows.

- [ ] **Step 3: Indent guides**

In the same walk, for depth ≥ 1 prepend per-level guide columns to the row content: a 1px `theme::divider()` vertical rect inside a fixed 14px-wide spacer per depth level (replacing plain padding), so nesting reads as lines. Skip this if the row builders indent via a single left-padding value AND the change would touch >1 function per row type — in that case add one guide at the deepest level only. Do not regress row alignment.

- [ ] **Step 4: PROJECTS header ＋ and footer Add Project row**

- Header: after the "PROJECTS" label, `Expanded` spacer, then `self.icon_button("projects-add", icons::FOLDER_PLUS, <existing AddProject action>)` (find the action: `rg -n "Add Project" src/warpui/shell.rs`).
- Replace the boxed bottom button with a quiet row: 1px `divider()` top border, `icons::PLUS` + "Add Project" at 11px `text_muted()`, hover → `hover_wash()` + `text_hover()`; same dispatch action.

- [ ] **Step 5: Build, visual check, commit**

`cargo build`; run: hover any row = soft wash; selected terminal = brightest row w/ white text; its chain = whisper tint; other projects quiet; ＋ in header and footer row both open Add Project.

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): two-tier left panel selection with hover washes"
```

### Task 5: Right Panel underline tabs

**Files:**
- Modify: `src/warpui/shell.rs:7931-7948` (`fn tab_label`), `shell.rs:8129-8148` (tabs row in `right_sidebar`), `shell.rs:8234-8249` (`changes_tab_label`).

**Interfaces:**
- Consumes: Task 1 washes; same underline pattern as Task 3.
- Produces: restyled `tab_label` (keeps its signature `(&self, text: &'static str, active: bool, action: CraneShellAction)`).

- [ ] **Step 1: Restyle `tab_label`** — same structure as a file tab (Task 3 Step 1 chip): 26px tall, 12px horizontal padding, active = bright text + 2px accent underline, inactive = muted + hover wash, PointingHand. Key hover state as `format!("rtab:{text}")`. The loose-disabled Changes chip (in `changes_tab_label`) stays inert: muted `pane_dim()` text, no hover, no underline.

- [ ] **Step 2: Divider under the tab row** — in `right_sidebar`, follow the tabs container with a 1px `divider()` rect (same pattern as Task 2), and drop the old ad-hoc paddings (`with_padding_top(8.0)` → 0; the 26px chip centers itself).

- [ ] **Step 3: Build, visual check, commit**

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): right panel switcher as underline tabs"
```

### Task 6: Premium context menus

**Files:**
- Modify: `src/warpui/shell.rs:2774-2830` (`menu_item`, `menu_separator`), the menu container builders (anchor: `rg -n "context_menu|fn .*menu" src/warpui/shell.rs`, includes the folder-group menu at ~3416 and row menus at ~3054), swatch builders at shell.rs:2857/2935/3384.

**Interfaces:**
- Consumes: Task 1 washes.
- Produces: `fn menu_item_hint(&self, glyph, label, hint: Option<&str>, danger: bool, action) -> Box<dyn Element>`; `menu_item` becomes a thin wrapper (`hint: None, danger: false`). `fn menu_label(&self, text: &'static str) -> Box<dyn Element>` (small-caps section label).

- [ ] **Step 1: Menu container chrome** — every context-menu surface gets:

```rust
        Container::new(items_column)
            .with_background_color(theme::surface())
            .with_border(Border::all(1.0, theme::border()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_drop_shadow(DropShadow::new_with_standard_offset_and_spread(theme::menu_shadow()))
            .with_uniform_padding(5.0)
            .finish()
```

(Match the existing `Border` constructor: `rg -n "Border::" src/warpui/shell.rs` and copy that idiom.)

- [ ] **Step 2: `menu_item_hint`** — extend the existing `menu_item` body: item Container gains `with_corner_radius(…Pixels(5.0))`; hover bg = `selection_wash()` (or `danger_wash()` + `theme::error()` text when `danger`); after the label add `Expanded` spacer + optional hint `Text::new(hint, ui_font, 10.0).with_color(theme::text_muted())`. Add `fn menu_label`: 10px, `text_muted()`, `with_padding_left(9.0).with_padding_top(6.0).with_padding_bottom(3.0)`.

- [ ] **Step 3: Wire real hints + danger flags** — "Remove"/"Delete" items → `danger: true`; items with shortcuts (New Workspace etc. — cross-check the shortcuts table at shell.rs:4086) get their hint strings.

- [ ] **Step 4: Circular swatches** — at each swatch site (2857/2935/3384): size 18×18, `CornerRadius::with_all(Radius::Pixels(9.0))`, hover = 2px `theme::text_hover()` border (skip scale animation — no transform primitive), active = 2px white border, plus a hollow "none" swatch (transparent bg, 2px `border()` border) mapped to the existing clear-tint action. Group them under `menu_label("HIGHLIGHT")`.

- [ ] **Step 5: Build, visual check (right-click a project / folder group / right-panel row), commit**

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): rounded shadowed context menus with sectioned swatches"
```

### Task 7: Top bar — capsule breadcrumb + ＋ New Pane dropdown; theme pill removed

**Files:**
- Modify: `src/warpui/shell.rs:8816-8863` (`fn top_bar`), action enum (`CraneShellAction` at ~11495) + `handle_action` (~11937), modal/overlay dispatch if menus render as overlays (reuse the context-menu overlay plumbing — anchor `rg -n "context_menu.*Some|OpenContextMenu" src/warpui/shell.rs`).

**Interfaces:**
- Consumes: Task 6 menu chrome (`menu_item_hint`, `menu_label`).
- Produces: `CraneShellAction::ToggleNewPaneMenu`, state field `new_pane_menu_open: bool` (plus anchor position if the overlay system needs one); breadcrumb capsule dispatching `CraneShellAction::OpenSwitchBranch`.

- [ ] **Step 1: Breadcrumb capsule** — replace the plain `crumb` label: Hoverable capsule, 24px tall, `sidebar_bg()` fill, 1px `border()` border, `CornerRadius::with_all(Radius::Pixels(12.0))`, content = `icons::CUBE` + project name (11px `text()`) + `icons::GIT_BRANCH` in `accent()` + branch (11px `text_hover()`); hover = border color `accent()` @ 40% alpha (`ColorU { ..accent, a: 102 }` via a small local helper in theme.rs: `pub fn accent_soft() -> ColorU`); click → `OpenSwitchBranch`. Loose project (no branch): capsule shows project name only and is inert.

- [ ] **Step 2: ＋ New Pane button + menu** — remove the Terminal/Browser/theme `pill_button`s. Add one bordered 24px button "＋ New Pane" (`icons::PLUS` + label + `icons::CARET_DOWN` 9px) toggling `ToggleNewPaneMenu`. Menu content (Task 6 chrome), rendered through the same overlay path as context menus:

```rust
        Flex::column()
            .with_child(self.menu_item_hint(icons::TERMINAL_WINDOW, "Terminal", Some("⌘T"), false, CraneShellAction::SplitFocused(Dir::Horizontal)))
            .with_child(self.menu_item_hint(icons::GLOBE, "Browser", Some("⌘⇧B"), false, CraneShellAction::OpenBrowser))
            .with_child(self.menu_item_hint(icons::FILE, "File…", Some("⌘O"), false, /* existing Cmd+O open-file action — rg -n "\"Cmd\\+O\"" then trace its action */))
            .with_child(self.menu_separator())
            .with_child(self.menu_label("SPLIT"))
            .with_child(self.menu_item_hint(icons::SQUARE_SPLIT_HORIZONTAL, "Split right", Some("⌘D"), false, CraneShellAction::SplitFocused(Dir::Horizontal)))
            .with_child(self.menu_item_hint(icons::SQUARE_SPLIT_VERTICAL, "Split down", Some("⌘⇧D"), false, CraneShellAction::SplitFocused(Dir::Vertical)))
            .finish()
```

(Icon names: verify in `src/warpui/icons.rs`; substitute the closest existing glyphs — do NOT add raw Unicode.) Menu items already dispatch `CloseContextMenu`; make `ToggleNewPaneMenu` close on any item dispatch and on outside click, same as context menus.

- [ ] **Step 3: Top-lit gradient** — check for a gradient fill: `rg -n "Gradient|LinearGradient" vendor/warp/crates/warpui_core/src/scene.rs`. If present, apply vertical topbar gradient (topbar_bg lightened 4% → topbar_bg); if absent, fake it: 1px `ColorU{255,255,255,a:10}` rect pinned at the bar's top edge. Do not build a gradient primitive for this.

- [ ] **Step 4: Build, visual check, commit** — bar shows: toggle, capsule, spacer, ＋ New Pane, git log, toggle; each menu item works; theme pill gone (theme switching returns in Task 8).

```bash
git add src/warpui/shell.rs src/warpui/theme.rs
git commit -m "feat(warpui): top bar capsule breadcrumb and new-pane menu"
```

### Task 8: Status bar — live pulse + gear menu (theme picker moves here)

**Files:**
- Modify: `src/warpui/shell.rs:8865-8960` (`fn status_bar`), action enum + handler (`SetTheme` already exists), state field `gear_menu_open: bool`.

**Interfaces:**
- Consumes: Task 6 menu chrome; `self.changes` (dirty = non-empty), `crate::warpui::git::diff_numstat` cached totals already surfaced for the active workspace (reuse the same source the Left Panel badge uses — `rg -n "diff_stat" src/warpui/shell.rs`); `crate::theme::load_all()`.
- Produces: gear menu listing themes (replaces the removed top-bar cycle pill).

- [ ] **Step 1: Branch cluster + state dot + chip** — prepend an 7×7 dot (rounded rect, radius 3.5) to the branch cluster: `theme::success()` when `self.changes.is_empty()`, `theme::warning()` otherwise. After the cluster, when dirty, a chip (18px tall, `surface()` bg, radius 9): `+{added}` in `success()`, `−{deleted}` in `error()`, 10px font, from the active workspace's cached numstat. Chip + cluster share the existing `OpenSwitchBranch` click.
- [ ] **Step 2: Hover language** — wrap Ln/Col cluster and the new ⚙ button (far right, `self.icon_button("sb-gear", icons::GEAR /* verify glyph name in icons.rs */, CraneShellAction::ToggleGearMenu)`) with hover washes. Bar height uses `theme::STATUS_H` (26).
- [ ] **Step 3: Gear menu** — overlay menu (Task 6 chrome) anchored bottom-right: `menu_label("THEME")` + one `menu_item_hint(icons::PAINT_BRUSH, <name>, active-marker-as-hint, false, CraneShellAction::SetTheme(name))` per `crate::theme::load_all()` entry (active theme's hint = "active"), separator, "Keyboard Shortcuts" (dispatching the existing shortcuts-modal action — `rg -n "Keyboard Shortcuts" src/warpui/shell.rs`).
- [ ] **Step 4: Defer agent-activity indicator** — spec Open Item; do not implement now.
- [ ] **Step 5: Build, visual check, commit**

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): status bar repo pulse with gear theme menu"
```

### Task 9: Live +x/−y counters for every watched repo

**Files:**
- Modify: `src/warpui/shell.rs:6743-6812` (`fn drain_fs_events`)
- Test: `src/warpui/file_watcher.rs` (existing tests keep passing; no new unit test — the change is shell wiring, verified behaviorally)

**Interfaces:**
- Consumes: existing `self.fs_events`, `self.spawn_git_scan(ctx, generation, paths)` (see its use at shell.rs:6792), `self.projects[..].worktrees[..].path`.
- Produces: badge refresh for ANY repo the watcher reports, debounced per root.

- [ ] **Step 1: Track all touched roots, not just active** — in `drain_fs_events`, collect every distinct `ev.root` into a `Vec<PathBuf>` (`touched`). Keep the existing active-repo block untouched. After it, for non-active touched roots, map each to its worktree checkout paths (same canonicalize-compare as the active block) and spawn a scoped scan:

```rust
        // Refresh sidebar badges for NON-active repos too — an agent or a git op
        // in a background workspace should tick its +N/−M without a project switch.
        let mut bg_paths: Vec<PathBuf> = Vec::new();
        for root in &touched {
            if Some(root) == active_canon.as_ref() { continue; }
            bg_paths.extend(
                self.projects.iter().flat_map(|p| p.worktrees.iter())
                    .filter(|w| !w.path.is_empty()
                        && std::fs::canonicalize(&w.path).ok().as_deref() == Some(root.as_path()))
                    .map(|w| PathBuf::from(&w.path)),
            );
        }
        if !bg_paths.is_empty()
            && self.bg_badge_last_scan.elapsed() >= std::time::Duration::from_millis(500)
        {
            self.bg_badge_last_scan = std::time::Instant::now();
            self.spawn_git_scan(ctx, "bg-diff".to_string(), bg_paths);
        }
```

Add field `bg_badge_last_scan: std::time::Instant` (init `Instant::now()` in the constructor near `git_log_last_reload` — find with `rg -n "git_log_last_reload" src/warpui/shell.rs`). Confirm `spawn_git_scan` generations: the active block uses `"active-diff"`; verify a second generation string doesn't cancel it (read `fn spawn_git_scan`) — if generations are exclusive, reuse `"active-diff"` semantics by passing all paths in one call instead.

- [ ] **Step 2: Bust the numstat cache** — `src/warpui/projects.rs:70` caches numstat keyed by path. Read `cached_diff_numstat` and its invalidation; ensure `spawn_git_scan` results bypass/update that cache (if the scan writes `w.diff_stat` directly — see shell.rs:6549 — nothing to do).

- [ ] **Step 3: Verify behaviorally + commit** — run Crane with two projects; `touch`/edit a file in the NON-active project's checkout from another terminal; its +N/−M badge updates within ~1s without switching projects. `make test` passes (file_watcher tests unaffected).

```bash
git add src/warpui/shell.rs src/warpui/projects.rs
git commit -m "fix(warpui): background workspaces refresh change badges live"
```

### Task 10: Notification pulse at full frame rate

**Files:**
- Modify: `src/warpui/shell.rs:1352-1387` (fast tick), `~1392` (browser ticker block, as the pattern to copy)

**Interfaces:**
- Consumes: `self.any_attention_active()`, `self.toasts`, `warpui::r#async::Timer::interval`, `guarded_tick`.
- Produces: a 33ms animation ticker that notifies only while an animation is live.

- [ ] **Step 1: Add the anim ticker** — next to the browser ticker creation, add:

```rust
        // Animation tick — 33ms while a toast or attention pulse is on screen.
        // The 250ms fast tick owns lifecycle (expiry, drain); this one only
        // repaints so the glow/dot breathe at full rate instead of 4 FPS.
        let anim_ticker =
            warpui::r#async::Timer::interval(std::time::Duration::from_millis(33));
        let anim_tick = ctx.spawn_stream_local(
            anim_ticker,
            |this: &mut Self, _instant, vctx| {
                guarded_tick(this, vctx, "anim", |this, vctx| {
                    if !this.toasts.is_empty() || this.any_attention_active() {
                        vctx.notify();
                    }
                });
            },
            |_this, _vctx| {},
        );
```

Store/leak the returned task handle exactly the way `browser_tick`'s is handled (see what happens to `browser_tick` a few lines below — mirror it). Remove the `vctx.notify()` responsibility from the fast tick's animation condition (keep its expiry sweep + the one final notify when `toasts.len() != before`).

- [ ] **Step 2: Verify + commit** — trigger a notification (background terminal bell — e.g. `sleep 1 && printf '\a'` in a non-focused pane's shell); sidebar glow breathes smoothly. Idle CPU unchanged when no animation is active (the tick is a no-op check).

```bash
git add src/warpui/shell.rs
git commit -m "fix(warpui): notification pulse animates at full frame rate"
```

### Task 11: ⌘⇧B Browser shortcut + shortcuts-modal entries

**Files:**
- Modify: `src/warpui/shell.rs` — key handling (find the ⌘T/⌘D handling: `rg -n "SplitFocused|key_char.*'t'|Cmd\\+T" src/warpui/shell.rs`), shortcuts table at shell.rs:4086.

**Interfaces:**
- Consumes: `CraneShellAction::OpenBrowser` (exists — used by the old Browser pill).
- Produces: ⌘⇧B → `OpenBrowser`; table rows for ⌘⇧B; ＋ New Pane menu hints (Task 7) now truthful.

- [ ] **Step 1: Bind ⌘⇧B** — locate the keydown match arm handling Cmd+Shift+T vs Cmd+T (same modifier pattern); add the `'b'`+shift+cmd arm dispatching `OpenBrowser`. Confirm plain ⌘B (ToggleLeft) is untouched — shift must be checked first, mirroring the T arms.
- [ ] **Step 2: Shortcuts modal** — add `("Cmd+Shift+B", "Split active pane with a browser")` to the table (⌘O already listed at 4086).
- [ ] **Step 3: Build, verify both ⌘B and ⌘⇧B, `make test`, commit**

```bash
git add src/warpui/shell.rs
git commit -m "feat(warpui): Cmd+Shift+B opens a browser pane"
```

### Task 12: Final sweep + release

**Files:** none new.

- [ ] **Step 1: Consistency pass** — `rg -n "with_uniform_padding\(5.0\)|15\.0, theme::text_muted" src/warpui/shell.rs`: any surviving old-style icon button (e.g. `status_icon_button`) migrates to Task 1's `icon_button`. `rg -n "row_active\(\)" src/warpui/shell.rs`: confirm remaining uses are intentional (non-left-panel).
- [ ] **Step 2: Full verify** — `cargo build --release && make test`; manual pass over every touched region with the spec §1–§9 as checklist.
- [ ] **Step 3: Ship** — working tree must be clean (all tasks committed). Then per project rules: `make ship-minor` (user-noticeable feature set → 0.6.0).
