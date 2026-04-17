use crate::terminal_view;
use crate::views::{browser_view, diff_view, file_view, markdown_view};
use crate::workspace::{Dir, Node, PaneContent, PaneId, Workspace};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

pub const HEADER_H: f32 = 26.0;
const BORDER_W: f32 = 1.0;
const SPLITTER_W: f32 = 4.0;

const FOCUS_BORDER: Color32 = Color32::from_rgb(100, 140, 220);
const INACTIVE_BORDER: Color32 = Color32::from_rgb(36, 40, 52);
const HEADER_BG_ACTIVE: Color32 = Color32::from_rgb(30, 34, 48);
const HEADER_BG_INACTIVE: Color32 = Color32::from_rgb(22, 25, 36);
const HEADER_FG: Color32 = Color32::from_rgb(200, 204, 220);
const HEADER_FG_DIM: Color32 = Color32::from_rgb(130, 136, 150);
const CLOSE_HOVER_BG: Color32 = Color32::from_rgb(180, 60, 60);
const SPLITTER_COLOR: Color32 = Color32::from_rgb(22, 25, 36);

pub enum PaneAction {
    None,
    Focus(PaneId),
    Close(PaneId),
    ResizeSplit { path: Vec<usize>, ratio: f32 },
}

pub fn render_workspace(
    ui: &mut egui::Ui,
    workspace: &mut Workspace,
    font_size: f32,
    rect: Rect,
) -> PaneAction {
    let mut action = PaneAction::None;
    let root = workspace.root.take();
    if let Some(root) = root {
        render_node(ui, workspace, &root, rect, font_size, &mut action, &[]);
        workspace.root = Some(root);
    }
    action
}

fn render_node(
    ui: &mut egui::Ui,
    workspace: &mut Workspace,
    node: &Node,
    rect: Rect,
    font_size: f32,
    action: &mut PaneAction,
    path: &[usize],
) {
    match node {
        Node::Leaf(id) => {
            render_pane(ui, workspace, *id, rect, font_size, action);
        }
        Node::Split {
            direction,
            first,
            second,
            ratio,
        } => {
            let (r1, splitter, r2) = split_rect(rect, *direction, *ratio);
            let mut first_path = path.to_vec();
            first_path.push(0);
            let mut second_path = path.to_vec();
            second_path.push(1);
            render_node(ui, workspace, first, r1, font_size, action, &first_path);
            render_node(ui, workspace, second, r2, font_size, action, &second_path);
            render_splitter(ui, splitter, *direction, path, rect, action);
        }
    }
}

fn split_rect(rect: Rect, dir: Dir, ratio: f32) -> (Rect, Rect, Rect) {
    match dir {
        Dir::Horizontal => {
            let split = rect.min.x + rect.width() * ratio;
            let left = Rect::from_min_max(rect.min, Pos2::new(split - SPLITTER_W * 0.5, rect.max.y));
            let splitter = Rect::from_min_max(
                Pos2::new(split - SPLITTER_W * 0.5, rect.min.y),
                Pos2::new(split + SPLITTER_W * 0.5, rect.max.y),
            );
            let right =
                Rect::from_min_max(Pos2::new(split + SPLITTER_W * 0.5, rect.min.y), rect.max);
            (left, splitter, right)
        }
        Dir::Vertical => {
            let split = rect.min.y + rect.height() * ratio;
            let top = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, split - SPLITTER_W * 0.5));
            let splitter = Rect::from_min_max(
                Pos2::new(rect.min.x, split - SPLITTER_W * 0.5),
                Pos2::new(rect.max.x, split + SPLITTER_W * 0.5),
            );
            let bottom =
                Rect::from_min_max(Pos2::new(rect.min.x, split + SPLITTER_W * 0.5), rect.max);
            (top, splitter, bottom)
        }
    }
}

fn render_splitter(
    ui: &mut egui::Ui,
    rect: Rect,
    dir: Dir,
    path: &[usize],
    parent: Rect,
    action: &mut PaneAction,
) {
    ui.painter().rect_filled(rect, 0.0, SPLITTER_COLOR);
    let id = egui::Id::new(("splitter", path.to_vec()));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(match dir {
            Dir::Horizontal => egui::CursorIcon::ResizeHorizontal,
            Dir::Vertical => egui::CursorIcon::ResizeVertical,
        });
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let ratio = match dir {
                Dir::Horizontal => (pos.x - parent.min.x) / parent.width(),
                Dir::Vertical => (pos.y - parent.min.y) / parent.height(),
            };
            *action = PaneAction::ResizeSplit {
                path: path.to_vec(),
                ratio,
            };
        }
    }
}

fn render_pane(
    ui: &mut egui::Ui,
    workspace: &mut Workspace,
    id: PaneId,
    rect: Rect,
    font_size: f32,
    action: &mut PaneAction,
) {
    let is_focus = workspace.focus == Some(id);
    let border_color = if is_focus {
        FOCUS_BORDER
    } else {
        INACTIVE_BORDER
    };

    ui.painter().rect_stroke(
        rect,
        4.0,
        Stroke::new(BORDER_W, border_color),
        StrokeKind::Inside,
    );

    let inner = rect.shrink(BORDER_W);
    let header_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), HEADER_H));
    let body_outer = Rect::from_min_max(Pos2::new(inner.min.x, inner.min.y + HEADER_H), inner.max);
    let body_rect = body_outer.shrink2(Vec2::new(5.0, 3.0));

    render_header(ui, workspace, id, header_rect, is_focus, action);

    let pane = match workspace.panes.get_mut(&id) {
        Some(p) => p,
        None => return,
    };
    let clicked_inside = ui.input(|i| {
        i.pointer.primary_clicked()
            && i.pointer
                .interact_pos()
                .map(|p| rect.contains(p))
                .unwrap_or(false)
    });
    if clicked_inside && !is_focus {
        *action = PaneAction::Focus(id);
    }

    ui.painter()
        .rect_filled(body_outer, 0.0, Color32::from_rgb(14, 16, 24));
    let mut child = ui.new_child(UiBuilder::new().max_rect(body_rect));
    child.set_clip_rect(body_rect);

    match &mut pane.content {
        PaneContent::Terminal(term) => {
            terminal_view::render_terminal(&mut child, term, font_size, is_focus);
        }
        PaneContent::Files(files) => {
            file_view::render(&mut child, id, files, font_size, &mut pane.title);
        }
        PaneContent::Markdown(md) => {
            markdown_view::render(&mut child, md, font_size, &mut pane.title);
        }
        PaneContent::Diff(diff) => {
            diff_view::render(&mut child, diff, font_size, &mut pane.title);
        }
        PaneContent::Browser(browser) => {
            browser_view::render(&mut child, browser, &mut pane.title);
        }
    }
}

fn render_header(
    ui: &mut egui::Ui,
    workspace: &Workspace,
    id: PaneId,
    rect: Rect,
    is_focus: bool,
    action: &mut PaneAction,
) {
    let bg = if is_focus {
        HEADER_BG_ACTIVE
    } else {
        HEADER_BG_INACTIVE
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let pane = match workspace.panes.get(&id) {
        Some(p) => p,
        None => return,
    };

    let close_size = rect.height();
    let close_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - close_size, rect.min.y),
        Vec2::splat(close_size),
    );
    let close_response = ui.interact(close_rect, egui::Id::new(("close", id)), Sense::click());
    if close_response.hovered() {
        ui.painter().rect_filled(close_rect, 0.0, CLOSE_HOVER_BG);
    }
    ui.painter().text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        "×",
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
        HEADER_FG,
    );
    if close_response.clicked() {
        *action = PaneAction::Close(id);
    }

    let title_rect = Rect::from_min_max(
        Pos2::new(rect.min.x + 10.0, rect.min.y),
        Pos2::new(close_rect.min.x - 6.0, rect.max.y),
    );
    let title_response = ui.interact(title_rect, egui::Id::new(("header", id)), Sense::click());
    if title_response.clicked() {
        *action = PaneAction::Focus(id);
    }
    let label = format!("{}  ·  {}", pane.title, pane.content.kind_label());
    let fg = if is_focus { HEADER_FG } else { HEADER_FG_DIM };
    ui.painter().text(
        Pos2::new(title_rect.min.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        fg,
    );
}
