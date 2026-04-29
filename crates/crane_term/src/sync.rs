//! `?2026` synchronized-output buffer state.
//!
//! When a TUI emits `\e[?2026h`, subsequent bytes are stashed into
//! `SyncBuffer::Active` until `\e[?2026l` arrives — at which point
//! the buffer is replayed through the parser as a single batch.
//! Two safety caps stop a stuck or runaway block from holding the
//! grid frozen forever:
//!
//! * **Time cap** (`MAX_SYNC_DURATION`, 150 ms): a sync block
//!   exceeding this gets force-flushed and the screen redraws.
//! * **Size cap** (`MAX_SYNC_BUFFER_BYTES`, 2 MiB): a buffer
//!   exceeding this gets force-flushed.
//!
//! Both caps mirror Warp's behavior — same trade-off (partial
//! state on the live grid is better than indefinite freeze).

use std::time::{Duration, Instant};

pub const MAX_SYNC_DURATION: Duration = Duration::from_millis(150);
pub const MAX_SYNC_BUFFER_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Default)]
pub enum SyncBuffer {
    #[default]
    Inactive,
    Active {
        buffer: Vec<u8>,
        last_activated: Instant,
    },
}

impl SyncBuffer {
    pub fn is_active(&self) -> bool {
        matches!(self, SyncBuffer::Active { .. })
    }

    /// Transition to `Active`, or extend the timeout if already
    /// active. Called when `?2026h` is parsed.
    pub fn activate(&mut self) {
        match self {
            SyncBuffer::Inactive => {
                *self = SyncBuffer::Active {
                    buffer: Vec::with_capacity(8 * 1024),
                    last_activated: Instant::now(),
                };
            }
            SyncBuffer::Active { last_activated, .. } => {
                *last_activated = Instant::now();
            }
        }
    }

    /// Take the buffered bytes and return to `Inactive`. Used at
    /// `?2026l` and when a safety cap trips.
    pub fn deactivate(&mut self) -> Option<Vec<u8>> {
        match std::mem::replace(self, SyncBuffer::Inactive) {
            SyncBuffer::Inactive => None,
            SyncBuffer::Active { buffer, .. } => Some(buffer),
        }
    }

    /// Append a byte and report whether a safety cap has tripped.
    pub fn push(&mut self, byte: u8) -> SyncPushOutcome {
        if let SyncBuffer::Active {
            buffer,
            last_activated,
        } = self
        {
            buffer.push(byte);
            if buffer.len() >= MAX_SYNC_BUFFER_BYTES {
                return SyncPushOutcome::SizeCapTripped;
            }
            if last_activated.elapsed() > MAX_SYNC_DURATION {
                return SyncPushOutcome::TimeCapTripped;
            }
            SyncPushOutcome::Buffered
        } else {
            SyncPushOutcome::NotActive
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SyncPushOutcome {
    NotActive,
    Buffered,
    SizeCapTripped,
    TimeCapTripped,
}
