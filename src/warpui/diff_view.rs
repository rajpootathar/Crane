//! `WarpDiffView` — the unified diff Pane (warpui port of old Crane's
//! `views/diff_view.rs`, wave 2). Renders HEAD-vs-working-copy with:
//!
//! - per-line SYNTAX HIGHLIGHTING (syntect via `crate::syntax`, per side:
//!   deletion rows colored from the HEAD text, insert/context rows from the
//!   working copy) layered over the add/delete background tints;
//! - a per-hunk STAGE/UNSTAGE GUTTER (circle → check-circle affordance at each
//!   visual hunk start, with connector lines through rows that share one git
//!   hunk) wired to `crate::git::{stage_hunk, unstage_hunk, is_hunk_staged}`;
//! - a right-edge MINIMAP strip (add/del markers scaled to file length,
//!   click / drag to jump);
//! - HUNK NAV buttons + an `N / M` counter in an in-body header row that also
//!   carries the old header's rename `old -> new` path coloring;
//! - a real scroll model: windowed painting around a fractional row offset
//!   (no element-tree blowup, no 2000-row cap) plus a draggable
//!   [`LineScrollbar`] at the right edge;
//! - an ERROR row when `git show HEAD:` or the working-copy read fails (the
//!   old view silently diffed empty text), and a dismissible banner for
//!   stage/unstage failures;
//! - a BINARY / IMAGE guard: extension- or content-detected binaries render a
//!   "no text diff" row instead of garbage `TextDiff` output. (The old egui
//!   view rendered the image itself; warpui has no image element yet, so the
//!   guard row stands in.)
//!
//! The whole body is one custom [`Element`] (`DiffBodyElement`) following the
//! `GitLogListElement` idiom: viewport-aware painting, internal wheel
//! scrolling via a shared fractional row offset, and clicks dispatched as
//! typed [`WarpDiffAction`]s so no shell wiring is required.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Fill, Flex, Hoverable,
    MouseStateHandle, ParentElement, Point, Rect, Stack, Text,
};
use warpui::event::{DispatchedEvent, Event};
use warpui::fonts::{FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EventContext, LayoutContext, PaintContext,
    SingletonEntity as _, SizeConstraint, TypedActionView, View, ViewContext,
};

use crate::warpui::icons;
use crate::warpui::scrollbar_element::LineScrollbar;
use crate::warpui::theme;

/// Body font size (matches v1 / the Files Pane).
const FONT_SIZE: f32 = 12.0;
/// Stage-hunk control gutter width (1:1 old egui `stage_btn_w`).
const STAGE_W: f32 = 28.0;
/// Minimap strip width (1:1 old egui `MINIMAP_W`).
const MINIMAP_W: f32 = 10.0;

/// Phosphor check-circle — the one diff-gutter glyph `icons.rs` doesn't carry
/// yet (value matches egui_phosphor 0.12 regular, same as the other consts).
const CHECK_CIRCLE: &str = "\u{E184}";

/// Blend `c` down to alpha `a` for a translucent row tint (mirrors the way
/// `theme::drop_zone` derives a wash from `accent`).
fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU { r: c.r, g: c.g, b: c.b, a }
}

/// Sentinel span color meaning "no syntect foreground — use `theme::text()`".
const NO_COLOR: ColorU = ColorU { r: 0, g: 0, b: 0, a: 0 };

/// One rendered diff line — the old egui `diff_view::Row` plus per-span syntax
/// colors. `old_ln` / `new_ln` are pre-space-padded to the gutter width so a
/// monospace font aligns them with zero width math. `old_lno` / `new_lno`
/// (1-based, absent on the opposite side of an insert/delete) match similar's
/// row-level hunks against git's line-range hunks.
struct Row {
    tag: ChangeTag,
    old_ln: String,
    new_ln: String,
    /// Syntect spans: `(fg color, text)`. `NO_COLOR` alpha-0 = theme text.
    spans: Vec<(ColorU, String)>,
    old_lno: Option<usize>,
    new_lno: Option<usize>,
}

/// Pure result of diffing HEAD vs working copy + git hunk plumbing. Built once
/// per compute (off the UI thread); render/paint read it with zero git calls.
struct DiffComputed {
    rows: Vec<Row>,
    /// Row index of each visual hunk start (first changed row of a run).
    hunk_starts: Vec<usize>,
    /// Per visual hunk: the git patch to (un)stage. `None` = the hunk shares a
    /// git hunk with an earlier visual hunk (deduped) or matched no git hunk.
    hunk_patches: Vec<Option<String>>,
    /// Per visual hunk: true if already in the index (action = unstage).
    hunk_staged: Vec<bool>,
    row_to_hunk: Vec<Option<usize>>,
    /// Rows that share a git hunk with an earlier visual hunk — the gutter
    /// draws a vertical connector through them.
    row_in_shared_group: Vec<bool>,
    ldigits: usize,
    rdigits: usize,
    /// Binary / image guard message. Set → render this instead of rows.
    binary: Option<String>,
    /// Load failure (git show HEAD / disk read). Set → render this, no diff.
    error: Option<String>,
}

impl DiffComputed {
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            hunk_starts: Vec::new(),
            hunk_patches: Vec::new(),
            hunk_staged: Vec::new(),
            row_to_hunk: Vec::new(),
            row_in_shared_group: Vec::new(),
            ldigits: 3,
            rdigits: 3,
            binary: None,
            error: None,
        }
    }

    fn with_error(msg: String) -> Self {
        Self { error: Some(msg), ..Self::empty() }
    }

    fn with_binary(msg: String) -> Self {
        Self { binary: Some(msg), ..Self::empty() }
    }
}

/// True when `path`'s extension marks an image (old `file_util::is_image_path`).
/// The extension list itself lives in `shell::IMAGE_EXTS` — the same one that
/// routes a file to an Image pane — so this diff guard and that routing
/// decision can never disagree about what an image is. This wrapper exists
/// only to take a `&str` (git gives paths as strings here).
fn is_image_path_str(path: &str) -> bool {
    crate::warpui::shell::is_image_path(Path::new(path))
}

/// Content-sniff for binary data: a NUL in the first 8 KB (the classic git
/// heuristic) or invalid UTF-8 anywhere — either way `TextDiff` output would
/// be garbage, so the caller renders the binary guard row instead.
fn looks_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8000)];
    if probe.contains(&0) {
        return true;
    }
    std::str::from_utf8(bytes).is_err()
}

/// The syntect theme to render with — same resolution as `editor_view`: honor
/// the user's configured `syntax_theme`, else a sensible dark default.
fn render_theme() -> &'static syntect::highlighting::Theme {
    let all = &crate::syntax::themes().themes;
    let requested = crate::theme::current().syntax_theme.clone();
    all.get(&requested)
        .or_else(|| all.get("OneHalfDark"))
        .or_else(|| all.get("base16-eighties.dark"))
        .or_else(|| all.get("base16-ocean.dark"))
        .unwrap_or_else(|| {
            all.values()
                .next()
                .unwrap_or_else(|| crate::syntax::fallback_theme())
        })
}

/// Syntect-highlight one SIDE of the diff (whole text, stateful line-by-line so
/// multi-line constructs color correctly) into per-line span lists. Tabs are
/// expanded to 4 spaces — the monospace painter advances one cell per char.
fn highlight_side(text: &str, ext: &str) -> Vec<Vec<(ColorU, String)>> {
    let ss = crate::syntax::syntaxes();
    let syntax = ss
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, render_theme());
    text.lines()
        .map(|line| {
            let with_nl = format!("{line}\n");
            match hl.highlight_line(&with_nl, ss) {
                Ok(spans) => spans
                    .into_iter()
                    .filter_map(|(st, t)| {
                        let t = t.trim_end_matches('\n');
                        if t.is_empty() {
                            return None;
                        }
                        let c = st.foreground;
                        let color = if c.a == 0 {
                            NO_COLOR
                        } else {
                            ColorU { r: c.r, g: c.g, b: c.b, a: 255 }
                        };
                        Some((color, t.replace('\t', "    ")))
                    })
                    .collect(),
                Err(_) => vec![(NO_COLOR, line.replace('\t', "    "))],
            }
        })
        .collect()
}

/// Row indexes where a visual hunk (contiguous run of non-Equal rows) starts.
fn visual_hunk_starts(tags: &[ChangeTag]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut in_hunk = false;
    for (i, tag) in tags.iter().enumerate() {
        let changed = !matches!(tag, ChangeTag::Equal);
        if changed && !in_hunk {
            starts.push(i);
        }
        in_hunk = changed;
    }
    starts
}

/// Match each visual hunk to a parsed git hunk BY LINE NUMBER (not index): git
/// groups adjacent changes by context proximity, similar by contiguity, so the
/// counts differ and zipping by index would stage the wrong patch. Probes the
/// hunk's first rows for a usable line number (a pure-deletion hunk's first
/// row has no `new_lno`; fall back to `old_lno`). Dedupes by git-hunk identity
/// — two visual hunks resolving to the same git hunk would otherwise both
/// stage it and both read as staged.
///
/// `lnos` = per-row `(old_lno, new_lno)`; `parsed` = per-git-hunk
/// `(old_start, old_count, new_start, new_count)`. Returns, per visual hunk,
/// the index into `parsed` (or `None` for deduped / unmatched).
fn match_hunks(
    hunk_starts: &[usize],
    lnos: &[(Option<usize>, Option<usize>)],
    parsed: &[(usize, usize, usize, usize)],
) -> Vec<Option<usize>> {
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    hunk_starts
        .iter()
        .map(|&start_row| {
            let mut new_target: Option<usize> = None;
            let mut old_target: Option<usize> = None;
            for &(old_lno, new_lno) in &lnos[start_row..(start_row + 5).min(lnos.len())] {
                if new_target.is_none() {
                    new_target = new_lno;
                }
                if old_target.is_none() {
                    old_target = old_lno;
                }
                if new_target.is_some() && old_target.is_some() {
                    break;
                }
            }
            let matched = parsed.iter().enumerate().find(|(_, h)| {
                if let Some(n) = new_target {
                    if n >= h.2 && n < h.2 + h.3 {
                        return true;
                    }
                }
                if let Some(o) = old_target {
                    if o >= h.0 && o < h.0 + h.1 {
                        return true;
                    }
                }
                false
            });
            match matched {
                Some((idx, _)) if seen.insert(idx) => Some(idx),
                _ => None,
            }
        })
        .collect()
}

/// Mark rows that share a git hunk with an earlier visual hunk (downstream
/// rows of a multi-visual-hunk group). The gutter renders a vertical connector
/// through them so it's visible the changes ship on one stage action. A
/// group's connector ends at the last CHANGE row of its last shared visual
/// hunk — never through trailing Equal rows (1:1 old egui logic).
fn shared_group_flags(
    hunk_starts: &[usize],
    has_patch: &[bool],
    tags: &[ChangeTag],
) -> Vec<bool> {
    let total = tags.len();
    let mut flags = vec![false; total];
    let change_end = |start: usize| -> usize {
        let mut end = start;
        while end < tags.len() && !matches!(tags[end], ChangeTag::Equal) {
            end += 1;
        }
        end
    };
    let mut hi = 0;
    while hi < hunk_starts.len() {
        if !has_patch.get(hi).copied().unwrap_or(false) {
            hi += 1;
            continue;
        }
        let mut j = hi + 1;
        while j < hunk_starts.len() && !has_patch.get(j).copied().unwrap_or(false) {
            j += 1;
        }
        if j > hi + 1 {
            let anchor_row = hunk_starts[hi];
            let last_shared = j - 1;
            let group_change_end = change_end(hunk_starts[last_shared]);
            for r in (anchor_row + 1)..group_change_end {
                if r < flags.len() {
                    flags[r] = true;
                }
            }
        }
        hi = j;
    }
    flags
}

/// Diff `old_text` vs `new_text` into display rows with per-side syntax spans.
/// Deletion rows color from the OLD side, insert/context rows from the NEW —
/// each side is highlighted statefully over its own full text so multi-line
/// constructs (block comments, strings) stay correct.
fn build_rows(old_text: &str, new_text: &str, ext: &str) -> (Vec<Row>, usize, usize) {
    let old_count = old_text.lines().count().max(1);
    let new_count = new_text.lines().count().max(1);
    let ldigits = old_count.to_string().len().max(3);
    let rdigits = new_count.to_string().len().max(3);
    let old_spans = highlight_side(old_text, ext);
    let new_spans = highlight_side(new_text, ext);

    let diff = TextDiff::from_lines(old_text, new_text);
    let rows = diff
        .iter_all_changes()
        .map(|c| {
            let old_lno = c.old_index().map(|i| i + 1);
            let new_lno = c.new_index().map(|i| i + 1);
            let spans = match c.tag() {
                ChangeTag::Delete => old_lno.and_then(|n| old_spans.get(n - 1)),
                _ => new_lno.and_then(|n| new_spans.get(n - 1)),
            }
            .cloned()
            .unwrap_or_else(|| {
                let t = c.value().trim_end_matches('\n').replace('\t', "    ");
                if t.is_empty() { Vec::new() } else { vec![(NO_COLOR, t)] }
            });
            Row {
                tag: c.tag(),
                old_ln: old_lno
                    .map(|n| format!("{:>w$}", n, w = ldigits))
                    .unwrap_or_else(|| " ".repeat(ldigits)),
                new_ln: new_lno
                    .map(|n| format!("{:>w$}", n, w = rdigits))
                    .unwrap_or_else(|| " ".repeat(rdigits)),
                spans,
                old_lno,
                new_lno,
            }
        })
        .collect();
    (rows, ldigits, rdigits)
}

/// The expensive bit — runs inside the spawned future, OFF the UI thread:
/// `git show HEAD:` (subprocess), the working-copy read, both-side syntect
/// highlighting, `TextDiff`, `git diff` hunk parsing, and the per-hunk
/// staged-state probes (`git apply --reverse --cached --check` each).
fn compute(repo_root: Option<PathBuf>, rel: String, abs: PathBuf) -> DiffComputed {
    // Image guard first (extension-based, cheap). The old egui view rendered
    // the image; warpui has no image element yet, so a guard row stands in.
    if is_image_path_str(&rel) {
        return DiffComputed::with_binary(
            "Image file — no text diff (image preview pending)".to_string(),
        );
    }

    // Working-copy side. A missing file is a legit all-deletions diff; a file
    // that EXISTS but won't read is a real error the user must see.
    let new_bytes: Option<Vec<u8>> = match std::fs::read(&abs) {
        Ok(b) => Some(b),
        Err(e) => {
            if abs.exists() {
                return DiffComputed::with_error(format!(
                    "Failed to read {}: {e}",
                    abs.display()
                ));
            }
            None
        }
    };

    // HEAD side. `Ok(None)` = not in HEAD (new/untracked file, unborn HEAD) —
    // diff against empty. `Err` = real git failure, surfaced instead of the
    // old behavior of silently diffing empty text.
    let old_bytes: Option<Vec<u8>> = match &repo_root {
        Some(root) => match crate::warpui::git::head_bytes(root, &rel) {
            Ok(b) => b,
            Err(e) => {
                return DiffComputed::with_error(format!("git show HEAD:{rel} failed: {e}"));
            }
        },
        None => None,
    };

    if new_bytes.as_deref().map(looks_binary).unwrap_or(false)
        || old_bytes.as_deref().map(looks_binary).unwrap_or(false)
    {
        return DiffComputed::with_binary("Binary file — no text diff".to_string());
    }

    // looks_binary() proved both sides valid UTF-8.
    let old_text = old_bytes
        .map(|b| String::from_utf8(b).unwrap_or_default())
        .unwrap_or_default();
    let new_text = new_bytes
        .map(|b| String::from_utf8(b).unwrap_or_default())
        .unwrap_or_default();

    let ext = Path::new(&rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (rows, ldigits, rdigits) = build_rows(&old_text, &new_text, &ext);

    let tags: Vec<ChangeTag> = rows.iter().map(|r| r.tag).collect();
    let total_rows = tags.len();
    let hunk_starts = visual_hunk_starts(&tags);

    // Per-hunk patches via `git diff --unified=0` + line-number matching, then
    // a staged-state probe per patch (see match_hunks for why not by index).
    let (hunk_patches, hunk_staged): (Vec<Option<String>>, Vec<bool>) = match &repo_root {
        Some(root) => {
            if let Some(raw) = crate::git::file_diff_raw(root, &rel) {
                let parsed = crate::git::parse_hunks_detailed(&raw);
                let ranges: Vec<(usize, usize, usize, usize)> = parsed
                    .iter()
                    .map(|h| (h.old_start, h.old_count, h.new_start, h.new_count))
                    .collect();
                let lnos: Vec<(Option<usize>, Option<usize>)> =
                    rows.iter().map(|r| (r.old_lno, r.new_lno)).collect();
                let matched = match_hunks(&hunk_starts, &lnos, &ranges);
                let patches: Vec<Option<String>> = matched
                    .iter()
                    .map(|m| m.map(|i| parsed[i].patch.clone()))
                    .collect();
                let staged = patches
                    .iter()
                    .map(|p| match p {
                        Some(patch) => crate::git::is_hunk_staged(root, patch),
                        None => false,
                    })
                    .collect();
                (patches, staged)
            } else {
                (vec![None; hunk_starts.len()], vec![false; hunk_starts.len()])
            }
        }
        None => (vec![None; hunk_starts.len()], vec![false; hunk_starts.len()]),
    };

    let mut row_to_hunk: Vec<Option<usize>> = vec![None; total_rows];
    for (hi, &start) in hunk_starts.iter().enumerate() {
        let end = hunk_starts.get(hi + 1).copied().unwrap_or(total_rows);
        for r in start..end.min(total_rows) {
            row_to_hunk[r] = Some(hi);
        }
    }

    let has_patch: Vec<bool> = hunk_patches.iter().map(|p| p.is_some()).collect();
    let row_in_shared_group = shared_group_flags(&hunk_starts, &has_patch, &tags);

    DiffComputed {
        rows,
        hunk_starts,
        hunk_patches,
        hunk_staged,
        row_to_hunk,
        row_in_shared_group,
        ldigits,
        rdigits,
        binary: None,
        error: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// View
// ─────────────────────────────────────────────────────────────────────────────

pub struct WarpDiffView {
    /// Mono font (loaded internally so the view is self-contained).
    font: FamilyId,
    /// Phosphor icon font for the gutter / nav / error glyphs.
    icon_font: FamilyId,
    /// Display title (the diffed file's name) for the shell-drawn pane header.
    title: String,
    /// Header display paths (old side / new side). Same file today (HEAD vs
    /// working copy), but the header keeps the old `old -> new` rename
    /// coloring for when they differ.
    left_path: String,
    right_path: String,
    repo_root: Option<PathBuf>,
    /// Repo-relative path (git side) and absolute path (disk side).
    rel: String,
    abs: PathBuf,
    /// Latest compute result. `None` until the first off-thread compute lands;
    /// kept (and re-rendered) while a recompute is in flight so a hunk stage
    /// doesn't flash the pane back to the spinner.
    computed: Option<Rc<DiffComputed>>,
    /// True while a compute is in flight.
    loading: bool,
    /// Stage/unstage failure surfaced as a dismissible banner.
    error: Option<String>,
    /// Fractional scroll offset in ROWS (shared with the body element, which
    /// clamps it to the content each paint).
    scroll: Rc<Cell<f32>>,
    /// Hovered hunk index (stage-gutter affordance highlight).
    hover_hunk: Rc<Cell<Option<usize>>>,
    /// Viewport height in rows, written by the body element each layout — the
    /// scrollbar and ScrollToFrac math read it (one frame stale is fine).
    viewport_rows: Rc<Cell<f32>>,
    /// Minimap drag-in-progress (persisted across per-frame element rebuilds).
    minimap_drag: Rc<Cell<bool>>,
    /// Scrollbar-thumb drag state (same persistence contract).
    scrollbar_drag: Rc<Cell<bool>>,
    /// Hover state for the nav-up / nav-down buttons and the error dismiss X.
    nav_up: MouseStateHandle,
    nav_down: MouseStateHandle,
    err_x: MouseStateHandle,
}

#[derive(Debug, Clone)]
pub enum WarpDiffAction {
    /// Jump to the previous (-1) / next (+1) hunk (header nav buttons).
    NavHunk(i32),
    /// Stage or unstage visual hunk `n` (gutter click) — direction depends on
    /// its current `hunk_staged` state — then recompute the diff.
    ToggleHunk(usize),
    /// Jump the scroll to a track fraction (scrollbar-thumb drag).
    ScrollToFrac(f32),
    /// Dismiss the stage-failure banner.
    DismissError,
}

impl WarpDiffView {
    /// Diff `path` (workspace-relative OR absolute) as HEAD vs the working copy.
    ///
    /// `repo_root` is the worktree dir used to (a) resolve a relative `path` to
    /// disk and (b) shell out `git show HEAD:<relpath>` (Crane's git-binary
    /// rule; never libgit2).
    pub fn new(ctx: &mut ViewContext<Self>, repo_root: Option<PathBuf>, path: PathBuf) -> Self {
        let font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            crate::warpui::bundled_fonts::mono(cache)
        });
        // The shell registers "phosphor" at startup; re-use it if present so we
        // don't grow the font cache per diff pane.
        let icon_font = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .family_id_for_name("phosphor")
                .map(Ok)
                .unwrap_or_else(|| {
                    cache.load_family_from_bytes(
                        "phosphor",
                        vec![include_bytes!("assets/Phosphor.ttf").to_vec()],
                    )
                })
                .expect("load phosphor")
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

        let mut this = Self {
            font,
            icon_font,
            title,
            left_path: format!("HEAD:{rel}"),
            right_path: rel.clone(),
            repo_root,
            rel,
            abs,
            computed: None,
            loading: false,
            error: None,
            scroll: Rc::new(Cell::new(0.0)),
            hover_hunk: Rc::new(Cell::new(None)),
            viewport_rows: Rc::new(Cell::new(0.0)),
            minimap_drag: Rc::new(Cell::new(false)),
            scrollbar_drag: Rc::new(Cell::new(false)),
            nav_up: MouseStateHandle::default(),
            nav_down: MouseStateHandle::default(),
            err_x: MouseStateHandle::default(),
        };
        this.spawn_compute(ctx);
        this
    }

    /// The diffed file's name (for the shell-drawn pane header).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Kick a fresh off-thread compute. The previous `computed` keeps
    /// rendering while the job runs — no flash-to-spinner after a hunk stage.
    fn spawn_compute(&mut self, ctx: &mut ViewContext<Self>) {
        self.loading = true;
        let repo_root = self.repo_root.clone();
        let rel = self.rel.clone();
        let abs = self.abs.clone();
        let fut = async move { compute(repo_root, rel, abs) };
        ctx.spawn(fut, |this, computed, vctx| {
            this.computed = Some(Rc::new(computed));
            this.loading = false;
            vctx.notify();
        });
    }

    /// The hunk the viewport currently sits in — derived from the scroll
    /// offset exactly like the old egui view (probe = top row + 2, last hunk
    /// start at or above it). `None` above the first hunk / when empty.
    fn derived_hunk_idx(&self, c: &DiffComputed) -> Option<usize> {
        if c.hunk_starts.is_empty() {
            return None;
        }
        let probe = (self.scroll.get().round().max(0.0) as usize).saturating_add(2);
        c.hunk_starts.iter().rposition(|&s| s <= probe)
    }

    /// Fixed-width horizontal gap.
    fn spacer(w: f32) -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().finish())
            .with_width(w)
            .with_height(1.0)
            .finish()
    }

    /// Full-width 1px divider under the header (old `ui.separator()`).
    fn divider() -> Box<dyn Element> {
        ConstrainedBox::new(Rect::new().with_background_color(theme::divider()).finish())
            .with_height(1.0)
            .finish()
    }

    /// Fill the pane background (theme::bg) behind the content.
    fn panel(&self, content: Box<dyn Element>) -> Box<dyn Element> {
        Stack::new()
            .with_child(Rect::new().with_background_color(theme::bg()).finish())
            .with_child(content)
            .finish()
    }

    fn mono(&self, s: impl Into<String>, size: f32, color: ColorU) -> Box<dyn Element> {
        Text::new(s.into(), self.font, size).with_color(color).finish()
    }

    /// One hunk-nav icon button. Disabled (no hunks) renders muted with no
    /// click handler — same as the old `add_enabled(...)`.
    fn nav_button(
        &self,
        state: MouseStateHandle,
        glyph: &'static str,
        enabled: bool,
        action: WarpDiffAction,
    ) -> Box<dyn Element> {
        let icon_font = self.icon_font;
        let fg = if enabled { theme::text() } else { theme::text_muted() };
        let hoverable = Hoverable::new(state, move |ms| {
            let bg = if enabled && ms.is_hovered() {
                theme::row_hover()
            } else {
                tint(theme::bg(), 0)
            };
            Container::new(
                Text::new(glyph.to_string(), icon_font, 12.0).with_color(fg).finish(),
            )
            .with_background_color(bg)
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
        });
        if enabled {
            hoverable
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(action.clone());
                })
                .finish()
        } else {
            hoverable.finish()
        }
    }

    /// In-body header: rename-aware path label on the left (`old -> new`
    /// colored del/add when the sides differ, bare filename in the add color
    /// when they match — 1:1 old header), hunk counter + prev/next on the
    /// right.
    fn header_row(&self, c: Option<&DiffComputed>) -> Box<dyn Element> {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        row = row.with_child(Self::spacer(6.0));

        let left_bare = self
            .left_path
            .strip_prefix("staged:")
            .or_else(|| self.left_path.strip_prefix("HEAD:"))
            .unwrap_or(&self.left_path);
        let right_bare = self
            .right_path
            .strip_prefix("staged:")
            .or_else(|| self.right_path.strip_prefix("HEAD:"))
            .unwrap_or(&self.right_path);
        if left_bare == right_bare {
            let display = Path::new(left_bare)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(left_bare);
            row = row.with_child(self.mono(display, 11.0, theme::success()));
        } else {
            row = row
                .with_child(self.mono(self.left_path.clone(), 11.0, theme::error()))
                .with_child(self.mono(" -> ", 11.0, theme::text_muted()))
                .with_child(self.mono(self.right_path.clone(), 11.0, theme::success()));
        }

        row = row.with_child(Expanded::new(1.0, Self::spacer(1.0)).finish());

        let hunk_count = c.map(|c| c.hunk_starts.len()).unwrap_or(0);
        if hunk_count > 0 {
            let label = match c.and_then(|c| self.derived_hunk_idx(c)) {
                Some(n) => format!("{} / {}", n + 1, hunk_count),
                None => format!("- / {}", hunk_count),
            };
            row = row
                .with_child(self.mono(label, 11.0, theme::text_muted()))
                .with_child(Self::spacer(6.0));
        }
        row = row
            .with_child(self.nav_button(
                self.nav_up.clone(),
                icons::ARROW_UP,
                hunk_count > 0,
                WarpDiffAction::NavHunk(-1),
            ))
            .with_child(Self::spacer(2.0))
            .with_child(self.nav_button(
                self.nav_down.clone(),
                icons::ARROW_DOWN,
                hunk_count > 0,
                WarpDiffAction::NavHunk(1),
            ))
            .with_child(Self::spacer(8.0));

        Container::new(row.finish())
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish()
    }

    /// Dismissible banner for stage/unstage failures (old egui `tab.error`
    /// surface — a failed `git apply` must not look like "click did nothing").
    fn error_banner(&self, err: &str) -> Box<dyn Element> {
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Self::spacer(8.0))
            .with_child(
                Text::new(icons::WARNING.to_string(), self.icon_font, 12.0)
                    .with_color(theme::error())
                    .finish(),
            )
            .with_child(Self::spacer(6.0))
            .with_child(self.mono(err.to_string(), 11.0, theme::error()))
            .with_child(Expanded::new(1.0, Self::spacer(1.0)).finish())
            .with_child(
                Hoverable::new(self.err_x.clone(), {
                    let icon_font = self.icon_font;
                    move |ms| {
                        let fg = if ms.is_hovered() { theme::text() } else { theme::text_muted() };
                        Container::new(
                            Text::new(icons::X.to_string(), icon_font, 11.0)
                                .with_color(fg)
                                .finish(),
                        )
                        .with_padding_left(6.0)
                        .with_padding_right(8.0)
                        .with_padding_top(3.0)
                        .with_padding_bottom(3.0)
                        .finish()
                    }
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(|ctx, _app, _pos| {
                    ctx.dispatch_typed_action(WarpDiffAction::DismissError);
                })
                .finish(),
            )
            .finish();
        Container::new(row)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
    }

    /// A muted single-line placeholder row (loading / no differences / binary
    /// guard / load error).
    fn placeholder(&self, msg: &str, color: ColorU) -> Box<dyn Element> {
        Container::new(self.mono(msg.to_string(), 12.0, color))
            .with_padding_left(10.0)
            .with_padding_top(8.0)
            .finish()
    }
}

impl Entity for WarpDiffView {
    type Event = ();
}

impl TypedActionView for WarpDiffView {
    type Action = WarpDiffAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WarpDiffAction::NavHunk(delta) => {
                if let Some(c) = self.computed.clone() {
                    if c.hunk_starts.is_empty() {
                        return;
                    }
                    let cur = self.derived_hunk_idx(&c);
                    let last = c.hunk_starts.len() - 1;
                    let target = if *delta > 0 {
                        match cur {
                            None => 0,
                            Some(n) => (n + 1).min(last),
                        }
                    } else {
                        match cur {
                            None => 0,
                            Some(n) => n.saturating_sub(1),
                        }
                    };
                    // Land the hunk two rows below the top edge (old jump_y).
                    let row = c.hunk_starts[target];
                    self.scroll.set((row as f32 - 2.0).max(0.0));
                }
            }
            WarpDiffAction::ToggleHunk(hi) => {
                let Some(c) = self.computed.clone() else { return };
                let Some(root) = self.repo_root.clone() else { return };
                let Some(Some(patch)) = c.hunk_patches.get(*hi).cloned() else { return };
                let is_unstage = c.hunk_staged.get(*hi).copied().unwrap_or(false);
                // Sync like the old egui view — `git apply --cached` on one
                // hunk is fast; the recompute (the slow part) is off-thread.
                let res = if is_unstage {
                    crate::git::unstage_hunk(&root, &patch)
                } else {
                    crate::git::stage_hunk(&root, &patch)
                };
                match res {
                    Ok(()) => {
                        self.error = None;
                        self.spawn_compute(ctx);
                    }
                    Err(e) => {
                        let verb = if is_unstage { "Unstage" } else { "Stage" };
                        self.error = Some(format!("{verb} hunk failed: {e}"));
                    }
                }
            }
            WarpDiffAction::ScrollToFrac(frac) => {
                if let Some(c) = &self.computed {
                    let max = (c.rows.len() as f32 - self.viewport_rows.get()).max(0.0);
                    self.scroll.set((frac * max).clamp(0.0, max));
                }
            }
            WarpDiffAction::DismissError => {
                self.error = None;
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
        let computed = self.computed.clone();

        let mut col = Flex::column();
        col = col.with_child(self.header_row(computed.as_deref()));
        col = col.with_child(Self::divider());
        if let Some(err) = &self.error {
            col = col.with_child(self.error_banner(err));
        }

        let body: Box<dyn Element> = match &computed {
            None => self.placeholder(
                if self.loading { "Computing diff…" } else { "No differences" },
                theme::text_muted(),
            ),
            Some(c) => {
                if let Some(err) = &c.error {
                    // Load failure — surface it instead of an empty diff.
                    self.placeholder(err, theme::error())
                } else if let Some(msg) = &c.binary {
                    self.placeholder(msg, theme::text_muted())
                } else if c.rows.is_empty() {
                    self.placeholder(
                        if self.loading { "Computing diff…" } else { "No differences" },
                        theme::text_muted(),
                    )
                } else {
                    let diff_body = DiffBodyElement::new(
                        c.clone(),
                        self.font,
                        self.icon_font,
                        self.scroll.clone(),
                        self.hover_hunk.clone(),
                        self.viewport_rows.clone(),
                        self.minimap_drag.clone(),
                    )
                    .finish();
                    let on_drag: Rc<dyn Fn(&mut EventContext, f32)> = Rc::new(|ctx, frac| {
                        ctx.dispatch_typed_action(WarpDiffAction::ScrollToFrac(frac));
                    });
                    let scrollbar = LineScrollbar::new(
                        c.rows.len(),
                        self.viewport_rows.get().max(1.0) as usize,
                        self.scroll.get().max(0.0) as usize,
                        theme::border(),
                    )
                    .draggable_with_ctx(self.scrollbar_drag.clone(), on_drag)
                    .finish();
                    Flex::row()
                        .with_child(Expanded::new(1.0, diff_body).finish())
                        .with_child(scrollbar)
                        .finish()
                }
            }
        };
        col = col.with_child(Expanded::new(1.0, body).finish());
        self.panel(col.finish())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Body element — windowed row painter + hunk gutter + minimap + wheel scroll
// ─────────────────────────────────────────────────────────────────────────────

struct DiffBodyElement {
    computed: Rc<DiffComputed>,
    font: FamilyId,
    icon_font: FamilyId,
    font_size: f32,
    /// Fractional scroll offset in ROWS (shared, persisted by the view).
    scroll: Rc<Cell<f32>>,
    /// Hovered hunk index (shared so the highlight survives rebuilds).
    hover_hunk: Rc<Cell<Option<usize>>>,
    /// Written each layout: viewport height in rows (view reads for scrollbar).
    viewport_rows: Rc<Cell<f32>>,
    /// Minimap drag state (persisted by the view).
    minimap_drag: Rc<Cell<bool>>,

    // Layout/paint scratch.
    size: Option<Vector2F>,
    origin: Option<Point>,
    origin_vec: Option<Vector2F>,
    row_h: f32,
    cell_w: f32,
}

impl DiffBodyElement {
    fn new(
        computed: Rc<DiffComputed>,
        font: FamilyId,
        icon_font: FamilyId,
        scroll: Rc<Cell<f32>>,
        hover_hunk: Rc<Cell<Option<usize>>>,
        viewport_rows: Rc<Cell<f32>>,
        minimap_drag: Rc<Cell<bool>>,
    ) -> Self {
        Self {
            computed,
            font,
            icon_font,
            font_size: FONT_SIZE,
            scroll,
            hover_hunk,
            viewport_rows,
            minimap_drag,
            size: None,
            origin: None,
            origin_vec: None,
            row_h: FONT_SIZE * 1.35,
            cell_w: FONT_SIZE * 0.6,
        }
    }

    fn total_rows(&self) -> usize {
        self.computed.rows.len()
    }

    /// Max scroll offset in rows (content minus viewport, floored at 0).
    fn max_scroll(&self) -> f32 {
        let view_rows = self.size.map(|s| s.y() / self.row_h).unwrap_or(0.0);
        (self.total_rows() as f32 - view_rows).max(0.0)
    }

    /// Clamp the shared scroll offset to `[0, max_scroll]`.
    fn clamp_scroll(&self) -> f32 {
        let clamped = self.scroll.get().clamp(0.0, self.max_scroll());
        self.scroll.set(clamped);
        clamped
    }

    /// Row index under window point `p` given the current scroll.
    fn row_at(&self, o: Vector2F, p: &Vector2F) -> Option<usize> {
        let rel_y = p.y() - o.y();
        if rel_y < 0.0 {
            return None;
        }
        let idx = (self.scroll.get() + rel_y / self.row_h).floor() as usize;
        if idx < self.total_rows() { Some(idx) } else { None }
    }

    /// The stage-button hunk under `p`, if `p` sits in the stage gutter on a
    /// hunk-start row that actually has a patch to (un)stage.
    fn hunk_button_at(&self, o: Vector2F, p: &Vector2F) -> Option<usize> {
        if p.x() < o.x() || p.x() > o.x() + STAGE_W {
            return None;
        }
        let i = self.row_at(o, p)?;
        let hi = self.computed.row_to_hunk.get(i).copied().flatten()?;
        if self.computed.hunk_starts.get(hi) == Some(&i)
            && self
                .computed
                .hunk_patches
                .get(hi)
                .map(|p| p.is_some())
                .unwrap_or(false)
        {
            Some(hi)
        } else {
            None
        }
    }

    /// True when `p` is inside the minimap strip.
    fn in_minimap(&self, o: Vector2F, s: Vector2F, p: &Vector2F) -> bool {
        p.x() >= o.x() + s.x() - MINIMAP_W
            && p.x() <= o.x() + s.x()
            && p.y() >= o.y()
            && p.y() <= o.y() + s.y()
    }

    /// Jump the scroll so the click fraction maps across the whole file
    /// (old egui minimap: frac × (total − viewport)).
    fn minimap_jump(&self, o: Vector2F, s: Vector2F, p: &Vector2F) {
        let frac = ((p.y() - o.y()) / s.y().max(1.0)).clamp(0.0, 1.0);
        self.scroll.set(frac * self.max_scroll());
    }
}

impl Element for DiffBodyElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let fc = app.font_cache();
        let font = fc.select_font(self.font, Properties::default());
        self.cell_w = fc
            .glyph_for_char(font, '0', false)
            .and_then(|(gid, mfont)| fc.glyph_advance(mfont, self.font_size, gid).ok())
            .map(|a| a.x())
            .unwrap_or(self.font_size * 0.6);
        self.row_h = fc.line_height(self.font_size, 1.35);
        let size = constraint.apply(constraint.max);
        self.size = Some(size);
        self.viewport_rows.set(size.y() / self.row_h.max(1.0));
        size
    }

    fn after_layout(&mut self, _: &mut AfterLayoutContext, _: &AppContext) {}

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.origin_vec = Some(origin);
        let size = self.size.unwrap_or_else(|| vec2f(0.0, 0.0));

        // Background fill + the single hit rect covering the whole body.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(Fill::Solid(theme::bg()));

        let c = &self.computed;
        let total = c.rows.len();
        if total == 0 {
            return;
        }

        let fc = app.font_cache();
        let font = fc.select_font(self.font, Properties::default());
        let icon_font = fc.select_font(self.icon_font, Properties::default());
        let ascent = fc.ascent(font, self.font_size);
        let descent = fc.descent(font, self.font_size);
        let text_h = ascent - descent;
        let baseline = ((self.row_h - text_h) * 0.5) + ascent;

        let scroll = self.clamp_scroll();
        let first = scroll.floor() as usize;
        let y_off = -(scroll.fract()) * self.row_h;
        let visible = (size.y() / self.row_h).ceil() as usize + 1;

        // Column geometry (window coords).
        let x0 = origin.x();
        let gutter_old_x = x0 + STAGE_W;
        let gutter_old_w = self.cell_w * c.ldigits as f32 + 10.0;
        let gutter_new_x = gutter_old_x + gutter_old_w;
        let gutter_new_w = self.cell_w * c.rdigits as f32 + 10.0;
        let sign_x = gutter_new_x + gutter_new_w;
        let sign_w = self.cell_w * 2.0 + 8.0;
        let text_x = sign_x + sign_w;
        let minimap_x = x0 + size.x() - MINIMAP_W;

        let text_col = theme::text();
        let text_muted = theme::text_muted();
        let success = theme::success();
        let error = theme::error();
        let add_bg = tint(success, 40);
        let del_bg = tint(error, 40);
        let connector = tint(success, 90);
        let hover_hunk = self.hover_hunk.get();

        // Monospace text drawer: fixed advance, truncates at `max_x`.
        let draw_text = |ctx: &mut PaintContext,
                         x: f32,
                         y_baseline: f32,
                         s: &str,
                         color: ColorU,
                         max_x: f32| {
            let mut cx = x;
            for ch in s.chars() {
                if cx + self.cell_w > max_x {
                    break;
                }
                if ch != ' ' {
                    if let Some((gid, rf)) = fc.glyph_for_char(font, ch, true) {
                        ctx.scene
                            .draw_glyph(vec2f(cx, y_baseline), gid, rf, self.font_size, color);
                    }
                }
                cx += self.cell_w;
            }
        };

        // Centered phosphor glyph (stage-gutter affordance).
        let draw_icon = |ctx: &mut PaintContext,
                         center: Vector2F,
                         glyph: &str,
                         glyph_size: f32,
                         color: ColorU| {
            let Some(ch) = glyph.chars().next() else { return };
            if let Some((gid, rf)) = fc.glyph_for_char(icon_font, ch, true) {
                let adv = fc
                    .glyph_advance(rf, glyph_size, gid)
                    .map(|a| a.x())
                    .unwrap_or(glyph_size * 0.6);
                let ia = fc.ascent(icon_font, glyph_size);
                let id = fc.descent(icon_font, glyph_size);
                let ih = ia - id;
                ctx.scene.draw_glyph(
                    vec2f(center.x() - adv * 0.5, center.y() - ih * 0.5 + ia),
                    gid,
                    rf,
                    glyph_size,
                    color,
                );
            }
        };

        for vi in 0..visible {
            let i = first + vi;
            if i >= total {
                break;
            }
            let row_top = origin.y() + y_off + vi as f32 * self.row_h;
            if row_top >= origin.y() + size.y() {
                break;
            }
            let r = &c.rows[i];
            let base_y = row_top + baseline;

            // Row tint under everything, stage gutter + minimap excluded
            // (1:1 old egui: the button gutter stays bg-colored).
            let bg = match r.tag {
                ChangeTag::Insert => Some(add_bg),
                ChangeTag::Delete => Some(del_bg),
                ChangeTag::Equal => None,
            };
            if let Some(bg) = bg {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(gutter_old_x, row_top),
                        vec2f((minimap_x - gutter_old_x).max(0.0), self.row_h),
                    ))
                    .with_background(Fill::Solid(bg));
            }

            // Stage affordance at hunk starts that carry a patch.
            let is_anchor = c
                .row_to_hunk
                .get(i)
                .copied()
                .flatten()
                .filter(|&hi| c.hunk_starts.get(hi) == Some(&i))
                .filter(|&hi| c.hunk_patches.get(hi).map(|p| p.is_some()).unwrap_or(false));

            // Connector line through downstream rows of a multi-visual-hunk
            // git group (and from an anchor's center down when the group
            // continues on the next row).
            let in_group = c.row_in_shared_group.get(i).copied().unwrap_or(false);
            let next_in_group = c.row_in_shared_group.get(i + 1).copied().unwrap_or(false);
            if in_group || (is_anchor.is_some() && next_in_group) {
                let cx = x0 + STAGE_W * 0.5;
                let top_y = if in_group { row_top } else { row_top + self.row_h * 0.5 };
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(cx - 0.75, top_y),
                        vec2f(1.5, row_top + self.row_h - top_y),
                    ))
                    .with_background(Fill::Solid(connector));
            }

            if let Some(hi) = is_anchor {
                let is_staged = c.hunk_staged.get(hi).copied().unwrap_or(false);
                let hovered = hover_hunk == Some(hi);
                let center = vec2f(x0 + STAGE_W * 0.5, row_top + self.row_h * 0.5);
                if hovered {
                    // Subtle round hover disc behind the glyph.
                    let r_disc = self.row_h * 0.42;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(
                            vec2f(center.x() - r_disc, center.y() - r_disc),
                            vec2f(r_disc * 2.0, r_disc * 2.0),
                        ))
                        .with_corner_radius(warpui::elements::CornerRadius::with_all(
                            warpui::elements::Radius::Pixels(r_disc),
                        ))
                        .with_background(Fill::Solid(add_bg));
                }
                // Same three-state affordance as old Crane: empty CIRCLE muted
                // when unstaged, CHECK_CIRCLE green when staged or hovered.
                let glyph = if is_staged || hovered { CHECK_CIRCLE } else { icons::CIRCLE };
                let glyph_color = if is_staged || hovered { success } else { text_muted };
                let glyph_size = if is_staged && hovered { 18.0 } else { 16.0 };
                draw_icon(ctx, center, glyph, glyph_size, glyph_color);
            }

            // Dual line-number gutters (strings pre-padded to fixed width).
            draw_text(ctx, gutter_old_x + 4.0, base_y, &r.old_ln, text_muted, gutter_new_x);
            draw_text(ctx, gutter_new_x + 4.0, base_y, &r.new_ln, text_muted, sign_x);

            // +/- sign, centered in its column.
            let (sign, sign_fg) = match r.tag {
                ChangeTag::Insert => ("+", success),
                ChangeTag::Delete => ("-", error),
                ChangeTag::Equal => (" ", text_muted),
            };
            draw_text(ctx, sign_x + (sign_w - self.cell_w) * 0.5, base_y, sign, sign_fg, text_x);

            // Syntax-colored content spans (truncated at the minimap edge).
            let mut cx = text_x;
            'spans: for (color, text) in &r.spans {
                let fg = if color.a == 0 { text_col } else { *color };
                for ch in text.chars() {
                    if cx + self.cell_w > minimap_x - 2.0 {
                        break 'spans;
                    }
                    if ch != ' ' {
                        if let Some((gid, rf)) = fc.glyph_for_char(font, ch, true) {
                            ctx.scene
                                .draw_glyph(vec2f(cx, base_y), gid, rf, self.font_size, fg);
                        }
                    }
                    cx += self.cell_w;
                }
            }
        }

        // Minimap: add/del markers scaled to the whole file (old egui 748-790).
        if total > 1 {
            let track_h = size.y();
            if track_h > 0.0 {
                let marker_h = (track_h / total as f32).max(2.0);
                for (i, r) in c.rows.iter().enumerate() {
                    let color = match r.tag {
                        ChangeTag::Insert => success,
                        ChangeTag::Delete => error,
                        ChangeTag::Equal => continue,
                    };
                    let y = origin.y() + i as f32 * track_h / total as f32;
                    ctx.scene
                        .draw_rect_without_hit_recording(RectF::new(
                            vec2f(minimap_x + 1.0, y),
                            vec2f(MINIMAP_W - 2.0, marker_h),
                        ))
                        .with_background(Fill::Solid(color));
                }
            }
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _app: &AppContext,
    ) -> bool {
        let (Some(o), Some(s)) = (self.origin_vec, self.size) else {
            return false;
        };
        let in_bounds = |p: &Vector2F| -> bool {
            p.x() >= o.x() && p.x() <= o.x() + s.x() && p.y() >= o.y() && p.y() <= o.y() + s.y()
        };

        match event.raw_event() {
            // Bounds-gated so an open Diff pane can't eat wheel events meant
            // for other panes/panels (events dispatch tree-wide).
            Event::ScrollWheel { delta, precise, position, .. } if in_bounds(position) => {
                // Same feel as the Git Log list: precise (trackpad) deltas are
                // pixels → rows; line deltas map 1:1 to rows.
                let dy = delta.y();
                let delta_rows = if *precise { dy / self.row_h } else { dy };
                self.scroll.set((self.scroll.get() - delta_rows).max(0.0));
                let _ = self.clamp_scroll();
                ctx.notify();
                return true;
            }
            Event::MouseMoved { position, .. } if in_bounds(position) => {
                let next = self.hunk_button_at(o, position);
                if self.hover_hunk.get() != next {
                    self.hover_hunk.set(next);
                    ctx.notify();
                }
                if next.is_some() || self.in_minimap(o, s, position) {
                    if let Some(p) = self.origin {
                        ctx.set_cursor(Cursor::PointingHand, p.z_index());
                    }
                }
            }
            Event::MouseMoved { .. } => {
                if self.hover_hunk.get().is_some() {
                    self.hover_hunk.set(None);
                    ctx.notify();
                }
            }
            Event::LeftMouseDown { position, .. } if in_bounds(position) => {
                if self.in_minimap(o, s, position) {
                    self.minimap_drag.set(true);
                    self.minimap_jump(o, s, position);
                    ctx.notify();
                    return true;
                }
                if let Some(hi) = self.hunk_button_at(o, position) {
                    ctx.dispatch_typed_action(WarpDiffAction::ToggleHunk(hi));
                    return true;
                }
            }
            Event::LeftMouseDragged { position, .. } if self.minimap_drag.get() => {
                self.minimap_jump(o, s, position);
                ctx.notify();
                return true;
            }
            Event::LeftMouseUp { .. } => {
                let was = self.minimap_drag.get();
                self.minimap_drag.set(false);
                return was;
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── visual hunk detection ────────────────────────────────────────────

    #[test]
    fn hunk_starts_on_change_runs() {
        use ChangeTag::{Delete as D, Equal as E, Insert as I};
        let tags = [E, D, I, E, E, I, I, E];
        assert_eq!(visual_hunk_starts(&tags), vec![1, 5]);
    }

    #[test]
    fn hunk_starts_empty_for_all_equal() {
        let tags = [ChangeTag::Equal; 4];
        assert!(visual_hunk_starts(&tags).is_empty());
    }

    // ── visual-hunk → git-hunk matching ─────────────────────────────────

    /// Aligned case: each visual hunk lands inside a distinct git range.
    #[test]
    fn match_hunks_by_new_line_number() {
        // Rows: (old_lno, new_lno). Hunk 0 at row 0 (new line 3),
        // hunk 1 at row 2 (new line 10).
        let lnos = vec![(None, Some(3)), (Some(3), Some(4)), (None, Some(10))];
        let parsed = vec![(2, 1, 3, 1), (9, 0, 10, 1)];
        assert_eq!(match_hunks(&[0, 2], &lnos, &parsed), vec![Some(0), Some(1)]);
    }

    /// Two visual hunks inside ONE git hunk (git merged by context): the
    /// second dedupes to None so clicking either can't stage both twice.
    #[test]
    fn match_hunks_dedupes_shared_git_hunk() {
        let lnos = vec![(None, Some(5)), (Some(5), Some(6)), (None, Some(7))];
        let parsed = vec![(4, 2, 5, 3)];
        assert_eq!(match_hunks(&[0, 2], &lnos, &parsed), vec![Some(0), None]);
    }

    /// Pure-deletion hunk: first row has no new_lno — matching falls back to
    /// the old-side line number.
    #[test]
    fn match_hunks_deletion_falls_back_to_old_lno() {
        let lnos = vec![(Some(8), None), (Some(9), None)];
        let parsed = vec![(8, 2, 7, 0)];
        assert_eq!(match_hunks(&[0], &lnos, &parsed), vec![Some(0)]);
    }

    /// No git range covers the visual hunk → None (no stage affordance).
    #[test]
    fn match_hunks_unmatched_is_none() {
        let lnos = vec![(None, Some(100))];
        let parsed = vec![(1, 1, 1, 1)];
        assert_eq!(match_hunks(&[0], &lnos, &parsed), vec![None]);
    }

    // ── shared-group connector rows ──────────────────────────────────────

    #[test]
    fn shared_group_spans_anchor_to_last_change_row() {
        use ChangeTag::{Equal as E, Insert as I};
        // Rows: I I E I I E — two visual hunks (0, 3); the second deduped to
        // the first's git hunk (has_patch = [true, false]). Connector marks
        // rows 1..5 (through the Equal gap and the second hunk's change run,
        // but NOT the trailing Equal).
        let tags = [I, I, E, I, I, E];
        let flags = shared_group_flags(&[0, 3], &[true, false], &tags);
        assert_eq!(flags, vec![false, true, true, true, true, false]);
    }

    #[test]
    fn independent_hunks_get_no_connector() {
        use ChangeTag::{Equal as E, Insert as I};
        let tags = [I, E, I, E];
        let flags = shared_group_flags(&[0, 2], &[true, true], &tags);
        assert_eq!(flags, vec![false; 4]);
    }

    // ── binary / image detection ─────────────────────────────────────────

    #[test]
    fn nul_byte_is_binary() {
        assert!(looks_binary(b"MZ\x00\x01payload"));
    }

    #[test]
    fn invalid_utf8_is_binary() {
        assert!(looks_binary(&[0x66, 0x6f, 0xff, 0xfe]));
    }

    #[test]
    fn plain_text_is_not_binary() {
        assert!(!looks_binary("fn main() {}\n// ünïcödé ok\n".as_bytes()));
    }

    #[test]
    fn image_extensions_detected_case_insensitively() {
        assert!(is_image_path_str("assets/logo.PNG"));
        assert!(is_image_path_str("a/b/photo.jpeg"));
        assert!(!is_image_path_str("src/main.rs"));
        assert!(!is_image_path_str("Makefile"));
    }

    // ── row building (line numbers + gutter padding + spans) ────────────

    #[test]
    fn build_rows_pads_gutters_and_numbers_sides() {
        let (rows, ldigits, rdigits) = build_rows("a\nb\n", "a\nc\n", "txt");
        assert_eq!(ldigits, 3);
        assert_eq!(rdigits, 3);
        // Equal a, Delete b, Insert c.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].tag, ChangeTag::Equal);
        assert_eq!(rows[0].old_ln, "  1");
        assert_eq!(rows[0].new_ln, "  1");
        assert_eq!(rows[1].tag, ChangeTag::Delete);
        assert_eq!(rows[1].old_ln, "  2");
        assert_eq!(rows[1].new_ln, "   "); // no new-side number on a deletion
        assert_eq!(rows[1].old_lno, Some(2));
        assert_eq!(rows[1].new_lno, None);
        assert_eq!(rows[2].tag, ChangeTag::Insert);
        assert_eq!(rows[2].new_lno, Some(2));
        // Span text round-trips the content.
        let joined: String = rows[2].spans.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(joined, "c");
    }

    #[test]
    fn build_rows_expands_tabs_in_spans() {
        let (rows, _, _) = build_rows("", "\tx\n", "txt");
        let joined: String = rows[0].spans.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(joined, "    x");
    }
}
