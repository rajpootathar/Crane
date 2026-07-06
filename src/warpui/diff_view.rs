//! `WarpDiffView` — a read-only UNIFIED diff pane (the warpui port of old
//! Crane's `views/diff_view.rs`). v1 renders HEAD-vs-working-copy as a scrollable
//! list of rows with dual line-number gutters and add/delete background tints.
//!
//! Deliberately scoped DOWN from the egui original: NO hunk-staging gutter, NO
//! minimap, NO per-hunk navigation — those land in a later wave. The Row model
//! and the `similar::TextDiff` compute mirror the old view so behavior matches.
//!
//! Syntax highlighting is NOT applied in v1 (each line renders as one plain
//! `Text` in the diff foreground colors). The egui view syntect-highlights per
//! line by building a multi-segment layout job; the warpui equivalent would be a
//! `Flex::row` of colored `Text` spans per line — a follow-up once the read-only
//! shape is settled.

use std::path::PathBuf;

use similar::{ChangeTag, TextDiff};
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, DispatchEventResult, Element, EventHandler, Expanded, Flex,
    ParentElement, Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Entity, SingletonEntity as _, TypedActionView, View, ViewContext};

use crate::warpui::theme;

/// Render cap: a huge diff can't blow up the element tree. The full `rows` Vec is
/// retained; only a window of `RENDER_LINES` from the scroll offset is drawn
/// (same manual-scroll approach as `FileView`). Recomputing on scroll is cheap —
/// the diff itself is computed once in `new`.
const RENDER_LINES: usize = 2000;

/// One rendered diff line — a 1:1 subset of old Crane's `diff_view::Row`
/// (dropping the git-hunk plumbing this read-only v1 doesn't need). `old_ln` /
/// `new_ln` are pre-space-padded to the gutter width so a monospace font aligns
/// them with zero width math.
struct Row {
    tag: ChangeTag,
    old_ln: String,
    new_ln: String,
    text: String,
}

/// Blend `c` down to alpha `a` for a translucent row tint (mirrors the way
/// `theme::drop_zone` derives a wash from `accent`).
fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU { r: c.r, g: c.g, b: c.b, a }
}

/// Build the diff rows for `old_text` (HEAD) vs `new_text` (working copy).
/// Line-based, exactly like the egui original: `iter_all_changes` yields one row
/// per line, tagged Equal / Insert / Delete, with 1-based old/new line numbers
/// (absent on the opposite side of an insert/delete).
fn compute_rows(old_text: &str, new_text: &str) -> Vec<Row> {
    let diff = TextDiff::from_lines(old_text, new_text);
    let old_count = old_text.lines().count().max(1);
    let new_count = new_text.lines().count().max(1);
    let ldigits = old_count.to_string().len().max(3);
    let rdigits = new_count.to_string().len().max(3);

    diff.iter_all_changes()
        .map(|c| {
            let old_lno = c.old_index().map(|i| i + 1);
            let new_lno = c.new_index().map(|i| i + 1);
            Row {
                tag: c.tag(),
                old_ln: old_lno
                    .map(|n| format!("{:>w$}", n, w = ldigits))
                    .unwrap_or_else(|| " ".repeat(ldigits)),
                new_ln: new_lno
                    .map(|n| format!("{:>w$}", n, w = rdigits))
                    .unwrap_or_else(|| " ".repeat(rdigits)),
                // Keep leading whitespace (indentation); only drop the newline.
                text: c.value().trim_end_matches('\n').to_string(),
            }
        })
        .collect()
}

pub struct WarpDiffView {
    /// Mono font (loaded internally so the view is self-contained).
    font: FamilyId,
    /// Display title (the diffed file's name) for the shell-drawn pane header.
    title: String,
    /// Diff rows. Empty until the off-thread `head_content` + read + `TextDiff`
    /// compute lands (see `new`), then filled by the `ctx.spawn` callback.
    rows: Vec<Row>,
    /// First visible row (manual scroll offset, in rows).
    scroll: usize,
    /// True while the async diff compute is in flight — render shows a
    /// "Computing diff…" placeholder instead of "No differences" so an empty
    /// pane isn't mistaken for a clean file mid-load.
    loading: bool,
}

impl WarpDiffView {
    /// Diff `path` (workspace-relative OR absolute) as HEAD vs the working copy.
    ///
    /// `repo_root` is the worktree dir used to (a) resolve a relative `path` to
    /// disk and (b) shell out `git show HEAD:<relpath>` for the old content via
    /// `crate::git::head_content` (Crane's git-binary rule; never libgit2).
    ///
    /// Robust to new / untracked files: `head_content` returns an empty string
    /// on any git failure, so the diff naturally renders every line as an add.
    /// If the disk read fails, the new side is empty and every line renders as a
    /// delete — either way the view stays populated instead of panicking.
    pub fn new(ctx: &mut ViewContext<Self>, repo_root: Option<PathBuf>, path: PathBuf) -> Self {
        let font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache.load_system_font("Menlo").expect("load Menlo")
        });

        // Absolute path on disk (for the working-copy side).
        let abs = if path.is_absolute() {
            path.clone()
        } else if let Some(root) = &repo_root {
            root.join(&path)
        } else {
            path.clone()
        };

        // Repo-relative path for `git show HEAD:<relpath>`. Prefer stripping the
        // repo root off the absolute path; fall back to the raw (relative) path,
        // then to the bare file name. Normalize separators to '/' for git.
        let rel = repo_root
            .as_ref()
            .and_then(|root| abs.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));

        let title = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        // Compute the diff OFF the UI thread: `git show HEAD:<rel>` (subprocess),
        // the working-copy read, and the whole-file `similar::TextDiff` all run in
        // the spawned future — a large/changed file would otherwise stall the frame
        // that opens the pane. The pane paints immediately with a "Computing diff…"
        // placeholder; the callback fills `rows` and notifies. Read-only view, so
        // there is nothing to guard beyond the view still being alive.
        let repo_root_fut = repo_root.clone();
        let fut = async move {
            let old_text = match &repo_root_fut {
                Some(root) => crate::git::head_content(root, &rel),
                None => String::new(),
            };
            let new_text = std::fs::read_to_string(&abs).unwrap_or_default();
            compute_rows(&old_text, &new_text)
        };
        ctx.spawn(fut, |this, rows, vctx| {
            this.rows = rows;
            this.loading = false;
            vctx.notify();
        });

        Self {
            font,
            title,
            rows: Vec::new(),
            scroll: 0,
            loading: true,
        }
    }

    /// The diffed file's name (for the shell-drawn pane header).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Fixed-width horizontal gap between gutter / sign / content columns.
    fn spacer(w: f32) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().finish())
            .with_width(w)
            .with_height(1.0)
            .finish()
    }

    /// Fill the pane background (theme::bg) behind the scrolling content.
    fn panel(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(content)
            .finish()
    }

    /// One diff row: `[old  new] [sign] [text]`, wrapped in a Container whose
    /// background carries the add/delete tint (transparent for Equal rows).
    fn row_element(&self, r: &Row) -> Box<dyn Element> {
        let (sign, sign_color, bg) = match r.tag {
            ChangeTag::Insert => ("+", theme::success(), tint(theme::success(), 40)),
            ChangeTag::Delete => ("-", theme::error(), tint(theme::error(), 40)),
            ChangeTag::Equal => (" ", theme::text_muted(), tint(theme::bg(), 0)),
        };
        // Dual line-number gutter as one padded, monospace-aligned Text.
        let gutter = Text::new(
            format!("{} {}", r.old_ln, r.new_ln),
            self.font,
            12.0,
        )
        .with_color(theme::text_muted())
        .finish();
        let sign_el = Text::new(sign.to_string(), self.font, 12.0)
            .with_color(sign_color)
            .finish();
        let content = Text::new(r.text.clone(), self.font, 12.0)
            .with_color(theme::text())
            .finish();
        let row = Flex::row()
            .with_child(gutter)
            .with_child(Self::spacer(10.0))
            .with_child(sign_el)
            .with_child(Self::spacer(8.0))
            .with_child(content)
            .finish();
        Container::new(row)
            .with_background_color(bg)
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .finish()
    }
}

impl Entity for WarpDiffView {
    type Event = ();
}

#[derive(Debug, Clone)]
pub enum WarpDiffAction {
    /// Scroll by N rows (positive = down). Dispatched by the scroll wheel.
    Scroll(i32),
}

impl TypedActionView for WarpDiffView {
    type Action = WarpDiffAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WarpDiffAction::Scroll(delta) => {
                let max = self.rows.len().saturating_sub(1);
                let next = self.scroll as i64 + *delta as i64;
                self.scroll = next.clamp(0, max as i64) as usize;
            }
        }
        ctx.notify();
    }
}

impl View for WarpDiffView {
    fn ui_name() -> &'static str {
        "WarpDiffView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        let mut body = Flex::column();
        if self.rows.is_empty() {
            let msg = if self.loading { "Computing diff…" } else { "No differences" };
            body = body.with_child(
                Container::new(
                    Text::new(msg.to_string(), self.font, 12.0)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_top(8.0)
                .finish(),
            );
        } else {
            // Draw a window of rows from the scroll offset (manual scroll — same
            // bounded-element-tree approach as FileView / the terminal).
            let start = self.scroll.min(self.rows.len().saturating_sub(1));
            for r in self.rows.iter().skip(start).take(RENDER_LINES) {
                body = body.with_child(self.row_element(r));
            }
        }
        // Scroll wheel adjusts the row window (mirrors FileView's nesting +
        // divisor feel: outer Expanded fills the column, inner Expanded fills the
        // EventHandler so the whole pane is a scroll target).
        let scroll_body = EventHandler::new(Expanded::new(1.0, body.finish()).finish())
            .on_scroll_wheel(move |ctx, _app, delta, _mods| {
                let lines = (-delta.y() / 8.0).round() as i32;
                if lines != 0 {
                    ctx.dispatch_typed_action(WarpDiffAction::Scroll(lines));
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
