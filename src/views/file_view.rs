use crate::workspace::FilesPane;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use egui_phosphor::regular as icons;

pub fn render(
    ui: &mut egui::Ui,
    pane_id: u64,
    pane: &mut FilesPane,
    font_size: f32,
    title: &mut String,
) {
    ui.push_id(("files_pane", pane_id), |ui| {
        render_inner(ui, pane, font_size, title);
    });
}

fn render_inner(ui: &mut egui::Ui, pane: &mut FilesPane, font_size: f32, title: &mut String) {
    if pane.tabs.is_empty() {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                RichText::new("No files open")
                    .size(14.0)
                    .color(Color32::from_rgb(200, 204, 220)),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Click a file in the Files sidebar to open it here")
                    .color(Color32::from_rgb(130, 136, 150))
                    .size(11.5),
            );
        });
        return;
    }

    // Tab bar
    let mut close_idx: Option<usize> = None;
    let mut activate_idx: Option<usize> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.add_space(4.0);
        for (idx, tab) in pane.tabs.iter().enumerate() {
            let is_active = idx == pane.active;
            let label = if tab.dirty() {
                format!("● {}", tab.name)
            } else {
                tab.name.clone()
            };
            let (clicked, close_clicked) = draw_file_tab(ui, &label, is_active, idx);
            if clicked {
                activate_idx = Some(idx);
            }
            if close_clicked {
                close_idx = Some(idx);
            }
        }
    });
    if let Some(idx) = activate_idx {
        pane.active = idx;
    }
    if let Some(idx) = close_idx {
        pane.close(idx);
        if pane.tabs.is_empty() {
            return;
        }
    }
    ui.add_space(2.0);

    let active_idx = pane.active.min(pane.tabs.len() - 1);
    pane.active = active_idx;

    // Save shortcut
    let save_pressed = ui.input(|i| {
        (i.modifiers.command || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::S)
    });

    {
        let tab = &mut pane.tabs[active_idx];
        let name_label = if tab.dirty() {
            format!("● {}", tab.name)
        } else {
            tab.name.clone()
        };
        *title = format!("Files · {name_label}");

        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(
                RichText::new(&tab.path)
                    .size(10.5)
                    .color(Color32::from_rgb(130, 136, 150)),
            );
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let save_btn = ui.add_enabled(
                        tab.dirty(),
                        egui::Button::new(
                            RichText::new(format!("{}  Save", icons::FLOPPY_DISK))
                                .size(11.5),
                        )
                        .min_size(egui::vec2(0.0, 24.0)),
                    );
                    if save_btn.clicked() || (save_pressed && tab.dirty()) {
                        if let Err(e) = std::fs::write(&tab.path, &tab.content) {
                            eprintln!("save failed: {e}");
                        } else {
                            tab.original_content = tab.content.clone();
                        }
                    }
                },
            );
        });
        ui.add_space(2.0);

        let font = FontId::new(font_size, FontFamily::Monospace);
        ScrollArea::both()
            .id_salt(("file_scroll", active_idx))
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let editor = egui::TextEdit::multiline(&mut tab.content)
                    .code_editor()
                    .font(font)
                    .desired_width(f32::INFINITY)
                    .desired_rows(30);
                ui.add(editor);
            });
    }
}

fn draw_file_tab(
    ui: &mut egui::Ui,
    name: &str,
    is_active: bool,
    idx: usize,
) -> (bool, bool) {
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let close_font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let text_w = ui
        .fonts_mut(|f| f.layout_no_wrap(name.to_string(), font.clone(), egui::Color32::WHITE))
        .size()
        .x;
    let padding_x = 10.0;
    let gap = 6.0;
    let close_size = 16.0;
    let height = 24.0;
    let width = padding_x + text_w + gap + close_size + padding_x - 2.0;

    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.max.x - padding_x - close_size + 2.0,
            rect.min.y + (height - close_size) / 2.0,
        ),
        egui::vec2(close_size, close_size),
    );
    let close_response = ui.interact(
        close_rect,
        ui.id().with(("file_tab_close", idx)),
        egui::Sense::click(),
    );

    let (bg, fg) = if is_active {
        (
            egui::Color32::from_rgb(56, 100, 170),
            egui::Color32::from_rgb(240, 243, 250),
        )
    } else if response.hovered() || close_response.hovered() {
        (
            egui::Color32::from_rgb(42, 47, 62),
            egui::Color32::from_rgb(220, 224, 236),
        )
    } else {
        (
            egui::Color32::TRANSPARENT,
            egui::Color32::from_rgb(170, 176, 190),
        )
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, bg);
    }
    ui.painter().text(
        egui::pos2(rect.min.x + padding_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        font,
        fg,
    );
    if close_response.hovered() {
        ui.painter().rect_filled(
            close_rect.shrink(1.0),
            4.0,
            egui::Color32::from_rgb(180, 60, 60),
        );
    }
    ui.painter().text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        icons::X,
        close_font,
        fg,
    );
    if response.hovered() || close_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (
        response.clicked() && !close_response.hovered(),
        close_response.clicked(),
    )
}
