//! `WarpBrowserView` — the Browser Pane's warpui view: per-pane tab strip,
//! URL toolbar (Back / Forward / Reload / address field / Go / open-external)
//! and a footer status bar (tab count + WebKit memory). The native WKWebView
//! itself is managed by `warpui::browser::BrowserHost`; this view only draws
//! the chrome and reserves the body rect (recorded via `RectProbe`) that the
//! shell's browser tick hands to the host each sync.
//!
//! Port of old Crane's `views/browser_view.rs`. The URL field follows this
//! port's simplified text-input pattern (append/backspace + paste + caret
//! block, same as the find bar) rather than egui's full `TextEdit`.

use std::cell::Cell;
use std::rc::Rc;

use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, DispatchEventResult, Element, EventHandler,
    Expanded, Flex, ParentElement, Radius, Rect, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::vec2f;
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext};

use crate::warpui::browser::{self, SlotKey};
use crate::warpui::layout::PaneId;
use crate::warpui::rect_probe::RectProbe;
use crate::warpui::{icons, theme};

/// Footer status-bar height (matches the old egui FOOTER_H).
const FOOTER_H: f32 = 22.0;

/// One browser tab: its committed URL, live page title, and the URL field's
/// edit buffer (kept separate so typing doesn't navigate until Enter/Go).
pub struct BrowserTab {
    pub id: u32,
    pub url: String,
    pub title: String,
    pub input_buf: String,
}

pub struct WarpBrowserView {
    pane_id: PaneId,
    tabs: Vec<BrowserTab>,
    active: usize,
    next_tab_id: u32,
    /// True while the URL field owns typing (shell routes keys here).
    input_active: bool,
    /// True when Cmd+A has selected the whole URL buffer: the field renders
    /// highlighted and the next typed char / paste / backspace replaces it.
    /// Cleared on any edit, blur, or commit. The simplified field has no caret
    /// model, so "select all" is the only selection state it supports.
    input_sel_all: bool,
    /// Body rect recorded at paint time by the RectProbe — window-space, the
    /// exact rect the WKWebView should cover. Read by the shell browser tick.
    body_rect: Rc<Cell<RectF>>,
    ui_font: FamilyId,
    icon_font: FamilyId,
}

#[derive(Debug, Clone)]
pub enum BrowserAction {
    ActivateTab(usize),
    CloseTab(usize),
    NewTab,
    /// Commit the URL field: normalize, navigate, blur.
    Go,
    Back,
    Forward,
    Reload,
    OpenExternal,
    /// Click into the URL field — take typing focus.
    FocusInput,
    /// Esc — drop typing focus, keep the buffer.
    Blur,
    InputChar(String),
    InputBackspace,
    Paste(String),
}

impl WarpBrowserView {
    pub fn new(
        pane_id: PaneId,
        ui_font: FamilyId,
        icon_font: FamilyId,
        restored: Vec<(String, String)>,
        restored_active: usize,
    ) -> Self {
        let mut next_tab_id = 0u32;
        let mut tabs: Vec<BrowserTab> = restored
            .into_iter()
            .map(|(url, title)| {
                let id = next_tab_id;
                next_tab_id += 1;
                BrowserTab {
                    id,
                    input_buf: url.clone(),
                    url,
                    title,
                }
            })
            .collect();
        if tabs.is_empty() {
            tabs.push(BrowserTab {
                id: 0,
                url: String::new(),
                title: String::new(),
                input_buf: String::new(),
            });
            next_tab_id = 1;
        }
        let active = restored_active.min(tabs.len() - 1);
        Self {
            pane_id,
            tabs,
            active,
            next_tab_id,
            // A fresh blank tab wants a URL immediately; a restored session
            // already has pages to show.
            input_active: false,
            input_sel_all: false,
            body_rect: Rc::new(Cell::new(RectF::new(vec2f(0.0, 0.0), vec2f(0.0, 0.0)))),
            ui_font,
            icon_font,
        }
    }

    // ── Shell-tick read API ──────────────────────────────────────────────

    /// (key, window-rect, url) for the active tab — what the WKWebView of
    /// this pane should show when the pane is visible.
    pub fn active_slot(&self) -> (SlotKey, RectF, String) {
        let t = &self.tabs[self.active];
        ((self.pane_id, t.id), self.body_rect.get(), t.url.clone())
    }

    /// (key, url) of every non-active tab — kept alive but hidden.
    pub fn inactive_slots(&self) -> Vec<(SlotKey, String)> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active)
            .map(|(_, t)| ((self.pane_id, t.id), t.url.clone()))
            .collect()
    }

    /// Every (pane, tab) key this pane owns — feeds `all_keys` so the host
    /// keeps webviews of panes on non-active Tabs alive.
    pub fn all_keys(&self) -> Vec<SlotKey> {
        self.tabs.iter().map(|t| (self.pane_id, t.id)).collect()
    }

    /// Pane-header title: the active tab's short label.
    pub fn title(&self) -> String {
        short_title(&self.tabs[self.active])
    }

    /// Apply a WKWebView-reported URL change (redirect, link click, SPA
    /// route) back to the owning tab so the URL bar tracks the page.
    pub fn apply_url_update(&mut self, key: SlotKey, new_url: &str) -> bool {
        if new_url.is_empty() {
            return false;
        }
        let mut changed = false;
        for t in &mut self.tabs {
            if (self.pane_id, t.id) == key && t.url != new_url {
                t.url = new_url.to_string();
                t.input_buf = new_url.to_string();
                changed = true;
            }
        }
        changed
    }

    /// Persistence snapshot: (url, title) per tab + active index.
    pub fn persist_tabs(&self) -> (Vec<(String, String)>, usize) {
        (
            self.tabs
                .iter()
                .map(|t| (t.url.clone(), t.title.clone()))
                .collect(),
            self.active,
        )
    }

    fn active_key(&self) -> SlotKey {
        (self.pane_id, self.tabs[self.active].id)
    }

    /// Shell `SendKeys` entry: typing routes to the URL field while it owns
    /// focus (same simplified field model as the find bar — chars, backspace,
    /// Enter commits, Escape blurs). Inert otherwise: the WKWebView is a
    /// native first responder and receives its own keys directly from AppKit.
    pub fn input_key(&mut self, ks: &warpui::keymap::Keystroke, ctx: &mut ViewContext<Self>) {
        if !self.input_active || ks.cmd || ks.ctrl {
            return;
        }
        let action = match ks.key.as_str() {
            "enter" => Some(BrowserAction::Go),
            "escape" => Some(BrowserAction::Blur),
            "backspace" => Some(BrowserAction::InputBackspace),
            "space" => Some(BrowserAction::InputChar(" ".to_string())),
            k if k.chars().count() == 1 => Some(BrowserAction::InputChar(k.to_string())),
            _ => None,
        };
        if let Some(a) = action {
            self.handle_action(&a, ctx);
        }
    }

    /// Cmd+A on the focused URL field: select the whole buffer (next edit
    /// replaces it). No-op on an empty buffer.
    pub fn url_select_all(&mut self, ctx: &mut ViewContext<Self>) {
        if self.input_active && !self.tabs[self.active].input_buf.is_empty() {
            self.input_sel_all = true;
            ctx.notify();
        }
    }

    /// Cmd+C on the focused URL field: copy the whole buffer.
    pub fn url_copy(&self, ctx: &mut ViewContext<Self>) {
        let buf = self.tabs[self.active].input_buf.clone();
        if self.input_active && !buf.is_empty() {
            ctx.clipboard()
                .write(warpui::clipboard::ClipboardContent::plain_text(buf));
        }
    }

    /// Cmd+X on the focused URL field: copy the whole buffer, then clear it.
    pub fn url_cut(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.input_active {
            return;
        }
        let buf = std::mem::take(&mut self.tabs[self.active].input_buf);
        self.input_sel_all = false;
        if !buf.is_empty() {
            ctx.clipboard()
                .write(warpui::clipboard::ClipboardContent::plain_text(buf));
            ctx.notify();
        }
    }

    /// Cmd+V on the focused URL field: insert clipboard text (replacing the
    /// buffer when it is select-all'd), newlines stripped — a URL is one line.
    pub fn url_paste(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.input_active {
            return;
        }
        let text = ctx.clipboard().read().plain_text;
        let text: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if !text.is_empty() {
            self.handle_action(&BrowserAction::Paste(text), ctx);
        }
    }

    // ── Chrome pieces ────────────────────────────────────────────────────

    fn tab_strip(&self) -> Box<dyn Element> {
        let mut row = Flex::row();
        for (idx, tab) in self.tabs.iter().enumerate() {
            let is_active = idx == self.active;
            let mut chip = Flex::row();
            // Loading spinner slot — the port keeps the old fallback (static
            // accent glyph; egui had no rotation primitive either).
            if browser::is_loading((self.pane_id, tab.id)) {
                chip = chip.with_child(
                    Container::new(
                        Text::new(icons::ARROW_CLOCKWISE.to_string(), self.icon_font, 11.0)
                            .with_color(theme::accent())
                            .finish(),
                    )
                    .with_padding_right(4.0)
                    .finish(),
                );
            }
            let label = Text::new(short_title(tab), self.ui_font, 11.5)
                .with_color(if is_active {
                    theme::text()
                } else {
                    theme::text_muted()
                })
                .finish();
            chip = chip.with_child(label);
            if self.tabs.len() > 1 {
                let close = EventHandler::new(
                    Container::new(
                        Text::new(icons::X.to_string(), self.icon_font, 10.0)
                            .with_color(theme::text_muted())
                            .finish(),
                    )
                    .with_padding_left(6.0)
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action(BrowserAction::CloseTab(idx));
                    DispatchEventResult::StopPropagation
                })
                .finish();
                chip = chip.with_child(close);
            }
            let chip = EventHandler::new(
                Container::new(chip.finish())
                    .with_background_color(if is_active {
                        theme::surface()
                    } else {
                        theme::bg()
                    })
                    .with_border(Border::all(1.0).with_border_color(if is_active {
                        theme::accent()
                    } else {
                        theme::border()
                    }))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(BrowserAction::ActivateTab(idx));
                DispatchEventResult::StopPropagation
            })
            .finish();
            row = row.with_child(Container::new(chip).with_padding_right(2.0).finish());
        }
        // Trailing "+" — new tab.
        let plus = EventHandler::new(
            Container::new(
                Text::new(icons::PLUS.to_string(), self.icon_font, 12.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(BrowserAction::NewTab);
            DispatchEventResult::StopPropagation
        })
        .finish();
        row = row.with_child(plus);
        Container::new(row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(2.0)
            .finish()
    }

    fn icon_button(&self, glyph: &str, action: BrowserAction) -> Box<dyn Element> {
        EventHandler::new(
            Container::new(
                Text::new(glyph.to_string(), self.icon_font, 13.0)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    }

    fn toolbar(&self) -> Box<dyn Element> {
        let tab = &self.tabs[self.active];
        let mut row = Flex::row();
        row = row.with_child(self.icon_button(icons::ARROW_LEFT, BrowserAction::Back));
        row = row.with_child(self.icon_button(icons::ARROW_RIGHT, BrowserAction::Forward));
        row = row.with_child(self.icon_button(icons::ARROW_CLOCKWISE, BrowserAction::Reload));

        // URL field — simplified editable field (append/backspace/paste),
        // caret block while it owns typing. Click to focus.
        let shown = if tab.input_buf.is_empty() && !self.input_active {
            "https://…".to_string()
        } else {
            tab.input_buf.clone()
        };
        let text_el = Text::new(shown, self.ui_font, 12.0)
            .with_color(if tab.input_buf.is_empty() && !self.input_active {
                theme::text_muted()
            } else {
                theme::text()
            })
            .finish();
        // Cmd+A highlights the whole buffer; render it on a selection band and
        // drop the caret (the entire field is "selected").
        let sel_all = self.input_active && self.input_sel_all && !tab.input_buf.is_empty();
        let text_el = if sel_all {
            Container::new(text_el)
                .with_background_color(theme::row_active())
                .finish()
        } else {
            text_el
        };
        let mut field = Flex::row().with_child(text_el);
        if self.input_active && !sel_all {
            field = field.with_child(
                ConstrainedBox::new(Rect::new().with_background_color(theme::accent()).finish())
                    .with_width(2.0)
                    .with_height(14.0)
                    .finish(),
            );
        }
        let field = EventHandler::new(
            Container::new(field.finish())
                .with_background_color(theme::bg())
                .with_border(Border::all(1.0).with_border_color(if self.input_active {
                    theme::accent()
                } else {
                    theme::border()
                }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(BrowserAction::FocusInput);
            DispatchEventResult::StopPropagation
        })
        .finish();
        row = row.with_child(Expanded::new(1.0, field).finish());

        // "Go" commit button.
        let go = EventHandler::new(
            Container::new(
                Text::new("Go".to_string(), self.ui_font, 11.5)
                    .with_color(theme::text())
                    .finish(),
            )
            .with_background_color(theme::surface())
            .with_border(Border::all(1.0).with_border_color(theme::border()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish(),
        )
        .on_left_mouse_down(|ctx, _app, _pos| {
            ctx.dispatch_typed_action(BrowserAction::Go);
            DispatchEventResult::StopPropagation
        })
        .finish();
        row = row.with_child(Container::new(go).with_padding_left(4.0).finish());
        row = row.with_child(
            self.icon_button(icons::ARROW_SQUARE_OUT, BrowserAction::OpenExternal),
        );

        Container::new(row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(2.0)
            .with_padding_bottom(4.0)
            .finish()
    }

    /// The webview's reserved body. On macOS the WKWebView paints natively
    /// above this surface; on other platforms it's a launcher card that hands
    /// off to the system browser.
    fn body(&self) -> Box<dyn Element> {
        let surface: Box<dyn Element> = if cfg!(target_os = "macos") {
            Rect::new().with_background_color(theme::surface()).finish()
        } else {
            let tab = &self.tabs[self.active];
            let msg = if tab.url.is_empty() {
                "Type a URL above and press Enter.".to_string()
            } else {
                format!("Embedded browser unavailable here — {} opens externally.", tab.url)
            };
            Container::new(
                Text::new(msg, self.ui_font, 12.5)
                    .with_color(theme::text_muted())
                    .finish(),
            )
            .with_padding_left(16.0)
            .with_padding_top(16.0)
            .finish()
        };
        Expanded::new(
            1.0,
            Box::new(RectProbe::new(surface, self.body_rect.clone())),
        )
        .finish()
    }

    fn footer(&self) -> Box<dyn Element> {
        let snap = browser::memory_snapshot();
        let tab_count = self.tabs.len();
        let (mem_color, right_label) = if snap.total_bytes == 0 {
            (theme::text_muted(), "WebKit memory: —".to_string())
        } else {
            let proc_suffix = if snap.process_count == 1 { "" } else { "es" };
            let label = if snap.total_bytes >= browser::memory::DANGER_BYTES {
                format!(
                    "WebKit: {} (heavy — close tabs)  ·  {} process{proc_suffix}",
                    browser::memory::human_bytes(snap.total_bytes),
                    snap.process_count
                )
            } else {
                format!(
                    "WebKit: {}  ·  {} process{proc_suffix}",
                    browser::memory::human_bytes(snap.total_bytes),
                    snap.process_count
                )
            };
            let color = if snap.total_bytes >= browser::memory::DANGER_BYTES {
                theme::error()
            } else if snap.total_bytes >= browser::memory::WARN_BYTES {
                theme::warning()
            } else {
                theme::text_muted()
            };
            (color, label)
        };
        let row = Flex::row()
            .with_child(
                Text::new(
                    format!("{tab_count} tab{}", if tab_count == 1 { "" } else { "s" }),
                    self.ui_font,
                    12.0,
                )
                .with_color(theme::text_muted())
                .finish(),
            )
            .with_child(Expanded::new(1.0, Flex::row().finish()).finish())
            .with_child(
                Text::new(right_label, self.ui_font, 12.0)
                    .with_color(mem_color)
                    .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(row)
                .with_background_color(theme::bg())
                .with_border(Border::top(1.0).with_border_color(theme::divider()))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(4.0)
                .finish(),
        )
        .with_height(FOOTER_H)
        .finish()
    }
}

impl TypedActionView for WarpBrowserView {
    type Action = BrowserAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            BrowserAction::ActivateTab(i) => {
                if *i < self.tabs.len() {
                    self.active = *i;
                }
            }
            BrowserAction::CloseTab(i) => {
                if self.tabs.len() > 1 && *i < self.tabs.len() {
                    let removed = self.tabs.remove(*i);
                    browser::queue_action(
                        (self.pane_id, removed.id),
                        browser::Action::Close,
                    );
                    if self.active >= self.tabs.len() {
                        self.active = self.tabs.len() - 1;
                    } else if self.active > *i {
                        self.active -= 1;
                    }
                }
            }
            BrowserAction::NewTab => {
                let id = self.next_tab_id;
                self.next_tab_id += 1;
                self.tabs.push(BrowserTab {
                    id,
                    url: String::new(),
                    title: String::new(),
                    input_buf: String::new(),
                });
                self.active = self.tabs.len() - 1;
                self.input_active = true;
            }
            BrowserAction::Go => {
                let tab = &mut self.tabs[self.active];
                let url = browser::normalize_url(tab.input_buf.trim());
                if !url.is_empty() {
                    tab.url = url.clone();
                    tab.title = url.clone();
                    tab.input_buf = url.clone();
                    let key = (self.pane_id, tab.id);
                    if cfg!(target_os = "macos") {
                        browser::queue_action(key, browser::Action::Load(url));
                    } else {
                        // No embedded backend on this platform — hand off.
                        let _ = webbrowser::open(&url);
                    }
                }
                self.input_active = false;
                self.input_sel_all = false;
            }
            BrowserAction::Back => {
                browser::queue_action(self.active_key(), browser::Action::Back);
            }
            BrowserAction::Forward => {
                browser::queue_action(self.active_key(), browser::Action::Forward);
            }
            BrowserAction::Reload => {
                browser::queue_action(self.active_key(), browser::Action::Reload);
            }
            BrowserAction::OpenExternal => {
                let url = &self.tabs[self.active].url;
                if !url.is_empty() {
                    let _ = webbrowser::open(url);
                }
            }
            BrowserAction::FocusInput => {
                self.input_active = true;
                self.input_sel_all = false;
            }
            BrowserAction::Blur => {
                self.input_active = false;
                self.input_sel_all = false;
            }
            BrowserAction::InputChar(s) => {
                if self.input_active {
                    // A selected-all buffer is replaced by the first keystroke.
                    if self.input_sel_all {
                        self.tabs[self.active].input_buf.clear();
                        self.input_sel_all = false;
                    }
                    self.tabs[self.active].input_buf.push_str(s);
                }
            }
            BrowserAction::InputBackspace => {
                if self.input_active {
                    // Backspace over a full selection clears the whole buffer.
                    if self.input_sel_all {
                        self.tabs[self.active].input_buf.clear();
                        self.input_sel_all = false;
                    } else {
                        self.tabs[self.active].input_buf.pop();
                    }
                }
            }
            BrowserAction::Paste(s) => {
                if self.input_active {
                    if self.input_sel_all {
                        self.tabs[self.active].input_buf.clear();
                        self.input_sel_all = false;
                    }
                    self.tabs[self.active].input_buf.push_str(s);
                }
            }
        }
        ctx.notify();
    }
}

impl View for WarpBrowserView {
    fn ui_name() -> &'static str {
        "WarpBrowserView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Flex::column()
            .with_child(self.tab_strip())
            .with_child(self.toolbar())
            .with_child(self.body())
            .with_child(self.footer())
            .finish()
    }
}

impl Entity for WarpBrowserView {
    type Event = ();
}

fn short_title(tab: &BrowserTab) -> String {
    if !tab.title.is_empty() && tab.title != tab.url {
        return truncate(&tab.title, 18);
    }
    if tab.url.is_empty() {
        return "New Tab".into();
    }
    let s = tab
        .url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let s = s.split('/').next().unwrap_or(s);
    truncate(s, 18)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
