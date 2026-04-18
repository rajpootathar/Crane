//! Incremental syntect highlighting with per-line state caching.
//!
//! ## Why this exists
//!
//! Our previous path re-ran `HighlightLines` over the *whole buffer* on every
//! keystroke. For TSX files (two-face's grammar uses `fancy-regex` with deep
//! backtracking for JSX + template literals), that was 50–150 ms per
//! keystroke. egui's UI thread blocks on the layouter returning a galley, so
//! the editor felt unusable.
//!
//! ## What this does
//!
//! syntect's lower-level API (`ParseState` + `HighlightState`) is
//! state-carrying: processing line N produces the state required to parse
//! line N+1. We cache, per source line, a tuple of
//!   (text_hash, post_parse_state, post_highlight_state, highlighted_segments).
//!
//! On each call:
//!   1. Walk cached lines in order. While their `text_hash` matches the
//!      corresponding live line, resume our local parse + highlight state
//!      from that entry and bump `first_diff`.
//!   2. Truncate the cache at `first_diff`.
//!   3. Re-highlight from `first_diff` to end-of-file, pushing new entries.
//!
//! For the common case of typing near the bottom of a file, step 3 is a
//! single line. For typing near the top, step 3 re-runs from that point to
//! the end, which is still far cheaper than the whole buffer and matches
//! what editors like Sublime / bat / delta do.
//!
//! ## What this does NOT address
//!
//! egui's `TextEdit` still lays out the whole galley each frame the text
//! changes (glyph positions for every char). For very large files (> ~5 k
//! lines) egui's layout cost dominates regardless. That needs a
//! viewport-culled custom editor widget, which is a separate project.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use syntect::highlighting::{HighlightIterator, HighlightState, Highlighter, Style, Theme};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

#[derive(Clone)]
pub struct LineHL {
    text_hash: u64,
    parse_state: ParseState,
    highlight_state: HighlightState,
    /// Owned segments so callers don't have to thread borrow lifetimes.
    /// Style is Copy, String is cheap compared to the syntect parse cost.
    pub segments: Vec<(Style, String)>,
}

#[derive(Clone, Default)]
pub struct LineHighlightCache {
    /// Fingerprint of (theme name, syntax name). Any change wipes lines.
    context_hash: u64,
    pub lines: Vec<LineHL>,
}

fn hash_line(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Update `cache` to reflect the current `text`, running syntect only on
/// lines whose text changed (or whose preceding state changed because an
/// earlier line was rehighlighted).
pub fn rehighlight(
    cache: &mut LineHighlightCache,
    text: &str,
    syntax: &'static SyntaxReference,
    theme: &'static Theme,
    syntaxes: &'static SyntaxSet,
    context_hash: u64,
) {
    if cache.context_hash != context_hash {
        cache.lines.clear();
        cache.context_hash = context_hash;
    }

    let lines: Vec<&str> = LinesWithEndings::from(text).collect();

    // Walk the cache while it still matches the live buffer. Each matched
    // line hands us the state we need to process the NEXT line.
    let mut first_diff = 0usize;
    let mut resumed_parse = ParseState::new(syntax);
    let highlighter = Highlighter::new(theme);
    let mut resumed_hl = HighlightState::new(&highlighter, ScopeStack::new());

    for (i, line) in lines.iter().enumerate() {
        match cache.lines.get(i) {
            Some(entry) if entry.text_hash == hash_line(line) => {
                resumed_parse = entry.parse_state.clone();
                resumed_hl = entry.highlight_state.clone();
                first_diff = i + 1;
            }
            _ => break,
        }
    }

    cache.lines.truncate(first_diff);

    let mut parse_state = resumed_parse;
    let mut highlight_state = resumed_hl;

    for line in &lines[first_diff..] {
        let ops = parse_state.parse_line(line, syntaxes).unwrap_or_default();
        let iter = HighlightIterator::new(
            &mut highlight_state,
            &ops[..],
            line,
            &highlighter,
        );
        let segments: Vec<(Style, String)> =
            iter.map(|(st, piece)| (st, piece.to_string())).collect();
        cache.lines.push(LineHL {
            text_hash: hash_line(line),
            parse_state: parse_state.clone(),
            highlight_state: highlight_state.clone(),
            segments,
        });
    }
}
