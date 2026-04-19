use crate::state::layout::MarkdownPane;
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::path::Path;

pub fn render(ui: &mut egui::Ui, pane: &mut MarkdownPane, font_size: f32, title: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Path:");
        let response = ui.text_edit_singleline(&mut pane.input_buf);
        let load = ui.button("Load").clicked()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if load {
            load_md(pane, title);
        }
    });
    if let Some(err) = &pane.error {
        ui.colored_label(Color32::from_rgb(220, 100, 100), err);
        return;
    }
    if pane.content.is_empty() {
        ui.label("Enter a markdown path and press Load.");
        return;
    }
    ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| render_md(ui, &pane.content, font_size));
}

fn load_md(pane: &mut MarkdownPane, title: &mut String) {
    let path = pane.input_buf.trim();
    if path.is_empty() {
        pane.error = Some("Path is empty".into());
        return;
    }
    match std::fs::read_to_string(path) {
        Ok(s) => {
            pane.content = s;
            pane.path = path.to_string();
            pane.error = None;
            if let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) {
                *title = name.to_string();
            }
        }
        Err(e) => pane.error = Some(format!("{e}")),
    }
}

pub fn render_md(ui: &mut egui::Ui, src: &str, font_size: f32) {
    let parser = Parser::new_ext(src, Options::all());
    let mono = FontId::new(font_size, FontFamily::Monospace);
    let prop = FontId::new(font_size, FontFamily::Proportional);

    let mut bold = false;
    let mut italic = false;
    let code = false;
    let mut heading: Option<HeadingLevel> = None;
    let mut in_list = false;
    let mut in_code_block = false;
    let mut line_buf: Vec<RichText> = Vec::new();
    let fg = Color32::from_rgb(210, 214, 224);
    let dim = Color32::from_rgb(140, 146, 160);
    let accent = Color32::from_rgb(120, 170, 230);

    let flush = |ui: &mut egui::Ui, line: &mut Vec<RichText>| {
        if line.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for r in line.drain(..) {
                ui.label(r);
            }
        });
    };

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(ui, &mut line_buf);
                heading = Some(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(ui, &mut line_buf);
                heading = None;
                ui.add_space(6.0);
            }
            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,
            Event::Start(Tag::CodeBlock(_)) => {
                flush(ui, &mut line_buf);
                in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                ui.add_space(4.0);
            }
            Event::Start(Tag::List(_)) => {
                flush(ui, &mut line_buf);
                in_list = true;
            }
            Event::End(TagEnd::List(_)) => {
                in_list = false;
                ui.add_space(4.0);
            }
            Event::Start(Tag::Item) => {
                line_buf.push(RichText::new("• ").color(accent).font(prop.clone()));
            }
            Event::End(TagEnd::Item) => {
                flush(ui, &mut line_buf);
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush(ui, &mut line_buf);
                ui.add_space(4.0);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                line_buf.push(RichText::new("▍ ").color(dim).font(prop.clone()));
            }
            Event::End(TagEnd::BlockQuote) => {
                flush(ui, &mut line_buf);
            }
            Event::Code(text) => {
                let mut r = RichText::new(text.to_string())
                    .font(mono.clone())
                    .background_color(Color32::from_rgb(28, 32, 44))
                    .color(Color32::from_rgb(210, 180, 120));
                if bold { r = r.strong(); }
                line_buf.push(r);
            }
            Event::Text(text) => {
                if in_code_block {
                    for line in text.lines() {
                        ui.label(
                            RichText::new(line)
                                .font(mono.clone())
                                .color(Color32::from_rgb(210, 180, 120)),
                        );
                    }
                    continue;
                }
                let mut r = if let Some(level) = heading {
                    let scale = match level {
                        HeadingLevel::H1 => 1.8,
                        HeadingLevel::H2 => 1.5,
                        HeadingLevel::H3 => 1.3,
                        _ => 1.15,
                    };
                    RichText::new(text.to_string())
                        .font(FontId::new(font_size * scale, FontFamily::Proportional))
                        .color(accent)
                        .strong()
                } else {
                    RichText::new(text.to_string()).font(prop.clone()).color(fg)
                };
                if bold { r = r.strong(); }
                if italic { r = r.italics(); }
                if code { r = r.font(mono.clone()); }
                line_buf.push(r);
            }
            Event::SoftBreak | Event::HardBreak => {
                line_buf.push(RichText::new(" ").font(prop.clone()));
            }
            Event::Rule => {
                flush(ui, &mut line_buf);
                ui.separator();
            }
            _ => {}
        }
    }
    flush(ui, &mut line_buf);
    let _ = (in_list, code);
}
