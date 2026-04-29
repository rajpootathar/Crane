# crane_term integration plan

**Status**: phase 1 complete (crane_term API ready); phase 2/3 pending.
**Branch**: `feat/crane-term`.

## Why

`alacritty_terminal` 0.25 routes `linefeed` to scroll-into-history
whenever the cursor advance hits the visible bottom, regardless of
whether the cursor is mid-redraw of a TUI region. Ink-based TUIs
(Claude Code, neovim cmdline, lazygit, etc.) use `cursor-up + LF` to
step back through their own UI region — those LFs land mid-region
and must NOT push to scrollback. They do, hence the duplicate-prompt
artifact tracked in `CLAUDE.md`.

Three prior wrappers tried to fix this and failed:
1. Strip `?2026` from the byte stream (`b2856e1`, reverted).
2. `SyncAwareHandler` LF→`move_down` heuristic (`660d1f5`, dead code in
   `src/terminal/sync_handler.rs:56` — *"clamped move_down(1) and
   produced garbled redraws at the bottom row"*).
3. `split_sync_markers` shadow-grid snapshot/restore (`45e512a`,
   currently active — works partially, doesn't catch all cases).

A wrapper around alacritty's `Term` cannot fully fix it: alacritty's
`Handler::linefeed` returns `()`, and its internal scroll path
unconditionally pushes evicted rows to the grid's history when on the
main screen. There's no API to bypass that.

The fix is in `crane_term`: `Term::linefeed` only calls
`scroll_up_one` when `cursor_at_scroll_bottom()` is true. Mid-region
LFs just bump the cursor row; nothing reaches scrollback. Tests in
`crates/crane_term/src/term.rs` (16 + 4 = 20 passing) verify this.

## Correction (after writing this spec)

**The original framing was incomplete.** Reading alacritty 0.25's
`Term::linefeed` (`/Users/.../alacritty_terminal-0.25.1/src/term/mod.rs:1423`)
revealed it has the same scroll-region-bottom check `crane_term`
started with:

```rust
if next == self.scroll_region.end {
    self.scroll_up(1);  // pushes to history
}
```

So the bug isn't "alacritty scrolls more aggressively than
crane_term." Both push to history when an LF lands the cursor at
the scroll-region bottom AND the region starts at row 0 (the
default).

**The actual fix** (committed as `6ac0942`): suppress scrollback
eviction *during a `?2026` sync replay*. While the
`crane_term::Processor` is replaying a buffered sync block, it
calls `Handler::set_sync_frame(true)` on the term; `Term::scroll_up_one`
checks that flag and drops the evicted row instead of preserving
it. After the replay, `set_sync_frame(false)` restores normal
scrollback behavior for streaming output.

A new test, `sync_block_landing_at_screen_bottom_does_not_evict`,
reproduces the exact real-world failure pattern (sync block whose
last LF lands at screen bottom). It failed before this fix and
passes after. 29/29 tests green.

The earlier `tui_redraw_does_not_pollute_scrollback` test passed
under a wrong premise — its setup never landed an LF at the scroll
region bottom, so neither alacritty nor crane_term would have
pushed under that condition.

## Phase 1 — `crane_term` API surface (complete)

Done in commits `8ce489e`, `6442df0`, `678366f`. Crate at
`crates/crane_term/` exposes:

- **Storage**: `Cell` (24 B fast path + boxed `CellExtra`), `Row`
  with `occ` upper bound, `Grid` with display offset, `Scrollback`
  capped FIFO, `TermMode` bitflag bag.
- **Handler trait**: scroll methods return `ScrollDelta`. Parser
  glue via `vte::ansi::Handler` bridge in `perform.rs`.
- **Term** implements Handler with: SGR, clear modes, scroll
  up/down, reverse index, insert/delete chars/lines, insert blank,
  line wrap (DECAWM), cursor save/restore, alt-screen 1049 swap,
  wide chars (CJK / emoji via `unicode-width`), zero-width
  combining-mark stacking on `CellExtra`, mode bits.
- **Processor**: byte-feed loop with `?2026h/l` buffer-and-replay,
  150 ms / 2 MiB safety caps from `sync.rs`.
- **Helpers**: `display_offset()`, `scroll_display(delta)`,
  `scroll_to_bottom()`, `snapshot_text()`,
  `is_alt_screen()`/`is_app_cursor()`/`is_bracketed_paste()`,
  `Grid::cell_at(row, col)`.
- **Test coverage**: 20/20 passing including
  `tui_redraw_does_not_pollute_scrollback` and
  `sync_block_replays_without_scrollback_growth`.

## Phase 2 — replace alacritty in `src/terminal/term.rs`

Touchpoints (~672 lines today):

- `term: Arc<Mutex<alacritty::Term<WakeListener>>>` →
  `term: Arc<Mutex<crane_term::Term>>`. The Processor lives next to
  the term, since crane_term separates parser+state from the term
  itself: store a `parser: Arc<Mutex<crane_term::Processor>>` on
  the Terminal struct and pass both to the reader thread.
- `WakeListener` event listener — drop. crane_term doesn't have an
  EventListener; PTY reader thread:
  1. Calls `processor.lock().parse_bytes(&mut *term.lock(), &buf)`.
  2. After parsing, drains DSR/DA replies with
     `term.lock().take_pty_replies()`. Writes them back to
     `master.take_writer()`.
  3. Compares the term's `dirty_epoch` to the previous frame's
     value. If different, calls `ctx.request_repaint()`. This
     replaces the per-byte `request_repaint` storm that contributed
     to the 18-30% CPU finding.
- Reader-thread `split_sync_markers` snapshot/restore loop (lines
  415–520) — delete. crane_term's `Processor::parse_bytes` handles
  `?2026` internally, and `set_sync_frame` around the replay
  prevents scrollback duplication (verified by
  `sync_block_landing_at_screen_bottom_does_not_evict` test).
- `ghost_texts: Arc<Mutex<VecDeque<String>>>` field — delete (it
  exists only to dedup duplicates that won't happen anymore).
- `snapshot_text()` — replace alacritty grid iteration with
  `Term::snapshot_text()` (already implemented).
- `resize()` — replace `Term::resize(TermSize)` with
  `crane_term::Term::resize(rows, cols)`. Also resize the master
  PTY (already there) and the parser doesn't need resize info.
- Transcript replay (lines 365–383) — replace alacritty's Processor
  with crane_term's: `processor.parse_bytes(&mut term, text)` then
  the padding `\r\n.repeat(rows)` and `\x1b[H` work identically.
  After replay, `term.scroll_to_bottom()` instead of
  `Scroll::Bottom`.

**Sequence of edits to keep the build at red→green points**:

1. Switch the field types and constructor in one edit; this WILL
   break compilation everywhere `term.lock()` is called from
   view.rs. Treat that as expected — phase 3 fixes it.
2. Inside term.rs: rewrite the reader thread loop. Drop sync
   handler import.
3. Inside term.rs: rewrite `snapshot_text`, `resize`,
   `flush_scroll_to_bottom`, `flush_pty_replies` (becomes a
   no-op or a `take_pty_replies` drainer used in the reader).
4. Stop. Commit term.rs alone with a "WIP" note. View.rs will not
   build at this checkpoint.
5. In a separate commit, rewrite view.rs (phase 3).

**Missing API to add to `crane_term` first** (estimate: 200 LOC):

1. **PTY writer plumbing** — DSR / DA / title-ack reply path.
   Approach: `Term` accumulates outbound bytes in a `pty_replies:
   Vec<u8>` field; `Processor` exposes `take_pty_replies()` that the
   PTY reader drains and writes back to the master fd.

2. **`has_foreground_process()` equivalent** — currently checks
   `term.mode().contains(TermMode::ALT_SCREEN)` plus PID bookkeeping.
   The mode check is one line on `crane_term::Term` (already there
   via `is_alt_screen()`); PID lookup stays where it is.

3. **EOF / shell-exit detection** — `alive` flag survives the swap as
   the PTY reader thread sets it; nothing crane_term-specific.

## Phase 3 — replace alacritty in `src/terminal/view.rs`

Touchpoints (~1675 lines today). View.rs reaches into alacritty
internals heavily:

- `alacritty::index::{Point, Line, Column, Side}` — needs `crane_term`
  equivalents. Approach: simple structs in `crane_term::index` that
  match the call shape view.rs uses.
- `alacritty::selection::{Selection, SelectionType, SelectionRange}`
  — port to `crane_term::selection`. Simple/Block/Semantic/Lines
  variants. The range computation is straightforward grid-index
  arithmetic.
- `Term::renderable_content()` returns an iterator over `(Point,
  Cell)` for the visible viewport including scrollback rows when
  `display_offset > 0`. Add this as a method on
  `crane_term::Term`.
- `term.grid().display_offset()`, `term.history_size()`,
  `term.columns()`, `term.screen_lines()` — wrap as Term methods
  (already partial).
- `term.scroll_display(Scroll::Delta(n))` and `Scroll::Bottom` —
  already on `crane_term::Term::scroll_display(i32)`.
- `Color`, `NamedColor`, cell `Flags` — reference `crane_term`
  versions; the bit shapes match (already designed for this).

## Phase 4 — cleanup

- Delete `src/terminal/sync_handler.rs` (entire file, 352 lines).
- Delete `ghost_texts` field and the dedup pass in the renderer.
- Remove `alacritty_terminal` and the `xterm-crane` terminfo install
  (terminfo Sync flag stays useful — Ink TUIs key off it; keep the
  install code, drop the alacritty dep).
- Update `CLAUDE.md`: tech-stack line for terminal moves from
  `alacritty_terminal 0.25` to `crane_term + vte 0.15`.

## Verification

Smoke tests after phase 4:

1. `cargo test --workspace` — full suite, including crane_term unit
   tests.
2. Capture a real Claude Code splash via `CRANE_VT_TRACE=1` and
   replay through both old and new term paths (a tiny
   `examples/replay.rs` in `crane_term`). Diff scrollback growth —
   should be zero with crane_term, nonzero with alacritty.
3. Open Claude Code in a Terminal pane manually and exercise the
   redraw-heavy flows (auto mode toggle, status spinners, prompt
   submit). Visually confirm no duplicate splash.
4. Run vim, htop, less, lazygit; confirm no regressions.

## Risk

Largest risk is view.rs renderer regressions — that file is the
hot path for every terminal frame. The full-screen renderer is
sensitive to off-by-one row/col and selection range translation
between coordinate systems. Mitigation: write a small fixture-based
test in `crates/crane_term/tests/` that constructs a Term with
known content and asserts `cell_at` / `display_iter` outputs match a
golden snapshot, before changing view.rs.

## Estimated remaining work

- Phase 2 (term.rs + missing API): ~6–8 hours focused.
- Phase 3 (view.rs): ~6–10 hours focused.
- Phase 4 (cleanup): ~1 hour.

Total: roughly 1.5–2 focused days.
