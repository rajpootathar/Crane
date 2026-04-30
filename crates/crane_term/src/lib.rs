//! Crane's in-house terminal core.
//!
//! Owns the `linefeed` / scroll-into-history decision so TUI scrollback
//! duplication (Claude Code, Ink, neovim cmdline rewriting their UI
//! region) is fixed at this layer: `linefeed` only pushes a row into
//! scrollback when the cursor sits at the bottom of the active scroll
//! region. A cursor mid-region just moves down — no history write.
//! `?2026` synchronized output is buffer-and-replay at the
//! [`processor`] layer; the live-grid scroll routing in [`term`] is
//! what actually prevents pollution.
//!
//! Module roles:
//! * [`cell`] — single grid cell (char + colors + flag bits).
//! * [`row`] — a row of cells with an `occ` upper bound on dirty
//!   columns so callers can skip equal-to-template tails.
//! * [`grid`] — live viewport: ring of rows + cursor + scroll region.
//! * [`scrollback`] — capped FIFO of evicted rows.
//! * [`mode`] — `TermMode` bitflags (DEC private modes we honor).
//! * [`handler`] — our `Handler` trait. Scroll-producing methods
//!   return [`ScrollDelta`] so the renderer / selection layer can
//!   adjust without re-deriving it.
//! * [`term`] — `Term` glues grid + scrollback + cursor + mode and
//!   implements [`handler::Handler`].
//! * [`alt_screen`] — alt-screen wrapper (no scrollback).
//! * [`perform`] — `vte::ansi::Handler` impl that forwards into our
//!   own `Handler`. Bridges the parser's typed callbacks into our
//!   trait shape.
//! * [`processor`] — owns the `vte::Parser`, the `?2026` buffer, and
//!   the parse loop.
//! * [`sync`] — `SyncBuffer` state machine + 150 ms / 2 MiB safety
//!   caps for synchronized output.

pub mod cell;
pub mod row;
pub mod grid;
pub mod scrollback;
pub mod mode;
pub mod handler;
pub mod term;
pub mod alt_screen;
pub mod perform;
pub mod processor;
pub mod reflow;
pub mod sync;
pub mod index;
pub mod selection;

pub use cell::{Cell, CellExtra, Color, Flags, NamedColor};
pub use handler::{Handler, ProcessorInput, ScrollDelta};
pub use index::{Column, Line, Point, Side};
pub use mode::TermMode;
pub use processor::Processor;
pub use selection::{Selection, SelectionRange, SelectionType};
pub use term::Term;
