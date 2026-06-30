//! `WarpEditorView` — the file editor pane backed by Warp's REAL text editor
//! (`warp_editor`), so the file pane is warp-quality (cursor, click, wrap,
//! selection, undo) instead of the hand-rolled `FileView`. This is the v1
//! single-file editor; multi-tab wraps several of these later.

use std::sync::Arc;

use string_offset::CharOffset;
use warp_editor::content::buffer::Buffer;
use warp_editor::content::selection_model::BufferSelectionModel;
use warp_editor::content::text::IndentBehavior;
use warp_editor::editor::{EditorView, EmbeddedItemModel, RunnableCommandModel};
use warp_editor::render::element::{
    DisplayOptions, DisplayState, DisplayStateHandle, RichTextAction, RichTextElement,
};
use warp_editor::render::model::{
    BrokenLinkStyle, CheckBoxStyle, HorizontalRuleStyle, InlineCodeStyle, Location, ParagraphStyles,
    RenderState, RichTextStyles, TableStyle, DEFAULT_BLOCK_SPACINGS, PARAGRAPH_MIN_HEIGHT,
};
use warpui::color::ColorU;
use warpui::elements::{Axis, Border, Element, Fill};
use warpui::event::ModifiersState;
use warpui::fonts::{FamilyId, Weight};
use warpui::geometry::vector::Vector2F;
use warpui::units::Pixels;
use warpui::{
    AppContext, Entity, ModelHandle, TypedActionView, View, ViewContext, WeakViewHandle,
};

use crate::warpui::theme;

const BASELINE: f32 = 0.78;

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

/// No-op editor action — read-only v1 (no typing yet). Editing comes next.
/// (`Action` is provided by warpui's blanket `impl<T> Action for T`.)
#[derive(Debug, Clone)]
pub enum EditAction {}

impl<V> RichTextAction<V> for EditAction {
    fn scroll(_d: Pixels, _a: Axis) -> Option<Self> {
        None
    }
    fn user_typed(_c: String, _v: &WeakViewHandle<V>, _x: &AppContext) -> Option<Self> {
        None
    }
    fn vim_user_typed(_c: String, _v: &WeakViewHandle<V>, _x: &AppContext) -> Option<Self> {
        None
    }
    fn left_mouse_down(
        _l: Location,
        _m: ModifiersState,
        _cc: u32,
        _fm: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        None
    }
    fn left_mouse_dragged(
        _l: Location,
        _cmd: bool,
        _sh: bool,
        _v: &WeakViewHandle<V>,
        _x: &AppContext,
    ) -> Option<Self> {
        None
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

pub struct WarpEditorView {
    buffer: ModelHandle<Buffer>,
    selection: ModelHandle<BufferSelectionModel>,
    render_state: ModelHandle<RenderState>,
    self_handle: WeakViewHandle<Self>,
    display_state: DisplayStateHandle,
}

impl WarpEditorView {
    pub fn new(ctx: &mut ViewContext<Self>, content: String, font: FamilyId) -> Self {
        let buffer = ctx.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = ctx.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        let sel2 = selection.clone();
        buffer.update(ctx, |buf, mctx| {
            *buf = Buffer::from_plain_text(
                &content,
                None,
                Box::new(|_, _| IndentBehavior::Ignore),
                sel2,
                mctx,
            );
        });
        let st = styles(font);
        let render_state = ctx.add_model(|mctx| RenderState::new(st, false, None, mctx));
        WarpEditorView {
            buffer,
            selection,
            render_state,
            self_handle: ctx.handle(),
            display_state: Arc::new(DisplayState::default()),
        }
    }
}

impl Entity for WarpEditorView {
    type Event = ();
}

impl TypedActionView for WarpEditorView {
    type Action = EditAction;
    fn handle_action(&mut self, _action: &Self::Action, _ctx: &mut ViewContext<Self>) {}
}

impl View for WarpEditorView {
    fn ui_name() -> &'static str {
        "WarpEditorView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        let _ = (&self.buffer, &self.selection);
        RichTextElement::new(
            self.render_state.clone(),
            self.self_handle.clone(),
            DisplayOptions::default(),
            self.display_state.clone(),
            None,
            Vec::new(),
        )
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
}

// Silence unused until editing lands.
impl WarpEditorView {
    #[allow(dead_code)]
    fn _touch(&self) -> Option<Vector2F> {
        None
    }
}
