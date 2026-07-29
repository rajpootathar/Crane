//! Bundled fonts — JetBrains Mono for terminal / editor / mono UI text.
//!
//! JetBrains Mono Regular (~264 KB, OFL 1.1 — license alongside the TTF at
//! `assets/JetBrainsMono-OFL.txt`) is embedded in the binary the same way the
//! Phosphor icon font is, so terminal and editor metrics are identical on
//! every machine instead of depending on the host's Menlo. If the embedded
//! bytes ever fail to parse, we degrade to system Menlo (then Monaco /
//! Courier) instead of panicking.
//!
//! Non-Latin coverage (CJK / Arabic / Hebrew / Devanagari / Braille) needs no
//! manual fallback registration here: warpui's macOS FontDB builds every
//! font's fallback chain from CoreText's cascade list
//! (`cascade_list_for_languages`, which also appends Apple Symbols), and the
//! mac text-layout path shapes with CTLine, which substitutes fonts per glyph
//! natively. A memory-loaded font is still a CTFont, so bundled JetBrains
//! Mono gets the full system cascade for free.

use warpui::fonts::{Cache, FamilyId};

/// Family name the bundled monospace font registers under.
pub const MONO_FAMILY: &str = "JetBrains Mono";

const JETBRAINS_MONO_TTF: &[u8] = include_bytes!("assets/JetBrainsMono-Regular.ttf");

/// Monospace family for terminal grids, the editor, diffs, and the git-log
/// graph: bundled JetBrains Mono, falling back to system Menlo (then Monaco /
/// Courier) if the embedded bytes fail to parse. Idempotent — repeat calls
/// hit the cache's family table instead of re-parsing the TTF.
pub fn mono(cache: &mut Cache) -> FamilyId {
    if let Some(id) = cache.family_id_for_name(MONO_FAMILY) {
        return id;
    }
    match cache.load_family_from_bytes(MONO_FAMILY, vec![JETBRAINS_MONO_TTF.to_vec()]) {
        Ok(id) => id,
        Err(err) => {
            log::warn!(
                "bundled JetBrains Mono failed to load ({err}); falling back to system monospace"
            );
            system_mono(cache)
        }
    }
}

fn system_mono(cache: &mut Cache) -> FamilyId {
    ["Menlo", "Monaco", "Courier"]
        .iter()
        .find_map(|name| cache.get_or_load_system_font(name).ok())
        .expect("no monospace font available (bundled JetBrains Mono unparsable; Menlo, Monaco, and Courier all missing)")
}

/// Proportional UI family: system Helvetica Neue, degrading to the bundled
/// monospace rather than panicking if it is unavailable.
pub fn ui(cache: &mut Cache) -> FamilyId {
    cache
        .get_or_load_system_font("Helvetica Neue")
        .unwrap_or_else(|err| {
            log::warn!("Helvetica Neue unavailable ({err}); using the monospace family for UI text");
            mono(cache)
        })
}
