//! `WarpEditorView` — the file editor pane backed by Warp's REAL text editor
//! (`warp_editor`), so the file pane is warp-quality (cursor, click, wrap,
//! selection, undo) instead of the hand-rolled `FileView`. This is the v1
//! single-file editor; multi-tab wraps several of these later.

use std::sync::Arc;

use string_offset::CharOffset;
use warp_editor::content::buffer::Buffer;
use warp_editor::content::selection_model::BufferSelectionModel;
use warp_editor::content::text::{IndentBehavior, TextStyles};
use warp_editor::editor::{EditorView, EmbeddedItemModel, RunnableCommandModel};
use warp_editor::model::{CoreEditorModel, PlainTextEditorModel};
use warp_editor::render::element::{
    DisplayOptions, DisplayState, DisplayStateHandle, RichTextAction, RichTextElement,
};
use warp_editor::render::model::{
    BrokenLinkStyle, CheckBoxStyle, HorizontalRuleStyle, InlineCodeStyle, Location, ParagraphStyles,
    RenderState, RichTextStyles, TableStyle, DEFAULT_BLOCK_SPACINGS, PARAGRAPH_MIN_HEIGHT,
};
use warp_editor::selection::SelectionModel;
use warpui::color::ColorU;
use warpui::elements::{Axis, Border, DispatchEventResult, Element, EventHandler, Fill};
use warpui::event::ModifiersState;
use warpui::fonts::{FamilyId, Weight};
use warpui::units::Pixels;
use warpui::{
    AppContext, Entity, ModelHandle, TypedActionView, View, ViewContext, WeakViewHandle,
};

use rangemap::RangeMap;

use crate::warpui::theme;

const BASELINE: f32 = 0.78;

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
    let mut hl = HighlightLines::new(syntax, crate::views::file_view::fallback_theme());
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
        text_color: theme::TEXT,
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
        placeholder_color: theme::TEXT_MUTED,
        selection_fill: solid(theme::ROW_ACTIVE),
        cursor_fill: solid(theme::ACCENT),
        inline_code_style: InlineCodeStyle {
            font_family: font,
            background: theme::SURFACE,
            font_color: theme::TEXT,
        },
        check_box_style: CheckBoxStyle {
            border_width: 2.0,
            border_color: theme::BORDER,
            icon_path: "bundled/svg/check-thick.svg",
            background: theme::SURFACE,
            hover_background: theme::ROW_HOVER,
        },
        horizontal_rule_style: HorizontalRuleStyle {
            rule_height: 1.0,
            color: theme::BORDER,
        },
        broken_link_style: BrokenLinkStyle {
            icon_path: "bundled/svg/link-broken-02.svg",
            icon_color: theme::ERROR,
        },
        block_spacings: DEFAULT_BLOCK_SPACINGS,
        show_placeholder_text_on_empty_block: false,
        minimum_paragraph_height: Some(PARAGRAPH_MIN_HEIGHT),
        cursor_width: 2.0,
        highlight_urls: true,
        table_style: TableStyle {
            border_color: theme::BORDER,
            header_background: theme::SURFACE,
            cell_background: theme::BG,
            alternate_row_background: None,
            text_color: theme::TEXT,
            header_text_color: theme::TEXT,
            scrollbar_nonactive_thumb_color: theme::BORDER,
            scrollbar_active_thumb_color: theme::ACCENT,
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
    InsertChars(String),
    Backspace,
    Enter,
    Scroll { delta: Pixels, axis: Axis },
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
        vec![]
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

    /// Write the buffer back to disk (Cmd+S). Returns true on success.
    pub fn save(&self, app: &AppContext) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        let buffer = self.model.as_ref(app).buffer.clone();
        let text = buffer.as_ref(app).text().to_string();
        std::fs::write(&self.path, text).is_ok()
    }

    pub fn new(
        ctx: &mut ViewContext<Self>,
        content: String,
        font: FamilyId,
        path: std::path::PathBuf,
    ) -> Self {
        let buffer = ctx.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let buffer_sel = ctx.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        let bsel2 = buffer_sel.clone();
        buffer.update(ctx, |buf, mctx| {
            *buf = Buffer::from_plain_text(
                &content,
                None,
                Box::new(|_, _| IndentBehavior::Ignore),
                bsel2,
                mctx,
            );
        });
        let st = styles(font);
        let render_state = ctx.add_model(|mctx| RenderState::new(st, false, None, mctx));
        let selection = {
            let (b, r, bs) = (buffer.clone(), render_state.clone(), buffer_sel.clone());
            ctx.add_model(|mctx| SelectionModel::new(b, r, bs, None, mctx))
        };
        let model = ctx.add_model(|_| CodeModel {
            buffer,
            buffer_sel,
            selection,
            render_state,
        });
        let colors = highlight(&content, &path);
        WarpEditorView {
            model,
            self_handle: ctx.handle(),
            display_state: Arc::new(DisplayState::default()),
            path,
            colors,
        }
    }
}

impl Entity for WarpEditorView {
    type Event = ();
}

impl TypedActionView for WarpEditorView {
    type Action = EditAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        let model = self.model.clone();
        match action {
            EditAction::CursorPlace { offset } | EditAction::SelectionExtend { offset } => {
                let off = *offset;
                model.update(ctx, |m: &mut CodeModel, mctx| {
                    let sel = m.selection.clone();
                    sel.update(mctx, |s, sctx| s.set_cursor(off, sctx));
                });
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
                model.update(ctx, |m: &mut CodeModel, mctx| m.enter(mctx));
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
        }
        ctx.notify();
    }
}

impl View for WarpEditorView {
    fn ui_name() -> &'static str {
        "WarpEditorView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let render_state = self.model.as_ref(app).render_state.clone();
        let element = RichTextElement::new(
            render_state,
            self.self_handle.clone(),
            DisplayOptions::default(),
            self.display_state.clone(),
            None,
            Vec::new(),
        )
        .finish();
        // The element handles typed chars + mouse itself, but NOT editing keys —
        // intercept Backspace/Delete/Enter and dispatch them.
        EventHandler::new(element)
            .on_keydown(|ctx, _app, ks| {
                if ks.cmd || ks.ctrl || ks.alt {
                    return DispatchEventResult::PropagateToParent;
                }
                match ks.key.as_str() {
                    "backspace" | "delete" => {
                        ctx.dispatch_typed_action(EditAction::Backspace);
                        DispatchEventResult::StopPropagation
                    }
                    "enter" | "return" | "numpadenter" => {
                        ctx.dispatch_typed_action(EditAction::Enter);
                        DispatchEventResult::StopPropagation
                    }
                    _ => DispatchEventResult::PropagateToParent,
                }
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
