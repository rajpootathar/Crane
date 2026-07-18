//! `WarpMarkdownView` — a self-contained warpui `View` that renders a Markdown
//! document. The warpui port of old Crane's `views/markdown_view.rs`: parse with
//! `pulldown_cmark` once at construction into an owned block model, then rebuild
//! warpui elements from that model each frame (elements are transient; the model
//! persists). Read-only in v1 — links/images render as plain text, matching the
//! old egui behavior.

use std::path::PathBuf;

use markdown_parser::{FormattedText, FormattedTextFragment, FormattedTextLine};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    FormattedTextElement, ParentElement, Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext};

use crate::warpui::theme;

/// Base prose font size. Headings scale off this; code is one point smaller.
const BASE: f32 = 14.0;
/// Line height ratio for wrapped prose (a touch of leading for readability).
const LINE_H: f32 = 1.35;
/// How many blocks to build per frame from the scroll offset. A render cap (NOT
/// a storage cap — the full parsed model is kept) so a huge doc can't blow up the
/// element tree. Mirrors `FileView::RENDER_LINES`.
const RENDER_BLOCKS: usize = 400;

// ── Owned block model (parsed once, rendered each frame) ─────────────────────

/// Inline emphasis for one text run. Bold is simulated with a brighter color —
/// the shipped proportional font has no bold face (same rationale as old egui's
/// markdown view). Italic is honored via the font `Style`.
#[derive(Clone, Copy, PartialEq)]
enum Emph {
    Normal,
    Bold,
    Italic,
    Code,
}

/// One styled inline run inside a block.
struct Run {
    text: String,
    emph: Emph,
}

/// One table cell's inline content.
type Cell = Vec<Run>;

/// A block-level element in the rendered document.
enum Block {
    Heading { level: u8, text: String },
    Para(Vec<Run>),
    Bullet(Vec<Run>),
    Quote(Vec<Run>),
    Code(Vec<String>),
    Table { headers: Vec<Cell>, rows: Vec<Vec<Cell>> },
    Rule,
}

fn heading_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Walk the pulldown-cmark event stream into the owned `Block` model. Faithful
/// port of the token loop in old Crane `views/markdown_view.rs`: one inline
/// accumulator (`runs`) per block, flushed at the block's end tag; the block
/// type is chosen from the current list-item / block-quote context.
fn parse(src: &str) -> Vec<Block> {
    let parser = Parser::new_ext(src, Options::all());

    let mut blocks: Vec<Block> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut head_buf = String::new();
    let mut code_buf = String::new();

    let mut bold = false;
    let mut italic = false;
    let mut heading: Option<u8> = None;
    let mut in_code = false;
    let mut in_bullet = false;
    let mut in_quote = false;
    let mut table_headers: Vec<Cell> = Vec::new();
    let mut table_rows: Vec<Vec<Cell>> = Vec::new();
    let mut table_row: Vec<Cell> = Vec::new();
    let mut in_table_head = false;
    // Holds prose accumulated for an enclosing bullet/quote while a nested
    // table is being parsed. `runs` itself cannot hold it across the table:
    // `Start(Tag::TableCell)` unconditionally clears `runs` for each cell, so
    // merely skipping the clear at table start is not enough — the first
    // cell would still wipe it out. Stashed here at `Start(Tag::Table)` and
    // restored into `runs` at `End(TagEnd::Table)`, once cell processing is
    // done.
    let mut pending_container_runs: Vec<Run> = Vec::new();

    let emph_now = |bold: bool, italic: bool| {
        if bold {
            Emph::Bold
        } else if italic {
            Emph::Italic
        } else {
            Emph::Normal
        }
    };

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(heading_u8(level));
                head_buf.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                if !head_buf.trim().is_empty() {
                    blocks.push(Block::Heading {
                        level: heading.unwrap_or(1),
                        text: std::mem::take(&mut head_buf),
                    });
                }
                head_buf.clear();
                heading = None;
            }
            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,
            Event::Start(Tag::CodeBlock(_)) => {
                in_code = true;
                code_buf.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                let lines: Vec<String> = code_buf
                    .strip_suffix('\n')
                    .unwrap_or(&code_buf)
                    .split('\n')
                    .map(str::to_string)
                    .collect();
                blocks.push(Block::Code(lines));
                code_buf.clear();
                in_code = false;
            }
            Event::Start(Tag::Item) => {
                in_bullet = true;
                runs.clear();
            }
            Event::End(TagEnd::Item) => {
                // A nested table's own End(TagEnd::Table) clears `runs`, so a
                // list item whose only content was a table must not push an
                // empty Block::Bullet here.
                let runs = std::mem::take(&mut runs);
                if !runs.iter().all(|r| r.text.trim().is_empty()) {
                    blocks.push(Block::Bullet(runs));
                }
                in_bullet = false;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                in_quote = true;
                runs.clear();
            }
            Event::End(TagEnd::BlockQuote) => {
                // Same guard as Item above: a nested table's End(TagEnd::Table)
                // clears `runs`, so a blockquote whose only content was a
                // table must not push an empty Block::Quote here.
                let runs = std::mem::take(&mut runs);
                if !runs.iter().all(|r| r.text.trim().is_empty()) {
                    blocks.push(Block::Quote(runs));
                }
                in_quote = false;
            }
            Event::End(TagEnd::Paragraph) => {
                // Bullets/quotes flush at their own end tag (a paragraph may nest
                // inside them); a bare paragraph flushes here.
                if !in_bullet && !in_quote {
                    blocks.push(Block::Para(std::mem::take(&mut runs)));
                }
            }
            Event::Code(text) => {
                runs.push(Run {
                    text: text.into_string(),
                    emph: Emph::Code,
                });
            }
            Event::Text(text) => {
                if in_code {
                    code_buf.push_str(&text);
                } else if heading.is_some() {
                    head_buf.push_str(&text);
                } else {
                    runs.push(Run {
                        text: text.into_string(),
                        emph: emph_now(bold, italic),
                    });
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_code {
                    code_buf.push('\n');
                } else if heading.is_some() {
                    head_buf.push(' ');
                } else {
                    runs.push(Run {
                        text: " ".to_string(),
                        emph: Emph::Normal,
                    });
                }
            }
            Event::Rule => blocks.push(Block::Rule),
            Event::Start(Tag::Table(_)) => {
                table_headers.clear();
                table_rows.clear();
                table_row.clear();
                // `runs` may still hold prose pending for an enclosing bullet
                // or quote (which only flush at their own end tag). Stash it
                // instead of clearing it outright — cell processing below
                // needs `runs` to start empty, but the pending prose must
                // survive to be restored once the table is done.
                if in_bullet || in_quote {
                    pending_container_runs = std::mem::take(&mut runs);
                } else {
                    runs.clear();
                }
            }
            Event::Start(Tag::TableHead) => {
                in_table_head = true;
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                table_headers = std::mem::take(&mut table_row);
                in_table_head = false;
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !in_table_head {
                    table_rows.push(std::mem::take(&mut table_row));
                }
            }
            Event::Start(Tag::TableCell) => {
                runs.clear();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(std::mem::take(&mut runs));
            }
            Event::End(TagEnd::Table) => {
                blocks.push(Block::Table {
                    headers: std::mem::take(&mut table_headers),
                    rows: std::mem::take(&mut table_rows),
                });
                table_row.clear();
                // Restore whatever prose was stashed at table start (empty if
                // none was pending — equivalent to the old unconditional
                // clear for a top-level table). `runs` is already empty here
                // (the last cell's End(TagEnd::TableCell) took it), so this
                // never drops in-progress cell content.
                runs = std::mem::take(&mut pending_container_runs);
            }
            // A paragraph START must reset the inline accumulator. Without this, any
            // unflushed runs (from an unhandled construct) silently merge into this
            // paragraph — the root cause of the table content-loss bug.
            Event::Start(Tag::Paragraph) => {
                if !in_bullet && !in_quote {
                    runs.clear();
                }
            }
            _ => {}
        }
    }
    // Trailing unterminated block (defensive — well-formed docs end cleanly).
    if !runs.is_empty() {
        blocks.push(Block::Para(runs));
    }
    blocks
}

// ── View ─────────────────────────────────────────────────────────────────────

pub struct WarpMarkdownView {
    /// Proportional font for prose + headings.
    prose: FamilyId,
    /// Monospace font for inline code + fenced code blocks.
    mono: FamilyId,
    /// Display title (file name, or a caller-supplied title).
    title: String,
    /// Parsed document, or an error line when the file could not be read.
    blocks: Vec<Block>,
    /// First visible block index (manual scroll window, like `FileView`).
    scroll: usize,
}

impl WarpMarkdownView {
    /// Open and render the Markdown file at `path`.
    pub fn new(ctx: &mut ViewContext<Self>, path: PathBuf) -> Self {
        let (prose, mono) = Self::fonts(ctx);
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let blocks = match std::fs::read_to_string(&path) {
            Ok(src) => parse(&src),
            Err(e) => vec![Block::Para(vec![Run {
                text: format!("Cannot read {}: {e}", path.display()),
                emph: Emph::Normal,
            }])],
        };
        Self {
            prose,
            mono,
            title,
            blocks,
            scroll: 0,
        }
    }

    /// Render raw Markdown `text` under `title` (e.g. an in-memory doc / preview).
    /// Kept for future callers (git-log / markdown preview); not yet wired.
    #[allow(dead_code)]
    pub fn from_source(ctx: &mut ViewContext<Self>, title: String, text: String) -> Self {
        let (prose, mono) = Self::fonts(ctx);
        Self {
            prose,
            mono,
            title,
            blocks: parse(&text),
            scroll: 0,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Load the proportional UI font (headings/prose) + the mono font (code).
    /// Mirrors the shell's `ui_font` load and `FileView`'s mono load.
    fn fonts(ctx: &mut ViewContext<Self>) -> (FamilyId, FamilyId) {
        warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            let prose = crate::warpui::bundled_fonts::ui(cache);
            let mono = crate::warpui::bundled_fonts::mono(cache);
            (prose, mono)
        })
    }

    // ── element builders ─────────────────────────────────────────────────────

    fn block_element(&self, block: &Block) -> Box<dyn Element> {
        match block {
            Block::Heading { level, text } => self.heading_element(*level, text),
            Block::Para(runs) => self.pad_block(self.inline_element(runs, theme::text()), 4.0),
            Block::Bullet(runs) => self.bullet_element(runs),
            Block::Quote(runs) => self.quote_element(runs),
            Block::Code(lines) => self.code_element(lines),
            Block::Table { headers, rows } => self.table_element(headers, rows),
            Block::Rule => self.rule_element(),
        }
    }

    fn heading_element(&self, level: u8, text: &str) -> Box<dyn Element> {
        let scale = match level {
            1 => 1.8,
            2 => 1.5,
            3 => 1.3,
            _ => 1.15,
        };
        let t = Text::new(text.to_string(), self.prose, BASE * scale)
            .with_color(theme::text_header())
            .soft_wrap(true)
            .finish();
        Container::new(t)
            .with_padding_top(if level <= 2 { 12.0 } else { 8.0 })
            .with_padding_bottom(4.0)
            .finish()
    }

    /// One inline block. Fast path: a single soft-wrapping `Text` when the block
    /// is uniform prose (no inline code, no emphasis) — the common case, and the
    /// cheapest path. Mixed blocks (inline code, bold, or italic) build a
    /// `FormattedTextElement`, warp's multi-style body-text element, which wraps
    /// by default and renders inline code as a chip natively — replacing the old
    /// `Flex::row` fallback, which could not wrap by construction.
    fn inline_element(&self, runs: &[Run], base_color: ColorU) -> Box<dyn Element> {
        if runs.is_empty() {
            return Text::new(String::new(), self.prose, BASE)
                .with_color(base_color)
                .finish();
        }
        let mixed = runs
            .iter()
            .any(|r| !matches!(r.emph, Emph::Normal));
        if !mixed {
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            return Text::new(text, self.prose, BASE)
                .with_color(base_color)
                .with_line_height_ratio(LINE_H)
                .soft_wrap(true)
                .finish();
        }

        // Mixed inline styling. A Flex::row cannot wrap by construction — that
        // was the cause of clipped paragraphs. FormattedTextElement is warp's
        // shipped multi-style body-text element and wraps by default.
        FormattedTextElement::new(
            FormattedText::new([FormattedTextLine::Line(self.fragments(runs))]),
            BASE,
            self.prose,
            self.mono,
            base_color,
            Default::default(),
        )
        .with_line_height_ratio(LINE_H)
        .finish()
    }

    /// Convert the owned `Run` model into FormattedTextFragments. Bold is
    /// brightened via a separate color rather than a bold face — the bundled
    /// proportional font has no bold face (see `Emph`'s doc comment above).
    fn fragments(&self, runs: &[Run]) -> Vec<FormattedTextFragment> {
        runs.iter()
            .map(|r| match r.emph {
                Emph::Code => FormattedTextFragment::inline_code(r.text.clone()),
                _ => FormattedTextFragment::plain_text(r.text.clone()),
            })
            .collect()
    }

    fn bullet_element(&self, runs: &[Run]) -> Box<dyn Element> {
        // A small filled dot as the marker (NOT a Unicode "•" glyph — the bundled
        // fonts don't cover it and it would render as tofu). Nudged down to sit on
        // the text baseline.
        let dot = ConstrainedBox::new(
            Container::new(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::accent()).finish(),
                )
                .with_width(5.0)
                .with_height(5.0)
                .finish(),
            )
            .with_padding_top(BASE * 0.45)
            .with_padding_left(8.0)
            .with_padding_right(9.0)
            .finish(),
        )
        .with_width(22.0)
        .finish();

        let row = Flex::row()
            .with_child(dot)
            .with_child(Expanded::new(1.0, self.inline_element(runs, theme::text())).finish())
            .finish();
        self.pad_block(row, 2.0)
    }

    fn quote_element(&self, runs: &[Run]) -> Box<dyn Element> {
        // Left accent bar drawn as a full-height Rect underneath the indented
        // text (a Rect fills its Stack's height while constrained to 3px width).
        let bar = ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
            .with_width(3.0)
            .finish();
        let body = Container::new(self.inline_element(runs, theme::text_muted()))
            .with_padding_left(12.0)
            .finish();
        let stack = Stack::new().with_child(bar).with_child(body).finish();
        self.pad_block(stack, 4.0)
    }

    fn code_element(&self, lines: &[String]) -> Box<dyn Element> {
        let mut col = Flex::column();
        for line in lines {
            col = col.with_child(
                Text::new(line.clone(), self.mono, BASE - 1.0)
                    .with_color(theme::text())
                    .soft_wrap(false)
                    .finish(),
            );
        }
        let panel = Container::new(col.finish())
            .with_background_color(theme::surface())
            .with_uniform_padding(10.0)
            .finish();
        self.pad_block(panel, 4.0)
    }

    /// A table as a column of rows, each row a Flex::row of equal-weight cells.
    /// Header cells are brightened; a hairline separates each row.
    fn table_element(&self, headers: &[Cell], rows: &[Vec<Cell>]) -> Box<dyn Element> {
        let mut col = Flex::column();

        if !headers.is_empty() {
            let mut head = Flex::row();
            for cell in headers {
                head = head.with_child(
                    Expanded::new(
                        1.0,
                        Container::new(self.inline_element(cell, theme::text_header()))
                            .with_uniform_padding(6.0)
                            .finish(),
                    )
                    .finish(),
                );
            }
            col = col.with_child(head.finish());
            col = col.with_child(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::border()).finish(),
                )
                .with_height(1.0)
                .finish(),
            );
        }

        for row in rows {
            let mut r = Flex::row();
            for cell in row {
                r = r.with_child(
                    Expanded::new(
                        1.0,
                        Container::new(self.inline_element(cell, theme::text()))
                            .with_uniform_padding(6.0)
                            .finish(),
                    )
                    .finish(),
                );
            }
            col = col.with_child(r.finish());
            col = col.with_child(
                ConstrainedBox::new(
                    Rect::new().with_background_color(theme::border()).finish(),
                )
                .with_height(1.0)
                .finish(),
            );
        }

        let panel = Container::new(col.finish())
            .with_background_color(theme::surface())
            .finish();
        self.pad_block(panel, 4.0)
    }

    fn rule_element(&self) -> Box<dyn Element> {
        let line = ConstrainedBox::new(Rect::new().with_background_color(theme::border()).finish())
            .with_height(1.0)
            .finish();
        Container::new(line)
            .with_vertical_padding(8.0)
            .with_horizontal_padding(2.0)
            .finish()
    }

    /// Wrap a block with a little vertical breathing room between blocks.
    fn pad_block(&self, child: Box<dyn Element>, gap: f32) -> Box<dyn Element> {
        Container::new(child)
            .with_padding_top(gap)
            .with_padding_bottom(gap)
            .finish()
    }

    /// Outer panel: a background Rect under the content (mirrors `FileView::panel`).
    fn panel(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(content)
            .finish()
    }
}

impl Entity for WarpMarkdownView {
    type Event = ();
}

#[derive(Debug, Clone)]
pub enum MarkdownViewAction {
    /// Scroll by N blocks (positive = down).
    Scroll(i32),
}

impl TypedActionView for WarpMarkdownView {
    type Action = MarkdownViewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            MarkdownViewAction::Scroll(delta) => {
                let max = self.blocks.len().saturating_sub(1);
                let next = self.scroll as i64 + *delta as i64;
                self.scroll = next.clamp(0, max as i64) as usize;
            }
        }
        ctx.notify();
    }
}

impl View for WarpMarkdownView {
    fn ui_name() -> &'static str {
        "WarpMarkdownView"
    }

    fn render(&self, _ctx: &AppContext) -> Box<dyn Element> {
        // Render a WINDOW of blocks from the scroll offset (manual scroll — same
        // approach as FileView/terminal, avoids an unbounded element tree). The
        // scroll unit is a block, not a pixel, so a wheel tick advances whole
        // blocks; a pixel-smooth pass can replace this once a clipped scroll
        // container lands.
        let mut body = Flex::column();
        let start = self.scroll.min(self.blocks.len().saturating_sub(1));
        for block in self.blocks.iter().skip(start).take(RENDER_BLOCKS) {
            body = body.with_child(self.block_element(block));
        }
        // Left/right gutter around the text column.
        let padded = Container::new(body.finish())
            .with_horizontal_padding(16.0)
            .with_vertical_padding(10.0)
            .finish();

        let scroll_body = EventHandler::new(Expanded::new(1.0, padded).finish())
            .on_scroll_wheel(move |ctx, _app, delta, _mods| {
                // Coarser divisor than FileView's line scroll — blocks are tall.
                let blocks = (-delta.y() / 30.0).round() as i32;
                if blocks != 0 {
                    ctx.dispatch_typed_action(MarkdownViewAction::Scroll(blocks));
                }
                DispatchEventResult::StopPropagation
            })
            .finish();

        let content = Flex::column()
            .with_child(Expanded::new(1.0, scroll_body).finish())
            .finish();
        self.panel(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para_texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para(runs) => {
                    Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn table_cell_text_does_not_leak_into_following_paragraph() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n\nAfter the table.\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert_eq!(
            para_texts(&blocks),
            vec!["After the table."],
            "table cell text must not leak into the following paragraph"
        );
    }

    #[test]
    fn table_headers_and_rows_are_captured() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(src);
        let table = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { headers, rows } => Some((headers, rows)),
                _ => None,
            })
            .expect("Block::Table present");
        assert_eq!(table.0.len(), 2, "two header cells");
        assert_eq!(table.1.len(), 1, "one body row");
        assert_eq!(table.1[0].len(), 2, "two cells in the body row");
    }

    fn quote_texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Quote(runs) => {
                    Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect()
    }

    fn bullet_texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet(runs) => {
                    Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn table_inside_blockquote_preceded_by_prose_retains_prose() {
        // Finding 1, quote case: an unguarded runs.clear() on Tag::Table start
        // must not clobber prose still pending for the enclosing blockquote.
        let src = "> Intro text.\n>\n> | A | B |\n> |---|---|\n> | 1 | 2 |\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert_eq!(
            quote_texts(&blocks),
            vec!["Intro text."],
            "prose preceding a nested table must survive in the enclosing Block::Quote"
        );
    }

    #[test]
    fn table_inside_bullet_preceded_by_prose_retains_prose() {
        // Finding 1, bullet case: same defect for a table nested in a list item.
        let src = "- Intro text.\n\n  | A | B |\n  |---|---|\n  | 1 | 2 |\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert_eq!(
            bullet_texts(&blocks),
            vec!["Intro text."],
            "prose preceding a nested table must survive in the enclosing Block::Bullet"
        );
    }

    #[test]
    fn bullet_containing_only_a_table_produces_no_empty_bullet_block() {
        // Finding 2, bullet case: End(TagEnd::Table) clears `runs`, so the
        // subsequent End(TagEnd::Item) must not unconditionally push an empty
        // Block::Bullet.
        let src = "- | A | B |\n  |---|---|\n  | 1 | 2 |\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert!(
            bullet_texts(&blocks).is_empty(),
            "a list item containing only a table must not produce an empty Block::Bullet"
        );
    }

    #[test]
    fn blockquote_containing_only_a_table_produces_no_empty_quote_block() {
        // Finding 2, quote case: same defect for a table that is the sole
        // content of a blockquote.
        let src = "> | A | B |\n> |---|---|\n> | 1 | 2 |\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(tables, 1, "a table must produce exactly one Block::Table");

        assert!(
            quote_texts(&blocks).is_empty(),
            "a blockquote containing only a table must not produce an empty Block::Quote"
        );
    }

    #[test]
    fn mixed_runs_produce_wrapping_inline_content() {
        // A paragraph mixing prose and inline code is the common technical case
        // and is exactly what the old Flex::row path failed to wrap.
        let src = "Set `CRANE_GPU_TERM=1` in the environment to enable the renderer.\n";
        let blocks = parse(src);
        let runs = blocks
            .iter()
            .find_map(|b| match b {
                Block::Para(runs) => Some(runs),
                _ => None,
            })
            .expect("one paragraph");

        assert!(
            runs.iter().any(|r| matches!(r.emph, Emph::Code)),
            "the code span must survive parsing as an Emph::Code run"
        );
        assert!(
            runs.iter().any(|r| matches!(r.emph, Emph::Normal)),
            "surrounding prose must survive as Normal runs"
        );
    }
}
