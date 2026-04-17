use crate::terminal::Terminal;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Vec2};

const BG: Color32 = Color32::from_rgb(14, 16, 24);
const FG: Color32 = Color32::from_rgb(176, 180, 192);
const SELECTION_BG: Color32 = Color32::from_rgba_premultiplied(60, 110, 180, 120);

fn point_in_selection(point: Point, range: &alacritty_terminal::selection::SelectionRange) -> bool {
    if range.is_block {
        point.line >= range.start.line
            && point.line <= range.end.line
            && point.column >= range.start.column
            && point.column <= range.end.column
    } else if point.line < range.start.line || point.line > range.end.line {
        false
    } else if range.start.line == range.end.line {
        point.column >= range.start.column && point.column <= range.end.column
    } else if point.line == range.start.line {
        point.column >= range.start.column
    } else if point.line == range.end.line {
        point.column <= range.end.column
    } else {
        true
    }
}

fn pixel_to_point(pos: Pos2, origin: Pos2, cell_w: f32, cell_h: f32, cols: usize, rows: usize) -> (Point, Side) {
    let rel_x = (pos.x - origin.x).max(0.0);
    let rel_y = (pos.y - origin.y).max(0.0);
    let col_f = rel_x / cell_w;
    let line_f = rel_y / cell_h;
    let col = (col_f.floor() as usize).min(cols.saturating_sub(1));
    let line = (line_f.floor() as usize).min(rows.saturating_sub(1));
    let side = if col_f - col_f.floor() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (Point::new(Line(line as i32), Column(col)), side)
}

pub fn render_terminal(ui: &mut egui::Ui, terminal: &mut Terminal, font_size: f32, has_focus: bool) {
    let font_id = FontId::new(font_size, FontFamily::Monospace);
    let cell_w = ui.fonts_mut(|f| f.glyph_width(&font_id, 'M'));
    let cell_h = ui.fonts_mut(|f| f.row_height(&font_id));

    let available = ui.available_size();
    let cols = ((available.x / cell_w).floor() as usize).max(20);
    let rows = ((available.y / cell_h).floor() as usize).max(5);
    terminal.resize(cols, rows);

    let (response, painter) = ui.allocate_painter(
        Vec2::new(cols as f32 * cell_w, rows as f32 * cell_h),
        Sense::click_and_drag().union(Sense::focusable_noninteractive()),
    );
    let origin = response.rect.min;

    painter.rect_filled(response.rect, 0.0, BG);

    // I-beam over the terminal so it feels like selectable text.
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // Drag: plain range select.
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows);
            let mut guard = terminal.term.lock();
            guard.selection = Some(Selection::new(SelectionType::Simple, point, side));
        }
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows);
            let mut guard = terminal.term.lock();
            if let Some(sel) = guard.selection.as_mut() {
                sel.update(point, side);
            }
        }
    }

    // Clicks: 1 → clear, 2 → word (Semantic), 3 → line (Lines),
    // Shift+click → extend existing selection to click point.
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows);
            let shift_held = ui.input(|i| i.modifiers.shift);
            let now = std::time::Instant::now();
            let is_multi = terminal
                .last_click
                .map(|(t, line, col)| {
                    now.duration_since(t) < std::time::Duration::from_millis(500)
                        && line == point.line.0
                        && col == point.column.0
                })
                .unwrap_or(false);
            terminal.click_count = if is_multi { terminal.click_count + 1 } else { 1 };
            terminal.last_click = Some((now, point.line.0, point.column.0));

            let mut guard = terminal.term.lock();
            if shift_held && guard.selection.is_some() {
                if let Some(sel) = guard.selection.as_mut() {
                    sel.update(point, side);
                }
            } else {
                match terminal.click_count {
                    2 => {
                        guard.selection =
                            Some(Selection::new(SelectionType::Semantic, point, Side::Left));
                    }
                    3 => {
                        guard.selection =
                            Some(Selection::new(SelectionType::Lines, point, Side::Left));
                    }
                    _ => {
                        guard.selection = None;
                    }
                }
            }
        }
    }

    let snapshot = {
        let guard = terminal.term.lock();
        let content = guard.renderable_content();
        let cursor = (content.cursor.point.column.0, content.cursor.point.line.0);
        let selection = content.selection;
        let cells: Vec<_> = content
            .display_iter
            .map(|item| (item.point, item.cell.clone()))
            .collect();
        (cells, cursor, selection)
    };
    let (cells, (cursor_col, cursor_line), selection) = snapshot;

    for (point, cell) in cells {
        let col = point.column.0;
        let line = point.line.0;
        if line < 0 {
            continue;
        }
        let line = line as usize;

        let x = origin.x + col as f32 * cell_w;
        let y = origin.y + line as f32 * cell_h;
        let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_w, cell_h));

        let bg = color_to_egui(cell.bg, false);
        if bg != BG {
            painter.rect_filled(rect, 0.0, bg);
        }

        let in_selection = selection
            .map(|sel| point_in_selection(point, &sel))
            .unwrap_or(false);
        if in_selection {
            painter.rect_filled(rect, 0.0, SELECTION_BG);
        }

        if cell.c != ' ' && cell.c != '\0' {
            let fg = color_to_egui(cell.fg, true);
            painter.text(
                Pos2::new(x, y),
                egui::Align2::LEFT_TOP,
                cell.c,
                font_id.clone(),
                fg,
            );
            if cell.flags.contains(CellFlags::UNDERLINE) {
                painter.line_segment(
                    [
                        Pos2::new(x, y + cell_h - 1.0),
                        Pos2::new(x + cell_w, y + cell_h - 1.0),
                    ],
                    egui::Stroke::new(1.0, fg),
                );
            }
        }
    }

    let cx = origin.x + cursor_col as f32 * cell_w;
    let cy = origin.y + cursor_line as f32 * cell_h;
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(cell_w, cell_h)),
        0.0,
        Color32::from_rgba_unmultiplied(176, 180, 192, 120),
    );

    if !has_focus {
        return;
    }

    let mut copy_text: Option<String> = None;
    let mut paste_text: Option<String> = None;
    ui.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Copy => {
                    let guard = terminal.term.lock();
                    if let Some(t) = guard.selection_to_string() {
                        if !t.is_empty() {
                            copy_text = Some(t);
                        }
                    }
                }
                egui::Event::Key {
                    key: egui::Key::A,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.mac_cmd || modifiers.command => {
                    let mut guard = terminal.term.lock();
                    let start = Point::new(Line(0), Column(0));
                    let end = Point::new(
                        Line(rows.saturating_sub(1) as i32),
                        Column(cols.saturating_sub(1)),
                    );
                    let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
                    sel.update(end, Side::Right);
                    guard.selection = Some(sel);
                }
                egui::Event::Paste(text) => {
                    if !text.is_empty() {
                        paste_text = Some(text.clone());
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.ctrl {
                        if let Some(letter) = key_letter(*key) {
                            terminal.write_input(&[letter - b'a' + 1]);
                            continue;
                        }
                    }
                    if modifiers.mac_cmd || modifiers.command {
                        continue;
                    }
                    if let Some(bytes) = named_key_bytes(*key) {
                        terminal.write_input(&bytes);
                    }
                }
                egui::Event::Text(text) => {
                    terminal.write_input(text.as_bytes());
                }
                _ => {}
            }
        }
    });
    if let Some(t) = copy_text {
        ui.ctx().copy_text(t);
    }
    if let Some(t) = paste_text {
        terminal.write_input(t.as_bytes());
    }
}

fn color_to_egui(color: TermColor, is_fg: bool) -> Color32 {
    match color {
        TermColor::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        TermColor::Indexed(idx) => {
            let (r, g, b) = palette(idx);
            Color32::from_rgb(r, g, b)
        }
        TermColor::Named(named) => match named {
            NamedColor::Foreground => FG,
            NamedColor::Background => BG,
            NamedColor::Cursor => FG,
            other => {
                let idx = other as u16;
                if idx < 16 {
                    let (r, g, b) = palette(idx as u8);
                    Color32::from_rgb(r, g, b)
                } else if is_fg {
                    FG
                } else {
                    BG
                }
            }
        },
    }
}

fn palette(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => (0x1a, 0x1c, 0x28),
        1 => (0xcc, 0x55, 0x55),
        2 => (0x44, 0xaa, 0x99),
        3 => (0xe8, 0x92, 0x2a),
        4 => (0x5a, 0x7a, 0xbf),
        5 => (0xaa, 0x66, 0xcc),
        6 => (0x55, 0xaa, 0xaa),
        7 => (0xb0, 0xb4, 0xc0),
        8 => (0x4a, 0x4c, 0x5a),
        9 => (0xff, 0x66, 0x66),
        10 => (0x55, 0xcc, 0xbb),
        11 => (0xff, 0xaa, 0x44),
        12 => (0x77, 0x99, 0xdd),
        13 => (0xcc, 0x77, 0xdd),
        14 => (0x77, 0xcc, 0xcc),
        15 => (0xdd, 0xdd, 0xee),
        16..=231 => {
            let i = idx - 16;
            let r = (i / 36) * 51;
            let g = ((i % 36) / 6) * 51;
            let b = (i % 6) * 51;
            (r, g, b)
        }
        232..=255 => {
            let gray = 8 + (idx - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn key_letter(key: egui::Key) -> Option<u8> {
    use egui::Key;
    match key {
        Key::A => Some(b'a'),
        Key::B => Some(b'b'),
        Key::C => Some(b'c'),
        Key::D => Some(b'd'),
        Key::E => Some(b'e'),
        Key::F => Some(b'f'),
        Key::G => Some(b'g'),
        Key::H => Some(b'h'),
        Key::I => Some(b'i'),
        Key::J => Some(b'j'),
        Key::K => Some(b'k'),
        Key::L => Some(b'l'),
        Key::M => Some(b'm'),
        Key::N => Some(b'n'),
        Key::O => Some(b'o'),
        Key::P => Some(b'p'),
        Key::Q => Some(b'q'),
        Key::R => Some(b'r'),
        Key::S => Some(b's'),
        Key::T => Some(b't'),
        Key::U => Some(b'u'),
        Key::V => Some(b'v'),
        Key::W => Some(b'w'),
        Key::X => Some(b'x'),
        Key::Y => Some(b'y'),
        Key::Z => Some(b'z'),
        _ => None,
    }
}

fn named_key_bytes(key: egui::Key) -> Option<Vec<u8>> {
    use egui::Key;
    match key {
        Key::Enter => Some(b"\r".to_vec()),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Backspace => Some(vec![0x7f]),
        Key::Escape => Some(vec![0x1b]),
        Key::ArrowUp => Some(b"\x1b[A".to_vec()),
        Key::ArrowDown => Some(b"\x1b[B".to_vec()),
        Key::ArrowRight => Some(b"\x1b[C".to_vec()),
        Key::ArrowLeft => Some(b"\x1b[D".to_vec()),
        Key::Home => Some(b"\x1b[H".to_vec()),
        Key::End => Some(b"\x1b[F".to_vec()),
        Key::PageUp => Some(b"\x1b[5~".to_vec()),
        Key::PageDown => Some(b"\x1b[6~".to_vec()),
        Key::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}
