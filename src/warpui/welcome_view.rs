//! `WarpWelcomeView` — the landing / "new tab" pane (warpui port of old Crane's
//! `views/welcome_view.rs`). A centered column: a large `Crane` wordmark, a
//! subtitle, three primary action buttons (New Terminal / Open Files / New
//! Browser), and a keyboard cheat-sheet.
//!
//! v1 has NO logo image — warpui texture upload is a separate wave, so the
//! branding is a text wordmark. The buttons don't act on their own (spawning a
//! terminal / toggling a panel needs the shell), so the view exposes a
//! `WelcomeAction` and an optional `on_action` callback the shell wires at
//! construction time. On click the view invokes that callback with an
//! `EventContext`; the shell's closure translates the `WelcomeAction` into the
//! matching `CraneShellAction` and dispatches it. See the module-level wiring
//! notes returned with this file for the exact shell hookup.

use std::rc::Rc;

use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, EventContext,
    Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius, Rect,
    Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::platform::Cursor;
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext};

use crate::warpui::icons;
use crate::warpui::theme;

/// Which primary action the user clicked on the landing page. The view itself
/// can't perform any of these (they need the shell), so it hands this enum to
/// the `on_action` callback, which maps it to a `CraneShellAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeAction {
    /// Spawn / split in a new terminal.
    Terminal,
    /// Reveal the Files surface (Right Panel / Files tab).
    Files,
    /// Open a new Browser pane.
    Browser,
}

/// Callback the shell supplies so a button click can reach it. It runs inside
/// the click event with the live `EventContext`, so the shell's closure just
/// does `ctx.dispatch_typed_action(CraneShellAction::…)`. `Rc` (not `Box`)
/// because the view clones it into each button's per-frame click handler.
pub type WelcomeCallback = Rc<dyn Fn(WelcomeAction, &mut EventContext)>;

pub struct WarpWelcomeView {
    /// Proportional UI font for the wordmark, labels, and cheat-sheet.
    ui_font: FamilyId,
    /// Phosphor icon font for the button glyphs.
    icon_font: FamilyId,
    /// Persistent per-button hover state (index 0=Terminal, 1=Files, 2=Browser).
    /// MUST live on the view, not the transient `Hoverable`, so hover survives
    /// the re-render between mouse-in and the styled repaint.
    hover: [MouseStateHandle; 3],
    /// Shell-supplied dispatcher; `None` = the view is standalone (clicks fall
    /// back to a self-dispatched `WelcomeAction`, a no-op handled below).
    on_action: Option<WelcomeCallback>,
}

impl WarpWelcomeView {
    pub fn new(
        _ctx: &mut ViewContext<Self>,
        ui_font: FamilyId,
        icon_font: FamilyId,
        on_action: Option<WelcomeCallback>,
    ) -> Self {
        Self {
            ui_font,
            icon_font,
            hover: [
                MouseStateHandle::default(),
                MouseStateHandle::default(),
                MouseStateHandle::default(),
            ],
            on_action,
        }
    }

    /// One primary action button — a fixed-size rounded card whose border turns
    /// accent on hover. Uses `Hoverable` so hover styling + click live in one
    /// element and hover changes self-notify a repaint.
    fn button(
        &self,
        idx: usize,
        glyph: &'static str,
        label: &'static str,
        hint: &'static str,
        chord: &'static str,
        action: WelcomeAction,
    ) -> Box<dyn Element> {
        let state = self.hover[idx].clone();
        let cb = self.on_action.clone();
        let ui_font = self.ui_font;
        let icon_font = self.icon_font;
        Hoverable::new(state, move |ms| {
            button_face(ui_font, icon_font, ms.is_hovered(), glyph, label, hint, chord)
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| match &cb {
            Some(cb) => cb(action, ctx),
            // No shell wired — dispatch the typed action so it's at least
            // observable; handled as a no-op by this view's `handle_action`.
            None => ctx.dispatch_typed_action(action),
        })
        .finish()
    }

    /// The three action buttons in a centered row.
    fn buttons_row(&self) -> Box<dyn Element> {
        Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.button(
                0,
                icons::TERMINAL_WINDOW,
                "New Terminal",
                "Spawn a shell in this pane",
                "Cmd T",
                WelcomeAction::Terminal,
            ))
            .with_child(hgap(16.0))
            .with_child(self.button(
                1,
                icons::FOLDER_OPEN,
                "Open Files",
                "Show the workspace tree",
                "Cmd /",
                WelcomeAction::Files,
            ))
            .with_child(hgap(16.0))
            .with_child(self.button(
                2,
                icons::GLOBE,
                "New Browser",
                "Embedded web tab",
                "Opt Cmd T",
                WelcomeAction::Browser,
            ))
            .finish()
    }

    /// Two-column keyboard cheat-sheet: chord (accent) + description (muted).
    /// Modifiers are spelled out (Cmd / Shift / Opt) rather than using the
    /// glyphs, which aren't guaranteed to be covered by the loaded UI font.
    fn shortcuts_block(&self) -> Box<dyn Element> {
        const ITEMS: [(&str, &str); 8] = [
            ("Cmd T", "Split pane - new terminal"),
            ("Cmd Shift T", "New tab in workspace"),
            ("Cmd D / Cmd Shift D", "Split horizontal / vertical"),
            ("Cmd W / Cmd Shift W", "Close pane / tab"),
            ("Cmd [ / Cmd ]", "Focus previous / next pane"),
            ("Cmd B / Cmd /", "Toggle Left / Right Panel"),
            ("Cmd = / Cmd -", "Font size up / down"),
            ("Cmd 0", "Reset font size"),
        ];
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(
                Text::new("SHORTCUTS", self.ui_font, 10.5)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_child(vgap(10.0));
        for (chord, desc) in ITEMS {
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(
                        Text::new(chord, self.ui_font, 11.5)
                            .with_color(theme::accent())
                            .finish(),
                    )
                    .with_width(190.0)
                    .finish(),
                )
                .with_child(
                    Text::new(desc, self.ui_font, 11.5)
                        .with_color(theme::text_muted())
                        .finish(),
                )
                .finish();
            col = col.with_child(row).with_child(vgap(6.0));
        }
        col.finish()
    }
}

/// The visual face of a button, restyled by `hovered`. A free fn (not a method)
/// so the `Hoverable` build closure captures only `Copy` values, never `self`.
fn button_face(
    ui_font: FamilyId,
    icon_font: FamilyId,
    hovered: bool,
    glyph: &'static str,
    label: &'static str,
    hint: &'static str,
    chord: &'static str,
) -> Box<dyn Element> {
    let bg = if hovered {
        theme::row_hover()
    } else {
        theme::topbar_bg()
    };
    let border_color = if hovered {
        theme::accent()
    } else {
        theme::border()
    };
    let col = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::Center)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(glyph, icon_font, 24.0)
                .with_color(theme::accent())
                .finish(),
        )
        .with_child(vgap(8.0))
        .with_child(
            Text::new(label, ui_font, 14.0)
                .with_color(theme::text())
                .finish(),
        )
        .with_child(vgap(4.0))
        .with_child(
            Text::new(hint, ui_font, 11.0)
                .with_color(theme::text_muted())
                .finish(),
        )
        .with_child(vgap(6.0))
        .with_child(
            Text::new(chord, ui_font, 10.5)
                .with_color(theme::text_muted())
                .finish(),
        )
        .finish();
    ConstrainedBox::new(
        Container::new(col)
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_uniform_padding(12.0)
            .finish(),
    )
    .with_width(170.0)
    .with_height(96.0)
    .finish()
}

/// A transparent vertical spacer of height `h` (1px wide, harmless in a
/// centered column).
fn vgap(h: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Rect::new().finish())
        .with_width(1.0)
        .with_height(h)
        .finish()
}

/// A transparent horizontal spacer of width `w`.
fn hgap(w: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Rect::new().finish())
        .with_width(w)
        .with_height(1.0)
        .finish()
}

impl Entity for WarpWelcomeView {
    type Event = ();
}

impl TypedActionView for WarpWelcomeView {
    type Action = WelcomeAction;

    /// Deliberately a no-op. The real work happens in the shell via the
    /// `on_action` callback (which dispatches a `CraneShellAction`). This impl
    /// exists so the shell can create the view with `add_typed_action_view` —
    /// that records the shell as the responder-chain parent, which is what lets
    /// the callback's shell action bubble up to the shell. It also makes the
    /// standalone (`on_action = None`) fallback dispatch a safe no-op.
    fn handle_action(&mut self, _action: &Self::Action, _ctx: &mut ViewContext<Self>) {}
}

impl View for WarpWelcomeView {
    fn ui_name() -> &'static str {
        "WarpWelcomeView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        // The centered content column: wordmark → subtitle → buttons → cheat.
        let content = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new("Crane", self.ui_font, 34.0)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_child(vgap(6.0))
            .with_child(
                Text::new(
                    "Pick a surface to begin, or use a shortcut below.",
                    self.ui_font,
                    13.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(vgap(28.0))
            .with_child(self.buttons_row())
            .with_child(vgap(28.0))
            .with_child(self.shortcuts_block())
            .finish();
        // Fill the pane and center `content` both axes: the row stretches to the
        // full height and centers the column horizontally; the column (Max +
        // Center) centers its content vertically.
        let centered = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(content)
            .finish();
        // Flat surface background behind the centered stack (mirrors the egui
        // welcome view, which painted the pane in the surface colour).
        Stack::new()
            .with_child(
                Rect::new()
                    .with_background_color(theme::surface())
                    .finish(),
            )
            .with_child(centered)
            .finish()
    }
}
