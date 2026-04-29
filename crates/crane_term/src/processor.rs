//! Top-level driver: owns the `vte::Parser`, the [`SyncBuffer`],
//! and the byte-by-byte parse loop.
//!
//! The linefeed routing in [`crate::term`] is the primary fix for
//! the TUI scrollback bug. Sync buffering here is belt-and-braces:
//! when a TUI wraps a redraw in `?2026h ... ?2026l`, we stash the
//! bytes and replay them as one batch at the end. Any safety cap
//! trip (150 ms / 2 MiB) force-flushes so the screen can't get
//! stuck.

use crate::handler::{Handler, ProcessorInput};
use crate::perform::Bridge;
use crate::sync::{SyncBuffer, SyncPushOutcome};
use vte::ansi::Processor as VteProcessor;

const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
const SYNC_END: &[u8] = b"\x1b[?2026l";

pub struct Processor {
    parser: VteProcessor,
    sync: SyncBuffer,
}

impl Default for Processor {
    fn default() -> Self {
        Self::new()
    }
}

impl Processor {
    pub fn new() -> Self {
        Self {
            parser: VteProcessor::new(),
            sync: SyncBuffer::default(),
        }
    }

    pub fn sync_active(&self) -> bool {
        self.sync.is_active()
    }

    /// Feed bytes through the parser into the handler.
    ///
    /// Sync handling: bytes that arrive while a `?2026h` block is
    /// active accumulate in [`SyncBuffer`] without touching the
    /// parser. We watch the tail for `?2026h` (extends timeout) or
    /// `?2026l` (replay + deactivate). If a safety cap trips, we
    /// force-flush and the parser sees the partial buffer.
    pub fn parse_bytes<H>(&mut self, handler: &mut H, bytes: &[u8])
    where
        H: Handler,
    {
        let mut i = 0;
        while i < bytes.len() {
            if self.sync.is_active() {
                let outcome = self.sync.push(bytes[i]);
                i += 1;
                match outcome {
                    SyncPushOutcome::Buffered => {
                        if self.tail_matches(SYNC_BEGIN) {
                            self.trim_tail(SYNC_BEGIN.len());
                            self.sync.activate();
                        } else if self.tail_matches(SYNC_END) {
                            self.trim_tail(SYNC_END.len());
                            self.flush_sync(handler);
                        }
                    }
                    SyncPushOutcome::SizeCapTripped | SyncPushOutcome::TimeCapTripped => {
                        self.flush_sync(handler);
                    }
                    SyncPushOutcome::NotActive => unreachable!("guarded by is_active()"),
                }
                continue;
            }

            // Sync inactive — scan ahead for either an in-stream
            // `?2026h` or end-of-buffer, then push the run through
            // the parser in one batch (cheap path) and flip into
            // sync mode if needed.
            let stretch_end = match find_subsequence(&bytes[i..], SYNC_BEGIN) {
                Some(off) => i + off,
                None => bytes.len(),
            };
            if stretch_end > i {
                self.feed_parser(handler, &bytes[i..stretch_end]);
            }
            if stretch_end < bytes.len() {
                // Skip past `?2026h` and enter sync mode. Don't feed
                // it to the parser; we own this state machine now.
                self.sync.activate();
                i = stretch_end + SYNC_BEGIN.len();
            } else {
                i = stretch_end;
            }
        }

        handler.on_finish_byte_processing(&ProcessorInput {
            bytes,
            is_sync_frame: false,
        });
    }

    fn feed_parser<H>(&mut self, handler: &mut H, chunk: &[u8])
    where
        H: Handler,
    {
        let mut bridge = Bridge { inner: handler };
        self.parser.advance(&mut bridge, chunk);
    }

    /// Replay buffered bytes through the parser as a single sync
    /// frame, then mark the boundary so the renderer can elide
    /// per-byte repaints during the flush.
    ///
    /// `set_sync_frame(true)` is called on the handler around the
    /// replay so it can suppress scrollback eviction for rows that
    /// fall off the top during the redraw — those rows are
    /// intermediate state of an atomic frame, not real history.
    /// Without this guard, every Ink-style redraw whose last LF
    /// lands at screen bottom pushes one duplicate row into
    /// scrollback (the "duplicate splash" artifact).
    fn flush_sync<H>(&mut self, handler: &mut H)
    where
        H: Handler,
    {
        let buffered = match self.sync.deactivate() {
            Some(b) => b,
            None => return,
        };
        handler.set_sync_frame(true);
        self.feed_parser(handler, &buffered);
        handler.set_sync_frame(false);
        handler.on_finish_byte_processing(&ProcessorInput {
            bytes: &buffered,
            is_sync_frame: true,
        });
    }

    /// True when the last `n` bytes of the sync buffer equal
    /// `needle`. Used to spot `?2026h/l` markers without re-
    /// scanning the whole buffer per byte.
    fn tail_matches(&self, needle: &[u8]) -> bool {
        if let SyncBuffer::Active { buffer, .. } = &self.sync {
            buffer.len() >= needle.len()
                && &buffer[buffer.len() - needle.len()..] == needle
        } else {
            false
        }
    }

    fn trim_tail(&mut self, n: usize) {
        if let SyncBuffer::Active { buffer, .. } = &mut self.sync {
            let new_len = buffer.len().saturating_sub(n);
            buffer.truncate(new_len);
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Term;

    /// A `?2026h ... ?2026l` block wrapped around a TUI redraw
    /// (cursor-up + LFs) buffers the bytes and replays them as
    /// one frame. Net scrollback growth: zero.
    #[test]
    fn sync_block_replays_without_scrollback_growth() {
        let mut term = Term::new(10, 20);
        let mut proc_ = Processor::new();
        // Pre-fill the screen so the cursor sits on row 9 (last
        // visible row) at the start of the redraw.
        for _ in 0..9 {
            proc_.parse_bytes(&mut term, b"hello\r\n");
        }
        // Now the redraw block: enter sync, cursor up 5, write
        // five rows stepping back down via LF, exit sync.
        let before = term.scrollback.len();
        let redraw = b"\x1b[?2026h\x1b[5Aredraw1\nredraw2\nredraw3\nredraw4\nredraw5\x1b[?2026l";
        proc_.parse_bytes(&mut term, redraw);
        // The five LFs landed mid-region (cursor was at row 4
        // after the up-5), so no scrollback eviction.
        assert_eq!(term.scrollback.len(), before);
    }

    /// THE actual bug we're fixing: an Ink-style redraw block whose
    /// last LF lands the cursor at the screen bottom row. In a
    /// terminal without sync-frame suppression (alacritty 0.25 OR
    /// the pre-fix crane_term), this pushes the top row into
    /// scrollback for every redraw — that's the duplicate-prompt
    /// artifact in CLAUDE.md.
    #[test]
    fn sync_block_landing_at_screen_bottom_does_not_evict() {
        let mut term = Term::new(5, 10);
        let mut proc_ = Processor::new();
        // Fill the screen first.
        proc_.parse_bytes(&mut term, b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
        let before = term.scrollback.len();

        // Now wrap a redraw in ?2026: cursor up 5 to the top, then
        // emit 5 lines of content with LFs between. The last LF
        // lands the cursor at the bottom row — exactly the case
        // that triggers scrollback eviction.
        let redraw = b"\x1b[?2026h\x1b[5A\
            R0\nR1\nR2\nR3\nR4\
            \x1b[?2026l";
        proc_.parse_bytes(&mut term, redraw);

        assert_eq!(
            term.scrollback.len(),
            before,
            "sync replay must not promote rows into scrollback"
        );
    }

    /// Plain non-sync streaming input still scrolls normally
    /// when the cursor reaches the bottom.
    #[test]
    fn streaming_input_scrolls_normally() {
        let mut term = Term::new(5, 10);
        let mut proc_ = Processor::new();
        for _ in 0..10 {
            proc_.parse_bytes(&mut term, b"abc\r\n");
        }
        // 10 LFs from rows 0..4, only the ones at the scroll
        // bottom evict — exactly 6 (10 LFs - 4 to reach bottom).
        assert!(term.scrollback.len() >= 5);
    }

    /// Sync state survives across split parse calls — half the
    /// block in one call, half in the next.
    #[test]
    fn sync_state_persists_across_parse_calls() {
        let mut term = Term::new(5, 20);
        let mut proc_ = Processor::new();
        proc_.parse_bytes(&mut term, b"\x1b[?2026hpartial");
        assert!(proc_.sync_active());
        proc_.parse_bytes(&mut term, b" more\x1b[?2026l");
        assert!(!proc_.sync_active());
    }
}
