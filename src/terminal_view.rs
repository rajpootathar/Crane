use crate::terminal::Terminal;
use crate::theme;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Vec2};

fn term_bg() -> Color32 {
    theme::current().terminal_bg.to_color32()
}
fn term_fg() -> Color32 {
    theme::current().terminal_fg.to_color32()
}
fn selection_bg() -> Color32 {
    let a = theme::current().accent;
    Color32::from_rgba_premultiplied(a.r, a.g, a.b, 100)
}

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

    let bg_theme = term_bg();
    painter.rect_filled(response.rect, 0.0, bg_theme);

    // I-beam over the terminal so it feels like selectable text.
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    // Scrollback: mouse wheel → alacritty Scroll::Delta. Positive delta
    // is upward in egui (history); alacritty's convention is that a
    // positive delta scrolls up into history. `scroll_display` is a
    // no-op when no history is available, so no guard needed.
    if response.hovered() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 0.5 {
            let lines = (wheel / cell_h).round() as i32;
            if lines != 0 {
                terminal.term.lock().scroll_display(Scroll::Delta(lines));
            }
        }
    }

    // Drag: plain range select.
    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows);
            let mut guard = terminal.term.lock();
            guard.selection = Some(Selection::new(SelectionType::Simple, point, side));
        }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos() {
            let (point, side) = pixel_to_point(pos, origin, cell_w, cell_h, cols, rows);
            let mut guard = terminal.term.lock();
            if let Some(sel) = guard.selection.as_mut() {
                sel.update(point, side);
            }
        }

    // Clicks: 1 → clear, 2 → word (Semantic), 3 → line (Lines),
    // Shift+click → extend existing selection to click point.
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos() {
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

    let snapshot = {
        let guard = terminal.term.lock();
        let content = guard.renderable_content();
        let offset = content.display_offset as i32;
        let cursor = (
            content.cursor.point.column.0,
            content.cursor.point.line.0 + offset,
        );
        let selection = content.selection;
        let cells: Vec<_> = content
            .display_iter
            .map(|item| (item.point, item.cell.clone()))
            .collect();
        // history_size lives on the Dimensions trait (Grid impls it).
        let history = guard.history_size();
        (cells, cursor, selection, offset, history)
    };
    let (cells, (cursor_col, cursor_line), selection, display_offset, history_size) = snapshot;

    // Group cells by line, then batch each line into a single LayoutJob
    // grouped by contiguous runs of same (fg, bg, flags). This cuts paint
    // calls from one-per-cell (~4800 for 120×40) down to a small handful
    // per row (~3–10), and hands the font layout off to egui once per row
    // instead of once per glyph.
    let cols_count = cols;
    let mut by_row: std::collections::BTreeMap<i32, Vec<(usize, alacritty_terminal::term::cell::Cell, bool)>> =
        std::collections::BTreeMap::new();
    for (point, cell) in cells {
        // alacritty yields display_iter items in grid-absolute line
        // coordinates, which go negative into history when the user has
        // scrolled up. Translate to viewport-local (0..screen_lines) by
        // adding the current display offset.
        let viewport_line = point.line.0 + display_offset;
        if viewport_line < 0 || viewport_line as usize >= rows {
            continue;
        }
        let in_selection = selection
            .map(|sel| point_in_selection(point, &sel))
            .unwrap_or(false);
        by_row
            .entry(viewport_line)
            .or_default()
            .push((point.column.0, cell, in_selection));
    }

    let fallback_fg = term_fg();
    for (line, mut row_cells) in by_row {
        row_cells.sort_by_key(|(c, _, _)| *c);
        let row_y = (origin.y + line as f32 * cell_h).round();
        let row_x = origin.x.round();
        let row_rect = Rect::from_min_size(
            Pos2::new(row_x, row_y),
            Vec2::new(cols_count as f32 * cell_w, cell_h.round()),
        );

        // Coalesce runs of same (fg, bg, underline) into one LayoutJob
        // section. Selection bg wins over cell bg.
        let mut job = LayoutJob::default();
        let mut cur_fg: Option<Color32> = None;
        let mut cur_bg: Option<Color32> = None;
        let mut cur_underline = false;
        let mut buf = String::new();

        let flush = |job: &mut LayoutJob,
                     buf: &mut String,
                     fg: Option<Color32>,
                     bg: Option<Color32>,
                     underline: bool| {
            if buf.is_empty() {
                return;
            }
            let color = fg.unwrap_or(fallback_fg);
            let background = bg
                .filter(|&b| b != bg_theme)
                .unwrap_or(Color32::TRANSPARENT);
            let stroke = if underline {
                egui::Stroke::new(1.0, color)
            } else {
                egui::Stroke::NONE
            };
            job.append(
                buf,
                0.0,
                TextFormat {
                    font_id: font_id.clone(),
                    color,
                    background,
                    underline: stroke,
                    ..Default::default()
                },
            );
            buf.clear();
        };

        // Walk columns strictly 0..cols_count, pulling the cell for
        // each column from `row_cells`. This keeps buf's character
        // count === visual column, which is the invariant that was
        // being violated (resized grids occasionally emit display_iter
        // cells with col values that no longer align to the current
        // viewport, leading to packed/misaligned text). Row_cells was
        // already sorted ascending above, so we walk both in lockstep.
        let mut idx = 0;
        let default_cell = alacritty_terminal::term::cell::Cell::default();
        for col in 0..cols_count {
            while idx < row_cells.len() && row_cells[idx].0 < col {
                idx += 1;
            }
            let (cell, in_selection) = if idx < row_cells.len() && row_cells[idx].0 == col {
                (&row_cells[idx].1, row_cells[idx].2)
            } else {
                (&default_cell, false)
            };
            // Skip the spacer half of a wide char — alacritty emits a
            // WIDE_CHAR cell followed by a WIDE_CHAR_SPACER for east-
            // asian glyphs; rendering both doubles them up visually.
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }
            let fg = color_to_egui(cell.fg, true);
            let bg = if in_selection {
                selection_bg()
            } else {
                color_to_egui(cell.bg, false)
            };
            let underline = cell.flags.contains(CellFlags::UNDERLINE);
            if Some(fg) != cur_fg || Some(bg) != cur_bg || underline != cur_underline {
                flush(&mut job, &mut buf, cur_fg, cur_bg, cur_underline);
                cur_fg = Some(fg);
                cur_bg = Some(bg);
                cur_underline = underline;
            }
            // Sanitize control characters that would otherwise break
            // galley layout. `\n` in particular was the smoking gun for
            // the "ls rows appear continuous" bug — egui wraps internally
            // on a newline, which collapses our row stride. `\t` / `\r`
            // are equally wrong to emit verbatim; treat all as spaces.
            let ch = match cell.c {
                '\0' | '\n' | '\r' | '\t' => ' ',
                c => c,
            };
            buf.push(ch);
        }
        flush(&mut job, &mut buf, cur_fg, cur_bg, cur_underline);

        if !job.is_empty() {
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(row_rect.min, galley, fallback_fg);
        }
    }

    // Snap cursor to integer pixels so it aligns with char cells. Subpixel
    // drift accumulates on long lines and makes the cursor look "off by
    // one" vs where the next character will print.
    let cx = (origin.x + cursor_col as f32 * cell_w).round();
    let cy = (origin.y + cursor_line as f32 * cell_h).round();
    let cw = cell_w.round();
    let ch = cell_h.round();
    let cursor_color = {
        let c = theme::current().terminal_fg;
        Color32::from_rgba_unmultiplied(c.r, c.g, c.b, 130)
    };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(cw, ch)),
        0.0,
        cursor_color,
    );

    // Scrollbar — right-edge thumb whose height reflects the visible
    // viewport's share of (history + viewport), and whose y reflects
    // the current display_offset. Drag scrolls; no scrollbar drawn
    // when there's no history yet.
    let total = history_size + rows;
    if history_size > 0 && total > rows {
        let track_w = 6.0;
        let track_rect = Rect::from_min_max(
            Pos2::new(response.rect.max.x - track_w, response.rect.min.y),
            Pos2::new(response.rect.max.x, response.rect.max.y),
        );
        let thumb_h = (track_rect.height() * rows as f32 / total as f32).max(20.0);
        // display_offset = 0 → thumb at bottom; display_offset = history
        // → thumb at top. The scrollable thumb range is
        // (track_height - thumb_h).
        let scrollable = (track_rect.height() - thumb_h).max(1.0);
        let y_from_top =
            scrollable * (1.0 - display_offset as f32 / history_size as f32);
        let thumb_rect = Rect::from_min_size(
            Pos2::new(track_rect.min.x, track_rect.min.y + y_from_top),
            Vec2::new(track_w, thumb_h),
        );
        let t = theme::current();
        let track_col = Color32::from_rgba_unmultiplied(255, 255, 255, 8);
        painter.rect_filled(track_rect, 3.0, track_col);
        let scroll_id = ui.id().with("terminal_scrollbar");
        let thumb_resp = ui.interact(thumb_rect, scroll_id, egui::Sense::drag());
        let thumb_col = if thumb_resp.dragged() {
            t.accent.to_color32()
        } else if thumb_resp.hovered() {
            Color32::from_rgba_unmultiplied(255, 255, 255, 90)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 55)
        };
        painter.rect_filled(thumb_rect, 3.0, thumb_col);
        if thumb_resp.hovered() || thumb_resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }
        if thumb_resp.dragged() {
            let dy = thumb_resp.drag_delta().y;
            // Drag down → positive dy → scroll toward newer content
            // (decrease display_offset). One thumb-pixel equals
            // `history / scrollable` history-lines.
            let lines_per_px = history_size as f32 / scrollable;
            let delta_lines = -(dy * lines_per_px).round() as i32;
            if delta_lines != 0 {
                terminal
                    .term
                    .lock()
                    .scroll_display(Scroll::Delta(delta_lines));
            }
        }
    }

    if !has_focus {
        return;
    }
    // True when another egui widget (e.g. tab-rename TextEdit) owns
    // keyboard focus. We still want terminal-level command shortcuts
    // (Cmd+K, Cmd+A, Copy, Paste) to work globally — only the raw-key
    // fall-through that writes to the PTY must skip in this case.
    let other_widget_focused = ui.memory(|m| m.focused().is_some());

    let mut copy_text: Option<String> = None;
    let mut paste_text: Option<String> = None;
    let mut clear_requested = false;
    ui.input(|i| {
        for event in &i.events {
            match event {
                egui::Event::Copy => {
                    let guard = terminal.term.lock();
                    if let Some(t) = guard.selection_to_string()
                        && !t.is_empty() {
                            copy_text = Some(t);
                        }
                }
                egui::Event::Key {
                    key: egui::Key::K,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.mac_cmd || modifiers.command => {
                    // Queue; actual work happens after the input
                    // closure unlocks Context. Driving the ANSI parser
                    // inside `ui.input` used to deadlock because
                    // alacritty's WakeListener calls
                    // ctx.request_repaint() on certain escape events,
                    // and that call takes a Context write lock while
                    // our ui.input closure still holds its read lock.
                    clear_requested = true;
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
                    // Another widget (rename TextEdit, branch-picker
                    // filter, etc.) owns keyboard focus — swallow the
                    // key so it doesn't also echo into the PTY.
                    if other_widget_focused {
                        continue;
                    }
                    if modifiers.ctrl
                        && let Some(letter) = key_letter(*key) {
                            terminal.write_input(&[letter - b'a' + 1]);
                            continue;
                        }
                    if modifiers.mac_cmd || modifiers.command {
                        continue;
                    }
                    // Alt/Option + arrow: emit word-navigation sequences
                    // most shells expect (bash, zsh, fish all read ESC b / f
                    // for word back / forward). Also covers Alt + letter as
                    // generic "ESC + <char>".
                    if modifiers.alt {
                        match *key {
                            egui::Key::ArrowLeft => {
                                terminal.write_input(b"\x1bb");
                                continue;
                            }
                            egui::Key::ArrowRight => {
                                terminal.write_input(b"\x1bf");
                                continue;
                            }
                            egui::Key::Backspace => {
                                // Alt+Backspace → delete previous word.
                                terminal.write_input(b"\x1b\x7f");
                                continue;
                            }
                            _ => {
                                if let Some(letter) = key_letter(*key) {
                                    terminal.write_input(&[0x1b, letter]);
                                    continue;
                                }
                            }
                        }
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
    if clear_requested {
        // \x1b[H → cursor home, \x1b[2J → erase display, \x1b[3J → erase
        // scrollback. Safe to run here because ui.input's read lock on
        // Context has been released, so alacritty's WakeListener calling
        // ctx.request_repaint() won't deadlock.
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        {
            let mut guard = terminal.term.lock();
            processor.advance(&mut *guard, b"\x1b[H\x1b[2J\x1b[3J");
            guard.scroll_display(Scroll::Bottom);
        }
        terminal.write_input(b"\x0c");
        terminal.history.lock().clear();
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
            NamedColor::Foreground => term_fg(),
            NamedColor::Background => term_bg(),
            NamedColor::Cursor => term_fg(),
            other => {
                let idx = other as u16;
                if idx < 16 {
                    let (r, g, b) = palette(idx as u8);
                    Color32::from_rgb(r, g, b)
                } else if is_fg {
                    term_fg()
                } else {
                    term_bg()
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
