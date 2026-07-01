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
    BrokenLinkStyle, CheckBoxStyle, HorizontalRuleStyle, InlineCodeStyle, Location, ParagraphStyles,
    RenderState, RichTextStyles, TableStyle, WidthSetting, DEFAULT_BLOCK_SPACINGS,
    PARAGRAPH_MIN_HEIGHT,
};
use warp_editor::selection::{SelectionModel, TextDirection, TextUnit};
use warpui::text::word_boundaries::WordBoundariesPolicy;
use warpui::color::ColorU;
use warpui::elements::{
    Axis, Border, DispatchEventResult, Element, EventHandler, Expanded, Fill, Flex, ParentElement,
    ZIndex,
};
use warpui::platform::Cursor;
use warpui::event::ModifiersState;
use warpui::fonts::{FamilyId, Weight};
use warpui::units::Pixels;
use warpui::{
    AppContext, Entity, ModelHandle, TypedActionView, View, ViewContext, WeakViewHandle,
};

use rangemap::RangeMap;

use crate::warpui::theme;

const BASELINE: f32 = 0.78;

/// The syntect theme to render with — mirrors the egui file view: honor the
/// user's configured `syntax_theme`, else a sensible dark default. NOT the empty
/// `fallback_theme()` (which paints near-black text that clashes with the UI).
fn render_theme() -> &'static syntect::highlighting::Theme {
    let all = &crate::views::file_view::themes().themes;
    let requested = crate::theme::current().syntax_theme.clone();
    all.get(&requested)
        .or_else(|| all.get("OneHalfDark"))
        .or_else(|| all.get("base16-eighties.dark"))
        .or_else(|| all.get("base16-ocean.dark"))
        .unwrap_or_else(|| {
            all.values()
                .next()
                .unwrap_or_else(|| crate::views::file_view::fallback_theme())
        })
}

/// Syntect-highlight `content` into a CharOffset→color map for warp's editor
/// `text_decorations`. Reuses the egui app's shared SyntaxSet + theme.
fn highlight(content: &str, path: &std::path::Path) -> RangeMap<CharOffset, ColorU> {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;
    let ss = crate::views::file_view::syntaxes();
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

/// Build a RichTextStyles for plain-code editing from our mono font + theme.
fn styles(font: FamilyId) -> RichTextStyles {
    let para = |tab: Option<u8>| ParagraphStyles {
        font_family: font,
        font_size: 13.0,
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
            font_size: 13.0,
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
        _m: ModifiersState,
        _cc: u32,
        _fm: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        Some(EditAction::CursorPlace { offset: offset_of(&l) })
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

pub struct WarpEditorView {
    model: ModelHandle<CodeModel>,
    self_handle: WeakViewHandle<Self>,
    display_state: DisplayStateHandle,
    path: std::path::PathBuf,
    /// Syntect color map (CharOffset → fg color) for syntax highlighting.
    colors: RangeMap<CharOffset, ColorU>,
    /// Mono font for the line-number gutter (same face as the editor text).
    gutter_font: FamilyId,
    /// Last on-disk content; `is_dirty` compares the live buffer against it.
    saved_text: String,
    /// True while a text-selection drag is active (a mouse-down landed inside
    /// the editor). Gates `SelectionExtend` so dragging the pane splitter or
    /// header — which never sends the editor a mouse-down — can't select text.
    selecting: std::cell::Cell<bool>,
    /// Persisted scrollbar-thumb drag state (element rebuilt each frame).
    scrollbar_drag: std::rc::Rc<std::cell::Cell<bool>>,
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
    /// Copy the current selection to the clipboard (Cmd+C).
    pub fn copy(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| {
            let content = m.read_selected_text_as_clipboard_content(mctx);
            mctx.clipboard().write(content);
        });
    }
    /// Cut the current selection to the clipboard (Cmd+X): writes to clipboard
    /// and deletes the selection.
    pub fn cut(&self, ctx: &mut ViewContext<Self>) {
        let m = self.model.clone();
        m.update(ctx, |m: &mut CodeModel, mctx| {
            m.delete(TextDirection::Backwards, TextUnit::Character, true, mctx);
        });
    }

    /// Write the buffer back to disk (Cmd+S). Returns true on success. Updates
    /// the saved snapshot so `is_dirty` reflects a clean state after saving.
    pub fn save(&mut self, app: &AppContext) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        let buffer = self.model.as_ref(app).buffer.clone();
        let text = buffer.as_ref(app).text().to_string();
        if std::fs::write(&self.path, &text).is_ok() {
            self.saved_text = text;
            true
        } else {
            false
        }
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
        let buffer = ctx.add_model(|_| {
            Buffer::new(Box::new(|_, _| IndentBehavior::TabIndent(IndentUnit::Space(4))))
        });
        let buffer_sel = ctx.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        let bsel2 = buffer_sel.clone();
        buffer.update(ctx, |buf, mctx| {
            *buf = Buffer::from_plain_text(
                &content,
                None,
                Box::new(|_, _| IndentBehavior::TabIndent(IndentUnit::Space(4))),
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
        let colors = highlight(&content, &path);
        WarpEditorView {
            model,
            self_handle: ctx.handle(),
            display_state: Arc::new(DisplayState::default()),
            path,
            colors,
            gutter_font: font,
            saved_text: content,
            selecting: std::cell::Cell::new(false),
            scrollbar_drag: std::rc::Rc::new(std::cell::Cell::new(false)),
        }
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
                model.update(ctx, |m: &mut CodeModel, mctx| match close {
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
                });
            }
            EditAction::Backspace => {
                model.update(ctx, |m: &mut CodeModel, mctx| m.backspace(mctx));
            }
            EditAction::Enter => {
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    // Capture the leading whitespace of the current line so Enter
                    // auto-indents to match the previous line's indentation.
                    let leading_ws: String = {
                        let cursor_off =
                            m.selection.as_ref(mctx).cursors(mctx).last().as_usize();
                        // Buffer offset 0 is a sentinel; text characters start at index
                        // cursor_off - 1 within the string returned by text().
                        let text = m.buffer.as_ref(mctx).text().into_string();
                        let text_idx = cursor_off.saturating_sub(1).min(text.len());
                        let line_start = text[..text_idx]
                            .rfind('\n')
                            .map(|p| p + 1)
                            .unwrap_or(0);
                        text[line_start..text_idx]
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect()
                    };
                    m.enter(mctx);
                    if !leading_ws.is_empty() {
                        m.user_insert(&leading_ws, mctx);
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
                    rs.update(mctx, |r, rctx| {
                        let view_h = r.viewport().height().as_f32();
                        let delta = if down { view_h } else { -view_h };
                        r.scroll(Pixels::new(delta), rctx);
                    });
                });
            }
        }
        ctx.notify();
    }

    /// Shell keyboard-routing entry point: translate a raw `Keystroke` into an
    /// `EditAction` and apply it. The shell owns keyboard input (per-view focus
    /// is unreliable), so typing/backspace/enter reach the editor through here
    /// rather than the element's own focus-delivered events.
    pub fn input_key(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        // Command/control shortcuts are handled by the shell, not inserted.
        if ks.cmd || ks.ctrl {
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
        let gutter = crate::warpui::gutter_element::GutterElement::new(
            render_state.clone(),
            self.gutter_font,
            13.0,
            PARAGRAPH_MIN_HEIGHT.as_f32(),
            cursor_line,
            theme::text_muted(),
            theme::text(),
            theme::surface(),
        )
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
        Flex::row()
            .with_child(gutter)
            .with_child(Expanded::new(1.0, editor).finish())
            .with_child(scrollbar)
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
        _version: Option<warp_editor::content::version::BufferVersion>,
        _ctx: &'a AppContext,
    ) -> warp_editor::editor::TextDecoration<'a> {
        // Syntax highlighting via the precomputed syntect color map.
        warp_editor::editor::TextDecoration {
            override_color_map: Some(self.colors.clone()),
            ..Default::default()
        }
    }
}
