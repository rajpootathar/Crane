//! Top-level driver: owns the `vte::Parser`, the [`SyncBuffer`],
//! and the byte-by-byte parse loop.
//!
//! v1 dispatches through the [`Bridge`] adapter directly without
//! sync buffering — the linefeed-routing fix in [`crate::term`] is
//! the primary test surface here. `?2026` buffer-and-replay is
//! wired up in the next iteration once the bridge surface covers
//! enough of the Handler trait to round-trip a captured TUI redraw
//! losslessly.

use crate::handler::{Handler, ProcessorInput};
use crate::perform::Bridge;
use crate::sync::SyncBuffer;
use vte::ansi::Processor as VteProcessor;

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

    /// Feed bytes through the parser into the handler. Sync-output
    /// buffering is intentionally not yet wired here — see module
    /// note. The linefeed-routing test suite drives `Term` via this
    /// path to confirm the ANSI parser → Handler bridge reaches
    /// the right methods.
    pub fn parse_bytes<H>(&mut self, handler: &mut H, bytes: &[u8])
    where
        H: Handler,
    {
        {
            let mut bridge = Bridge { inner: handler };
            self.parser.advance(&mut bridge, bytes);
        }
        handler.on_finish_byte_processing(&ProcessorInput {
            bytes,
            is_sync_frame: false,
        });
    }
}
