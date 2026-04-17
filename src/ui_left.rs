use crate::state::App;
use crate::ui_util::icon_button;
use egui::{Color32, Pos2, Rect, Response, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icons;

pub const WIDTH: f32 = 240.0;

const HEADER: Color32 = Color32::from_rgb(140, 146, 162);
const TEXT: Color32 = Color32::from_rgb(210, 214, 226);
const MUTED: Color32 = Color32::from_rgb(150, 156, 172);
const ADD: Color32 = Color32::from_rgb(120, 210, 140);
const DEL: Color32 = Color32::from_rgb(220, 110, 110);
const ACCENT: Color32 = Color32::from_rgb(96, 140, 220);
const ROW_HOVER: Color32 = Color32::from_rgb(30, 34, 46);
const ROW_ACTIVE: Color32 = Color32::from_rgb(48, 56, 80);

const ROW_H: f32 = 26.0;
const INDENT_W: f32 = 14.0;
const CHEVRON_W: f32 = 14.0;

pub fn render(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let full = ui.available_rect_before_wrap();
    let footer_h = 44.0;
    let scroll_rect = Rect::from_min_max(full.min, Pos2::new(full.max.x, full.max.y - footer_h));
    let footer_rect = Rect::from_min_max(Pos2::new(full.min.x, full.max.y - footer_h), full.max);

    let mut scroll_ui = ui.new_child(egui::UiBuilder::new().max_rect(scroll_rect));
    scroll_ui.set_clip_rect(scroll_rect);
    render_tree(&mut scroll_ui, app, ctx);

    let mut footer_ui = ui.new_child(egui::UiBuilder::new().max_rect(footer_rect));
    footer_ui.set_clip_rect(footer_rect);
    footer_ui.painter().line_segment(
        [
            Pos2::new(footer_rect.min.x, footer_rect.min.y),
            Pos2::new(footer_rect.max.x, footer_rect.min.y),
        ],
        Stroke::new(1.0, Color32::from_rgb(36, 40, 52)),
    );
    footer_ui.add_space(8.0);
    footer_ui.horizontal(|ui| {
        ui.add_space(10.0);
        let btn = egui::Button::new(
            RichText::new(format!("{}  Add Project…", icons::FOLDER_PLUS))
                .size(12.5),
        )
        .min_size(Vec2::new(ui.available_width() - 10.0, 28.0));
        if ui
            .add(btn)
            .on_hover_text("Choose a folder")
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose project folder")
                .pick_folder()
            {
                app.add_project_from_path(path, ctx);
            }
        }
    });
}

fn render_tree(ui: &mut egui::Ui, app: &mut App, ctx: &egui::Context) {
    let _ = ctx;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("PROJECTS")
                .size(10.5)
                .color(HEADER)
                .strong(),
        );
    });
    ui.add_space(4.0);

    let mut set_active: Option<(u64, u64, u64)> = None;
    let mut toggle_project: Option<u64> = None;
    let mut toggle_worktree: Option<(u64, u64)> = None;
    let mut close_tab: Option<(u64, u64, u64)> = None;
    let mut new_tab_for_worktree: Option<(u64, u64)> = None;
    let mut new_workspace_for_project: Option<u64> = None;
    let mut remove_project: Option<u64> = None;

    egui::ScrollArea::vertical()
        .id_salt("left_projects")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for project in &app.projects {
                let row = draw_row(
                    ui,
                    RowConfig {
                        depth: 0,
                        expanded: Some(project.expanded),
                        leading: None,
                        label: &project.name,
                        is_active: false,
                        active_bar: false,
                        badge: None,
                    },
                );
                let project_trailing = draw_trailing(
                    ui,
                    row.rect,
                    row.hovered,
                    &[
                        (icons::PLUS, "New worktree", 0),
                        (icons::X, "Remove project", 1),
                    ],
                );
                if row.main_clicked {
                    toggle_project = Some(project.id);
                }
                if project_trailing[0] {
                    new_workspace_for_project = Some(project.id);
                }
                if project_trailing[1] {
                    remove_project = Some(project.id);
                }

                if project.expanded {
                    for wt in &project.worktrees {
                        let active_wt = app.active.map(|(_, w, _)| w == wt.id).unwrap_or(false);
                        let badge = wt.git_status.as_ref().and_then(|s| {
                            if s.added > 0 || s.deleted > 0 {
                                Some((s.added, s.deleted))
                            } else {
                                None
                            }
                        });
                        let wt_row = draw_row(
                            ui,
                            RowConfig {
                                depth: 1,
                                expanded: Some(wt.expanded),
                                leading: Some(icons::GIT_BRANCH),
                                label: &wt.name,
                                is_active: active_wt,
                                active_bar: active_wt,
                                badge,
                            },
                        );
                        let wt_trailing = draw_trailing(
                            ui,
                            wt_row.rect,
                            wt_row.hovered,
                            &[(icons::PLUS, "New tab", 0)],
                        );
                        if wt_row.main_clicked {
                            toggle_worktree = Some((project.id, wt.id));
                        }
                        if wt_trailing[0] {
                            new_tab_for_worktree = Some((project.id, wt.id));
                        }

                        if wt.expanded {
                            for tab in &wt.tabs {
                                let is_active = app
                                    .active
                                    .map(|(_, w, t)| w == wt.id && t == tab.id)
                                    .unwrap_or(false);
                                let tab_row = draw_row(
                                    ui,
                                    RowConfig {
                                        depth: 2,
                                        expanded: None,
                                        leading: Some(icons::TERMINAL_WINDOW),
                                        label: &tab.name,
                                        is_active,
                                        active_bar: is_active,
                                        badge: None,
                                    },
                                );
                                let tab_trailing = draw_trailing(
                                    ui,
                                    tab_row.rect,
                                    tab_row.hovered,
                                    &[(icons::X, "Close tab", 0)],
                                );
                                if tab_row.main_clicked {
                                    set_active = Some((project.id, wt.id, tab.id));
                                }
                                if tab_trailing[0] {
                                    close_tab = Some((project.id, wt.id, tab.id));
                                }
                            }
                        }
                    }
                }
            }
        });

    let _ = (MUTED, icon_button as fn(&mut egui::Ui, &str, f32, &str) -> Response);

    if let Some(pid) = toggle_project {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            p.expanded = !p.expanded;
        }
    }
    if let Some((pid, wid)) = toggle_worktree {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.worktrees.iter_mut().find(|w| w.id == wid) {
                w.expanded = !w.expanded;
                if let Some(tid) = w.active_tab {
                    app.active = Some((pid, wid, tid));
                }
            }
        }
    }
    if let Some((pid, wid, tid)) = set_active {
        app.set_active(pid, wid, tid);
    }
    if let Some((pid, wid)) = new_tab_for_worktree {
        app.active = app.active.map(|(_, _, t)| (pid, wid, t)).or(Some((pid, wid, 0)));
        app.last_worktree = Some((pid, wid));
        app.new_tab_in_active_worktree(ctx);
    }
    if let Some(pid) = new_workspace_for_project {
        app.open_new_workspace_modal(pid);
    }
    if let Some(pid) = remove_project {
        app.remove_project(pid);
    }
    if let Some((pid, wid, tid)) = close_tab {
        if let Some(p) = app.projects.iter_mut().find(|p| p.id == pid) {
            if let Some(w) = p.worktrees.iter_mut().find(|w| w.id == wid) {
                w.tabs.retain(|t| t.id != tid);
                w.active_tab = w.tabs.first().map(|t| t.id);
                if app.active.map(|(_, _, t)| t == tid).unwrap_or(false) {
                    app.active = w.active_tab.map(|nt| (pid, wid, nt));
                }
                app.last_worktree = Some((pid, wid));
            }
        }
    }
}

struct RowConfig<'a> {
    depth: usize,
    expanded: Option<bool>,
    leading: Option<&'a str>,
    label: &'a str,
    is_active: bool,
    active_bar: bool,
    badge: Option<(usize, usize)>,
}

struct RowResult {
    rect: Rect,
    main_clicked: bool,
    hovered: bool,
}

fn draw_row(ui: &mut egui::Ui, cfg: RowConfig<'_>) -> RowResult {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    let hovered = response.hovered();

    let bg = if cfg.is_active {
        ROW_ACTIVE
    } else if hovered {
        ROW_HOVER
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 4.0, bg);
    }
    if cfg.active_bar {
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.min.x + 4.0, rect.min.y + 3.0),
                Vec2::new(2.0, rect.height() - 6.0),
            ),
            1.0,
            ACCENT,
        );
    }

    let mut cursor_x = rect.min.x + 12.0 + (cfg.depth as f32 * INDENT_W);

    if let Some(expanded) = cfg.expanded {
        let glyph = if expanded {
            icons::CARET_DOWN
        } else {
            icons::CARET_RIGHT
        };
        painter.text(
            Pos2::new(cursor_x + CHEVRON_W / 2.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            if cfg.is_active { TEXT } else { MUTED },
        );
        cursor_x += CHEVRON_W + 2.0;
    } else {
        cursor_x += CHEVRON_W + 2.0;
    }

    if let Some(leading) = cfg.leading {
        painter.text(
            Pos2::new(cursor_x + 8.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            leading,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            if cfg.is_active { ACCENT } else { MUTED },
        );
        cursor_x += 18.0;
    }

    let text_color = if cfg.is_active { TEXT } else { TEXT };
    painter.text(
        Pos2::new(cursor_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        cfg.label,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        text_color,
    );

    if let Some((added, deleted)) = cfg.badge {
        let mut bx = rect.max.x - 10.0;
        if added > 0 || deleted > 0 {
            if deleted > 0 {
                let s = format!("-{deleted}");
                let galley = painter.layout_no_wrap(
                    s.clone(),
                    egui::FontId::new(10.5, egui::FontFamily::Proportional),
                    DEL,
                );
                bx -= galley.size().x + 4.0;
                painter.galley(
                    Pos2::new(bx, rect.center().y - galley.size().y / 2.0),
                    galley,
                    DEL,
                );
            }
            if added > 0 {
                let s = format!("+{added}");
                let galley = painter.layout_no_wrap(
                    s.clone(),
                    egui::FontId::new(10.5, egui::FontFamily::Proportional),
                    ADD,
                );
                bx -= galley.size().x + 4.0;
                painter.galley(
                    Pos2::new(bx, rect.center().y - galley.size().y / 2.0),
                    galley,
                    ADD,
                );
            }
        }
    }

    if hovered || response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    RowResult {
        rect,
        main_clicked: response.clicked(),
        hovered,
    }
}

fn draw_trailing(
    ui: &mut egui::Ui,
    rect: Rect,
    row_hovered: bool,
    actions: &[(&str, &str, usize)],
) -> [bool; 2] {
    let mut out = [false, false];
    if !row_hovered {
        return out;
    }
    let size = 20.0;
    let mut x = rect.max.x - 8.0;
    for (icon, tip, slot) in actions.iter().rev() {
        x -= size;
        let btn_rect = Rect::from_min_size(
            Pos2::new(x, rect.center().y - size / 2.0),
            Vec2::splat(size),
        );
        let id = ui.id().with(("trailing", rect.min.x as i32, rect.min.y as i32, *slot));
        let resp = ui.interact(btn_rect, id, Sense::click()).on_hover_text(*tip);
        let painter = ui.painter_at(btn_rect);
        if resp.hovered() {
            painter.rect_filled(btn_rect, 4.0, Color32::from_rgb(56, 62, 82));
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        painter.text(
            btn_rect.center(),
            egui::Align2::CENTER_CENTER,
            *icon,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
            TEXT,
        );
        if resp.clicked() {
            out[*slot] = true;
        }
        x -= 2.0;
    }
    out
}
