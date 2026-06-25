# Crane: egui → warpui Structural Migration Plan

> Generated from a multi-agent inventory of the egui UI + theme system.
> Constraint: keep model/state/theme/Layout-Pane tree and shortcuts 1:1; rewrite only the rendering layer (egui → warpui), preserving the same panels, layout, and visual design.

I have what I need. The codebase confirms the inventories: `ui()` override at main.rs:324, `persist_window: true`, the two multiline-TextEdit sites (`file_view.rs` is the real editor, `explorer.rs` is the commit box), and the full src tree. The fourth inventory ("doc-panes") is a stub, so I'll treat doc-panes / views from the actual tree rather than the placeholder data. Here is the master plan.

---

# Crane egui → warpui Structural Port Plan

**Constraint:** Keep model/state/theme/Layout-Pane tree and keyboard shortcuts 1:1. Rewrite only the rendering layer (egui draw calls → warpui Views/Elements). Same panels, same layout math intent, same visual design.

**One-line truth up front:** ~70% of the codebase (state, theme data, git, lsp, terminal backend, session, settings, jobs) ports unchanged. The render layer (`main.rs`, all of `src/ui/`, all of `src/views/`, modals, `startup::apply_style`, the terminal grid painter) is a near-total rewrite — but a *render* rewrite, not a *behavior* rewrite. The deferred-action / return-an-enum pattern that egui forced becomes warpui message handlers and is *simpler*.

---

## 1. File-by-file verdict table

Verdicts: **reuse** (compiles ~unchanged), **decouple** (reuse after stripping a thin egui leak), **rewrite-retained** (logic kept, render rewritten), **new**.

### Framework-agnostic core — reuse / decouple

| File | Verdict | warpui equivalent |
|---|---|---|
| `src/state/mod.rs` | reuse | unchanged (re-exports) |
| `src/state/state.rs` (~3300 ln) | decouple | unchanged data model; replace the dozen `&egui::Context` args (used only for repaint) with a `Wake` trait handle; `git_op_status` parking_lot Arc stays |
| `src/state/layout.rs` | decouple | Node/Pane/PaneContent reuse; move `FileTab::image_texture: egui::TextureHandle` out of the model into a view-side cache keyed by path |
| `src/state/session.rs` | decouple | reuse; swap the one `restore(ctx)` repaint arg for the `Wake` handle |
| `src/state/settings.rs` | reuse | unchanged (serde) |
| `src/state/project_cache.rs` | reuse | unchanged |
| `src/git.rs` | reuse | unchanged (shells out to `git`) |
| `src/git_log/{data,graph,refresh,refs,state}.rs` | reuse | unchanged (data/model) |
| `src/git_log/view/*` | rewrite-retained | log/details/refs renderers → warpui Views |
| `src/lsp/*` | reuse | unchanged (protocol/server/downloader) |
| `src/jobs/*` | reuse | unchanged (`std::thread` JobSystem) |
| `src/file_watcher.rs`, `src/dir_cache.rs`, `src/util.rs` | reuse | unchanged |
| `src/format/mod.rs` | reuse | unchanged (formatter shell-out) |
| `src/browser/{mod,memory}.rs` | decouple | reuse; `sync()` takes `eframe::Frame` for the NSWindow handle → swap for warpui's raw-window-handle accessor |
| `src/update/*` | decouple | reuse; `spawn_check(ctx)` repaint arg → `Wake` |
| `src/platform_menu.rs`, `src/mac_keys.rs` | reuse | OS-level (NSMenu / NSEvent monitors); queue into App fields the shell drains. Unchanged |
| `src/startup.rs` (path/icon/fonts) | decouple | `fix_path_for_gui_launch` reuse; `load_app_icon` returns egui `IconData` → return raw RGBA for warpui icon API; `load_fonts` → warpui font registration |

### Theme — data reuse, binding rewrite

| File | Verdict | warpui equivalent |
|---|---|---|
| `src/theme.rs` data (`Rgb`, `Theme`, 21 builtins, globals, TOML load) | reuse | unchanged; only `Rgb::to_color32()` becomes `Rgb::to_warp()` (one shim, §3) |
| `src/theme.rs` `is_dark()/diff_*()` | rewrite-retained | logic 1:1; return type → warpui color |
| `src/startup.rs::apply_style()` | rewrite-retained | the only nontrivial theme rewrite: egui `Visuals/Style` → warpui per-Element style defaults (§3) |

### Render layer — rewrite-retained

| File | Verdict | warpui equivalent |
|---|---|---|
| `src/main.rs` (1121 ln) | rewrite-retained | warpui `AppBuilder` + root `CraneView` (§2). Composition order, modal flags, autosave all preserved |
| `src/shortcuts.rs` | rewrite-retained | same chord→mutation table on warpui key dispatch; preserve `consume_key` semantics so terminal panes don't double-receive |
| `src/ui/mod.rs` | reuse | module decls |
| `src/ui/util.rs` — `draw_row`/`draw_trailing` | rewrite-retained | the crux row primitive → one custom-painted warpui Element (§4). Color-token accessors reuse |
| `src/ui/projects.rs` (~1402 ln) | rewrite-retained | tree-walk + deferred-dispatch + DnD scope logic reuse; ScrollArea/clip/painter render rewritten |
| `src/ui/explorer.rs` (~1802 ln) | rewrite-retained | git stage/diff + FS move/copy logic reuse; rows/footer/commit-box rewritten |
| `src/ui/top.rs` | rewrite-retained | 34px bar → horizontal Flex |
| `src/ui/pane_view.rs` (~620 ln) | rewrite-retained | PaneAction enum + split_rect/dock_zone geometry reuse; split tree → nested Flex, splitters → drag Elements |
| `src/ui/status.rs` | rewrite-retained | branch-picker job dispatch reuse; bar paint → Flex row |
| `src/ui/branch_picker.rs` | rewrite-retained | popup list → warpui popup |
| `src/modals/*` (all) | rewrite-retained | each `egui::Window` → conditional overlay View driven by the same App flag |
| `src/views/file_view.rs` + `file_find/file_save/file_status/file_util/highlight.rs` | rewrite-retained → **highest risk** | the multiline editor (§5). syntect highlighting reuse; the egui `TextEdit::multiline` editor has no warpui drop-in |
| `src/views/diff_view.rs` | rewrite-retained | similar TextDiff reuse; render rewritten |
| `src/views/markdown_view.rs` | rewrite-retained | pulldown-cmark parse reuse; RichText → warpui text runs |
| `src/views/browser_view.rs` | rewrite-retained | WKWebView overlay reconciliation; placeholder + overlay-rect math |
| `src/views/pdf_view.rs`, `welcome_view.rs`, `file_find.rs`, `diagnostics_overlay.rs` | rewrite-retained | painter → Elements |
| `src/views/mod.rs`, `src/views/file_util.rs` | reuse/decouple | helpers; decouple any Color32 returns |
| `src/terminal/term.rs`, `crates/crane_term/*` | reuse | PTY/grid/parser unchanged |
| `src/terminal/view.rs` grid painter | rewrite-retained | **already done** (terminal pane ported first); 256-color ANSI `palette()` is pure data, reuse |
| `src/terminal/gpu_render.rs` | decouple | takes `eframe::Frame` for wgpu device → swap for warpui RenderState accessor |

### New files

| File | Why |
|---|---|
| `src/warp_app.rs` | the `AppBuilder` shell replacing `main.rs` body |
| `src/theme_warp.rs` (or inline in theme.rs) | the `Rgb → ColorU/Fill` shim + `apply_style` warpui equivalent |
| `src/wake.rs` | `Wake` trait abstracting `request_repaint` so state/session/update/browser stop importing egui |
| `src/ui/row.rs` | the ported `draw_row` as a reusable warpui row Element |
| `src/views/editor/*` | the new multiline text editor (rope buffer + line layout + caret) — see §5 |

---

## 2. warpui App shell (replaces eframe/main.rs)

### Window setup (replaces `run_native` + `ViewportBuilder`)
```
AppBuilder::new("Crane")
  .with_inner_size(1480, 920)
  .with_min_inner_size(800, 500)
  .with_title("Crane")
  .with_icon(load_app_icon_rgba())      // startup::load_app_icon → raw RGBA
  .with_window_persistence(crane_window_geometry())  // eframe gave this free; we persist size/pos ourselves
```
- **Heartbeat:** `request_repaint_after(1500ms)` → warpui timer/wake on the `Wake` handle.
- **OS menus / mac_keys:** `platform_menu::install()` + `mac_keys::install_cmd_v_monitor()` port unchanged; they queue into App fields the shell drains each frame.
- **Persistence gotcha:** eframe's `persist_window: true` (ron) is gone — the shell must serialize size/pos to `~/.crane/` itself.

### Composition — the biggest structural shift
Crane today does **not** use egui panels. The `ui()` override (main.rs:324) hands it a bare frameless root Ui, and it places Left/Right/Center/StatusBar by absolute `Rect` math + `new_child(max_rect)+set_clip_rect`. **Do not port the Rect arithmetic** — express the same layout as a retained tree:

```
CraneView (root)
└─ VStack[fill]
   ├─ HStack[flex]                         // the three columns
   │  ├─ LeftSidebar     (width=left_panel_w, shown if show_left)   → ui::projects
   │  ├─ Splitter        (drag → left_panel_w, clamp 180..45%)
   │  ├─ CenterStack[flex=1]
   │  │  ├─ TopBar       (height = ui::top::TOTAL_H)                → ui::top
   │  │  ├─ LayoutCanvas[flex=1]            → pane_view::render_layout (the Node tree)
   │  │  └─ GitLogDock   (optional, height=dock_h, OUTSIDE Layout)  // sibling row, NOT a pane
   │  ├─ Splitter        (drag → right_panel_w, clamp 200..50%)
   │  └─ RightSidebar    (width=right_panel_w, shown if show_right) → ui::explorer
   └─ StatusBar          (height = ui::status::HEIGHT)              → ui::status
```
- Sidebars collapsible (`show_left`/`show_right`) and resizable via real Splitter Elements writing the same width fields with the same clamps.
- **Critical: git-log dock stays a sibling row in CenterStack, never a Pane in the Node tree** — same as today.
- The pervasive `new_child+set_clip_rect` → Flex children with overflow clip. The painter bg/divider fills → Rect Elements / View borders in a divider-color token.

### Per-frame composition order — preserve exactly
The 18-step `ui()` body order is load-bearing (menu drain → ensure_initial/poll → close-guard → tab-switcher-keys-before-generic-shortcuts → reap dead panes → browser pump → **layout** → modal gate → pane_view → git-log effects → modals/toasts last → deferred queues → status bar → browser sync → autosave). In warpui this splits cleanly:
- **Pre-layout side effects** (menu drain, polls, close-guard, shortcuts, reap, browser pump) → an `on_frame`/update hook that runs before the View pass.
- **Layout** → the View tree above.
- **Post-layout** (git-log effects, deferred `save_queue`/`goto_queue`, browser sync, `maybe_save`) → message handlers + an end-of-frame hook.

### The action pattern gets *better*
Today `pane_view::render_layout` returns a `PaneAction` enum and the shell mutates App after — an immediate-mode dance to dodge the mutable borrow, with `RefCell` queues (`save_queue`, `goto_queue`, `diag_fn`, `notify_saved`) as workarounds. In warpui, pane Views **emit messages** (`Focus/Close/ResizeSplit/SwapPanes/DockPane/ToggleMaximize/Replace*/OpenFile/OpenFileExternal`) that the shell applies to App in a normal handler. **Fold the RefCell queues back into ordinary message handlers** — they exist only because egui renders inline while App is mutably borrowed; the retained model removes the need.

### Modals — keep the flag-driven approach, no modal stack
Each modal is still a bool/`Option` field on App (`show_settings`, `show_help`, `new_workspace_modal`, `find_in_files`, `pending_*`, `pending_quit_modal`, `tab_switcher`, `missing_project_modals`). Render them as a **conditional overlay layer** rendered last (z-above center), anchored center. The center `ui.disable()` gate → make underlying Views non-interactive while any overlay is up (`modal_open`). Esc-to-cancel stays per-modal input handling. Toasts → transient overlay Elements with TTL. **Do not introduce a modal stack** — the user wants the structure identical.

### wgpu / browser hooks
`gpu_render::ensure_initialized(frame)` and `browser_host.sync(frame, …)` reach the wgpu device + raw NSWindow via `eframe::Frame`. warpui's frame/render-context equivalent must expose the wgpu `RenderState` and the raw window handle so the terminal GPU pipeline and WKWebView overlay keep working. `overlay_visible` ("hide webviews when any egui overlay paints") → "hide when any warpui overlay layer is visible."

---

## 3. Theme port — data reused unchanged, one conversion shim at the leaf

**Layer 1 (data, verbatim):** `Rgb{r,g,b}` and the 25-field `Theme` struct, all 21 builtin constructors' RGB literals, the `RwLock<Option<Theme>>` global with `init/set/current(fallback=dark)`, and TOML load/save (`~/.crane/themes/*.toml`) port **unchanged**. Keep the serde shape so existing on-disk theme files stay compatible. `is_dark()` (luminance `0.299r+0.587g+0.114b<128`) and `diff_added/modified/deleted()` (6 hardcoded RGBs) stay as functions, not stored fields.

**Layer 2 (binding, the only rewrite):** replace the single egui coupling
```rust
impl Rgb { pub fn to_color32(self) -> egui::Color32 { Color32::from_rgb(self.r,self.g,self.b) } }
```
with one warpui shim
```rust
impl Rgb { pub fn to_warp(self) -> warpui::ColorU { warpui::ColorU::rgb(self.r,self.g,self.b) } }
```
Then the convenience accessors in `ui/util.rs` / `ui/top.rs` (`text()/muted()/accent()/row_hover()/row_active()/header_fg()/trailing_hover()`) are the **seam** — re-point those at `to_warp()` and the hundreds of call sites need no change.

**`apply_style()` rewrite** — reproduce the token→state mapping as warpui per-Element style defaults (preserve the *intent*, not egui field names):
- `inactive = surface / border / text` · `hovered = surface_alt / border_strong / text_hover` · `active = surface_hi / border_strong / text_hover`
- `selection.bg = accent @ alpha 70`, `selection.stroke = accent`
- `window_fill = surface`, `panel_fill/extreme_bg = bg`, `code_bg = surface`, `faint_bg = row_hover`, `override_text = text`
- corner radii 6 (widgets) / 10 (window) / 8 (menu); button_padding (10,5); item_spacing (8,5)

**Theme gotchas to preserve:**
1. **Selection fallback:** if `Theme.selection == Rgb(0,0,0)` (old files), derive `accent @ alpha 72` — keep this branch in `selection_bg` or selection renders black/transparent.
2. **Light/dark base** must use the same luminance predicate everywhere so diff markers match.
3. `diff_*` are computed, not tokens.
4. **Terminal ANSI 256-palette is independent of Theme** — only `terminal_bg/fg/selection` feed from Theme. The 16 named ANSI literals + the 6×6×6 cube + grayscale ramp in `terminal/view.rs::palette()` port as fixed data; don't assume warpui's terminal palette matches.
5. `syntax_theme` is an opaque string naming a two_face `EmbeddedThemeName` — not a color; the highlighter resolves it, with a bg-brightness fallback.
6. `themes_dir()` is `~/.crane/themes` (the `~/.config/crane` doc comment is stale).

---

## 4. egui → warpui mapping cheat-sheet

| egui pattern | warpui equivalent | Notes |
|---|---|---|
| `ui.label(text)` | `Text` Element | use `Label::sense(click)` sites → clickable Text with EventHandler |
| `egui::Painter` (rect_filled / line_segment / text / galley / circle) | custom-painted Element / Stack of Rect+Text Elements | every chrome line is a painted primitive today → 1px Rect Elements or View borders |
| `ScrollArea::vertical().id_salt(..)` | `scrollable` View wrapping the row list | one per tree; **`id_salt` → n/a** (retained tree has stable identity) |
| `ui.new_child(max_rect)+set_clip_rect` | Flex/Stack child with overflow clip | **drop the Rect math entirely** |
| `SidePanel/CentralPanel/TopBottomPanel` | *(unused today)* → HStack/VStack Flex + Stack | Crane never used panels; manual rects → Flex |
| `TextEdit::singleline` | `TextField` with focus-on-mount | inline rename / new-entry / branch filter |
| `TextEdit::multiline` (file editor) | **no drop-in — custom editor** | see §5 — the hardest item |
| `Button::new(...)` | EventHandler + Rect + Text Element | `.frame(false)`/`visuals_mut()` overrides → just style the Element |
| `Sense::click_and_drag` / `ui.interact(rect,id,sense)` | EventHandler on the Element | splitters, header buttons, checkbox regions |
| `Response::context_menu` | warpui context-menu/popup attached to the Element | color-swatch buttons → swatch Elements |
| `egui::Window(anchor CENTER_CENTER)` | conditional overlay View, z-above center | driven by the same App bool/Option flag |
| `ui.disable()` | make underlying Views non-interactive while overlay up | the `modal_open` gate |
| `DragAndDrop` payload / `dnd_release_payload` | warpui drag model: drag-source + per-row drop-target | three flows: tree reorder, pane dock, FS move/copy |
| `Area::order(Tooltip)` floating drag chip | overlay/portal Element positioned at pointer | FS drag chip |
| `animate_bool_with_time(id,b,secs)` | warpui transition driver, or per-row anim state keyed by node id + repaint-while-active | hover 0.09, chevron 0.11, checkbox 0.13/0.08, attention breathing |
| `ctx.set_cursor_icon(...)` | warpui cursor API | PointingHand/Grab/Resize/Copy |
| `ui.push_id((key,id),..)` | **n/a** — but key row identity by node id | needed for anim/focus/drag state, not id-collision |
| `memory-gated one-shot request_focus` | focus-on-mount on the TextField | the per-frame-focus-steals-clicks rule disappears |
| `RichText::new(s).color(c)` | Text with style/color | |
| egui_phosphor glyphs | **keep the phosphor font; render as Text** | never substitute unicode (CLAUDE.md) |

---

## 5. Ordered execution phases (with risks)

**Phase 0 — decoupling pass (prereq, no warpui yet).** Land on a branch while still building against egui:
- Introduce `Wake` trait; thread it through `state.rs`/`session.rs`/`update`/`browser` replacing the `&egui::Context`-for-repaint args.
- Move `FileTab::image_texture` out of `layout.rs` into a view-side cache keyed by path.
- Add `Rgb::to_warp()` alongside `to_color32()`.
- Expose wgpu `RenderState` + raw window handle behind a small accessor trait so `gpu_render`/`browser` stop needing `eframe::Frame`.
*Risk: low. This is the safe, mergeable groundwork that shrinks every later phase.*

**Phase 1 — shell + theme (warpui boots, blank panels).** Build `warp_app.rs` `AppBuilder`, root `CraneView`, the Flex composition (Left/Right/Center/StatusBar + splitters), the modal overlay layer (flag-driven), `apply_style` warpui version, autosave hook, OS-menu/mac_keys drain, wgpu+browser frame hooks. Panels render placeholder content.
*Risk: medium. Window persistence (no more free eframe `persist_window`), and verifying wgpu RenderState + NSWindow handle reach the terminal pipeline and WKWebView.*

**Phase 2 — terminal pane.** *(stated already done)* Validate the grid painter + 256-color palette + selection under the new shell; it's the proof the GPU path works.

**Phase 3 — panels (projects / explorer / top / status / pane_view).** Port `draw_row` to `ui/row.rs` first (it's the crux — every tree depends on it), then the three trees, the split-tree renderer + splitters, and the three DnD flows. Reuse all the deferred-dispatch/scope/validation logic; rewrite render.
*Risks: (a) the `draw_row` animation set — if warpui lacks per-widget eased bools, drive per-row anim state keyed by node id + repaint-while-active (the existing `pulse_animating` gate already does this). (b) DnD: verify hover/drop-target detection works mid-drag — the egui `pointer.hover_pos` workaround may be unnecessary, but confirm. (c) `dock_zone`/`zone_rect` 5-region geometry ports verbatim; the dim-overlay focus (translucent black alpha 45 on inactive panes, NOT a border, painted last) must stay Warp-style. (d) splitter `ratio = (pointer-parent.min)/parent.size` math reused exactly.*

**Phase 4 — doc panes (markdown / diff / browser / pdf / welcome / git-log views).** Parse logic (pulldown-cmark, similar, syntect) reuses; render rewritten. Browser pane keeps the WKWebView overlay reconciliation.
*Risk: medium — browser overlay rect math under the new layout; markdown RichText → warpui text runs.*

**Phase 5 — the file editor (`views/file_view.rs`). HIGHEST RISK — call it out loud.**
egui's `TextEdit::multiline` is a full editor: undo/redo, cut/copy/paste, find/replace, goto-line, multi-line selection, IME, scroll-into-view. **warpui has no drop-in multiline editor.** This is the single largest unknown in the migration. Approach, in order of preference:

1. **If warpui ships a multiline text input** — use it, wire syntect highlighting as styled runs, re-implement find/replace/goto on top. Smallest effort; verify it supports large files, selection, undo, IME.
2. **If not — build a minimal editor View** over a rope buffer (`ropey`):
   - Model: rope + caret(s) + selection range + per-line cached syntect highlight (already have the highlighter from `highlight.rs`).
   - Layout: line-based; only lay out/paint visible lines (virtualized) — large files were fine in egui because it virtualized; match that.
   - Caret/selection: painted Rects; mouse hit-test → byte offset via line layout; arrow/word/line nav.
   - Editing: insert/delete/replace on the rope; an undo stack (egui gave this free — now explicit).
   - Reuse `file_find.rs` (find), `file_save.rs` (save + LSP didSave), `file_status.rs`, goto-line.
   - IME for non-ASCII input is the nastiest sub-risk — scope it explicitly.
3. **Fallback / phasing:** ship Phase 5 first as **read-only syntax-highlighted view** (trivial: painted highlighted lines + scroll) to unblock the rest of the migration, then iterate editing in. This keeps the app usable while the editor matures.

*Out-of-scope per CLAUDE.md stays out: no Vim mode, no LSP autocomplete, no custom GPU text backend — but full editing (find/replace, goto, undo, cut/copy/paste) must be preserved, so option 2/3 must reach feature parity with today's `TextEdit::multiline`.*

**Phase 6 — modals + toasts + shortcuts cleanup + teardown.** All `egui::Window` modals → overlay Views (mechanical once Phase 1's overlay layer exists). Port `shortcuts::handle` chord table preserving consume semantics. `on_exit` (`jobs.shutdown()` + drop watcher) → warpui shutdown hook. Delete egui deps last.
*Risk: low, but the shortcut consume semantics matter — terminal panes must not double-receive Cmd-chords.*

---

## 6. What CANNOT be a literal 1:1 swap (immediate vs retained)

"Same structure" here means **same layout and behavior, rewritten render code** — not the same API calls. Explicitly:

1. **The entire render layer.** egui draw calls (`Painter`, `allocate_exact_size`, `interact`, `Window`, `ScrollArea`, `TextEdit`) have no line-for-line warpui twin. Every `src/ui/*`, `src/views/*`, `src/modals/*`, and the `main.rs` body is rewritten. Logic underneath survives; the draw calls do not.

2. **Manual Rect composition → retained Flex tree.** Crane's hand-rolled absolute-Rect layout (`available_rect_before_wrap` + `new_child(max_rect)` + `set_clip_rect` + painter fills) is an immediate-mode idiom. It becomes a declarative Flex/Stack tree. The *visual result* is identical; the *mechanism* is the opposite paradigm. The Rect math is deleted, not translated.

3. **The PaneAction return-then-mutate pattern + RefCell deferred queues.** These exist **only** because egui renders inline while App is mutably borrowed (`save_queue`, `goto_queue`, `diag_fn`, `notify_saved`, `format_before_save`). In retained warpui, Views emit messages and the shell handles them in normal handlers — the borrow dance is gone. Same outcomes, different (simpler) control flow. You cannot keep the closures verbatim.

4. **`draw_row`** — one `allocate_exact_size` hit-rect + ~12 painter draws → a custom-painted Element with real child Elements for trailing buttons. The egui trick of separate `ui.interact` hit-rects on the right edge is dropped; warpui hangs real child Elements off the row.

5. **Animations.** `animate_bool_with_time` is an egui-managed per-id eased bool. warpui needs its own transition driver or per-row anim state keyed by node id with repaint-while-active. Not a function-name swap.

6. **`id_salt` / `push_id`.** egui's ID-collision defense (the red-flash guard). In a retained tree, identity is structural — these largely **disappear**. But row identity must still be keyed by node id for animation/focus/drag state.

7. **One-shot `request_focus` memory gate.** The "request focus once, not every frame, or it steals sibling clicks" rule (CLAUDE.md) is an immediate-mode hazard. warpui TextFields focus-on-mount; the whole memory-flag dance vanishes. You cannot port the gate; you delete it.

8. **`TextEdit::multiline` (file editor).** No drop-in exists. Must be rebuilt (§5). This is where "same structure" most clearly means "same feature set, entirely new implementation."

9. **`eframe::App::ui()` override + `persist_window`.** eframe's free frameless-CentralPanel wrapping and ron window persistence are eframe conveniences with no warpui equivalent — the shell reproduces both manually (bare root composition; self-managed geometry persistence).

10. **`eframe::Frame`-threaded wgpu/native-handle access.** The frame object that today reaches the wgpu device and NSWindow is eframe-specific; warpui's render-context must expose `RenderState` + raw window handle through a different (rewritten) accessor for the terminal GPU pipeline and WKWebView overlay.

**Everything else** — the App god-struct, Project/Workspace/Tab/Layout/Node/Pane tree, focus triple + `Layout.focus`, all 21 themes as data, git/lsp/jobs/terminal-backend/session/settings, the keyboard *bindings*, and the autosave logic — is genuine reuse. That's the ~70% that makes "keep the structure identical" achievable: the model and design are preserved exactly; only the paint is rewritten.

---

**Relevant absolute paths for the team:**
- Shell to rewrite: `/Users/rajpootathar/ideaProjects/crane/src/main.rs`, `/Users/rajpootathar/ideaProjects/crane/src/shortcuts.rs`
- Theme: `/Users/rajpootathar/ideaProjects/crane/src/theme.rs` (data reuse), `/Users/rajpootathar/ideaProjects/crane/src/startup.rs` (`apply_style` rewrite)
- Panels: `/Users/rajpootathar/ideaProjects/crane/src/ui/{util,projects,explorer,top,pane_view,status,branch_picker}.rs`
- The hard editor: `/Users/rajpootathar/ideaProjects/crane/src/views/file_view.rs` (+ `file_find/file_save/file_status/highlight.rs`)
- Decouple targets: `/Users/rajpootathar/ideaProjects/crane/src/state/{state,layout,session}.rs`, `/Users/rajpootathar/ideaProjects/crane/src/terminal/gpu_render.rs`, `/Users/rajpootathar/ideaProjects/crane/src/browser/mod.rs`

Note: the "doc-panes" inventory entry was a placeholder stub; I planned Phase 4 from the actual `src/views/` tree (`markdown_view`, `diff_view`, `browser_view`, `pdf_view`, `welcome_view`, `git_log/view/*`) instead.