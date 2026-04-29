//! Grid cell representation.
//!
//! The fast-path `Cell` is kept small (~24 bytes) so dense grids
//! don't blow cache. Rare attributes (zero-width grapheme stacks,
//! prompt markers) live on a boxed [`CellExtra`] sidecar so the
//! common cell stays compact.

use bitflags::bitflags;

/// 16-color ANSI named palette plus the `Foreground` / `Background`
/// sentinels that resolve against the active theme. Consumers map
/// `Named` → concrete RGB at render time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    /// Theme-default foreground; resolves at render time.
    Foreground,
    /// Theme-default background; resolves at render time.
    Background,
    /// Cursor cell foreground (when not overridden by SGR).
    CursorText,
    /// Cursor cell background.
    Cursor,
    /// Dim variants of the 8 base colors used when the SGR `DIM`
    /// flag is set without an explicit color.
    DimBlack,
    DimRed,
    DimGreen,
    DimYellow,
    DimBlue,
    DimMagenta,
    DimCyan,
    DimWhite,
}

/// All color forms a cell can carry. Variants are sized to keep
/// `Color` at 4 bytes — `Indexed` and `Rgb` use `u8`s, `Named`
/// fits in one byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

impl Color {
    /// Translate a `vte::ansi::Color` into our internal type. The
    /// vte enum's NamedColor has 30 variants vs our 28; the two we
    /// don't carry (`BrightForeground`, `DimForeground`) collapse
    /// onto plain `Foreground` since the renderer derives bright/
    /// dim from the SGR flag bits anyway.
    pub fn from_vte(c: vte::ansi::Color) -> Self {
        match c {
            vte::ansi::Color::Named(n) => Color::Named(NamedColor::from_vte(n)),
            vte::ansi::Color::Spec(rgb) => Color::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            },
            vte::ansi::Color::Indexed(i) => Color::Indexed(i),
        }
    }
}

impl NamedColor {
    pub fn from_vte(n: vte::ansi::NamedColor) -> Self {
        use vte::ansi::NamedColor as V;
        match n {
            V::Black => NamedColor::Black,
            V::Red => NamedColor::Red,
            V::Green => NamedColor::Green,
            V::Yellow => NamedColor::Yellow,
            V::Blue => NamedColor::Blue,
            V::Magenta => NamedColor::Magenta,
            V::Cyan => NamedColor::Cyan,
            V::White => NamedColor::White,
            V::BrightBlack => NamedColor::BrightBlack,
            V::BrightRed => NamedColor::BrightRed,
            V::BrightGreen => NamedColor::BrightGreen,
            V::BrightYellow => NamedColor::BrightYellow,
            V::BrightBlue => NamedColor::BrightBlue,
            V::BrightMagenta => NamedColor::BrightMagenta,
            V::BrightCyan => NamedColor::BrightCyan,
            V::BrightWhite => NamedColor::BrightWhite,
            V::Foreground | V::BrightForeground | V::DimForeground => NamedColor::Foreground,
            V::Background => NamedColor::Background,
            V::Cursor => NamedColor::Cursor,
            V::DimBlack => NamedColor::DimBlack,
            V::DimRed => NamedColor::DimRed,
            V::DimGreen => NamedColor::DimGreen,
            V::DimYellow => NamedColor::DimYellow,
            V::DimBlue => NamedColor::DimBlue,
            V::DimMagenta => NamedColor::DimMagenta,
            V::DimCyan => NamedColor::DimCyan,
            V::DimWhite => NamedColor::DimWhite,
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::Named(NamedColor::Foreground)
    }
}

bitflags! {
    /// SGR / cell-state bits packed into one u16. Mirrors the typical
    /// terminal-emulator flag set (xterm + DEC + a few extensions).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Flags: u16 {
        const INVERSE          = 1 << 0;
        const BOLD             = 1 << 1;
        const ITALIC           = 1 << 2;
        const UNDERLINE        = 1 << 3;
        /// Set on the *last* cell of a wrapped logical line — the
        /// renderer uses this to keep word-selection from breaking
        /// at the wrap, and copy-to-clipboard skips a CRLF here.
        const WRAPLINE         = 1 << 4;
        /// First half of a wide (CJK / emoji) glyph.
        const WIDE_CHAR        = 1 << 5;
        /// Second half — has no character of its own; renderer skips
        /// it because the wide glyph already covered both columns.
        const WIDE_CHAR_SPACER = 1 << 6;
        const DIM              = 1 << 7;
        const HIDDEN           = 1 << 8;
        const STRIKEOUT        = 1 << 9;
        /// Cell currently holds the cursor (rendered specially).
        const HAS_CURSOR       = 1 << 10;
        const DOUBLE_UNDERLINE = 1 << 11;
    }
}

/// Sidecar for the rare per-cell extras. Boxed so the common cell
/// path stays small. Only allocated when actually used.
#[derive(Clone, Debug, Default)]
pub struct CellExtra {
    /// Zero-width grapheme parts that stacked onto this cell after
    /// the base char (combining marks, ZWJ sequences). Bounded so a
    /// pathological PTY can't blow memory.
    pub zero_width: Vec<char>,
    /// Optional anchor used by future block-style features to mark
    /// "end of prompt" — set by shell integration, not by the VT
    /// parser. Unused in v1.
    pub prompt_marker: Option<u32>,
}

/// One cell of the grid.
#[derive(Clone, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    /// `None` for the common case; allocated on demand when a cell
    /// needs combining marks or a prompt marker.
    pub extra: Option<Box<CellExtra>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
            extra: None,
        }
    }
}

impl Cell {
    /// True when the cell holds nothing visible. Used by row
    /// dirty-bound updates and copy-to-clipboard trimming.
    pub fn is_empty(&self) -> bool {
        self.ch == ' ' && self.flags.is_empty() && self.extra.is_none()
    }

    /// Push a zero-width grapheme part onto this cell. Allocates
    /// `CellExtra` lazily.
    pub fn push_zero_width(&mut self, c: char) {
        let extra = self.extra.get_or_insert_with(|| Box::new(CellExtra::default()));
        // Cap at a generous bound — a single user-perceived character
        // shouldn't legitimately need more than this many combining
        // marks. Past the cap we drop further additions instead of
        // letting a pathological stream balloon a single cell.
        if extra.zero_width.len() < 16 {
            extra.zero_width.push(c);
        }
    }
}
