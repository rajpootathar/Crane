use crate::terminal::Terminal;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Vec2};

const BG: Color32 = Color32::from_rgb(14, 16, 24);
const FG: Color32 = Color32::from_rgb(176, 180, 192);

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

    let snapshot = {
        let guard = terminal.term.lock();
        let content = guard.renderable_content();
        let cursor = (content.cursor.point.column.0, content.cursor.point.line.0);
        let cells: Vec<_> = content
            .display_iter
            .map(|item| (item.point, item.cell.clone()))
            .collect();
        (cells, cursor)
    };
    let (cells, (cursor_col, cursor_line)) = snapshot;

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

    ui.input(|i| {
        for event in &i.events {
            match event {
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
