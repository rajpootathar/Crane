//! `WarpEditorView` — the file editor pane backed by Warp's REAL text editor
//! (`warp_editor`), so the file pane is warp-quality (cursor, click, wrap,
//! selection, undo) instead of the hand-rolled `FileView`. This is the v1
//! single-file editor; multi-tab wraps several of these later.

use std::sync::Arc;

use string_offset::CharOffset;
use warp_editor::content::buffer::{Buffer, BufferEvent};
use warp_editor::content::selection_model::BufferSelectionModel;
use warp_editor::content::text::{IndentBehavior, IndentUnit, TextStyles};
use warp_editor::editor::{EditorView, EmbeddedItemModel, RunnableCommandModel};
use warp_editor::model::{CoreEditorModel, PlainTextEditorModel};
use warp_editor::render::element::{
    DisplayOptions, DisplayState, DisplayStateHandle, RichTextAction, RichTextElement,
};
use warp_editor::render::model::{
    AutoScrollMode, BrokenLinkStyle, CheckBoxStyle, Decoration, HorizontalRuleStyle, InlineCodeStyle,
    Location, ParagraphStyles, RenderState, RichTextStyles, TableStyle, WidthSetting,
    DEFAULT_BLOCK_SPACINGS, PARAGRAPH_MIN_HEIGHT,
};
use warp_editor::selection::{SelectionModel, TextDirection, TextUnit};
use warpui::text::word_boundaries::WordBoundariesPolicy;
use warpui::color::ColorU;
use warpui::elements::{
    Axis, Border, Container, DispatchEventResult, Element, EventHandler, Expanded, Fill, Flex,
    ParentElement, Text, ZIndex,
};
use warpui::platform::Cursor;
use warpui::event::ModifiersState;
use warpui::fonts::{FamilyId, Weight};
use warpui::units::Pixels;
use warpui::{
    AppContext, Entity, ModelHandle, TypedActionView, View, ViewContext, WeakViewHandle,
};

use rangemap::RangeMap;

use crate::warpui::find_bar_element::{BarMode, FindBarElement};
use crate::warpui::gutter_element::DiffKind;
use crate::warpui::theme;

const BASELINE: f32 = 0.78;

/// How often `render` is allowed to re-stat the file for the external-change
/// reload banner. `render` runs every paint frame; hitting the filesystem that
/// often is wasteful, so the stat result is cached and refreshed at most once
/// per this interval.
const DISK_STAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

/// The syntect theme to render with — mirrors the egui file view: honor the
/// user's configured `syntax_theme`, else a sensible dark default. NOT the empty
/// `fallback_theme()` (which paints near-black text that clashes with the UI).
fn render_theme() -> &'static syntect::highlighting::Theme {
    let all = &crate::syntax::themes().themes;
    // Settings > Appearance override wins; otherwise pair with the UI theme.
    let requested = crate::syntax::theme_override()
        .unwrap_or_else(|| crate::theme::current().syntax_theme.clone());
    all.get(&requested)
        .or_else(|| all.get("OneHalfDark"))
        .or_else(|| all.get("base16-eighties.dark"))
        .or_else(|| all.get("base16-ocean.dark"))
        .unwrap_or_else(|| {
            all.values()
                .next()
                .unwrap_or_else(|| crate::syntax::fallback_theme())
        })
}

/// Syntect-highlight `content` into a CharOffset→color map for warp's editor
/// `text_decorations`. Reuses the egui app's shared SyntaxSet + theme.
fn highlight(content: &str, path: &std::path::Path) -> RangeMap<CharOffset, ColorU> {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;
    let ss = crate::syntax::syntaxes();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| ss.find_syntax_by_extension(e))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, render_theme());
    let mut map = RangeMap::new();
    let mut off: usize = 0;
    for line in LinesWithEndings::from(content) {
        let Ok(spans) = hl.highlight_line(line, ss) else {
            off += line.chars().count();
            continue;
        };
        for (style, text) in spans {
            let len = text.chars().count();
            if len > 0 {
                let c = style.foreground;
                let color = ColorU::new(c.r, c.g, c.b, 255);
                map.insert(CharOffset::from(off)..CharOffset::from(off + len), color);
            }
            off += len;
        }
    }
    map
}

fn solid(c: ColorU) -> Fill {
    Fill::Solid(c)
}

/// Underline color for an LSP diagnostic by severity: 1 = error, 2 = warning,
/// 3 = info, 4 = hint. Info/hint use the muted text color; warning uses the
/// theme's warning color.
fn diag_color(severity: u8) -> ColorU {
    match severity {
        1 => theme::error(),
        2 => theme::warning(),
        _ => theme::text_muted(),
    }
}

/// Build a RichTextStyles for plain-code editing from our mono font + theme.
fn styles(font: FamilyId) -> RichTextStyles {
    let para = |tab: Option<u8>| ParagraphStyles {
        font_family: font,
        font_size: crate::warpui::fontsize::editor(),
        font_weight: Weight::Normal,
        line_height_ratio: 1.3,
        text_color: theme::text(),
        baseline_ratio: BASELINE,
        fixed_width_tab_size: tab,
    };
    RichTextStyles {
        base_text: para(None),
        code_text: para(Some(4)),
        code_background: Fill::None,
        embedding_background: Fill::None,
        embedding_text: para(Some(4)),
        code_border: Border::new(0.0),
        placeholder_color: theme::text_muted(),
        selection_fill: solid(theme::row_active()),
        cursor_fill: solid(theme::accent()),
        inline_code_style: InlineCodeStyle {
            font_family: font,
            background: theme::surface(),
            font_color: theme::text(),
        },
        check_box_style: CheckBoxStyle {
            border_width: 2.0,
            border_color: theme::border(),
            icon_path: "bundled/svg/check-thick.svg",
            background: theme::surface(),
            hover_background: theme::row_hover(),
        },
        horizontal_rule_style: HorizontalRuleStyle {
            rule_height: 1.0,
            color: theme::border(),
        },
        broken_link_style: BrokenLinkStyle {
            icon_path: "bundled/svg/link-broken-02.svg",
            icon_color: theme::error(),
        },
        block_spacings: DEFAULT_BLOCK_SPACINGS,
        show_placeholder_text_on_empty_block: false,
        minimum_paragraph_height: Some(PARAGRAPH_MIN_HEIGHT),
        cursor_width: 2.0,
        highlight_urls: false,
        table_style: TableStyle {
            border_color: theme::border(),
            header_background: theme::surface(),
            cell_background: theme::bg(),
            alternate_row_background: None,
            text_color: theme::text(),
            header_text_color: theme::text(),
            scrollbar_nonactive_thumb_color: theme::border(),
            scrollbar_active_thumb_color: theme::accent(),
            font_family: font,
            font_size: crate::warpui::fontsize::editor(),
            cell_padding: 8.0,
            outer_border: true,
            column_dividers: true,
            row_dividers: true,
        },
    }
}

/// CharOffset from a hit-test Location.
fn offset_of(location: &Location) -> CharOffset {
    match location {
        Location::Text { char_offset, .. } => *char_offset,
        Location::Block { start_offset, .. } => *start_offset,
    }
}

/// Editor events produced by the RichTextElement, applied in `handle_action`.
/// (`Action` comes from warpui's blanket `impl<T> Action for T`.)
#[derive(Debug, Clone)]
pub enum EditAction {
    CursorPlace { offset: CharOffset },
    /// Cmd+LeftClick — place the caret AND fire the goto-definition callback with
    /// the `(line, character)` at the click. Non-mutating.
    GotoDefinitionAt { offset: CharOffset },
    /// Triple-click — select the whole line under the click point.
    SelectLineAt { offset: CharOffset },
    SelectionExtend { offset: CharOffset },
    /// Left mouse released — ends an in-editor selection drag.
    EndSelect,
    InsertChars(String),
    Backspace,
    Enter,
    /// Move the caret (arrows / home / end). `extend` = hold Shift to select.
    CursorMove {
        dir: TextDirection,
        unit: TextUnit,
        extend: bool,
    },
    Scroll { delta: Pixels, axis: Axis },
    /// Jump the vertical scroll to a track fraction (0.0 top … 1.0 bottom) —
    /// from dragging the scrollbar thumb.
    ScrollToFrac(f32),
    /// Indent the current line(s) forward (Tab) or backward (Shift+Tab).
    Indent { outdent: bool },
    /// Scroll up or down by one full viewport height (Page Up / Page Down).
    PageScroll { down: bool },
    // ── Find / Replace / Goto-line actions (dispatched by the find bar buttons
    //    or the editor's own key handling). ──────────────────────────────────
    /// Move selection to the next match (wraps). `false` = previous match.
    FindNext { forward: bool },
    /// Close the find/replace/goto bar and clear match highlights.
    FindClose,
    /// Replace the current match with the replacement text.
    FindReplaceCurrent,
    /// Replace every match with the replacement text (reverse-order edits).
    FindReplaceAll,
    /// Focus the find field (`true`) or the replace field (`false`) for typing.
    FindFocusField { find_field: bool },
    /// Toggle a line comment on the current line(s) / selection using the
    /// language's line-comment prefix (keyed off the file extension).
    ToggleComment,
    /// Move the current line (or the selected lines) up (`down = false`) or
    /// down (`down = true`) — Alt+Up / Alt+Down.
    MoveLine { down: bool },
    /// Duplicate the current line below the caret — Alt+Shift+Down.
    DuplicateLine,
    /// Re-read the file from disk (the reload banner's "Reload" button).
    ReloadFromDisk,
    /// Dismiss the external-change reload banner (its "Keep" button): reset the
    /// mtime baseline so the banner closes and keeps the in-buffer edits.
    DismissDiskBanner,
    /// Dismiss the red save-failure banner (buffer stays dirty; Cmd+S retries).
    DismissSaveError,
}

impl EditAction {
    /// True for actions that change the buffer text — gated by read-only mode
    /// and used to promote a preview tab on first edit. Caret motion, scrolling,
    /// selection, and find-navigation are NOT mutating.
    fn is_mutating(&self) -> bool {
        matches!(
            self,
            EditAction::InsertChars(_)
                | EditAction::Backspace
                | EditAction::Enter
                | EditAction::Indent { .. }
                | EditAction::ToggleComment
                | EditAction::MoveLine { .. }
                | EditAction::DuplicateLine
                | EditAction::FindReplaceCurrent
                | EditAction::FindReplaceAll
        )
    }
}

impl<V> RichTextAction<V> for EditAction {
    fn scroll(delta: Pixels, axis: Axis) -> Option<Self> {
        Some(EditAction::Scroll { delta, axis })
    }
    fn user_typed(chars: String, _v: &WeakViewHandle<V>, _x: &AppContext) -> Option<Self> {
        Some(EditAction::InsertChars(chars))
    }
    fn vim_user_typed(chars: String, v: &WeakViewHandle<V>, x: &AppContext) -> Option<Self> {
        Self::user_typed(chars, v, x)
    }
    fn left_mouse_down(
        l: Location,
        m: ModifiersState,
        cc: u32,
        _fm: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        let offset = offset_of(&l);
        // Cmd+Click triggers goto-definition (and still places the caret);
        // a triple click (cc >= 3) selects the whole line, matching the
        // terminal's own triple-click-selects-line convention; a plain click
        // just places the caret.
        if m.cmd {
            Some(EditAction::GotoDefinitionAt { offset })
        } else if cc >= 3 {
            Some(EditAction::SelectLineAt { offset })
        } else {
            Some(EditAction::CursorPlace { offset })
        }
    }
    fn left_mouse_dragged(
        l: Location,
        _cmd: bool,
        _sh: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        Some(EditAction::SelectionExtend { offset: offset_of(&l) })
    }
    fn left_mouse_up(
        _l: Location,
        _cmd: bool,
        _sh: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Vec<Self> {
        vec![EditAction::EndSelect]
    }
    fn mouse_hovered(
        _l: Option<Location>,
        _v: &WeakViewHandle<V>,
        _cmd: bool,
        _cov: bool,
        _x: &AppContext,
    ) -> Option<Self> {
        None
    }
    fn task_list_clicked(
        _b: CharOffset,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        None
    }
    fn middle_mouse_down(_x: &AppContext) -> Option<Self> {
        None
    }
}

/// The editor MODEL — holds the buffer + selection + render state and gets all
/// editing behavior (insert/backspace/enter/delete/copy/cursor) for free as
/// `CoreEditorModel` / `PlainTextEditorModel` trait defaults.
pub struct CodeModel {
    buffer: ModelHandle<Buffer>,
    buffer_sel: ModelHandle<BufferSelectionModel>,
    selection: ModelHandle<SelectionModel>,
    render_state: ModelHandle<RenderState>,
}

impl Entity for CodeModel {
    type Event = ();
}

impl CoreEditorModel for CodeModel {
    type T = CodeModel;
    fn content(&self) -> &ModelHandle<Buffer> {
        &self.buffer
    }
    fn buffer_selection_model(&self) -> &ModelHandle<BufferSelectionModel> {
        &self.buffer_sel
    }
    fn selection_model(&self) -> &ModelHandle<SelectionModel> {
        &self.selection
    }
    fn render_state(&self) -> &ModelHandle<RenderState> {
        &self.render_state
    }
    fn validate(&self, _ctx: &impl warpui::ModelAsRef) {}
    fn active_text_style(&self) -> TextStyles {
        TextStyles::default()
    }
}

impl PlainTextEditorModel for CodeModel {}

/// Which bar is showing.
#[derive(Clone, Copy, PartialEq)]
enum FindMode {
    Find,
    Replace,
    Goto,
}

/// Live state for the Find / Replace / Goto-line bar. `WarpEditorView::find` is
/// `None` when the bar is closed.
struct FindState {
    mode: FindMode,
    /// The search query (Find/Replace) or the line-number text (Goto).
    query: String,
    /// The replacement text (Replace mode).
    replace: String,
    /// Which field captures typed characters: `true` = query, `false` = replace.
    find_field_active: bool,
    /// Match ranges as 0-based char offsets into the buffer text `(start, end)`,
    /// `end` exclusive. Same offset space as the syntect color map.
    matches: Vec<(usize, usize)>,
    /// Index into `matches` of the currently-highlighted match.
    current: Option<usize>,
}

impl FindState {
    fn new(mode: FindMode) -> Self {
        Self {
            mode,
            query: String::new(),
            replace: String::new(),
            find_field_active: true,
            matches: Vec::new(),
            current: None,
        }
    }
    fn bar_mode(&self) -> BarMode {
        match self.mode {
            FindMode::Find => BarMode::Find,
            FindMode::Replace => BarMode::FindReplace,
            FindMode::Goto => BarMode::GotoLine,
        }
    }
}

/// Cache backing `WarpEditorView::colors`: the highlighted color map plus the
/// buffer version and syntax-theme name it was computed for. `text_decorations`
/// re-highlights whenever the version (an edit) or the theme name changes, so
/// colors track edits exactly (no delta-drift) and follow theme switches.
struct ColorCache {
    map: RangeMap<CharOffset, ColorU>,
    version: Option<warp_editor::content::version::BufferVersion>,
    theme: String,
}

/// Cache for the gutter git-diff markers. The `git diff` shell-out is expensive
/// (a subprocess), and the working-tree diff only changes on SAVE (unsaved
/// in-buffer edits aren't on disk yet) or an external git op — never per
/// keystroke — so `dirty` gates recomputation to those moments instead of every
/// paint. Keyed by 0-based render line, ready to hand straight to `GutterElement`.
struct DiffCache {
    map: RangeMap<u32, DiffKind>,
    dirty: bool,
}

/// Compute the per-line git-diff markers for `path`, keyed by **0-based** render
/// line (what `GutterElement`/`cursor_line` use). Uses the file's PARENT dir as
/// the `git -C` root: git walks up to discover the enclosing repo, and the
/// absolute `path` pathspec resolves regardless — so no shell `active_cwd` is
/// needed. NOTE(simplification): markers are keyed by BUFFER line; with soft
/// word-wrap ON these can drift from render (wrapped) lines. Word-wrap defaults
/// OFF (`InfiniteWidth`, 1 buffer line = 1 render line), so the common path is exact.
fn compute_diff(path: &std::path::Path) -> RangeMap<u32, DiffKind> {
    let mut map = RangeMap::new();
    if path.as_os_str().is_empty() {
        return map;
    }
    let Some(repo) = path.parent() else {
        return map;
    };
    for (line, kind) in crate::warpui::git::file_line_diff(repo, path) {
        let dk = match kind {
            'A' => DiffKind::Added,
            'M' => DiffKind::Modified,
            _ => DiffKind::Deleted,
        };
        // git returns 1-based NEW-file line numbers; the gutter keys 0-based.
        //
        // Added / Modified own a real NEW row, so `line - 1` is that row.
        //
        // A pure DELETION owns no row — git reports the surviving NEW line the
        // removal FOLLOWS (`@@ -6 +5,0 @@` → new_start = 5, the line ABOVE the
        // gap). The gutter paints the wedge at the *top boundary* of the keyed
        // 0-based row, so the boundary between line 5 and line 6 is the top of
        // 0-based row 5 — i.e. the raw `line`, NOT `line - 1` (which would land
        // the wedge one row too high, on the top of line 5).
        let idx = match dk {
            DiffKind::Deleted => line,
            DiffKind::Added | DiffKind::Modified => line.saturating_sub(1),
        };
        map.insert(idx..idx + 1, dk);
    }
    map
}

pub struct WarpEditorView {
    model: ModelHandle<CodeModel>,
    self_handle: WeakViewHandle<Self>,
    display_state: DisplayStateHandle,
    path: std::path::PathBuf,
    /// Syntect color-map cache (CharOffset → fg color) for syntax highlighting,
    /// recomputed lazily on edit / theme change — see `ColorCache`.
    colors: std::cell::RefCell<ColorCache>,
    /// Mono font for the line-number gutter (same face as the editor text).
    gutter_font: FamilyId,
    /// Last on-disk content; `is_dirty` compares the live buffer against it.
    saved_text: String,
    /// Why the last save failed (io error text), shown as a red banner until
    /// the next successful save or an explicit dismiss. `None` = last save OK.
    /// Never silent: the user must not believe a failed Cmd+S landed on disk.
    save_error: Option<String>,
    /// True while a text-selection drag is active (a mouse-down landed inside
    /// the editor). Gates `SelectionExtend` so dragging the pane splitter or
    /// header — which never sends the editor a mouse-down — can't select text.
    selecting: std::cell::Cell<bool>,
    /// Persisted scrollbar-thumb drag state (element rebuilt each frame).
    scrollbar_drag: std::rc::Rc<std::cell::Cell<bool>>,
    /// Find / Replace / Goto-line bar state; `None` = closed.
    find: Option<FindState>,
    /// One indent level's worth of whitespace, discovered per-file from the
    /// nearest `.prettierrc` / `package.json` "prettier" field (tabs / 2-space /
    /// 4-space). Used by Tab-indent and Enter auto-indent — mirrors old egui.
    indent_unit: String,
    /// When set, trailing whitespace is stripped before writing on save
    /// (old `prefs.trim_on_save`). Defaults to `false` to match the old egui
    /// default; wire to a real warpui pref when settings plumbing lands.
    trim_on_save: bool,
    /// When `true`, text soft-wraps to the viewport width
    /// (`WidthSetting::FitViewport`); when `false` (the default, matching warp's
    /// own code editor) lines never wrap (`WidthSetting::InfiniteWidth`).
    /// Toggled by `set_word_wrap`, which rebuilds the RenderState — the vendored
    /// `RenderState` has no public width-setting mutator, only the consuming
    /// `with_width_setting` builder, so a fresh state is the only in-crate lever.
    word_wrap: bool,
    /// Preview-tab flag: the shell opens single-clicked files in a shared
    /// "preview" tab styled differently, and promotes it to a permanent tab on
    /// the first edit. Cleared by `note_edit` when a mutating action lands.
    preview: bool,
    /// Read-only mode: text-mutating actions (insert / backspace / enter /
    /// indent / comment / move / duplicate) become no-ops.
    read_only: bool,
    /// The file's on-disk modification time captured at load / save. `disk_changed`
    /// re-stats the path and compares, so the shell can show a reload banner when
    /// the file is edited by another process.
    loaded_mtime: Option<std::time::SystemTime>,
    /// Gutter git-diff marker cache (recomputed on save / external git op, NOT
    /// per frame — see `DiffCache`). The gutter reads this each paint.
    diff: std::cell::RefCell<DiffCache>,
    /// LSP diagnostics for this file, pushed by the shell (which owns the
    /// `LspManager`). Rendered as dashed severity-colored underlines that coexist
    /// with the syntax color layer and the find-match highlights. The shell
    /// re-pushes fresh diagnostics whenever the buffer changes, so offsets don't
    /// drift; `refresh_decorations` re-maps line/col → CharOffset on each push.
    diagnostics: Vec<crate::lsp::Diagnostic>,
    /// Cmd+Click goto-definition callback. The shell wires this to dispatch an
    /// LSP goto action; the editor computes the 0-based `(line, character)` under
    /// the click and invokes it. `None` = goto disabled for this editor.
    #[allow(clippy::type_complexity)]
    goto_cb: Option<std::rc::Rc<dyn Fn(u32, u32, &mut ViewContext<WarpEditorView>)>>,
    /// Throttle for the reload banner's on-disk stat: `(last_stat_at, changed)`.
    /// `render` runs each paint frame, so it re-stats via `disk_changed()` at
    /// most once per `DISK_STAT_INTERVAL` and caches the boolean here, sparing
    /// the filesystem a `stat` on every frame. `Instant`/`bool` are `Copy`, so a
    /// `Cell` suffices even though `render` only has `&self`.
    disk_stat: std::cell::Cell<(Option<std::time::Instant>, bool)>,
}

/// The on-disk modification time of `path`, or `None` if it can't be stat'd.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The minimal `(start_char, old_end_char, replacement)` edit that turns `old`
/// into `new`, found by trimming the common prefix and suffix. `start_char` and
/// `old_end_char` are 0-based char offsets into `old` (end exclusive);
/// `replacement` is the run of new chars that fills that gap. `None` when the
/// strings are identical. Lets whole-buffer string transforms (comment toggle,
/// line move) apply as ONE small model edit — one undo step, no color re-smear.
fn minimal_char_diff(old: &str, new: &str) -> Option<(usize, usize, String)> {
    if old == new {
        return None;
    }
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let mut pre = 0usize;
    while pre < o.len() && pre < n.len() && o[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0usize;
    while suf < o.len() - pre && suf < n.len() - pre && o[o.len() - 1 - suf] == n[n.len() - 1 - suf]
    {
        suf += 1;
    }
    let old_end = o.len() - suf;
    let replacement: String = n[pre..n.len() - suf].iter().collect();
    Some((pre, old_end, replacement))
}

/// Strip one indent level off the front of `s`: up to `max_spaces` leading
/// spaces, or a single leading tab. Mirrors the old egui `remove_one_indent`.
fn remove_one_indent(s: &str, max_spaces: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() && i < max_spaces && chars[i] == ' ' {
        i += 1;
    }
    if i == 0 && chars.first() == Some(&'\t') {
        i = 1;
    }
    chars[i..].iter().collect()
}

/// `(line_start, line_end)` as 0-based char offsets into `text` for the line
/// containing char offset `char_idx`. `line_end` is the index *after* the
/// trailing `\n` (or the char count at EOF). Used for whole-line copy/cut.
fn line_char_range(text: &str, char_idx: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let idx = char_idx.min(n);
    let mut ls = 0;
    for i in (0..idx).rev() {
        if chars[i] == '\n' {
            ls = i + 1;
            break;
        }
    }
    let mut le = n;
    for (offset, ch) in chars[idx..].iter().enumerate() {
        if *ch == '\n' {
            le = idx + offset + 1;
            break;
        }
    }
    (ls, le)
}

impl WarpEditorView {
    pub fn undo(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| m.undo(mctx));
    }
    pub fn redo(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| m.redo(mctx));
    }
    pub fn select_all(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| m.select_all(mctx));
    }
    /// Insert clipboard text at the cursor (Cmd+V).
    pub fn paste(&self, text: &str, ctx: &mut ViewContext<Self>) {
        let text = text.to_string();
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| m.user_insert(&text, mctx));
    }
    /// Copy the current selection to the clipboard (Cmd+C). On an EMPTY
    /// selection, copies the whole current line (trailing newline included),
    /// matching old egui's empty-selection Cmd+C.
    pub fn copy(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| {
            let (start, end) = {
                let sel = m.selection.as_ref(mctx);
                (
                    sel.selection_start(mctx).as_usize(),
                    sel.selection_end(mctx).as_usize(),
                )
            };
            if end > start {
                let content = m.read_selected_text_as_clipboard_content(mctx);
                mctx.clipboard().write(content);
                return;
            }
            // Empty selection → whole-line copy (incl. trailing newline).
            let text = m.buffer.as_ref(mctx).text().into_string();
            let cursor_off = m.selection.as_ref(mctx).cursors(mctx).last().as_usize();
            let char_idx = cursor_off.saturating_sub(1);
            let (ls, le) = line_char_range(&text, char_idx);
            let line: String = text.chars().skip(ls).take(le - ls).collect();
            if !line.is_empty() {
                mctx.clipboard()
                    .write(warpui::clipboard::ClipboardContent::plain_text(line));
            }
        });
    }
    /// Cut the current selection to the clipboard (Cmd+X). On an EMPTY
    /// selection, cuts the whole current line (trailing newline included) as one
    /// undo step — matching old egui's empty-selection Cmd+X. Falls back to a
    /// normal selection cut when a selection exists.
    pub fn cut(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| {
            let (start, end) = {
                let sel = m.selection.as_ref(mctx);
                (
                    sel.selection_start(mctx).as_usize(),
                    sel.selection_end(mctx).as_usize(),
                )
            };
            if end > start {
                m.delete(TextDirection::Backwards, TextUnit::Character, true, mctx);
                return;
            }
            // Empty selection → whole-line cut (incl. trailing newline).
            let text = m.buffer.as_ref(mctx).text().into_string();
            let cursor_off = m.selection.as_ref(mctx).cursors(mctx).last().as_usize();
            let char_idx = cursor_off.saturating_sub(1);
            let (ls, le) = line_char_range(&text, char_idx);
            if le <= ls {
                return;
            }
            let cut: String = text.chars().skip(ls).take(le - ls).collect();
            if cut.is_empty() {
                return;
            }
            mctx.clipboard()
                .write(warpui::clipboard::ClipboardContent::plain_text(cut));
            // Select the whole line range, then delete it in one edit.
            let sel_start = CharOffset::from(ls).add_signed(1);
            let sel_end = CharOffset::from(le).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |s, sctx| {
                s.set_cursor(sel_start, sctx);
                s.set_last_head(sel_end, sctx);
            });
            m.backspace(mctx);
        });
    }

    /// Write the buffer back to disk (Cmd+S). Returns true on success. Updates
    /// the saved snapshot so `is_dirty` reflects a clean state after saving.
    pub fn save(&mut self, app: &AppContext) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        let buffer = self.model.as_ref(app).buffer.clone();
        let mut text = buffer.as_ref(app).text().to_string();
        // Old `save_tab` honored `prefs.trim_on_save`: strip trailing
        // whitespace before writing when the pref is set.
        if self.trim_on_save {
            text = crate::syntax::trim_trailing_whitespace(&text);
        }
        match std::fs::write(&self.path, &text) {
            Ok(()) => {
                self.saved_text = text;
                self.save_error = None;
                // Refresh the external-change baseline so saving our own edits
                // doesn't trip `disk_changed`.
                self.loaded_mtime = file_mtime(&self.path);
                // Working tree changed → the gutter diff is now stale.
                self.diff.borrow_mut().dirty = true;
                true
            }
            Err(e) => {
                self.save_error = Some(format!("Save failed: {e}"));
                false
            }
        }
    }

    /// Cmd+S entry point with optional format-on-save. **The shell must call
    /// this instead of [`save`](Self::save)** for editor panes.
    ///
    /// When `format_on_save` is `true`, the buffer isn't read-only, and a
    /// formatter for this file's language is installed on `PATH`, the formatter
    /// runs as a subprocess **off the UI thread** (`ctx.spawn`, the same
    /// background executor the diff view uses). Its stdout replaces the buffer
    /// (caret preserved as best as possible) and the *formatted* text is written
    /// to disk in the spawn callback. Otherwise — formatting disabled, no
    /// formatter installed, empty path, or read-only — the buffer is written
    /// straight to disk synchronously via [`save`](Self::save).
    ///
    /// Robustness guarantees (never lose or corrupt data):
    /// * A formatter that fails (missing binary, non-zero exit, non-UTF-8 or an
    ///   empty result) yields `None`; the callback then writes the **current
    ///   buffer text** unchanged — the file is never blanked by a broken tool.
    /// * If the user keeps typing after pressing Cmd+S (the buffer version moves
    ///   on before the async format lands), the stale formatted text is **not**
    ///   applied; the callback writes the current buffer text so the newer edits
    ///   aren't clobbered and the save still lands.
    /// * Large files can't deadlock — see [`formatter::run`].
    ///
    /// LSP note: when formatting runs async the shell's post-save `did_save`
    /// fires with the pre-format text, but `poll_lsp` observes the bumped buffer
    /// version and sends a `did_change` with the formatted text on the next tick,
    /// so the server re-syncs on its own — no extra shell wiring required.
    ///
    /// Returns `true` when the save was initiated (async) or completed (sync).
    pub fn save_on_cmd_s(&mut self, ctx: &mut ViewContext<Self>, format_on_save: bool) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        let formatter = if format_on_save && !self.read_only {
            crate::warpui::formatter::for_path(&self.path)
        } else {
            None
        };
        let Some(fmt) = formatter else {
            // No formatting for this file → the plain synchronous write.
            return self.save(ctx);
        };

        // Snapshot the exact bytes + buffer version we're handing the formatter.
        // The version lets the callback detect edits made after Cmd+S and refuse
        // to overwrite them with a now-stale reformat.
        let text = self.buffer_text(ctx);
        let submitted_version = self.buffer_version(ctx);

        let fut = async move { crate::warpui::formatter::run(&fmt, &text) };
        ctx.spawn(fut, move |this, formatted, vctx| {
            this.apply_format_result(formatted, submitted_version, vctx);
        });
        true
    }

    /// Spawn-callback for [`save_on_cmd_s`](Self::save_on_cmd_s): adopt the
    /// formatter output (when safe) and persist to disk. See that method for the
    /// data-safety rules encoded here.
    fn apply_format_result(
        &mut self,
        formatted: Option<String>,
        submitted_version: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        // Did the user edit after pressing Cmd+S? If so, never apply a reformat
        // computed from the older text — just persist what's on screen now.
        let raced = self.buffer_version(ctx) != submitted_version;

        let to_write = match (formatted, raced) {
            (Some(f), false) => {
                self.apply_formatted_text(&f, ctx);
                f
            }
            // Formatter failed, or the buffer moved on — write the live text so
            // the save still happens and disk matches what the user sees.
            _ => self.buffer_text(ctx),
        };

        match std::fs::write(&self.path, &to_write) {
            Ok(()) => {
                self.saved_text = to_write;
                self.save_error = None;
                self.loaded_mtime = file_mtime(&self.path);
                self.diff.borrow_mut().dirty = true;
            }
            Err(e) => self.save_error = Some(format!("Save failed: {e}")),
        }
        ctx.notify();
    }

    /// Replace the buffer with `formatted` as ONE model edit (a single undo step,
    /// no full re-highlight smear), preserving the caret's char offset across the
    /// reformat as best as possible. No-op when the text is already identical.
    fn apply_formatted_text(&mut self, formatted: &str, ctx: &mut ViewContext<Self>) {
        let old = self.buffer_text(ctx);
        let Some((s, old_end, replacement)) = minimal_char_diff(&old, formatted) else {
            return;
        };
        // Best-effort caret preservation: unchanged before the edit, shifted by
        // the net length delta after it, clamped to the edit end when the caret
        // sat inside the reformatted region.
        let cursor = self.cursor_text_offset(ctx);
        let new_len = replacement.chars().count();
        let removed = old_end - s;
        let raw_cursor = if cursor <= s {
            cursor
        } else if cursor >= old_end {
            (cursor + new_len).saturating_sub(removed)
        } else {
            s + new_len
        };
        let new_total = old.chars().count() + new_len - removed;
        let new_cursor = raw_cursor.min(new_total);

        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let cs = CharOffset::from(s).add_signed(1);
            let ce = CharOffset::from(old_end).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(cs, sctx);
                sm.set_last_head(ce, sctx);
            });
            m.user_insert(&replacement, mctx);
            let c = CharOffset::from(new_cursor).add_signed(1);
            sel.update(mctx, |sm, sctx| sm.set_cursor(c, sctx));
        });
        if self.find.is_some() {
            self.recompute_matches(ctx);
        } else {
            self.refresh_decorations(ctx);
        }
    }

    /// Invalidate the cached gutter git-diff so the next paint recomputes it.
    /// The shell calls this after a git op (stage / commit / checkout) that can
    /// change the working-tree-vs-HEAD diff without an edit to this buffer.
    pub fn mark_diff_dirty(&self) {
        self.diff.borrow_mut().dirty = true;
    }

    /// Re-read the file from disk into the buffer (the reload banner's "Reload").
    /// Replaces the whole buffer as one edit (select-all + insert), refreshes the
    /// saved snapshot + external-change baseline, and invalidates the gutter diff.
    pub fn reload_from_disk(&mut self, ctx: &mut ViewContext<Self>) {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let model = self.model.clone();
        let text = content.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            m.select_all(mctx);
            m.user_insert(&text, mctx);
        });
        self.saved_text = content;
        self.loaded_mtime = file_mtime(&self.path);
        self.diff.borrow_mut().dirty = true;
        if self.find.is_some() {
            self.recompute_matches(ctx);
        } else {
            // Re-map any diagnostic underlines onto the reloaded content (the
            // shell will also push a fresh set shortly).
            self.refresh_decorations(ctx);
        }
        ctx.notify();
    }

    /// True when the file on disk has been modified since we last loaded / saved
    /// it (an external edit) — lets the shell show a reload banner. Also true if
    /// the file has since been removed. `false` for unsaved (empty-path) buffers.
    pub fn disk_changed(&self) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        match (self.loaded_mtime, file_mtime(&self.path)) {
            (Some(loaded), Some(current)) => current != loaded,
            // File vanished after we had a baseline → treat as changed.
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Reset the external-change baseline to the current on-disk mtime — the
    /// shell calls this after the user dismisses / reloads to clear the banner.
    pub fn refresh_disk_mtime(&mut self) {
        self.loaded_mtime = file_mtime(&self.path);
        // A fresh baseline means "not changed" until the next interval elapses;
        // seed the throttle so the banner drops immediately after Reload/Keep
        // instead of lingering until the cached value ages out.
        self.disk_stat.set((Some(std::time::Instant::now()), false));
    }

    /// Throttled `disk_changed` for the per-frame render path: re-stats at most
    /// once per `DISK_STAT_INTERVAL`, otherwise returns the cached result — so
    /// the reload banner doesn't cost a filesystem `stat` on every paint.
    fn disk_changed_throttled(&self) -> bool {
        let (last, cached) = self.disk_stat.get();
        let fresh = last.is_some_and(|t| t.elapsed() < DISK_STAT_INTERVAL);
        if fresh {
            return cached;
        }
        let changed = self.disk_changed();
        self.disk_stat
            .set((Some(std::time::Instant::now()), changed));
        changed
    }

    // ── Ln/Col + selection status (shell renders a status row) ───────────────

    /// The caret's 1-based `(line, column)` in the buffer text. Column counts
    /// characters from the line start (1 = start of line).
    pub fn cursor_line_col(&self, app: &AppContext) -> (usize, usize) {
        let text = self.buffer_text(app);
        let off = self.cursor_text_offset(app);
        let mut line = 1usize;
        let mut col = 1usize;
        for ch in text.chars().take(off) {
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// When a non-empty selection exists, its `(char_count, line_count)` — where
    /// `line_count` is the number of lines the selection spans (1 for a
    /// single-line selection). `None` for an empty selection.
    pub fn selection_info(&self, app: &AppContext) -> Option<(usize, usize)> {
        let sel = self.selected_text(app)?;
        if sel.is_empty() {
            return None;
        }
        let chars = sel.chars().count();
        let lines = sel.matches('\n').count() + 1;
        Some((chars, lines))
    }

    // ── LSP surface (the shell owns the LspManager and drives it) ────────────

    /// This editor's file path (empty when the buffer is unsaved). The shell uses
    /// it as the LSP document key for `did_open` / `did_change` / `did_save` /
    /// `diagnostics` / `goto_dispatch`.
    pub fn file_path(&self) -> &std::path::Path {
        &self.path
    }

    /// A monotonically-increasing version that bumps on every USER content edit
    /// (type / paste / undo / redo / cut / replace / indent / …). The shell polls
    /// this and only sends `did_change` when it changes — cheap change detection
    /// without diffing text. Caret motion / scrolling / selection do NOT bump it.
    pub fn buffer_version(&self, app: &AppContext) -> u64 {
        self.model.as_ref(app).buffer.as_ref(app).version().as_u64()
    }

    /// The caret's 0-based `(line, character)` — the position form LSP wants for
    /// `goto_dispatch` / `hover`. (`cursor_line_col` returns the 1-based form for
    /// the status row; this is the LSP-shaped peer.)
    pub fn cursor_line_char(&self, app: &AppContext) -> (u32, u32) {
        let text = self.buffer_text(app);
        let off = self.cursor_text_offset(app);
        Self::line_char_at_offset(&text, off)
    }

    /// Store fresh LSP diagnostics and re-render their underline decorations.
    /// Coexists with syntax colors (a separate `text_decorations` layer) and find
    /// highlights. The shell pushes a fresh set whenever the buffer changes, so
    /// offsets track edits.
    pub fn set_diagnostics(
        &mut self,
        diags: Vec<crate::lsp::Diagnostic>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.diagnostics = diags;
        self.refresh_decorations(ctx);
        ctx.notify();
    }

    /// The 0-based `(line, character)` of char offset `offset` into `text`.
    /// `character` counts chars from the line start. Shared by `cursor_line_char`
    /// and the Cmd+Click goto path.
    fn line_char_at_offset(text: &str, offset: usize) -> (u32, u32) {
        let mut line = 0u32;
        let mut col = 0u32;
        for (i, ch) in text.chars().enumerate() {
            if i >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    // ── Preview / read-only ──────────────────────────────────────────────────

    /// True while this editor is a preview tab (see `preview` field).
    pub fn is_preview(&self) -> bool {
        self.preview
    }
    /// Mark / unmark this editor as a preview tab.
    pub fn set_preview(&mut self, preview: bool, ctx: &mut ViewContext<Self>) {
        self.preview = preview;
        ctx.notify();
    }
    /// Promote a preview tab to permanent (clears the preview flag). Returns
    /// `true` if it was a preview tab (the shell can then re-style the tab).
    pub fn clear_preview(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let was = self.preview;
        if was {
            self.preview = false;
            ctx.notify();
        }
        was
    }
    /// True when the editor is read-only (mutating actions are no-ops).
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    /// Enable / disable read-only mode.
    pub fn set_read_only(&mut self, read_only: bool, ctx: &mut ViewContext<Self>) {
        self.read_only = read_only;
        ctx.notify();
    }

    // ── Word wrap ────────────────────────────────────────────────────────────

    /// True when soft word-wrap is on.
    pub fn word_wrap(&self) -> bool {
        self.word_wrap
    }
    /// Flip word-wrap and re-apply it.
    pub fn toggle_word_wrap(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_word_wrap(!self.word_wrap, ctx);
    }
    /// Switch soft word-wrap on/off. `on` wraps to the viewport width
    /// (`WidthSetting::FitViewport`, which the RichTextElement re-measures each
    /// paint via `set_viewport_size`); `off` restores infinite width.
    ///
    /// The vendored `RenderState` exposes no public width-setting mutator (only
    /// the consuming `with_width_setting` builder), so we rebuild the RenderState
    /// and its dependent SelectionModel in place — the shared `Buffer` (content +
    /// undo history) is preserved, and the caret offset is restored after the
    /// relayout. Only runs on an explicit toggle, so the rebuild cost is fine.
    /// Settings > Editor "trim trailing whitespace on save" — applied by the
    /// shell to every open editor when the pref flips (and at construction).
    pub fn set_trim_on_save(&mut self, on: bool) {
        self.trim_on_save = on;
    }

    pub fn set_word_wrap(&mut self, on: bool, ctx: &mut ViewContext<Self>) {
        if self.word_wrap == on {
            return;
        }
        self.word_wrap = on;
        let setting = if on {
            WidthSetting::FitViewport
        } else {
            WidthSetting::InfiniteWidth
        };
        // Preserve the caret so the toggle doesn't jump the view.
        let cursor_char = self.cursor_text_offset(ctx);
        let (buffer, buffer_sel) = {
            let m = self.model.as_ref(ctx);
            (m.buffer.clone(), m.buffer_sel.clone())
        };
        let st = styles(self.gutter_font);
        let new_rs = ctx.add_model(|mctx| {
            RenderState::new(st, false, None, mctx).with_width_setting(setting)
        });
        let new_sel = {
            let (b, r, bs) = (buffer.clone(), new_rs.clone(), buffer_sel.clone());
            ctx.add_model(|mctx| SelectionModel::new(b, r, bs, None, mctx))
        };
        // Swap the handles into the model (the buffer→render forwarding
        // subscription reads `me.render_state` fresh, so it now targets the new
        // state), then re-lay-out the buffer content into the fresh render state.
        self.model.update(ctx, |m: &mut CodeModel, mctx| {
            m.render_state = new_rs;
            m.selection = new_sel;
            m.rebuild_layout(mctx);
            let c = CharOffset::from(cursor_char).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |s, sctx| s.set_cursor(c, sctx));
        });
        // Re-push decorations (find highlights + diagnostics) — the fresh render
        // state cleared them.
        self.refresh_decorations(ctx);
        ctx.notify();
    }

    // ── Comment toggle / move / duplicate line ───────────────────────────────

    /// Toggle a line comment on the current line(s) or selection, using the
    /// language's line-comment prefix (from the file extension). Reuses the old
    /// egui `file_util::{comment_prefix, toggle_line_comments}` so behavior is
    /// identical; the whole-buffer transform is applied as one minimal model edit.
    pub fn toggle_comment(&mut self, ctx: &mut ViewContext<Self>) {
        if self.read_only {
            return;
        }
        let text = self.buffer_text(ctx);
        let (start, end) = self.selection_char_range(ctx);
        let prefix =
            crate::syntax::comment_prefix(&self.path.to_string_lossy());
        let mut modified = text.clone();
        crate::syntax::toggle_line_comments(&mut modified, start, end, prefix);
        let Some((s, old_end, replacement)) = minimal_char_diff(&text, &modified) else {
            return;
        };
        self.note_edit();
        let new_end = s + replacement.chars().count();
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let cs = CharOffset::from(s).add_signed(1);
            let ce = CharOffset::from(old_end).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(cs, sctx);
                sm.set_last_head(ce, sctx);
            });
            m.user_insert(&replacement, mctx);
            // Re-select the toggled block so repeated toggles keep working.
            let a = CharOffset::from(s).add_signed(1);
            let b = CharOffset::from(new_end).add_signed(1);
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(a, sctx);
                sm.set_last_head(b, sctx);
            });
        });
        if self.find.is_some() {
            self.recompute_matches(ctx);
        }
        ctx.notify();
    }

    /// Move the current line (or the lines the selection spans) up or down.
    /// Ports old egui `file_view` Alt+Up / Alt+Down, translated to char space
    /// and applied as one model edit. Collapses to a single caret (as old did).
    fn move_line(&mut self, down: bool, ctx: &mut ViewContext<Self>) {
        if self.read_only {
            return;
        }
        let text = self.buffer_text(ctx);
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let (sel_start, sel_end) = self.selection_char_range(ctx);
        // Line block containing the selection: [line_start, line_end), where
        // line_end is one PAST the newline that ends the last selected line.
        let line_start = chars[..sel_start.min(n)]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let line_end = chars[sel_end.min(n)..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| sel_end.min(n) + i + 1)
            .unwrap_or(n);
        let (region_start, region_end, replacement, new_cursor) = if down {
            if line_end >= n {
                return; // already the last line
            }
            let next_end = chars[line_end..]
                .iter()
                .position(|c| *c == '\n')
                .map(|i| line_end + i + 1)
                .unwrap_or(n);
            let next: String = chars[line_end..next_end].iter().collect();
            let cur: String = chars[line_start..line_end].iter().collect();
            let cursor = sel_start + (next_end - line_end);
            (line_start, next_end, format!("{next}{cur}"), cursor)
        } else {
            if line_start == 0 {
                return; // already the first line
            }
            let prev_start = chars[..line_start - 1]
                .iter()
                .rposition(|c| *c == '\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let prev_len = line_start - prev_start;
            let cur: String = chars[line_start..line_end].iter().collect();
            let prev: String = chars[prev_start..line_start].iter().collect();
            let cursor = sel_start.saturating_sub(prev_len);
            (prev_start, line_end, format!("{cur}{prev}"), cursor)
        };
        self.note_edit();
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let cs = CharOffset::from(region_start).add_signed(1);
            let ce = CharOffset::from(region_end).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(cs, sctx);
                sm.set_last_head(ce, sctx);
            });
            m.user_insert(&replacement, mctx);
            let c = CharOffset::from(new_cursor).add_signed(1);
            sel.update(mctx, |sm, sctx| sm.set_cursor(c, sctx));
        });
        if self.find.is_some() {
            self.recompute_matches(ctx);
        }
        ctx.notify();
    }

    /// Duplicate the current line below the caret (Alt+Shift+Down). Inserts a
    /// clean copy on its own line — no blank line — and drops the caret onto the
    /// duplicate at the same column. (Old egui inserted the trailing newline into
    /// the copy, which left a blank line on interior lines; this is the intended
    /// clean behavior.)
    fn duplicate_line(&mut self, ctx: &mut ViewContext<Self>) {
        if self.read_only {
            return;
        }
        let text = self.buffer_text(ctx);
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let cursor = self.cursor_text_offset(ctx).min(n);
        let line_start = chars[..cursor]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        // End of the line's CONTENT (before the terminating '\n', or EOF).
        let line_end = chars[cursor..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| cursor + i)
            .unwrap_or(n);
        let line: String = chars[line_start..line_end].iter().collect();
        self.note_edit();
        let inserted = format!("\n{line}");
        let new_cursor = cursor + line.chars().count() + 1;
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let at = CharOffset::from(line_end).add_signed(1);
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| sm.set_cursor(at, sctx));
            m.user_insert(&inserted, mctx);
            let c = CharOffset::from(new_cursor).add_signed(1);
            sel.update(mctx, |sm, sctx| sm.set_cursor(c, sctx));
        });
        if self.find.is_some() {
            self.recompute_matches(ctx);
        }
        ctx.notify();
    }

    /// The current selection as a 0-based `(start, end)` char range in the buffer
    /// text (end exclusive). Collapses to `(caret, caret)` when there's no
    /// selection. Shared by comment/move/duplicate.
    fn selection_char_range(&self, ctx: &impl warpui::ModelAsRef) -> (usize, usize) {
        let m = self.model.as_ref(ctx);
        let sel = m.selection.as_ref(ctx);
        let start = sel.selection_start(ctx).as_usize().saturating_sub(1);
        let end = sel.selection_end(ctx).as_usize().saturating_sub(1);
        if end < start {
            (end, start)
        } else {
            (start, end)
        }
    }

    /// Called at the start of any mutating action: promotes a preview tab to a
    /// permanent one on first edit. (The shell reads `is_preview()` for styling.)
    fn note_edit(&mut self) {
        self.preview = false;
    }

    /// True when the buffer differs from the last on-disk snapshot (unsaved edits).
    pub fn is_dirty(&self, app: &AppContext) -> bool {
        let buffer = self.model.as_ref(app).buffer.clone();
        buffer.as_ref(app).text().to_string() != self.saved_text
    }

    pub fn new(
        ctx: &mut ViewContext<Self>,
        content: String,
        font: FamilyId,
        path: std::path::PathBuf,
    ) -> Self {
        // Per-file indent unit discovered from the nearest .prettierrc /
        // package.json "prettier" field (tabs / 2-space / 4-space), matching the
        // old egui Files pane — instead of a hardcoded 4-space unit.
        let style = crate::format::discover(&path);
        let indent_unit_str = style.indent_unit();
        let iu = if style.use_tabs {
            IndentUnit::Tab
        } else {
            IndentUnit::Space(style.tab_width)
        };
        let buffer = ctx.add_model(move |_| {
            Buffer::new(Box::new(move |_, _| IndentBehavior::TabIndent(iu)))
        });
        let buffer_sel = ctx.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        let bsel2 = buffer_sel.clone();
        buffer.update(ctx, |buf, mctx| {
            *buf = Buffer::from_plain_text(
                &content,
                None,
                Box::new(move |_, _| IndentBehavior::TabIndent(iu)),
                bsel2,
                mctx,
            );
        });
        let st = styles(font);
        // InfiniteWidth (like warp's own code editor): don't soft-wrap at the
        // viewport width — otherwise, because the viewport width isn't known at
        // construction time, every glyph wraps onto its own line.
        let render_state = ctx.add_model(|mctx| {
            RenderState::new(st, false, None, mctx).with_width_setting(WidthSetting::InfiniteWidth)
        });
        let selection = {
            let (b, r, bs) = (buffer.clone(), render_state.clone(), buffer_sel.clone());
            ctx.add_model(|mctx| SelectionModel::new(b, r, bs, None, mctx))
        };
        // The RenderState starts EMPTY. It only gets its blocks by processing
        // "pending edits" fed from the buffer. So the model must subscribe to the
        // buffer and, on every `ContentChanged`, forward the edit delta to the
        // render state (this is what makes typed/pasted text appear). Below, after
        // the model exists, we also `rebuild_layout` to push the INITIAL file
        // content in as one big edit.
        let sub_buffer = buffer.clone();
        let model = ctx.add_model(move |mctx| {
            mctx.subscribe_to_model(
                &sub_buffer,
                |me: &mut CodeModel, _buf, event: &BufferEvent, mctx| match event {
                    BufferEvent::ContentChanged {
                        delta,
                        buffer_version,
                        ..
                    } => {
                        let (delta, version) = (delta.clone(), *buffer_version);
                        me.render_state.update(mctx, move |rs, _| {
                            rs.add_pending_edit(delta, version);
                        });
                        mctx.notify();
                    }
                    // Cursor/selection moved (click, arrows, set_cursor). Feed the
                    // rendered selection set into the render state so the caret
                    // actually moves on screen.
                    BufferEvent::SelectionChanged { buffer_version, .. } => {
                        let version = *buffer_version;
                        let buffer_sel = me.buffer_sel.clone();
                        let mut selections = me
                            .buffer
                            .as_ref(mctx)
                            .to_rendered_selection_set(buffer_sel, mctx);
                        for s in selections.iter_mut() {
                            s.head -= CharOffset::from(1);
                            s.tail -= CharOffset::from(1);
                        }
                        me.render_state.update(mctx, move |rs, _| {
                            rs.update_selection(selections, version);
                        });
                        mctx.notify();
                    }
                    _ => {}
                },
            );
            CodeModel {
                buffer,
                buffer_sel,
                selection,
                render_state,
            }
        });
        // Lay out the initial buffer content into the render state.
        model.update(ctx, |m: &mut CodeModel, mctx| m.rebuild_layout(mctx));
        let colors = std::cell::RefCell::new(ColorCache {
            map: highlight(&content, &path),
            // `version: None` forces the first `text_decorations` call (which
            // carries the real render buffer version) to recompute once.
            version: None,
            theme: crate::theme::current().syntax_theme.clone(),
        });
        let loaded_mtime = file_mtime(&path);
        // Compute the gutter diff once at open (the "compute at open" trigger);
        // thereafter it's refreshed only on save / external git op / reload.
        let diff = std::cell::RefCell::new(DiffCache {
            map: compute_diff(&path),
            dirty: false,
        });
        WarpEditorView {
            model,
            self_handle: ctx.handle(),
            display_state: Arc::new(DisplayState::default()),
            path,
            colors,
            gutter_font: font,
            saved_text: content,
            save_error: None,
            selecting: std::cell::Cell::new(false),
            scrollbar_drag: std::rc::Rc::new(std::cell::Cell::new(false)),
            find: None,
            indent_unit: indent_unit_str,
            trim_on_save: false,
            word_wrap: false,
            preview: false,
            read_only: false,
            loaded_mtime,
            diff,
            diagnostics: Vec::new(),
            goto_cb: None,
            disk_stat: std::cell::Cell::new((None, false)),
        }
    }

    /// Builder: install the Cmd+Click goto-definition callback. Chains onto
    /// `new(...)` inside the view-construction closure.
    #[allow(clippy::type_complexity)]
    pub fn with_goto(
        mut self,
        cb: std::rc::Rc<dyn Fn(u32, u32, &mut ViewContext<WarpEditorView>)>,
    ) -> Self {
        self.goto_cb = Some(cb);
        self
    }

    /// Setter form of `with_goto`, for wiring the callback after construction
    /// (e.g. `handle.update(ctx, |v, _| v.set_goto(cb))`).
    #[allow(clippy::type_complexity)]
    pub fn set_goto(
        &mut self,
        cb: std::rc::Rc<dyn Fn(u32, u32, &mut ViewContext<WarpEditorView>)>,
    ) {
        self.goto_cb = Some(cb);
    }
}

impl WarpEditorView {
    // ── Public entry points (a shell keybinding can call these) ────────────

    /// Open the Find bar (Cmd+F). Pre-fills the query with the current
    /// single-line selection, if any.
    pub fn open_find(&mut self, ctx: &mut ViewContext<Self>) {
        self.open_find_mode(FindMode::Find, ctx);
    }
    /// Open the Find + Replace bar (Cmd+H).
    pub fn open_replace(&mut self, ctx: &mut ViewContext<Self>) {
        self.open_find_mode(FindMode::Replace, ctx);
    }
    /// Open the Goto-line bar (Cmd+G).
    pub fn open_goto_line(&mut self, ctx: &mut ViewContext<Self>) {
        self.find = Some(FindState::new(FindMode::Goto));
        ctx.notify();
    }
    /// Move the caret to the start of 1-based line `n` and autoscroll it into
    /// view. Used by Find-in-Files to jump to a clicked match. Peer of the
    /// internal `do_goto_line` but takes an explicit line number.
    pub fn goto_line(&mut self, n: usize, ctx: &mut ViewContext<Self>) {
        let n = n.max(1);
        let text = self.buffer_text(ctx);
        let offset = Self::line_start_offset(&text, n);
        let buf_off = CharOffset::from(offset).add_signed(1);
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| sm.set_cursor(buf_off, sctx));
            let rs = m.render_state.clone();
            rs.update(mctx, |r, _rctx| r.request_autoscroll());
        });
        ctx.notify();
    }
    /// Close the bar and clear match highlights (Escape).
    pub fn close_find(&mut self, ctx: &mut ViewContext<Self>) {
        if self.find.take().is_some() {
            self.clear_match_decorations(ctx);
        }
        ctx.notify();
    }
    /// True when the find/replace/goto bar is open (lets the shell gate keys).
    pub fn find_open(&self) -> bool {
        self.find.is_some()
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    fn open_find_mode(&mut self, mode: FindMode, ctx: &mut ViewContext<Self>) {
        // Keep an existing query when switching Find <-> Replace; otherwise
        // pre-fill from the current selection.
        let prev_query = self.find.as_ref().map(|f| f.query.clone());
        let mut state = FindState::new(mode);
        if let Some(q) = prev_query {
            state.query = q;
        } else if let Some(sel) = self.selected_text(ctx) {
            if !sel.is_empty() && !sel.contains('\n') {
                state.query = sel;
            }
        }
        self.find = Some(state);
        self.recompute_matches(ctx);
        ctx.notify();
    }

    /// The whole buffer as an owned `String`. Public so the shell can feed it to
    /// the LSP (`did_open` / `did_change` / `did_save`). `AppContext` satisfies
    /// `ModelAsRef`, so the shell calls `editor.buffer_text(app)`.
    pub fn buffer_text(&self, ctx: &impl warpui::ModelAsRef) -> String {
        self.model.as_ref(ctx).buffer.as_ref(ctx).text().to_string()
    }

    /// The current caret position as a 0-based char offset into `buffer_text`.
    fn cursor_text_offset(&self, ctx: &impl warpui::ModelAsRef) -> usize {
        let m = self.model.as_ref(ctx);
        let cur = m.selection.as_ref(ctx).cursors(ctx);
        cur.last().as_usize().saturating_sub(1)
    }

    /// The current selection as text, or `None` if the selection is empty.
    fn selected_text(&self, ctx: &impl warpui::ModelAsRef) -> Option<String> {
        let m = self.model.as_ref(ctx);
        let sel = m.selection.as_ref(ctx);
        let start = sel.selection_start(ctx).as_usize().saturating_sub(1);
        let end = sel.selection_end(ctx).as_usize().saturating_sub(1);
        if end <= start {
            return None;
        }
        let chars: Vec<char> = m.buffer.as_ref(ctx).text().to_string().chars().collect();
        if start >= chars.len() {
            return None;
        }
        Some(chars[start..end.min(chars.len())].iter().collect())
    }

    /// Case-SENSITIVE non-overlapping substring search (matches old egui's
    /// `content.matches(query)` / `str::find`). Returns `(start, end)` char
    /// offsets (end exclusive) into `text`.
    fn find_all(text: &str, query: &str) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let hay: Vec<char> = text.chars().collect();
        let needle: Vec<char> = query.chars().collect();
        let (n, m) = (hay.len(), needle.len());
        let mut out = Vec::new();
        if m == 0 || m > n {
            return out;
        }
        let mut i = 0;
        while i + m <= n {
            if hay[i..i + m] == needle[..] {
                out.push((i, i + m));
                i += m;
            } else {
                i += 1;
            }
        }
        out
    }

    /// The 0-based char offset of the start of 1-based `line`. Clamps to the
    /// buffer end when `line` exceeds the line count.
    fn line_start_offset(text: &str, line: usize) -> usize {
        if line <= 1 {
            return 0;
        }
        let mut newlines = 0usize;
        for (i, ch) in text.chars().enumerate() {
            if ch == '\n' {
                newlines += 1;
                if newlines == line - 1 {
                    return i + 1;
                }
            }
        }
        text.chars().count()
    }

    /// Re-run the search for the current query and refresh highlights. No-op in
    /// Goto mode.
    fn recompute_matches(&mut self, ctx: &mut ViewContext<Self>) {
        let (mode, query) = match self.find.as_ref() {
            Some(f) => (f.mode, f.query.clone()),
            None => return,
        };
        if mode == FindMode::Goto {
            return;
        }
        let text = self.buffer_text(ctx);
        let matches = Self::find_all(&text, &query);
        let cursor = self.cursor_text_offset(ctx);
        let current = if matches.is_empty() {
            None
        } else {
            Some(
                matches
                    .iter()
                    .position(|(s, _)| *s >= cursor)
                    .unwrap_or(0),
            )
        };
        if let Some(f) = self.find.as_mut() {
            f.matches = matches;
            f.current = current;
        }
        self.apply_match_decorations(ctx);
    }

    /// Map the stored LSP diagnostics to render-state underline decorations.
    /// Each diagnostic is single-line (`line`, `col_start`, `col_end`); columns
    /// are treated as char offsets into the line and clamped to the line length
    /// (mirroring the old egui overlay). Offsets are 0-based char indices into
    /// the buffer text — the same space the find-match decorations use — so they
    /// coexist correctly. Recomputed against the CURRENT buffer text on every
    /// call, so ranges track edits (the shell re-pushes on change).
    fn diag_decorations(&self, ctx: &impl warpui::ModelAsRef) -> Vec<Decoration> {
        if self.diagnostics.is_empty() {
            return Vec::new();
        }
        let text = self.buffer_text(ctx);
        // Char offset of each line start, precomputed once.
        let mut line_starts: Vec<usize> = vec![0];
        let mut idx = 0usize;
        for ch in text.chars() {
            idx += 1;
            if ch == '\n' {
                line_starts.push(idx);
            }
        }
        let total = idx;
        let mut out = Vec::new();
        for d in &self.diagnostics {
            let Some(&base) = line_starts.get(d.line as usize) else {
                continue;
            };
            let next = line_starts
                .get(d.line as usize + 1)
                .copied()
                .unwrap_or(total);
            let line_len = next.saturating_sub(base).saturating_sub(1);
            let cs = (d.col_start as usize).min(line_len);
            let ce = (d.col_end as usize).min(line_len).max(cs);
            let (mut s, mut e) = (base + cs, base + ce);
            // Zero-width diagnostic (col_start == col_end): underline one char so
            // it's still visible.
            if e <= s {
                e = (s + 1).min(total);
                if e <= s {
                    // At EOF with nothing to underline — nudge start back one.
                    s = s.saturating_sub(1);
                }
                if e <= s {
                    continue;
                }
            }
            out.push(
                Decoration::new(CharOffset::from(s), CharOffset::from(e))
                    .with_dashed_underline(diag_color(d.severity)),
            );
        }
        out
    }

    /// Rebuild the render-state decoration set from BOTH coexisting layers:
    /// diagnostic underlines (from the LSP) and find-match backgrounds (when the
    /// find bar is open). `set_text_decorations` REPLACES the whole vec, so the
    /// two layers must be pushed together or one clobbers the other.
    fn refresh_decorations(&self, ctx: &mut ViewContext<Self>) {
        let mut decs = self.diag_decorations(ctx);
        if let Some(f) = self.find.as_ref() {
            for (idx, (s, e)) in f.matches.iter().enumerate() {
                let fill = if Some(idx) == f.current {
                    *warp_editor::search::SELECTED_MATCH_FILL
                } else {
                    *warp_editor::search::MATCH_FILL
                };
                decs.push(
                    Decoration::new(CharOffset::from(*s), CharOffset::from(*e))
                        .with_background(fill),
                );
            }
        }
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let rs = m.render_state.clone();
            rs.update(mctx, |r, rctx| r.set_text_decorations(decs, rctx));
        });
    }

    /// Push the current find-match ranges (plus diagnostics) into the render
    /// state. Kept as a thin alias over `refresh_decorations` so the find code
    /// reads naturally.
    fn apply_match_decorations(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_decorations(ctx);
    }

    /// Clear find-match highlights (Escape / close). Diagnostics are preserved —
    /// `refresh_decorations` re-pushes them since `find` is now `None`.
    fn clear_match_decorations(&mut self, ctx: &mut ViewContext<Self>) {
        self.refresh_decorations(ctx);
    }

    /// Select match `idx` (as a selection range) and scroll it into view.
    fn goto_match(&mut self, idx: usize, ctx: &mut ViewContext<Self>) {
        let range = match self.find.as_ref() {
            Some(f) if idx < f.matches.len() => f.matches[idx],
            _ => return,
        };
        if let Some(f) = self.find.as_mut() {
            f.current = Some(idx);
        }
        let (s, e) = range;
        let start = CharOffset::from(s).add_signed(1);
        let end = CharOffset::from(e).add_signed(1);
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(start, sctx);
                sm.set_last_head(end, sctx);
            });
            let rs = m.render_state.clone();
            rs.update(mctx, |r, _rctx| {
                r.request_autoscroll_to(AutoScrollMode::ScrollOffsetsIntoViewport(start..end));
            });
        });
        self.apply_match_decorations(ctx);
        ctx.notify();
    }

    /// Move to the next (`forward`) or previous match, wrapping around.
    fn find_step(&mut self, forward: bool, ctx: &mut ViewContext<Self>) {
        let (len, current) = match self.find.as_ref() {
            Some(f) if !f.matches.is_empty() => (f.matches.len(), f.current),
            _ => return,
        };
        let idx = match current {
            Some(c) => {
                if forward {
                    (c + 1) % len
                } else {
                    (c + len - 1) % len
                }
            }
            None => {
                let cursor = self.cursor_text_offset(ctx);
                let pos = self
                    .find
                    .as_ref()
                    .unwrap()
                    .matches
                    .iter()
                    .position(|(s, _)| *s >= cursor)
                    .unwrap_or(0);
                if forward {
                    pos
                } else {
                    (pos + len - 1) % len
                }
            }
        };
        self.goto_match(idx, ctx);
    }

    /// Jump the caret to the line typed into the Goto bar, then close it.
    fn do_goto_line(&mut self, ctx: &mut ViewContext<Self>) {
        let n: usize = match self.find.as_ref() {
            Some(f) => f.query.trim().parse::<usize>().unwrap_or(1).max(1),
            None => return,
        };
        let text = self.buffer_text(ctx);
        let offset = Self::line_start_offset(&text, n);
        let buf_off = CharOffset::from(offset).add_signed(1);
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| sm.set_cursor(buf_off, sctx));
            let rs = m.render_state.clone();
            rs.update(mctx, |r, _rctx| r.request_autoscroll());
        });
        self.close_find(ctx);
    }

    /// Replace the current match with the replacement text, then re-search.
    fn replace_current(&mut self, ctx: &mut ViewContext<Self>) {
        let (range, replacement) = match self.find.as_ref() {
            Some(f) if f.mode == FindMode::Replace => match f.current {
                Some(c) if c < f.matches.len() => (f.matches[c], f.replace.clone()),
                _ => return,
            },
            _ => return,
        };
        let (s, e) = range;
        let start = CharOffset::from(s).add_signed(1);
        let end = CharOffset::from(e).add_signed(1);
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            let sel = m.selection.clone();
            sel.update(mctx, |sm, sctx| {
                sm.set_cursor(start, sctx);
                sm.set_last_head(end, sctx);
            });
            m.user_insert(&replacement, mctx);
        });
        self.recompute_matches(ctx);
        ctx.notify();
    }

    /// Replace every match. Applies edits in REVERSE offset order so that the
    /// yet-to-be-applied (earlier) match offsets stay valid as we go.
    fn replace_all(&mut self, ctx: &mut ViewContext<Self>) {
        let (mut matches, replacement) = match self.find.as_ref() {
            Some(f) if f.mode == FindMode::Replace && !f.matches.is_empty() => {
                (f.matches.clone(), f.replace.clone())
            }
            _ => return,
        };
        matches.sort_by(|a, b| b.0.cmp(&a.0));
        let model = self.model.clone();
        model.update(ctx, |m: &mut CodeModel, mctx| {
            for (s, e) in &matches {
                let start = CharOffset::from(*s).add_signed(1);
                let end = CharOffset::from(*e).add_signed(1);
                let sel = m.selection.clone();
                sel.update(mctx, |sm, sctx| {
                    sm.set_cursor(start, sctx);
                    sm.set_last_head(end, sctx);
                });
                m.user_insert(&replacement, mctx);
            }
        });
        self.recompute_matches(ctx);
        ctx.notify();
    }

    /// Route a typed string into the active find-bar field.
    fn find_type(&mut self, s: &str, ctx: &mut ViewContext<Self>) {
        let mut recompute = false;
        if let Some(f) = self.find.as_mut() {
            match f.mode {
                FindMode::Goto => {
                    for ch in s.chars() {
                        if ch.is_ascii_digit() {
                            f.query.push(ch);
                        }
                    }
                }
                _ => {
                    if f.find_field_active {
                        f.query.push_str(s);
                        recompute = true;
                    } else {
                        f.replace.push_str(s);
                    }
                }
            }
        }
        if recompute {
            self.recompute_matches(ctx);
        }
        ctx.notify();
    }

    /// Delete the last char of the active find-bar field.
    fn find_backspace(&mut self, ctx: &mut ViewContext<Self>) {
        let mut recompute = false;
        if let Some(f) = self.find.as_mut() {
            let goto = f.mode == FindMode::Goto;
            if goto || f.find_field_active {
                f.query.pop();
                recompute = !goto;
            } else {
                f.replace.pop();
            }
        }
        if recompute {
            self.recompute_matches(ctx);
        }
        ctx.notify();
    }
}

impl Entity for WarpEditorView {
    type Event = ();
}

impl TypedActionView for WarpEditorView {
    type Action = EditAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        self.apply(action, ctx);
    }
}

impl WarpEditorView {
    /// Apply an editing action to the model (shared by `handle_action` — which
    /// receives actions dispatched by the `RichTextElement` for mouse/typed
    /// input — and by `input_key`, the shell's keyboard-routing entry point).
    pub fn apply(&mut self, action: &EditAction, ctx: &mut ViewContext<Self>) {
        // Read-only editors swallow every text-mutating action.
        if self.read_only && action.is_mutating() {
            return;
        }
        // First edit promotes a preview tab to a permanent one.
        if action.is_mutating() {
            self.note_edit();
        }
        let model = self.model.clone();
        match action {
            EditAction::CursorPlace { offset } => {
                // A mouse-down inside the editor begins a possible selection drag.
                self.selecting.set(true);
                // Click Locations are render-space; the buffer has a leading
                // sentinel, so add 1 to land the caret buffer-side (matches the
                // -1 the selection→render sync applies).
                let off = offset.add_signed(1);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| s.set_cursor(off, sctx));
                });
            }
            EditAction::SelectLineAt { offset } => {
                // Triple-click: select the whole line under the click, same
                // line-boundary math + buffer-offset (+1) convention as the
                // empty-selection whole-line path in `copy`/`cut`.
                self.selecting.set(true);
                let text = self.buffer_text(ctx);
                let (ls, le) = line_char_range(&text, offset.as_usize());
                let sel_start = CharOffset::from(ls).add_signed(1);
                let sel_end = CharOffset::from(le).add_signed(1);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| {
                        s.set_cursor(sel_start, sctx);
                        s.set_last_head(sel_end, sctx);
                    });
                });
            }
            EditAction::GotoDefinitionAt { offset } => {
                // Place the caret at the click, exactly like CursorPlace …
                self.selecting.set(false);
                let boff = offset.add_signed(1);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| s.set_cursor(boff, sctx));
                });
                // … then fire the goto callback with the 0-based (line, char) at
                // the click (LSP positions are 0-based). Clone the Rc so the
                // borrow of `self.goto_cb` ends before we hand `ctx` to it.
                if let Some(cb) = self.goto_cb.clone() {
                    let text = self.buffer_text(ctx);
                    let (line, ch) = Self::line_char_at_offset(&text, offset.as_usize());
                    cb(line, ch, ctx);
                }
                ctx.notify();
                return;
            }
            EditAction::SelectionExtend { offset } => {
                // Only extend if the drag STARTED inside the editor. A pane
                // splitter / header drag passes the cursor over the editor but
                // never sent a mouse-down here, so `selecting` stays false.
                if !self.selecting.get() {
                    return;
                }
                // Drag: move the head to the offset (render→buffer +1), keeping
                // the anchor → a range.
                let off = offset.add_signed(1);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| s.set_last_head(off, sctx));
                });
            }
            EditAction::EndSelect => {
                self.selecting.set(false);
            }
            EditAction::InsertChars(chars) => {
                let chars = chars.clone();
                // Auto-close brackets: typing an opener inserts the pair and
                // places the cursor between them.
                let close = match chars.as_str() {
                    "{" => Some("}"),
                    "(" => Some(")"),
                    "[" => Some("]"),
                    _ => None,
                };
                // Skip-over: typing a closer when the next char is already that
                // same closer advances the caret past it (no doubled `))`).
                let is_closer = matches!(chars.as_str(), "}" | ")" | "]");
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    if is_closer {
                        let (start, end) = {
                            let sel = m.selection.as_ref(mctx);
                            (
                                sel.selection_start(mctx).as_usize(),
                                sel.selection_end(mctx).as_usize(),
                            )
                        };
                        // Only skip over on an empty selection (single cursor).
                        if end == start {
                            let text = m.buffer.as_ref(mctx).text().into_string();
                            let char_idx = end.saturating_sub(1);
                            let next_char = text.chars().nth(char_idx);
                            if next_char.map(|c| c.to_string()).as_deref() == Some(chars.as_str())
                            {
                                let sel = m.selection.clone();
                                sel.update(mctx, |s, sctx| {
                                    let cur = s.selection_end(sctx);
                                    s.set_cursor(cur.add_signed(1), sctx);
                                });
                                return;
                            }
                        }
                    }
                    match close {
                        Some(c) => {
                            let pair = format!("{chars}{c}");
                            m.user_insert(&pair, mctx);
                            let sel = m.selection.clone();
                            sel.update(mctx, |s, sctx| {
                                let end = s.selection_end(sctx);
                                s.set_cursor(end.add_signed(-1), sctx);
                            });
                        }
                        None => m.user_insert(&chars, mctx),
                    }
                });
            }
            EditAction::Backspace => {
                model.update(ctx, |m: &mut CodeModel, mctx| m.backspace(mctx));
            }
            EditAction::Enter => {
                // Port of old egui `auto_indent_context`: copy the current
                // line's indent, bump one level after an opening bracket, and
                // when the caret sits directly before a closer drop that closer
                // onto its own line at the parent indent. Also dedents after a
                // closing bracket.
                let indent = self.indent_unit.clone();
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let text = m.buffer.as_ref(mctx).text().into_string();
                    // Buffer offset 0 is a sentinel; text char index = off - 1.
                    let cursor_off = m.selection.as_ref(mctx).cursors(mctx).last().as_usize();
                    let char_idx = cursor_off.saturating_sub(1);
                    let byte = crate::format::char_idx_to_byte(&text, char_idx);
                    let (prev_indent, bump, dedent) =
                        crate::format::auto_indent_context(&text, byte);
                    let next_is_close = text
                        .as_bytes()
                        .get(byte)
                        .map(|c| matches!(c, b'}' | b')' | b']'))
                        .unwrap_or(false);
                    let dedented_indent = remove_one_indent(&prev_indent, indent.chars().count());
                    let body_indent = if bump {
                        format!("{prev_indent}{indent}")
                    } else if dedent && next_is_close {
                        // e.g. caret between `}` and `)` — keep at brace level.
                        prev_indent.clone()
                    } else if dedent {
                        dedented_indent
                    } else {
                        prev_indent.clone()
                    };
                    // First newline + the body-line indent.
                    m.enter(mctx);
                    if !body_indent.is_empty() {
                        m.user_insert(&body_indent, mctx);
                    }
                    // Caret sits directly before a closer after a bump/dedent:
                    // push that closer onto its own line at the parent indent.
                    if next_is_close && (bump || dedent) {
                        let tail = format!("\n{prev_indent}");
                        m.user_insert(&tail, mctx);
                        if bump {
                            // Old advance = 1 + body_indent chars: land the caret
                            // on the (indented) body line, above the closer.
                            let back = 1 + prev_indent.chars().count();
                            let sel = m.selection.clone();
                            sel.update(mctx, |s, sctx| {
                                let end = s.selection_end(sctx);
                                s.set_cursor(end.add_signed(-(back as isize)), sctx);
                            });
                        }
                        // dedent && next_is_close: old leaves the caret at the end
                        // (after the trailing indent) — no move needed.
                    }
                });
            }
            EditAction::CursorMove { dir, unit, extend } => {
                let (dir, unit, extend) = (*dir, unit.clone(), *extend);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| {
                        if extend {
                            s.extend_selection(dir, unit, sctx);
                        } else {
                            s.move_selection(dir, unit, sctx);
                        }
                    });
                });
            }
            EditAction::Scroll { delta, axis } => {
                let (d, a) = (*delta, *axis);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let rs = m.render_state.clone();
                    rs.update(mctx, |r, rctx| match a {
                        Axis::Vertical => r.scroll(d, rctx),
                        Axis::Horizontal => r.scroll_horizontal(d, rctx),
                    });
                });
            }
            EditAction::ScrollToFrac(frac) => {
                let frac = frac.clamp(0.0, 1.0);
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let rs = m.render_state.clone();
                    rs.update(mctx, |r, rctx| {
                        let content_h = r.height().as_f32();
                        let view_h = r.viewport().height().as_f32();
                        let cur = r.viewport().scroll_top().as_f32();
                        let target = (frac * (content_h - view_h).max(0.0)).max(0.0);
                        r.scroll(Pixels::new(target - cur), rctx);
                    });
                });
            }
            EditAction::Indent { outdent } => {
                let outdent = *outdent;
                model.update(ctx, |m: &mut CodeModel, mctx| m.indent(outdent, mctx));
            }
            EditAction::PageScroll { down } => {
                let down = *down;
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let rs = m.render_state.clone();
                    // A page in pixels, and how many rows fit in it. Rows lay out
                    // at PARAGRAPH_MIN_HEIGHT — the same constant the gutter uses —
                    // so the caret page and the gutter line numbers stay aligned.
                    let view_h = rs.as_ref(mctx).viewport().height().as_f32();
                    let lines =
                        ((view_h / PARAGRAPH_MIN_HEIGHT.as_f32()).floor() as usize).max(1);
                    // Move the viewport one page …
                    rs.update(mctx, |r, rctx| {
                        let delta = if down { view_h } else { -view_h };
                        r.scroll(Pixels::new(delta), rctx);
                    });
                    // … and move the caret the same page of lines so it follows the
                    // view instead of being left behind (otherwise a later arrow key
                    // snaps the viewport back to the stale caret position).
                    let dir = if down {
                        TextDirection::Forwards
                    } else {
                        TextDirection::Backwards
                    };
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| {
                        for _ in 0..lines {
                            s.move_selection(dir, TextUnit::Line, sctx);
                        }
                    });
                });
            }
            // ── Comment / move / duplicate line ────────────────────────────
            EditAction::ToggleComment => {
                self.toggle_comment(ctx);
                return;
            }
            EditAction::MoveLine { down } => {
                self.move_line(*down, ctx);
                return;
            }
            EditAction::DuplicateLine => {
                self.duplicate_line(ctx);
                return;
            }
            EditAction::ReloadFromDisk => {
                self.reload_from_disk(ctx);
                return;
            }
            EditAction::DismissDiskBanner => {
                // Keep the in-buffer edits; just re-baseline so the banner closes.
                self.refresh_disk_mtime();
                ctx.notify();
                return;
            }
            EditAction::DismissSaveError => {
                self.save_error = None;
                ctx.notify();
                return;
            }
            // ── Find / Replace / Goto ──────────────────────────────────────
            EditAction::FindNext { forward } => {
                self.find_step(*forward, ctx);
                return;
            }
            EditAction::FindClose => {
                self.close_find(ctx);
                return;
            }
            EditAction::FindReplaceCurrent => {
                self.replace_current(ctx);
                return;
            }
            EditAction::FindReplaceAll => {
                self.replace_all(ctx);
                return;
            }
            EditAction::FindFocusField { find_field } => {
                if let Some(f) = self.find.as_mut() {
                    f.find_field_active = *find_field;
                }
                ctx.notify();
                return;
            }
        }
        ctx.notify();
    }

    /// Shell keyboard-routing entry point: translate a raw `Keystroke` into an
    /// `EditAction` and apply it. The shell owns keyboard input (per-view focus
    /// is unreliable), so typing/backspace/enter reach the editor through here
    /// rather than the element's own focus-delivered events.
    pub fn input_key(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        // Find / Replace / Goto shortcuts — handled here IF the editor view
        // receives the keystroke. (The shell currently owns most Cmd shortcuts;
        // if it never routes these here, the public `open_find` / `open_replace`
        // / `open_goto_line` methods are the hookup — a 1-line shell keybinding.)
        if ks.cmd || ks.ctrl {
            match ks.key.as_str() {
                "f" => self.open_find(ctx),
                "h" => self.open_replace(ctx),
                "g" => self.open_goto_line(ctx),
                _ => {}
            }
            return;
        }
        // While the find bar is open, all typing is captured by it.
        if self.find.is_some() {
            match ks.key.as_str() {
                "escape" => self.close_find(ctx),
                "enter" | "return" | "numpadenter" => {
                    let mode = self.find.as_ref().map(|f| f.mode);
                    match mode {
                        Some(FindMode::Goto) => self.do_goto_line(ctx),
                        // Enter = next, Shift+Enter = previous.
                        _ => self.find_step(!ks.shift, ctx),
                    }
                }
                "backspace" | "delete" => self.find_backspace(ctx),
                "space" => self.find_type(" ", ctx),
                // Tab toggles between the find and replace fields (Replace mode).
                "tab" => {
                    if let Some(f) = self.find.as_mut() {
                        if f.mode == FindMode::Replace {
                            f.find_field_active = !f.find_field_active;
                        }
                    }
                    ctx.notify();
                }
                k if k.chars().count() == 1 => self.find_type(k, ctx),
                _ => {}
            }
            return;
        }
        // Caret motion: arrows / home / end. Alt = by word; Shift = extend selection.
        let word = || TextUnit::Word(WordBoundariesPolicy::Default);
        let mv = |dir, unit| EditAction::CursorMove {
            dir,
            unit,
            extend: ks.shift,
        };
        let action = match ks.key.as_str() {
            "backspace" | "delete" => EditAction::Backspace,
            "enter" | "return" | "numpadenter" => EditAction::Enter,
            // Shift+Tab outdents; plain Tab indents. "backtab" is an alternate
            // name some backends send for Shift+Tab.
            "tab" if ks.shift => EditAction::Indent { outdent: true },
            "tab" => EditAction::Indent { outdent: false },
            "backtab" => EditAction::Indent { outdent: true },
            "space" => EditAction::InsertChars(" ".to_string()),
            "left" => mv(
                TextDirection::Backwards,
                if ks.alt { word() } else { TextUnit::Character },
            ),
            "right" => mv(
                TextDirection::Forwards,
                if ks.alt { word() } else { TextUnit::Character },
            ),
            // Alt+Up / Alt+Down move the current line; Alt+Shift+Down duplicates
            // it. These must precede the plain up/down caret-motion arms.
            "up" if ks.alt => EditAction::MoveLine { down: false },
            "down" if ks.alt && ks.shift => EditAction::DuplicateLine,
            "down" if ks.alt => EditAction::MoveLine { down: true },
            "up" => mv(TextDirection::Backwards, TextUnit::Line),
            "down" => mv(TextDirection::Forwards, TextUnit::Line),
            "home" => mv(TextDirection::Backwards, TextUnit::LineBoundary),
            "end" => mv(TextDirection::Forwards, TextUnit::LineBoundary),
            "pageup" => EditAction::PageScroll { down: false },
            "pagedown" => EditAction::PageScroll { down: true },
            k if k.chars().count() == 1 => EditAction::InsertChars(k.to_string()),
            _ => return,
        };
        self.apply(&action, ctx);
    }
}

impl View for WarpEditorView {
    fn ui_name() -> &'static str {
        "WarpEditorView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let render_state = self.model.as_ref(app).render_state.clone();
        // The render line the caret is on, for the active-line gutter highlight.
        let cursor_line = {
            let m = self.model.as_ref(app);
            let sel = m.selection.clone();
            let cur = sel.as_ref(app).cursors(app);
            let off = *cur.last();
            Some(m.render_state.as_ref(app).offset_to_softwrap_point(off).row())
        };
        // Gutter git-diff markers: recompute only when the cache is dirty (set on
        // save / external git op / reload) — never per paint.
        let diff_map = {
            let mut d = self.diff.borrow_mut();
            if d.dirty {
                d.map = compute_diff(&self.path);
                d.dirty = false;
            }
            d.map.clone()
        };
        let gutter = crate::warpui::gutter_element::GutterElement::new(
            render_state.clone(),
            self.gutter_font,
            crate::warpui::fontsize::editor(),
            PARAGRAPH_MIN_HEIGHT.as_f32(),
            cursor_line,
            theme::text_muted(),
            theme::text(),
            theme::surface(),
        )
        .with_diff(diff_map)
        .finish();
        // Show the caret only when THIS editor pane holds keyboard focus (the
        // shell `ctx.focus()`es the active pane). Also gates the RichTextElement's
        // IBeam-on-hover logic.
        let focused = self
            .self_handle
            .upgrade(app)
            .map(|h| h.is_focused(app))
            .unwrap_or(false);
        let element = RichTextElement::new(
            render_state.clone(),
            self.self_handle.clone(),
            DisplayOptions {
                // Off, else warp paints blue-dashed block outlines + red margin marks.
                debug_bounds: false,
                focused,
                ..DisplayOptions::default()
            },
            self.display_state.clone(),
            None,
            Vec::new(),
        )
        .finish();
        // NOTE: no `on_keydown` here. The shell owns keyboard input and routes it
        // to the focused pane via `SendKeys → input_key`. An `on_keydown` on this
        // element would fire for the WHOLE window (the top-level shell dispatches
        // keydown through the entire tree), so it would swallow Enter/Backspace
        // from the terminal whenever an editor pane is merely open.
        let editor = EventHandler::new(element)
            .on_scroll_wheel(|ctx, _app, delta, _mods| {
                // The bare RichTextElement doesn't consume wheel events, so forward
                // them to RenderState::scroll (it viewports itself). Dominant axis
                // wins so a mostly-vertical gesture doesn't jitter horizontally.
                // 3x for a responsive wheel feel (raw pixel delta scrolls too slow).
                let (dx, dy) = (delta.x() * 3.0, delta.y() * 3.0);
                if dy.abs() >= dx.abs() {
                    ctx.dispatch_typed_action(EditAction::Scroll {
                        delta: Pixels::new(dy),
                        axis: Axis::Vertical,
                    });
                } else {
                    ctx.dispatch_typed_action(EditAction::Scroll {
                        delta: Pixels::new(dx),
                        axis: Axis::Horizontal,
                    });
                }
                DispatchEventResult::StopPropagation
            })
            .on_mouse_in(
                |ctx, _app, _pos| {
                    // Assert the I-beam every mouse-move frame (not just on the
                    // hover transition, which the parent SplitBox's per-frame
                    // reset_cursor would otherwise clobber). High z wins.
                    ctx.set_cursor(Cursor::IBeam, ZIndex::Overlay(10_000));
                    DispatchEventResult::PropagateToParent
                },
                None,
            )
            .finish();
        let on_drag: std::rc::Rc<dyn Fn(&mut warpui::elements::EventContext, f32)> =
            std::rc::Rc::new(|ctx, frac| {
                ctx.dispatch_typed_action(EditAction::ScrollToFrac(frac));
            });
        let scrollbar = crate::warpui::scrollbar_element::ScrollbarElement::new(
            render_state.clone(),
            theme::border(),
        )
        .draggable(self.scrollbar_drag.clone(), on_drag)
        .finish();
        let body = Flex::row()
            .with_child(gutter)
            .with_child(Expanded::new(1.0, editor).finish())
            .with_child(scrollbar)
            .finish();
        // Optional bars stacked above the editor body: the external-change reload
        // banner (highest), then the Find / Replace / Goto bar when open. A
        // failed save outranks the disk banner — data the user thinks is saved
        // but isn't beats data someone else changed.
        // The reload banner only shows when a reload would lose something: the
        // buffer is dirty, or the file vanished (nothing to silently re-read).
        // Clean buffers are silently reloaded by the shell's disk-change poll.
        let banner = if let Some(err) = &self.save_error {
            Some(self.save_error_banner(err))
        } else if self.disk_changed_throttled() && (self.is_dirty(app) || !self.path.exists()) {
            Some(self.reload_banner())
        } else {
            None
        };
        // Each find-bar button dispatches an `EditAction` that routes back to
        // `handle_action` → `apply` (same mechanism as the scroll wheel).
        let find_bar = self.find.as_ref().map(|find| {
            type Cb = std::rc::Rc<dyn Fn(&mut warpui::elements::EventContext)>;
            let mk = |action: EditAction| -> Cb {
                std::rc::Rc::new(move |ctx: &mut warpui::elements::EventContext| {
                    ctx.dispatch_typed_action(action.clone());
                })
            };
            FindBarElement::new(
                find.bar_mode(),
                find.query.clone(),
                find.replace.clone(),
                find.matches.len(),
                find.current,
                find.find_field_active,
                self.gutter_font,
                mk(EditAction::FindNext { forward: false }),
                mk(EditAction::FindNext { forward: true }),
                mk(EditAction::FindClose),
                mk(EditAction::FindReplaceCurrent),
                mk(EditAction::FindReplaceAll),
                mk(EditAction::FindFocusField { find_field: true }),
                mk(EditAction::FindFocusField { find_field: false }),
            )
            .finish()
        });
        if banner.is_none() && find_bar.is_none() {
            return body;
        }
        let mut col = Flex::column();
        if let Some(b) = banner {
            col = col.with_child(b);
        }
        if let Some(fb) = find_bar {
            col = col.with_child(fb);
        }
        col.with_child(Expanded::new(1.0, body).finish()).finish()
    }
}

impl WarpEditorView {
    /// The external-change reload banner shown at the top of the editor pane when
    /// the file changed on disk (`disk_changed`). "Reload" re-reads the file;
    /// "Keep" dismisses (keeps the in-buffer edits, re-baselines the mtime).
    /// NOTE: the on-disk stat that decides whether to show this is throttled to
    /// `DISK_STAT_INTERVAL` (see `disk_changed_throttled`), so the per-frame
    /// render path doesn't hit the filesystem every paint.
    fn reload_banner(&self) -> Box<dyn Element> {
        let msg = Container::new(
            Text::new(
                "This file changed on disk".to_string(),
                self.gutter_font,
                crate::warpui::fontsize::editor(),
            )
            .with_color(theme::warning())
            .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(12.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish();
        let reload = self.banner_button("Reload", theme::accent(), EditAction::ReloadFromDisk);
        let keep = self.banner_button("Keep", theme::surface(), EditAction::DismissDiskBanner);
        let row = Flex::row()
            .with_child(Expanded::new(1.0, msg).finish())
            .with_child(reload)
            .with_child(keep)
            .finish();
        Container::new(row)
            .with_background_color(theme::surface())
            .with_border(Border::bottom(1.0).with_border_color(theme::border()))
            .finish()
    }

    /// Red banner shown while `save_error` is set — the last Cmd+S did NOT
    /// reach disk. Dismiss hides it; the buffer stays dirty so Cmd+S retries.
    fn save_error_banner(&self, err: &str) -> Box<dyn Element> {
        let msg = Container::new(
            Text::new(
                err.to_string(),
                self.gutter_font,
                crate::warpui::fontsize::editor(),
            )
            .with_color(theme::error())
            .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(12.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish();
        let dismiss =
            self.banner_button("Dismiss", theme::surface(), EditAction::DismissSaveError);
        let row = Flex::row()
            .with_child(Expanded::new(1.0, msg).finish())
            .with_child(dismiss)
            .finish();
        Container::new(row)
            .with_background_color(theme::surface())
            .with_border(Border::bottom(1.0).with_border_color(theme::border()))
            .finish()
    }

    /// A small padded, clickable label used by the reload banner.
    fn banner_button(&self, label: &str, bg: ColorU, action: EditAction) -> Box<dyn Element> {
        EventHandler::new(
            Container::new(
                Text::new(
                    label.to_string(),
                    self.gutter_font,
                    crate::warpui::fontsize::editor(),
                )
                .with_color(theme::text())
                .finish(),
            )
            .with_background_color(bg)
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }
}

impl EditorView for WarpEditorView {
    type RichTextAction = EditAction;
    fn runnable_command_at<'a>(
        &self,
        _b: CharOffset,
        _x: &'a AppContext,
    ) -> Option<&'a dyn RunnableCommandModel> {
        None
    }
    fn embedded_item_at<'a>(
        &self,
        _b: CharOffset,
        _x: &'a AppContext,
    ) -> Option<&'a dyn EmbeddedItemModel> {
        None
    }

    fn text_decorations<'a>(
        &'a self,
        _viewport: rangemap::RangeSet<CharOffset>,
        version: Option<warp_editor::content::version::BufferVersion>,
        ctx: &'a AppContext,
    ) -> warp_editor::editor::TextDecoration<'a> {
        // Re-highlight lazily: the render state hands us the buffer version it is
        // about to paint, so recompute the color map whenever the content changes
        // (version bumps) or the user switches syntax themes. Computing once in
        // `new()` and returning it forever smeared colors after the first edit
        // (every range drifted by the edit delta) and went stale on theme change.
        // TODO(threading, audit editor H1): this whole-file `highlight()` runs
        // synchronously on the PAINT thread every time the buffer version bumps
        // (i.e. per keystroke) — the single worst stall in the editor domain on a
        // large file. Deferred deliberately, not overlooked: `text_decorations`
        // takes `&self` + `&AppContext` and must return a `TextDecoration`
        // synchronously — it has no `ViewContext` to `ctx.spawn` from, and the
        // color map is keyed by `BufferVersion`, so an off-thread result applied
        // to a newer buffer would smear every range (exactly the bug the comment
        // below warns about). A safe async version needs the highlight kicked off
        // from the editor's own edit/update path into a shared last-known map that
        // this reads — an editor render-path refactor out of scope for this pass,
        // and explicitly fenced off ("do not destabilize the editor"). Cheap fix
        // available meanwhile: re-highlight only the edited line range instead of
        // the whole file.
        let theme_name = crate::theme::current().syntax_theme.clone();
        let map = {
            let mut cache = self.colors.borrow_mut();
            if cache.version != version || cache.theme != theme_name {
                let text = self.model.as_ref(ctx).buffer.as_ref(ctx).text().to_string();
                cache.map = highlight(&text, &self.path);
                cache.version = version;
                cache.theme = theme_name;
            }
            cache.map.clone()
        };
        warp_editor::editor::TextDecoration {
            override_color_map: Some(map),
            ..Default::default()
        }
    }
}
