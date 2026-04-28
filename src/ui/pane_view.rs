use crate::views::{browser_view, diff_view, file_view, markdown_view, welcome_view};
use crate::state::layout::{Dir, DockEdge, Layout, Node, PaneContent, PaneId};
use crate::theme;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};
use egui_phosphor::regular as icons;

pub const HEADER_H: f32 = 26.0;
const BORDER_W: f32 = 1.0;
const SPLITTER_W: f32 = 4.0;

fn focus_border() -> Color32 {
    theme::current().focus_border.to_color32()
}
fn inactive_border() -> Color32 {
    theme::current().inactive_border.to_color32()
}
fn header_bg_active() -> Color32 {
    theme::current().surface.to_color32()
}
fn header_bg_inactive() -> Color32 {
    theme::current().topbar_bg.to_color32()
}
fn header_fg() -> Color32 {
    theme::current().text.to_color32()
}
fn header_fg_dim() -> Color32 {
    theme::current().text_muted.to_color32()
}
fn close_hover_bg() -> Color32 {
    theme::current().error.to_color32()
}
fn splitter_color() -> Color32 {
    theme::current().divider.to_color32()
}
fn pane_body_bg() -> Color32 {
    theme::current().bg.to_color32()
}

#[derive(Clone, Copy)]
struct DragPayload(PaneId);

pub enum PaneAction {
    None,
    Focus(PaneId),
    Close(PaneId),
    ResizeSplit { path: Vec<usize>, ratio: f32 },
    SwapPanes { a: PaneId, b: PaneId },
    DockPane { src: PaneId, target: PaneId, edge: DockEdge },
    ToggleMaximize(PaneId),
    /// Welcome → Terminal: replace the focused pane's content with a
    /// freshly spawned Terminal. Applied in main.rs where `ctx` is
    /// available for the PTY spawn.
    ReplaceWithTerminal(PaneId),
    /// Welcome → Browser: replace the focused pane with a Browser
    /// carrying a single blank tab.
    ReplaceWithBrowser(PaneId),
    /// Welcome → show the Right Panel (Files tree).
    ShowFilesPanel,
}

fn dock_zone(rect: Rect, pos: Pos2) -> DockEdge {
    let rel_x = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    let rel_y = ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
    // A small 30 %× 30 % center square means the outer 35 % on any side
    // docks to that edge. This matches VS Code / Warp's feel — if the
    // pointer is clearly closer to one edge, that edge wins.
    let center_min = 0.35;
    let center_max = 0.65;
    if rel_x >= center_min
        && rel_x <= center_max
        && rel_y >= center_min
        && rel_y <= center_max
    {
        return DockEdge::Center;
    }
    let dx = rel_x - 0.5;
    let dy = rel_y - 0.5;
    if dx.abs() >= dy.abs() {
        if dx < 0.0 {
            DockEdge::Left
        } else {
            DockEdge::Right
        }
    } else if dy < 0.0 {
        DockEdge::Top
    } else {
        DockEdge::Bottom
    }
}

fn zone_rect(rect: Rect, edge: DockEdge) -> Rect {
    match edge {
        DockEdge::Center => rect,
        DockEdge::Left => {
            Rect::from_min_size(rect.min, Vec2::new(rect.width() * 0.5, rect.height()))
        }
        DockEdge::Right => Rect::from_min_max(
            Pos2::new(rect.min.x + rect.width() * 0.5, rect.min.y),
            rect.max,
        ),
        DockEdge::Top => {
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), rect.height() * 0.5))
        }
        DockEdge::Bottom => Rect::from_min_max(
            Pos2::new(rect.min.x, rect.min.y + rect.height() * 0.5),
            rect.max,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_layout(
    ui: &mut egui::Ui,
    layout: &mut Layout,
    font_size: f32,
    rect: Rect,
    syntax_theme_override: Option<&str>,
    diagnostics_for: &dyn Fn(&str) -> Vec<crate::lsp::Diagnostic>,
    notify_saved: &dyn Fn(&str, &str),
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    goto_request: &dyn Fn(&str, u32, u32),
    workspace_root: Option<&std::path::Path>,
    prefs: crate::views::file_view::EditorPrefs,
) -> PaneAction {
    let mut action = PaneAction::None;
    // When a pane is maximized we bypass the layout tree entirely and
    // render that single pane at the full rect. Esc restores.
    let maximized_id = layout.maximized.and_then(|id| {
        if layout.panes.contains_key(&id) { Some(id) } else { None }
    });
    if let Some(id) = maximized_id {
        render_pane(
            ui,
            layout,
            id,
            rect,
            font_size,
            &mut action,
            syntax_theme_override,
            diagnostics_for,
            notify_saved,
            format_before_save,
            goto_request,
            workspace_root,
            prefs,
        );
        if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            action = PaneAction::ToggleMaximize(id);
        }
        return action;
    }
    let root = layout.root.take();
    if let Some(root) = root {
        render_node(
            ui,
            layout,
            &root,
            rect,
            font_size,
            &mut action,
            &[],
            syntax_theme_override,
            diagnostics_for,
            notify_saved,
            format_before_save,
            goto_request,
            workspace_root,
            prefs,
        );
        layout.root = Some(root);
    }
    action
}

fn render_node(
    ui: &mut egui::Ui,
    layout: &mut Layout,
    node: &Node,
    rect: Rect,
    font_size: f32,
    action: &mut PaneAction,
    path: &[usize],
    syntax_theme_override: Option<&str>,
    diagnostics_for: &dyn Fn(&str) -> Vec<crate::lsp::Diagnostic>,
    notify_saved: &dyn Fn(&str, &str),
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    goto_request: &dyn Fn(&str, u32, u32),
    workspace_root: Option<&std::path::Path>,
    prefs: crate::views::file_view::EditorPrefs,
) {
    match node {
        Node::Leaf(id) => {
            render_pane(
                ui,
                layout,
                *id,
                rect,
                font_size,
                action,
                syntax_theme_override,
                diagnostics_for,
                notify_saved,
                format_before_save,
                goto_request,
                workspace_root,
                prefs,
            );
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
            render_node(
                ui,
                layout,
                first,
                r1,
                font_size,
                action,
                &first_path,
                syntax_theme_override,
                diagnostics_for,
                notify_saved,
                format_before_save,
                goto_request,
                workspace_root,
                prefs,
            );
            render_node(
                ui,
                layout,
                second,
                r2,
                font_size,
                action,
                &second_path,
                syntax_theme_override,
                diagnostics_for,
                notify_saved,
                format_before_save,
                goto_request,
                workspace_root,
                prefs,
            );
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
    ui.painter().rect_filled(rect, 0.0, splitter_color());
    let id = egui::Id::new(("splitter", path.to_vec()));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(match dir {
            Dir::Horizontal => egui::CursorIcon::ResizeHorizontal,
            Dir::Vertical => egui::CursorIcon::ResizeVertical,
        });
    }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos() {
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

fn render_pane(
    ui: &mut egui::Ui,
    layout: &mut Layout,
    id: PaneId,
    rect: Rect,
    font_size: f32,
    action: &mut PaneAction,
    syntax_theme_override: Option<&str>,
    diagnostics_for: &dyn Fn(&str) -> Vec<crate::lsp::Diagnostic>,
    notify_saved: &dyn Fn(&str, &str),
    format_before_save: &dyn Fn(&str, &str) -> Option<String>,
    goto_request: &dyn Fn(&str, u32, u32),
    workspace_root: Option<&std::path::Path>,
    prefs: crate::views::file_view::EditorPrefs,
) {
    let is_focus = layout.focus == Some(id);
    let border_color = if is_focus {
        focus_border()
    } else {
        inactive_border()
    };

    let drag_payload = egui::DragAndDrop::payload::<DragPayload>(ui.ctx());
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let pointer_in = pointer.map(|p| rect.contains(p)).unwrap_or(false);
    let is_drop_target = drag_payload
        .as_ref()
        .map(|p| p.0 != id && pointer_in)
        .unwrap_or(false);
    let released = ui.input(|i| i.pointer.any_released());

    let drop_edge = if is_drop_target {
        pointer.map(|p| dock_zone(rect, p))
    } else {
        None
    };

    if is_drop_target && released
        && let Some(payload) = egui::DragAndDrop::take_payload::<DragPayload>(ui.ctx())
            && payload.0 != id
    {
        let edge = drop_edge.unwrap_or(DockEdge::Center);
        *action = if edge == DockEdge::Center {
            PaneAction::SwapPanes { a: payload.0, b: id }
        } else {
            PaneAction::DockPane { src: payload.0, target: id, edge }
        };
    }

    // No visible border on panes (Warp-style). Active/inactive is shown by
    // a subtle dim overlay painted after content renders, below.
    let _ = border_color;
    let inner = rect.shrink(BORDER_W);
    let header_rect = Rect::from_min_size(inner.min, Vec2::new(inner.width(), HEADER_H));
    let body_outer = Rect::from_min_max(Pos2::new(inner.min.x, inner.min.y + HEADER_H), inner.max);
    let body_rect = body_outer.shrink2(Vec2::new(5.0, 3.0));

    render_header(ui, layout, id, header_rect, is_focus, action);

    let pane = match layout.panes.get_mut(&id) {
        Some(p) => p,
        None => return,
    };
    // Focus on press (not on click-release) so starting a drag-selection
    // inside a non-focused pane still transfers focus to it. `primary_clicked`
    // only fires for press+release without drag, so a drag-to-select in a
    // sibling pane would otherwise leave focus on the old pane.
    let pressed_inside = ui.input(|i| {
        i.pointer.primary_pressed()
            && i.pointer
                .interact_pos()
                .map(|p| rect.contains(p))
                .unwrap_or(false)
    });
    if pressed_inside && !is_focus && matches!(action, PaneAction::None) {
        *action = PaneAction::Focus(id);
    }

    ui.painter()
        .rect_filled(body_outer, 0.0, pane_body_bg());
    let mut child = ui.new_child(UiBuilder::new().max_rect(body_rect));
    child.set_clip_rect(body_rect);

    // Welcome pane bubbles its button click out through this holder
    // so the match arm below can stay synchronous + borrow-clean.
    // Applied to `*action` after the match closes.
    let mut welcome_action_holder: Option<(PaneId, welcome_view::WelcomeAction)> = None;

    // Scope every pane's widgets under a unique id so that, e.g., two
    // Markdown panes' ScrollAreas don't fight over the same auto-id. egui
    // paints a red outline on id-collision and also pays a hashing cost
    // to detect them, so this helps both speed and the "red flash" bug.
    child.push_id(("pane_body", id), |child| match &mut pane.content {
        PaneContent::Terminal(tp) => {
            crate::terminal::view::render_terminal_pane(child, tp, font_size, is_focus, id);
        }
        PaneContent::Files(files) => {
            file_view::render(
                child,
                id,
                files,
                font_size,
                &mut pane.title,
                syntax_theme_override,
                diagnostics_for,
                notify_saved,
                format_before_save,
                goto_request,
                workspace_root,
                prefs,
            );
        }
        PaneContent::Markdown(md) => {
            markdown_view::render(child, md, font_size, &mut pane.title);
        }
        PaneContent::Diff(diff) => {
            diff_view::render(child, diff, font_size, &mut pane.title);
        }
        PaneContent::Browser(browser) => {
            browser_view::render(child, id, browser, &mut pane.title, is_drop_target, is_focus);
        }
        PaneContent::Welcome(_) => {
            if let Some(act) = welcome_view::render(child) {
                welcome_action_holder = Some((id, act));
            }
        }
    });
    if let Some((pid, wact)) = welcome_action_holder {
        *action = match wact {
            welcome_view::WelcomeAction::OpenTerminal => {
                PaneAction::ReplaceWithTerminal(pid)
            }
            welcome_view::WelcomeAction::OpenBrowser => {
                PaneAction::ReplaceWithBrowser(pid)
            }
            welcome_view::WelcomeAction::ToggleFilesPanel => PaneAction::ShowFilesPanel,
        };
    }

    // Warp-style active/inactive: dim inactive panes with a translucent
    // black overlay. No border, no highlight ring — just a subtle value
    // shift so your eye goes to the one you're working in.
    if !is_focus && drop_edge.is_none() {
        ui.painter().rect_filled(
            rect,
            4.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 45),
        );
    }

    // Drop-zone overlay is painted LAST so it sits above pane content and
    // only covers the actual target area (left/right/top/bottom half or the
    // middle square).
    if let Some(edge) = drop_edge {
        let zone = zone_rect(rect, edge);
        let painter = ui.painter();
        painter.rect_filled(zone, 4.0, Color32::from_rgba_unmultiplied(96, 140, 220, 90));
        painter.rect_stroke(
            zone,
            4.0,
            Stroke::new(2.0, Color32::from_rgb(96, 140, 220)),
            StrokeKind::Inside,
        );
    }
}

fn render_header(
    ui: &mut egui::Ui,
    layout: &Layout,
    id: PaneId,
    rect: Rect,
    is_focus: bool,
    action: &mut PaneAction,
) {
    let bg = if is_focus {
        header_bg_active()
    } else {
        header_bg_inactive()
    };
    ui.painter().rect_filled(rect, 0.0, bg);

    let pane = match layout.panes.get(&id) {
        Some(p) => p,
        None => return,
    };

    let btn_size = rect.height();
    let close_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - btn_size, rect.min.y),
        Vec2::splat(btn_size),
    );
    let close_response = ui.interact(close_rect, egui::Id::new(("close", id)), Sense::click());
    if close_response.hovered() {
        ui.painter().rect_filled(close_rect, 0.0, close_hover_bg());
    }
    ui.painter().text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        icons::X,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        header_fg(),
    );
    if close_response.clicked() {
        *action = PaneAction::Close(id);
    }

    // Maximize / restore button, pinned just left of close.
    let max_rect = Rect::from_min_size(
        Pos2::new(close_rect.min.x - btn_size, rect.min.y),
        Vec2::splat(btn_size),
    );
    let max_response = ui.interact(max_rect, egui::Id::new(("maximize", id)), Sense::click());
    if max_response.hovered() {
        ui.painter().rect_filled(
            max_rect,
            0.0,
            theme::current().row_hover.to_color32(),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let is_max = layout.maximized == Some(id);
    let glyph = if is_max {
        icons::ARROWS_IN_SIMPLE
    } else {
        icons::ARROWS_OUT_SIMPLE
    };
    ui.painter().text(
        max_rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        header_fg(),
    );
    let _ = max_response.clone().on_hover_text(if is_max { "Restore (Esc)" } else { "Maximize" });
    if max_response.clicked() {
        *action = PaneAction::ToggleMaximize(id);
    }

    let title_rect = Rect::from_min_max(
        Pos2::new(rect.min.x + 10.0, rect.min.y),
        Pos2::new(max_rect.min.x - 6.0, rect.max.y),
    );
    let title_response = ui.interact(
        title_rect,
        egui::Id::new(("header", id)),
        Sense::click_and_drag(),
    );
    if title_response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), DragPayload(id));
    }
    if title_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if title_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if title_response.clicked() {
        *action = PaneAction::Focus(id);
    }
    let label = format!("{}  ·  {}", pane.title, pane.content.kind_label());
    let fg = if is_focus { header_fg() } else { header_fg_dim() };
    ui.painter().text(
        Pos2::new(title_rect.min.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        fg,
    );
}
