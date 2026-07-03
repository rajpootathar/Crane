//! `WarpMarkdownView` — a self-contained warpui `View` that renders a Markdown
//! document. The warpui port of old Crane's `views/markdown_view.rs`: parse with
//! `pulldown_cmark` once at construction into an owned block model, then rebuild
//! warpui elements from that model each frame (elements are transient; the model
//! persists). Read-only in v1 — links/images render as plain text, matching the
//! old egui behavior.

use std::path::PathBuf;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    ParentElement, Rect, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Style};
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

/// A block-level element in the rendered document.
enum Block {
    Heading { level: u8, text: String },
    Para(Vec<Run>),
    Bullet(Vec<Run>),
    Quote(Vec<Run>),
    Code(Vec<String>),
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
                blocks.push(Block::Bullet(std::mem::take(&mut runs)));
                in_bullet = false;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                in_quote = true;
                runs.clear();
            }
            Event::End(TagEnd::BlockQuote) => {
                blocks.push(Block::Quote(std::mem::take(&mut runs)));
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
            let prose = cache
                .load_system_font("Helvetica Neue")
                .or_else(|_| cache.load_system_font("Menlo"))
                .expect("load prose font");
            let mono = cache.load_system_font("Menlo").expect("load Menlo");
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
    /// only path that wraps long paragraphs correctly. Mixed blocks fall back to
    /// a `Flex::row` of styled pieces (inline code becomes a bg-tinted chip); a
    /// row does not soft-wrap, the accepted v1 tradeoff for inline styling.
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
        let mut row = Flex::row();
        for r in runs {
            row = row.with_child(self.run_element(r, base_color));
        }
        row.finish()
    }

    fn run_element(&self, run: &Run, base_color: ColorU) -> Box<dyn Element> {
        match run.emph {
            Emph::Normal => Text::new(run.text.clone(), self.prose, BASE)
                .with_color(base_color)
                .soft_wrap(false)
                .finish(),
            // Bold has no dedicated face — brighten instead (see `Emph`).
            Emph::Bold => Text::new(run.text.clone(), self.prose, BASE)
                .with_color(theme::text_hover())
                .soft_wrap(false)
                .finish(),
            Emph::Italic => Text::new(run.text.clone(), self.prose, BASE)
                .with_color(base_color)
                .with_style(Properties {
                    style: Style::Italic,
                    ..Default::default()
                })
                .soft_wrap(false)
                .finish(),
            // Inline code: mono glyphs on a surface-tinted chip.
            Emph::Code => Container::new(
                Text::new(run.text.clone(), self.mono, BASE - 1.0)
                    .with_color(theme::warning())
                    .soft_wrap(false)
                    .finish(),
            )
            .with_background_color(theme::surface())
            .with_padding_left(3.0)
            .with_padding_right(3.0)
            .finish(),
        }
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
