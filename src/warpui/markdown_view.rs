//! `WarpMarkdownView` — a self-contained warpui `View` that renders a Markdown
//! document. The warpui port of old Crane's `views/markdown_view.rs`: parse with
//! `pulldown_cmark` once at construction into an owned block model, then rebuild
//! warpui elements from that model each frame (elements are transient; the model
//! persists). Read-only in v1. Links render through `FormattedTextElement`'s
//! hyperlink fragment support (destination URL carried on `Run::link`) with
//! click handling wired up explicitly via `register_default_click_handlers`
//! in `inline_element` — that registration is required, not automatic, see
//! that method's doc comment; inline images still render as plain text —
//! deferred, pending the `Image` element introduced by the image-viewer plan.

use std::path::PathBuf;

use markdown_parser::{FormattedText, FormattedTextFragment, FormattedTextLine};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use warpui::color::ColorU;
use warpui::elements::{
    Border, ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
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

/// Inline emphasis for one text run. Each variant maps to its own
/// `FormattedTextFragment` constructor in `fragments()` (bold/italic/code/
/// strikethrough all render distinctly — see that method's doc comment for
/// the bold-face finding on this app's prose font).
#[derive(Clone, Copy, PartialEq)]
enum Emph {
    Normal,
    Bold,
    Italic,
    Code,
    Strike,
}

/// One styled inline run inside a block.
struct Run {
    text: String,
    emph: Emph,
    /// Destination URL when this run sits inside a Markdown link. Wins over
    /// `emph` in `fragments()`: a linked run always renders as a hyperlink
    /// fragment regardless of any emphasis also active on it.
    link: Option<String>,
}

/// One table cell's inline content.
type Cell = Vec<Run>;

/// A block-level element in the rendered document.
enum Block {
    Heading { level: u8, text: String },
    Para(Vec<Run>),
    Bullet {
        runs: Vec<Run>,
        depth: usize,
        ordinal: Option<usize>,
        /// True when this is a later flush of an item whose marker already
        /// rendered on an earlier flush (Finding 1) — e.g. continuation
        /// prose after a nested list inside the same loose item. Gates
        /// `bullet_element`'s marker column: a continuation row renders no
        /// glyph at all, not the ordinal and not a substitute dot, just the
        /// same indent as the item's first row.
        continuation: bool,
    },
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

/// Which kind of enclosing container a run of pending prose belongs to.
/// Pushed onto `container_stack` at `Start(Tag::Item)` / `Start(Tag::BlockQuote)`,
/// popped at the matching `End` tag. Replaces two independent `in_bullet` /
/// `in_quote` booleans, which could not express "a blockquote nested inside
/// an already-open list item": `in_bullet` never got cleared when the
/// blockquote opened, so blockquote prose nested in a list item was
/// mistyped as `Block::Bullet` (and, in an ordered list, stole an ordinal it
/// had no business consuming). The stack always names the *innermost*
/// currently open container, which is exactly what a nested container's
/// `Start` event needs in order to flush the right thing.
///
/// `Bullet`'s `depth`/`ordinal` are computed once, at the item's own
/// `Start(Tag::Item)`, and carried on the stack entry rather than
/// recomputed later: `flush_open_container` may need to push this item's
/// content *before* its own `End(TagEnd::Item)` fires (when a nested
/// Item/BlockQuote is about to reuse `runs`), and the ordinal counter must
/// advance exactly once per item no matter which of those two sites ends up
/// doing the pushing.
///
/// `marker_emitted` tracks whether this item has already had one flush
/// render its marker. Because the stack entry survives every flush of the
/// same item (an early one at a nested container's `Start`, and — if prose
/// remains — the item's own `End`), a naive read of `ordinal` on each flush
/// would render the same "N." (or dot) every time a loose item is flushed
/// more than once. `flush_open_container` and `End(TagEnd::Item)` both
/// check/set this flag: the first flush renders the real marker and flips
/// it to `true`; every later flush of that same item is a markerless
/// continuation row (see `Block::Bullet::continuation`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum ContainerKind {
    Bullet {
        depth: usize,
        ordinal: Option<usize>,
        marker_emitted: bool,
    },
    Quote,
}

/// Take whatever prose is pending in `runs` and push it as a block belonging
/// to the innermost currently open container (the top of `container_stack`).
/// Called at `Start(Tag::Item)` / `Start(Tag::BlockQuote)` when the new
/// item/quote is itself nested inside an already-open bullet or quote:
/// unconditionally clearing `runs` for the nested container would wipe out
/// the *outer* container's own pending text — the container-start prose loss
/// bug, one level over from the table-in-container case that
/// `pending_container_runs` fixes.
///
/// This deliberately does not reuse the stash-then-restore-into-`runs` shape
/// of `pending_container_runs`: that shape relies on the *same* container's
/// own End tag firing once, after which `runs` is restored for continued
/// accumulation. Item/BlockQuote nest tag-for-tag (an inner `Item`'s own End
/// fires — and would push a block — strictly before the outer `Item`'s End),
/// so deferring the outer's flush that way would land its block after the
/// inner one in `blocks`, out of reading order. Flushing immediately, right
/// here, keeps blocks in document order.
///
/// Takes `container_stack` mutably (Finding 1): flushing a `Bullet` entry
/// needs to both read `ordinal` and flip `marker_emitted` on the *same* top
/// entry, so a later flush of the same item (its own `End(TagEnd::Item)`,
/// or a second nested container) knows the marker already rendered.
fn flush_open_container(
    blocks: &mut Vec<Block>,
    runs: &mut Vec<Run>,
    container_stack: &mut [ContainerKind],
) {
    let taken = std::mem::take(runs);
    if taken.iter().all(|r| r.text.trim().is_empty()) {
        return;
    }
    debug_assert!(
        !container_stack.is_empty(),
        "flush_open_container has non-empty pending prose but no open container to flush into"
    );
    match container_stack.last_mut() {
        Some(ContainerKind::Bullet { depth, ordinal, marker_emitted }) => {
            // Only the item's first flush renders a marker (Finding 1). A
            // later flush of the same item is a continuation row: no
            // number, and — per `Block::Bullet::continuation` — no
            // substitute dot either, just the indent.
            let continuation = *marker_emitted;
            *marker_emitted = true;
            blocks.push(Block::Bullet {
                runs: taken,
                depth: *depth,
                ordinal: if continuation { None } else { *ordinal },
                continuation,
            });
        }
        Some(ContainerKind::Quote) => {
            blocks.push(Block::Quote(taken));
        }
        None => {
            // Unreachable through the current call sites (each only calls
            // this once `container_stack` is confirmed non-empty) — the
            // debug_assert above already catches the mistake in debug/test
            // builds. Fall back to a plain paragraph in release rather than
            // silently dropping content, in case a future call site forgets
            // the guard.
            blocks.push(Block::Para(taken));
        }
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
    // Innermost-first stack of open bullet/blockquote containers. See
    // `ContainerKind`'s doc comment for why this replaced two independent
    // `in_bullet` / `in_quote` booleans.
    let mut container_stack: Vec<ContainerKind> = Vec::new();
    // One entry per open list. `Some(n)` = ordered list, next number is `n`;
    // `None` = unordered. Pushed at `Start(Tag::List)`, popped at
    // `End(TagEnd::List)`; `len() - 1` at an `Item`'s own start is that
    // item's nesting depth.
    let mut list_stack: Vec<Option<u64>> = Vec::new();
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
    // Destination URL of the innermost open link, if any. `Some` between
    // `Start(Tag::Link)` and `End(TagEnd::Link)`; cleared on the End tag.
    let mut link_url: Option<String> = None;
    // Whether a `~~...~~` span is currently open.
    let mut strike = false;

    let emph_now = |bold: bool, italic: bool, strike: bool| {
        if strike {
            Emph::Strike
        } else if bold {
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
            Event::Start(Tag::List(start)) => {
                list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                if container_stack.is_empty() {
                    runs.clear();
                } else {
                    flush_open_container(&mut blocks, &mut runs, &mut container_stack);
                }
                // The ordinal counter advances on every item, unconditionally
                // — an item whose only content is (say) a table or a
                // blockquote still legitimately occupies its slot in the
                // list: its own block renders, only the bullet marker/text
                // is suppressed by the empty-block guard in
                // `End(TagEnd::Item)` below. A guarded increment would
                // silently skip a number and mis-number every later sibling.
                // Reserved here, at Start, rather than at End, because
                // `flush_open_container` above may already push this item's
                // `Block::Bullet` before its own End tag fires (when a
                // nested Item/BlockQuote reuses `runs`) — computing
                // depth/ordinal once and carrying them on the stack entry
                // guarantees exactly one increment per item no matter which
                // site ends up doing the flushing.
                let depth = list_stack.len().saturating_sub(1);
                let ordinal = match list_stack.last_mut() {
                    Some(Some(n)) => {
                        let cur = *n as usize;
                        *n += 1;
                        Some(cur)
                    }
                    _ => None,
                };
                container_stack.push(ContainerKind::Bullet {
                    depth,
                    ordinal,
                    marker_emitted: false,
                });
            }
            Event::End(TagEnd::Item) => {
                let popped = container_stack.pop();
                debug_assert!(
                    matches!(popped, Some(ContainerKind::Bullet { .. })),
                    "End(TagEnd::Item) without a matching Bullet on the container stack"
                );
                // A nested table's own End(TagEnd::Table) clears `runs`, so a
                // list item whose only content was a table must not push an
                // empty Block::Bullet here.
                //
                // The take is unconditional (Finding 3), not nested inside
                // the `if let Some(Bullet)` guard below: on a kind mismatch
                // (unreachable in practice — the debug_assert above already
                // flags it) a conditional take would leave stale `runs` to
                // leak into the next block in release builds while debug
                // builds panic instead. Draining here keeps both profiles
                // in agreement on this impossible state.
                let runs = std::mem::take(&mut runs);
                if let Some(ContainerKind::Bullet { depth, ordinal, marker_emitted }) = popped {
                    if !runs.iter().all(|r| r.text.trim().is_empty()) {
                        // Finding 1: if an earlier flush of this same item
                        // already rendered the marker, this is a
                        // continuation row — no ordinal, no substitute dot.
                        blocks.push(Block::Bullet {
                            runs,
                            depth,
                            ordinal: if marker_emitted { None } else { ordinal },
                            continuation: marker_emitted,
                        });
                    }
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                if container_stack.is_empty() {
                    runs.clear();
                } else {
                    flush_open_container(&mut blocks, &mut runs, &mut container_stack);
                }
                container_stack.push(ContainerKind::Quote);
            }
            Event::End(TagEnd::BlockQuote) => {
                let popped = container_stack.pop();
                debug_assert!(
                    matches!(popped, Some(ContainerKind::Quote)),
                    "End(TagEnd::BlockQuote) without a matching Quote on the container stack"
                );
                // Same guard as Item above: a nested table's End(TagEnd::Table)
                // clears `runs`, so a blockquote whose only content was a
                // table must not push an empty Block::Quote here.
                //
                // Take is unconditional (Finding 3) for the same reason as
                // End(TagEnd::Item) above: debug and release must agree on
                // the (should-be unreachable) kind-mismatch case instead of
                // one panicking and the other silently leaking stale `runs`.
                let runs = std::mem::take(&mut runs);
                if matches!(popped, Some(ContainerKind::Quote)) {
                    if !runs.iter().all(|r| r.text.trim().is_empty()) {
                        blocks.push(Block::Quote(runs));
                    }
                }
            }
            Event::End(TagEnd::Paragraph) => {
                // Bullets/quotes flush at their own end tag (a paragraph may nest
                // inside them); a bare paragraph flushes here.
                if container_stack.is_empty() {
                    blocks.push(Block::Para(std::mem::take(&mut runs)));
                }
            }
            Event::Code(text) => {
                if heading.is_some() {
                    head_buf.push_str(&text);
                } else {
                    runs.push(Run {
                        text: text.into_string(),
                        emph: Emph::Code,
                        link: None,
                    });
                }
            }
            Event::Text(text) => {
                if in_code {
                    code_buf.push_str(&text);
                } else if heading.is_some() {
                    head_buf.push_str(&text);
                } else {
                    runs.push(Run {
                        text: text.into_string(),
                        emph: emph_now(bold, italic, strike),
                        link: link_url.clone(),
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
                        link: None,
                    });
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                link_url = None;
            }
            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,
            Event::TaskListMarker(done) => {
                // Drawn as text, not a Unicode checkbox glyph — the bundled
                // fonts don't cover that range and it would render as tofu.
                runs.push(Run {
                    text: if done { "[x] ".to_string() } else { "[ ] ".to_string() },
                    emph: Emph::Code,
                    link: None,
                });
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
                if !container_stack.is_empty() {
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
                if container_stack.is_empty() {
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

/// Whether a run of inline content needs the `FormattedTextElement` path
/// (multi-style, wraps by default) rather than the plain-`Text` fast path.
/// A free function, not a method, so it can be unit-tested directly without
/// standing up a `WarpMarkdownView` or a font instance — see the tests
/// module. True when any run carries emphasis other than `Emph::Normal`, or
/// carries a link: a run can have `link: Some(_)` while `emph` is still
/// `Normal` (a plain, unemphasized link), and the fast `Text` path has no way
/// to render hyperlink styling, so a linked run always forces the
/// `FormattedTextElement` path even when no `Emph` variant does.
fn needs_formatted_text(runs: &[Run]) -> bool {
    runs.iter()
        .any(|r| !matches!(r.emph, Emph::Normal) || r.link.is_some())
}

/// Convert the owned `Run` model into `FormattedTextFragment`s, one distinct
/// fragment kind per `Emph` variant (`FormattedTextFragment` ships a
/// purpose-built constructor for each — see
/// `vendor/warp/crates/markdown_parser/src/lib.rs`, `plain_text`/`bold`/
/// `italic`/`hyperlink`/`inline_code`/`strikethrough`). A run's `link` wins
/// over its `emph`: hyperlink fragments are colored by
/// `hyperlink_font_color`, and are only made clickable at all by
/// `inline_element`'s explicit `.register_default_click_handlers(...)` call
/// — `FormattedTextElement` does not wire up hyperlink click handling (or
/// the `hyperlink_font_color` highlight) on its own, so a linked run always
/// renders as a hyperlink fragment regardless of any emphasis also active on
/// it, but is inert without that registration.
///
/// A free function, not a method — like `needs_formatted_text` above, this
/// doesn't use `&self`, so it can be unit-tested directly without standing
/// up a `WarpMarkdownView` or a font instance. That matters here in
/// particular: `fragments()` used to be a private method with zero test
/// coverage, so deleting either the click-handler registration in
/// `inline_element` or the `link`-wins-over-`emph` early return below left
/// all tests green — see the `fragments` test module below.
///
/// Model limitation, out of scope here: `Emph` is a flat enum, so
/// `***bold italic***` collapses to whichever of Bold/Italic wins in
/// `emph_now` and can never reach `FormattedTextFragment::bold_italic`. The
/// same flattening means `~~**both**~~` collapses to a single `Emph::Strike`
/// run — strike wins `emph_now`'s precedence chain over bold/italic — so
/// bold-and-struck text renders as struck-only, never
/// `FormattedTextFragment::bold`+strikethrough combined.
fn fragments(runs: &[Run]) -> Vec<FormattedTextFragment> {
    runs.iter()
        .map(|r| {
            if let Some(url) = &r.link {
                return FormattedTextFragment::hyperlink(r.text.clone(), url.clone());
            }
            match r.emph {
                Emph::Code => FormattedTextFragment::inline_code(r.text.clone()),
                Emph::Bold => FormattedTextFragment::bold(r.text.clone()),
                Emph::Italic => FormattedTextFragment::italic(r.text.clone()),
                Emph::Strike => FormattedTextFragment::strikethrough(r.text.clone()),
                Emph::Normal => FormattedTextFragment::plain_text(r.text.clone()),
            }
        })
        .collect()
}

/// Scheme allowlist for hyperlink clicks opened via the OS `open`/`xdg-open`
/// command. Markdown files come from untrusted repositories, and `open` will
/// launch arbitrary URL schemes — `file://` to reveal local paths, or a
/// custom app scheme to trigger unwanted local behavior — not just web
/// URLs. Only `http`, `https` and `mailto` are considered safe enough to
/// hand to the OS opener; anything else (including no scheme at all) is
/// silently ignored. Checked via a case-insensitive prefix match — no URL
/// parsing crate is pulled in for this.
fn is_allowed_hyperlink_scheme(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
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
    /// Source file, when this view was opened from one. `None` for
    /// `from_source` (in-memory) documents, which cannot be persisted.
    path: Option<PathBuf>,
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
                link: None,
            }])],
        };
        Self {
            prose,
            mono,
            title,
            blocks,
            scroll: 0,
            path: Some(path),
        }
    }

    /// Render raw Markdown `text` under `title` — an in-memory document with NO
    /// backing file. The resulting view reports `path() == None`, which means
    /// the shell's save-side collector (`build_state`'s `markdowns` pass, which
    /// skips a pathless view) CANNOT persist it: the pane would come back as a
    /// terminal after a restart.
    ///
    /// So: never build a view for a REAL file with this constructor. Use
    /// `from_file_source` below, which takes the same in-memory text but binds
    /// it to the path it came from.
    pub fn from_source(ctx: &mut ViewContext<Self>, title: String, text: String) -> Self {
        let (prose, mono) = Self::fonts(ctx);
        Self {
            prose,
            mono,
            title,
            blocks: parse(&text),
            scroll: 0,
            path: None,
        }
    }

    /// Render `text` as the rendered preview of the file at `path`, WITHOUT
    /// re-reading that file from disk. This is what the Markdown pane's
    /// edit→preview toggle uses: the text comes from the editor's LIVE buffer,
    /// so unsaved edits show up in the preview immediately.
    ///
    /// The important half is that the view still carries `path` — a preview of
    /// a real file built through `from_source` alone would be pathless and
    /// therefore unpersistable (see `from_source`'s doc comment). Binding the
    /// path here, inside the constructor, is what makes that impossible to
    /// forget at a call site.
    pub fn from_file_source(ctx: &mut ViewContext<Self>, path: PathBuf, text: String) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut view = Self::from_source(ctx, title, text);
        view.path = Some(path);
        view
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Source file this view renders, if any. `None` for in-memory documents.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
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
            Block::Bullet { runs, depth, ordinal, continuation } => {
                self.bullet_element(runs, *depth, *ordinal, *continuation)
            }
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
    /// is uniform prose (no inline code, emphasis, or link) — the common case,
    /// and the cheapest path (see `needs_formatted_text`). Mixed blocks build a
    /// `FormattedTextElement`, warp's multi-style body-text element, which
    /// wraps by default — replacing the old `Flex::row` fallback, which could
    /// not wrap by construction. Inline code renders as a colored chip via
    /// `with_inline_code_properties`; bold, italic, strikethrough and links
    /// each render as their own distinct fragment style (see `fragments()`).
    /// `register_default_click_handlers` wires up hyperlink clicks: without
    /// it, `FormattedTextElement` never populates its mouse-handler table, so
    /// a hyperlink fragment renders with no click styling and does nothing on
    /// click (see `is_allowed_hyperlink_scheme` for why the click handler
    /// filters the URL before handing it to the OS opener).
    fn inline_element(&self, runs: &[Run], base_color: ColorU) -> Box<dyn Element> {
        if runs.is_empty() {
            return Text::new(String::new(), self.prose, BASE)
                .with_color(base_color)
                .finish();
        }
        if !needs_formatted_text(runs) {
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
            FormattedText::new([FormattedTextLine::Line(fragments(runs))]),
            BASE,
            self.prose,
            self.mono,
            base_color,
            Default::default(),
        )
        .with_inline_code_properties(Some(theme::warning()), Some(theme::surface()))
        .with_line_height_ratio(LINE_H)
        .register_default_click_handlers(|url, _ctx, _app| {
            let url = url.url;
            // Markdown is untrusted input (repos the user opens) — only ever
            // hand the OS opener a scheme we know is inert to click on. See
            // `is_allowed_hyperlink_scheme` for the rationale.
            if !is_allowed_hyperlink_scheme(&url) {
                return;
            }
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&url).spawn();
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        })
        .finish()
    }

    /// `depth` (0 = top-level) indents the whole row under its parent —
    /// applied as left padding on a wrapper `Container`, not folded into the
    /// marker column's width (see `MARKER_MIN_WIDTH` below for why).
    /// `ordinal` renders a "N." marker for ordered-list items; `None`
    /// (unordered) keeps the drawn dot marker (NOT a Unicode "•" glyph — the
    /// bundled fonts don't cover it and it would render as tofu), nudged
    /// down to sit on the text baseline. `continuation` (Finding 1)
    /// overrides both: a later flush of an item whose marker already
    /// rendered gets no glyph at all — the marker column still reserves
    /// `MARKER_MIN_WIDTH` so the row's text lines up with the item's own
    /// first row, it is just left empty.
    fn bullet_element(
        &self,
        runs: &[Run],
        depth: usize,
        ordinal: Option<usize>,
        continuation: bool,
    ) -> Box<dyn Element> {
        // A floor, not a ceiling, on the marker column's width.
        // `ConstrainedBox::with_width` (the previous approach) clamps *both*
        // min and max, forcing the column — and the marker `Text` inside it,
        // which uses `soft_wrap(false)` — to exactly that many pixels no
        // matter how wide its content is. At depth 0 that left roughly 8px
        // for the marker after its own padding, clipping any ordinal wider
        // than a single digit ("10.", "99."). `with_min_width` only ever
        // grows the column to fit wider content, so a single-digit ordinal
        // or the bullet dot still aligns into a consistent column while a
        // multi-digit ordinal simply widens the column instead of being cut
        // off.
        const MARKER_MIN_WIDTH: f32 = 22.0;

        let marker: Box<dyn Element> = if continuation {
            // Continuation row: no ordinal AND no substitute dot — this
            // prose belongs to an item whose marker already rendered on an
            // earlier flush. The empty Text still occupies the marker
            // column (reserved below via MARKER_MIN_WIDTH), so the row's
            // text lines up with the item's own first row.
            Text::new(String::new(), self.prose, BASE)
                .with_color(theme::text())
                .finish()
        } else {
            match ordinal {
                Some(n) => Container::new(
                    Text::new(format!("{n}."), self.prose, BASE)
                        .with_color(theme::accent())
                        .soft_wrap(false)
                        .finish(),
                )
                .with_padding_left(8.0)
                .with_padding_right(6.0)
                .finish(),
                None => Container::new(
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
            }
        };

        let row = Flex::row()
            .with_child(
                ConstrainedBox::new(marker)
                    .with_min_width(MARKER_MIN_WIDTH)
                    .finish(),
            )
            .with_child(Expanded::new(1.0, self.inline_element(runs, theme::text())).finish())
            .finish();

        // Nesting depth becomes left padding on the row, decoupled from the
        // marker column's own sizing — a wide ordinal at depth 2 can no
        // longer be squeezed by a width that was only ever computed for
        // depth 0.
        let indented = Container::new(row)
            .with_padding_left(depth as f32 * 18.0)
            .finish();
        self.pad_block(indented, 2.0)
    }

    fn quote_element(&self, runs: &[Run]) -> Box<dyn Element> {
        // Left accent bar drawn as the body Container's LEFT BORDER — NOT as a
        // `Rect` inside a `Stack`, which is what this used to be and which
        // crashed the app on any document containing a blockquote:
        //
        //   * `Rect::layout` returns `constraint.max` verbatim
        //     (vendor/warp/crates/warpui_core/src/elements/gui/rect.rs:110), so
        //     an unconstrained axis makes the Rect INFINITE on that axis.
        //   * `Stack::layout` hands each child its own incoming constraint
        //     unchanged and takes the max of the results
        //     (…/elements/gui/stack/mod.rs:184-196), so it neither bounds the
        //     Rect nor filters an infinite result back out.
        //   * Every Markdown block is a NON-flexible child of `render`'s
        //     `Flex::column`, and a column gives such children an UNBOUNDED max
        //     height by design (`SizeConstraint::child_constraint_along_axis`,
        //     …/presenter.rs:771-783 — the Flutter flex algorithm).
        //
        // So the bar was laid out at height ∞. `Container::layout` returns
        // `child_size + padding/border/margin` without clamping to its own
        // constraint (…/elements/gui/container.rs:269-279), so ∞ propagated
        // straight up through every ancestor Container to the pane wrapper,
        // which then painted an infinitely tall rect and tripped
        // `Scene::validate_rect` (…/scene.rs:567) — a debug_assert that aborts
        // the process.
        //
        // A border has no such failure mode: it is painted on the Container's
        // OWN rect, whose height is the (finite) text height plus padding, so
        // it hugs the quote exactly like the old bar did while being
        // structurally incapable of going infinite. `with_padding_left(9.0)`
        // plus the 3px border keeps the text at the same 12px inset as before
        // (`Container::paint` offsets the child past the border, container.rs:309-314).
        let body = Container::new(self.inline_element(runs, theme::text_muted()))
            .with_padding_left(9.0)
            .with_border(Border::left(3.0).with_border_color(theme::accent()))
            .finish();
        self.pad_block(body, 4.0)
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

    #[test]
    fn heading_with_inline_code_preserves_the_code_text() {
        // Event::Code (the inline-code arm) had no `heading.is_some()` guard,
        // so a code span inside a heading pushed straight into `runs` instead
        // of `head_buf` — the code text was then silently dropped (`runs`
        // is never read while a heading is open, and nothing flushes it
        // here since a heading has no enclosing container at the top
        // level).
        let src = "## Use `foo` here\n";
        let blocks = parse(src);
        let heading_text = blocks
            .iter()
            .find_map(|b| match b {
                Block::Heading { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .expect("one heading");
        assert_eq!(
            heading_text, "Use foo here",
            "an inline code span inside a heading must contribute its literal text to \
             the heading, not be silently dropped"
        );
    }

    #[test]
    fn heading_with_inline_code_inside_a_list_item_does_not_leak_into_a_bullet() {
        // Same defect, list-item case: with the code span misrouted into
        // `runs`, the first item's End(TagEnd::Item) then flushed that
        // leftover "foo" run as a phantom Block::Bullet — content that
        // belongs to the heading leaking into an unrelated sibling block.
        //
        // Once the code span correctly lands in `head_buf` instead, `runs`
        // stays empty for the whole first item (its only content is the
        // heading), so the item produces no Block::Bullet at all — the
        // same pre-existing empty-block guard this file already uses for
        // an item whose only content is a table (see
        // `bullet_containing_only_a_table_produces_no_empty_bullet_block`).
        // Only the second item's own bullet remains.
        let src = "- ## Use `foo` here\n- second\n";
        let blocks = parse(src);

        let heading_text = blocks
            .iter()
            .find_map(|b| match b {
                Block::Heading { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .expect("one heading");
        assert_eq!(
            heading_text, "Use foo here",
            "the heading nested in the first list item must still capture the inline \
             code span's text"
        );

        assert_eq!(
            bullet_texts(&blocks),
            vec!["second".to_string()],
            "the first item's only content is consumed by its heading and must not leave \
             a phantom Block::Bullet behind (\"foo\") — only the second item's own bullet \
             may remain"
        );
    }

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
                Block::Bullet { runs, .. } => {
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

    #[test]
    fn closing_a_nested_list_preserves_the_outer_items_prose() {
        // Not a phantom-bullet check (an earlier empty-block guard already
        // suppresses those) — this guards against *content loss*: the outer
        // item's own prose ("top one") must survive being flushed early, at
        // the moment its nested list's first item opens and starts reusing
        // `runs`. Asserting the actual text (not just non-emptiness) is the
        // point: a variant that replaced "top one" with something else must
        // fail this test.
        let src = "- top one\n  - nested\n- top two\n";
        let blocks = parse(src);
        assert_eq!(
            bullet_texts(&blocks),
            vec!["top one", "nested", "top two"],
            "the outer item's prose must survive intact as \"top one\", not merely \
             survive non-empty"
        );
    }

    #[test]
    fn nested_bullets_record_depth() {
        let src = "- top\n  - nested\n";
        let blocks = parse(src);
        let depths: Vec<usize> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(depths, vec![0, 1], "nested item must record depth 1");
    }

    #[test]
    fn ordered_lists_number_their_items() {
        let src = "1. first\n2. second\n";
        let blocks = parse(src);
        let ordinals: Vec<Option<usize>> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { ordinal, .. } => Some(*ordinal),
                _ => None,
            })
            .collect();
        assert_eq!(ordinals, vec![Some(1), Some(2)]);
    }

    #[test]
    fn ordinal_advances_even_when_an_item_only_contains_a_table() {
        // Finding 1: an ordered item whose only content is a table still
        // legitimately occupies its slot in the sequence — the table itself
        // renders as its own Block::Table; only the item's bullet
        // marker/text is suppressed by the empty-block guard. A guarded
        // increment (advance the counter only when a Block::Bullet is
        // actually pushed) would silently skip a number here, mis-numbering
        // every later sibling.
        let src = "1. first\n2.\n   | a | b |\n   | - | - |\n   | 1 | 2 |\n3. third\n";
        let blocks = parse(src);

        let tables = blocks
            .iter()
            .filter(|b| matches!(b, Block::Table { .. }))
            .count();
        assert_eq!(
            tables, 1,
            "the table-only item must still produce a Block::Table"
        );

        let ordinals: Vec<Option<usize>> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { ordinal, .. } => Some(*ordinal),
                _ => None,
            })
            .collect();
        assert_eq!(
            ordinals,
            vec![Some(1), Some(3)],
            "the table-only item must consume ordinal 2 (no Block::Bullet appears for it), \
             so the next sibling is still numbered 3"
        );
    }

    #[test]
    fn continuation_prose_after_a_nested_list_does_not_repeat_the_items_ordinal() {
        // Finding 1: `depth`/`ordinal` live on the ContainerKind::Bullet
        // stack entry, and that entry survives every flush of the same
        // item (an early one at a nested container's Start, and — if
        // prose remains — the item's own End). Before the fix, each flush
        // read the same `ordinal`, so a loose ordered item with a nested
        // list followed by continuation prose rendered its "1." marker
        // twice.
        let src = "1. one\n\n   - nested\n\n   more prose\n2. two\n";
        let blocks = parse(src);

        let ordinals: Vec<Option<usize>> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { ordinal, .. } => Some(*ordinal),
                _ => None,
            })
            .collect();
        assert_eq!(
            ordinals,
            vec![Some(1), None, None, Some(2)],
            "only the item's FIRST flush may carry its ordinal; the continuation \
             row after the nested list must not repeat \"1.\""
        );
    }

    #[test]
    fn nested_list_inside_blockquote_retains_intro_prose() {
        // Additional required fix: Start(Tag::Item) unconditionally cleared
        // `runs`, so a blockquote's leading prose was wiped the moment its
        // nested list's first item started — a content-loss bug in the same
        // family as the table-in-container case above, one container level
        // over (Item/BlockQuote start, rather than Table start).
        let src = "> Quote intro.\n>\n> - Item text\n";
        let blocks = parse(src);

        assert_eq!(
            quote_texts(&blocks),
            vec!["Quote intro."],
            "prose preceding a nested list must survive in the enclosing Block::Quote"
        );
        assert_eq!(
            bullet_texts(&blocks),
            vec!["Item text"],
            "the nested list item must still render as its own bullet"
        );
    }

    #[test]
    fn blockquote_prose_inside_an_ordered_item_types_as_quote_not_bullet() {
        // Finding 2: `in_bullet`/`in_quote` were independent booleans, and
        // `Start(Tag::BlockQuote)` never cleared `in_bullet` on entry, so
        // blockquote prose nested inside an *ordered* list item was
        // mistyped as Block::Bullet — stealing an ordinal that belonged to
        // the next sibling.
        let src = "1. one\n   > quoted\n   > - deep\n2. two\n";
        let blocks = parse(src);

        assert_eq!(
            quote_texts(&blocks),
            vec!["quoted"],
            "blockquote prose nested in a list item must render as Block::Quote, not Block::Bullet"
        );

        let bullets: Vec<(String, Option<usize>)> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { runs, ordinal, .. } => {
                    Some((runs.iter().map(|r| r.text.as_str()).collect(), *ordinal))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            bullets,
            vec![
                ("one".to_string(), Some(1)),
                ("deep".to_string(), None),
                ("two".to_string(), Some(2)),
            ],
            "the blockquote's nested unordered item ('deep') must not consume an ordinal meant \
             for 'two'"
        );
    }

    #[test]
    fn link_url_is_captured_on_the_run() {
        let src = "See [the docs](https://example.com/guide) for more.\n";
        let blocks = parse(src);
        let runs = blocks
            .iter()
            .find_map(|b| match b {
                Block::Para(runs) => Some(runs),
                _ => None,
            })
            .expect("one paragraph");
        let linked = runs
            .iter()
            .find(|r| r.link.is_some())
            .expect("a run must carry the link URL");
        assert_eq!(linked.text, "the docs");
        assert_eq!(linked.link.as_deref(), Some("https://example.com/guide"));
    }

    #[test]
    fn strikethrough_is_captured() {
        let src = "This is ~~gone~~ now.\n";
        let blocks = parse(src);
        let runs = blocks
            .iter()
            .find_map(|b| match b {
                Block::Para(runs) => Some(runs),
                _ => None,
            })
            .expect("one paragraph");
        assert!(
            runs.iter().any(|r| matches!(r.emph, Emph::Strike) && r.text == "gone"),
            "struck text must be an Emph::Strike run"
        );
    }

    #[test]
    fn task_list_markers_render_as_checkboxes() {
        let src = "- [x] done\n- [ ] pending\n";
        let blocks = parse(src);
        let texts: Vec<String> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Bullet { runs, .. } => {
                    Some(runs.iter().map(|r| r.text.as_str()).collect::<String>())
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "two task items");
        assert_eq!(
            texts[0], "[x] done",
            "the checked marker must render as literal \"[x] \" text, not be silently dropped"
        );
        assert_eq!(
            texts[1], "[ ] pending",
            "the unchecked marker must render as literal \"[ ] \" text, not be silently dropped"
        );
    }

    #[test]
    fn nested_blockquotes_inside_a_bullet_all_type_as_quote() {
        // Finding 2, unordered case: the same `in_bullet`-priority bug
        // mistyped blockquote prose nested inside a bullet item, including a
        // doubly nested blockquote.
        let src = "- a\n  > q1\n  > > q2\n";
        let blocks = parse(src);

        assert_eq!(
            bullet_texts(&blocks),
            vec!["a"],
            "the list item's own prose must still render as Block::Bullet"
        );
        assert_eq!(
            quote_texts(&blocks),
            vec!["q1", "q2"],
            "both the blockquote and its nested blockquote must render as Block::Quote"
        );
    }

    // ── needs_formatted_text (Finding 2: the rendering-layer decision that
    // no prior test exercised — reverting `|| r.link.is_some()` at this
    // check would previously have failed nothing) ──────────────────────────

    #[test]
    fn all_plain_runs_do_not_need_formatted_text() {
        // The fast path must be preserved for the common case: uniform
        // prose, no code/emphasis/link on any run.
        let runs = vec![
            Run { text: "plain ".to_string(), emph: Emph::Normal, link: None },
            Run { text: "prose".to_string(), emph: Emph::Normal, link: None },
        ];
        assert!(
            !needs_formatted_text(&runs),
            "all-Normal, link-free runs must take the plain-Text fast path"
        );
    }

    #[test]
    fn a_run_with_inline_code_needs_formatted_text() {
        let runs = vec![
            Run { text: "run ".to_string(), emph: Emph::Normal, link: None },
            Run { text: "code".to_string(), emph: Emph::Code, link: None },
        ];
        assert!(
            needs_formatted_text(&runs),
            "an Emph::Code run must force the FormattedTextElement path"
        );
    }

    #[test]
    fn a_plain_unemphasized_link_needs_formatted_text() {
        // The regression this task exists to guard: a run can be Emph::Normal
        // and still carry a link. The plain-Text fast path has no way to
        // render hyperlink styling or clicks, so this case alone must force
        // FormattedTextElement even though no `Emph` variant is non-Normal.
        let runs = vec![Run {
            text: "click me".to_string(),
            emph: Emph::Normal,
            link: Some("https://example.com".to_string()),
        }];
        assert!(
            needs_formatted_text(&runs),
            "a Normal-emph run carrying a link must still force FormattedTextElement, \
             or hyperlink styling/clicks silently disappear"
        );
    }

    // ── fragments (extracted to a free function so it can be exercised here
    // without a `WarpMarkdownView`/font instance — previously a private
    // method with zero test coverage: deleting
    // `.register_default_click_handlers(...)` in `inline_element`, or
    // deleting the `if let Some(url) = &r.link { return
    // FormattedTextFragment::hyperlink(...) }` early return below, left all
    // other tests green) ─────────────────────────────────────────────────

    #[test]
    fn a_link_wins_over_emphasis() {
        // The regression this test exists to catch: a run can carry both a
        // link AND emphasis (e.g. **[bold link](url)**). `fragments()` must
        // still emit a hyperlink fragment, not a bold one — deleting the
        // `link`-checking early return would silently fall through to the
        // `match r.emph` arm below and produce
        // `FormattedTextFragment::bold(...)` instead.
        let runs = vec![Run {
            text: "click me".to_string(),
            emph: Emph::Bold,
            link: Some("https://example.com".to_string()),
        }];
        assert_eq!(
            fragments(&runs),
            vec![FormattedTextFragment::hyperlink(
                "click me",
                "https://example.com"
            )],
            "a linked run must produce a hyperlink fragment even when it also carries \
             Emph::Bold — link wins over emphasis"
        );
    }

    #[test]
    fn each_emph_variant_maps_to_its_intended_fragment_constructor() {
        let runs = vec![
            Run { text: "code".to_string(), emph: Emph::Code, link: None },
            Run { text: "bold".to_string(), emph: Emph::Bold, link: None },
            Run { text: "italic".to_string(), emph: Emph::Italic, link: None },
            Run { text: "struck".to_string(), emph: Emph::Strike, link: None },
            Run { text: "plain".to_string(), emph: Emph::Normal, link: None },
        ];
        assert_eq!(
            fragments(&runs),
            vec![
                FormattedTextFragment::inline_code("code"),
                FormattedTextFragment::bold("bold"),
                FormattedTextFragment::italic("italic"),
                FormattedTextFragment::strikethrough("struck"),
                FormattedTextFragment::plain_text("plain"),
            ],
            "each Emph variant must map to its own dedicated FormattedTextFragment \
             constructor, not collapse into a shared one"
        );
    }

    // ── is_allowed_hyperlink_scheme (Finding 1: the security allowlist that
    // gates which URLs are handed to the OS `open`/`xdg-open` command) ──────

    #[test]
    fn http_https_and_mailto_schemes_are_allowed() {
        assert!(is_allowed_hyperlink_scheme("http://example.com"));
        assert!(is_allowed_hyperlink_scheme("https://example.com/guide"));
        assert!(is_allowed_hyperlink_scheme("mailto:someone@example.com"));
        // Case-insensitive prefix match, per the brief.
        assert!(is_allowed_hyperlink_scheme("HTTPS://Example.Com"));
    }

    #[test]
    fn other_schemes_are_rejected() {
        // A malicious README could point a link at a local-file or
        // custom-app scheme to trigger unwanted local behavior via `open`.
        assert!(!is_allowed_hyperlink_scheme("file:///etc/passwd"));
        assert!(!is_allowed_hyperlink_scheme("javascript:alert(1)"));
        assert!(!is_allowed_hyperlink_scheme("ftp://example.com/file"));
        assert!(!is_allowed_hyperlink_scheme("some-app://do-something"));
        assert!(!is_allowed_hyperlink_scheme("not a url at all"));
        assert!(!is_allowed_hyperlink_scheme(""));
    }

    // ── Layout regression tests ──────────────────────────────────────────────
    //
    // These build a REAL scene headlessly (warpui's test platform: stub window
    // manager + stub font DB), which runs the full layout + paint pass over the
    // rendered element tree. `Scene::validate_rect`
    // (vendor/warp/crates/warpui_core/src/scene.rs:550-574) debug-asserts that
    // no painted rect has a non-finite origin or size, so any element that
    // resolves to an infinite width/height aborts the test the same way it
    // aborted the app. Nothing else in the suite exercises layout at all.
    fn build_markdown_scene(src: &'static str) {
        use std::collections::HashSet;

        use warpui::geometry::vector::vec2f;
        use warpui::platform::WindowStyle;
        use warpui::{App, Presenter, WindowInvalidation};

        App::test((), |mut app| async move {
            let app = &mut app;
            let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpMarkdownView::from_source(ctx, "test.md".to_string(), src.to_string())
            });
            let mut presenter = Presenter::new(window_id);
            let mut updated = HashSet::new();
            updated.insert(app.root_view_id(window_id).unwrap());
            let invalidation = WindowInvalidation { updated, ..Default::default() };
            app.update(move |ctx| {
                presenter.invalidate(invalidation, ctx);
                // A concrete, finite window — the pane a Markdown view lives in
                // is always finitely sized; the infinities are produced INSIDE
                // the view, by children laid out under the unbounded main-axis
                // constraint that `Flex::column` hands every non-flexible child.
                let _ = presenter.build_scene(vec2f(900.0, 600.0), 1.0, None, ctx);
            });
        });
    }

    #[test]
    fn a_table_lays_out_finitely() {
        // The crash repro: a table-heavy document. Table cells sit in a
        // `Flex::row` nested in a `Flex::column`, and a column hands its
        // non-flexible children an unbounded max height
        // (`SizeConstraint::child_constraint_along_axis`,
        // presenter.rs:771-783). Anything in that subtree that sizes itself
        // to `constraint.max` — a bare `Rect` most of all (rect.rs:110) —
        // becomes infinitely tall and poisons every ancestor's height.
        build_markdown_scene(
            "| Service | User | Notes |\n\
             |---|---|---|\n\
             | one | a@example.com | first |\n\
             | two | b@example.com | second |\n\
             | three | c@example.com | third |\n",
        );
    }

    #[test]
    fn a_blockquote_lays_out_finitely() {
        // `quote_element`'s accent bar is a `Rect` in a `Stack`, and a `Stack`
        // passes its own constraint straight through to every child
        // (stack/mod.rs:184-196). Under the column's unbounded height that
        // Rect resolved to an infinitely tall bar.
        build_markdown_scene("> a quoted line\n>\n> and another\n");
    }

    #[test]
    fn a_bullet_list_lays_out_finitely() {
        // `bullet_element`'s dot marker is also a Rect; its ConstrainedBox
        // pins both axes, but the row around it must stay finite too.
        build_markdown_scene("- one\n- two\n  - nested\n1. first\n2. second\n");
    }

    #[test]
    fn a_mixed_document_lays_out_finitely() {
        // Everything at once, in the shape of the file that crashed: headings,
        // prose, a rule, a fenced code block, a table and a blockquote.
        build_markdown_scene(
            "# Logins\n\n\
             Some prose with `code`, **bold** and a [link](https://example.com).\n\n\
             ---\n\n\
             ```sh\necho hi\n```\n\n\
             | Env | URL |\n|---|---|\n| dev | https://dev.example.com |\n\n\
             > remember to rotate these\n\n\
             - a bullet\n",
        );
    }

    // ── Restore-path coverage ─────────────────────────────────────────────────
    //
    // The session-restore code in `shell.rs` rebuilds a Markdown pane via
    // `WarpMarkdownView::new(ctx, path)` — the SAME constructor a fresh "open
    // file" click uses, never `from_source`. Before this task that arm did not
    // exist at all: a saved Markdown pane fell through to a fresh terminal.
    // These tests exercise `new` (not `from_source`) directly, the way restore
    // does, to catch a regression in either the path plumbing or the layout
    // pass on a file-backed view specifically.
    #[test]
    fn new_from_a_real_path_records_that_path_for_persistence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.md");
        std::fs::write(&file, "# hello\n").expect("write temp md file");

        use warpui::platform::WindowStyle;
        use warpui::App;

        let file_for_view = file.clone();
        App::test((), move |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpMarkdownView::new(ctx, file_for_view.clone())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert_eq!(
                        v.path(),
                        Some(file_for_view.as_path()),
                        "a view built via `new` (the restore constructor) must report its \
                         source path, or the pane can never be found again by \
                         `build_state`'s save-side lookup"
                    );
                });
            });
        });
    }

    #[test]
    fn a_view_restored_via_new_lays_out_finitely() {
        // Proves the exact call the restore arm makes — WarpMarkdownView::new
        // on a real file — runs the full layout + paint pass without tripping
        // `Scene::validate_rect`, the same crash class the `_lays_out_finitely`
        // tests above guard for `from_source`.
        use std::collections::HashSet;

        use warpui::geometry::vector::vec2f;
        use warpui::platform::WindowStyle;
        use warpui::{App, Presenter, WindowInvalidation};

        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("restored.md");
        std::fs::write(
            &file,
            "# Restored\n\nSome prose with `code` and a [link](https://example.com).\n\n\
             | A | B |\n|---|---|\n| 1 | 2 |\n\n> a quote\n\n- a bullet\n",
        )
        .expect("write temp md file");

        App::test((), |mut app| async move {
            let app = &mut app;
            let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpMarkdownView::new(ctx, file)
            });
            let mut presenter = Presenter::new(window_id);
            let mut updated = HashSet::new();
            updated.insert(app.root_view_id(window_id).unwrap());
            let invalidation = WindowInvalidation { updated, ..Default::default() };
            app.update(move |ctx| {
                presenter.invalidate(invalidation, ctx);
                let _ = presenter.build_scene(vec2f(900.0, 600.0), 1.0, None, ctx);
            });
        });
    }

    #[test]
    fn from_source_has_no_path() {
        // The other half of the path/None split this task introduces:
        // in-memory documents cannot be persisted, so `path()` must be None
        // for them, not an empty PathBuf standing in for "unset".
        use warpui::platform::WindowStyle;
        use warpui::App;

        App::test((), |mut app| async move {
            let app = &mut app;
            let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                WarpMarkdownView::from_source(ctx, "in-memory.md".to_string(), "# hi\n".to_string())
            });
            app.update(move |ctx| {
                view.update(ctx, |v, _vctx| {
                    assert_eq!(
                        v.path(),
                        None,
                        "an in-memory document built via from_source must report no path"
                    );
                });
            });
        });
    }
}
