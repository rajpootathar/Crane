//! Browser pane with per-pane tabs. Each tab owns its own native
//! WKWebView (managed by `browser::BrowserHost`); switching tabs just
//! hides the previous webview and shows the next one, so page state
//! (forms, scroll, auth) survives tab switches. The egui-side shell
//! here only draws the tab strip + URL toolbar + reserves a rect for
//! the webview.

use crate::state::layout::{BrowserPane, PaneId};
use crate::theme;
use egui::RichText;
use egui_phosphor::regular as icons;

pub fn render(
    ui: &mut egui::Ui,
    pane_id: PaneId,
    pane: &mut BrowserPane,
    title: &mut String,
    // True when the pane is currently a drag-drop target. The native
    // WKWebView sits above egui's GPU surface in the OS compositor, so
    // the blue drop overlay painted by pane_view would render beneath
    // the webview. Reporting the active tab as inactive hides the
    // webview for the frame without destroying it (page state is kept).
    native_hidden: bool,
    // True when this pane is the focused leaf of its layout. When set
    // (and the webview is visible), we report the active tab's slot
    // to `browser::report_focused_pane` so Cmd+C/V/X/A routes to the
    // embedded WKWebView via mac_keys's NSEvent monitor.
    is_focus: bool,
) {
    // Tab strip (always visible). Left-to-right chips + a trailing `+`.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut to_activate: Option<usize> = None;
        let mut to_close: Option<usize> = None;
        for (idx, tab) in pane.tabs.iter().enumerate() {
            let is_active = idx == pane.active;
            let label = short_title(tab);
            let chip = egui::Frame::default()
                .fill(if is_active {
                    theme::current().surface.to_color32()
                } else {
                    theme::current().topbar_bg.to_color32()
                })
                .stroke(egui::Stroke::new(
                    1.0,
                    if is_active {
                        theme::current().focus_border.to_color32()
                    } else {
                        theme::current().inactive_border.to_color32()
                    },
                ))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        #[cfg(target_os = "macos")]
                        {
                            let key = composite_id(pane_id, tab.id);
                            if crate::browser::is_loading(key) {
                                // Rotating-arrow spinner — egui has no
                                // native one; we animate with time-based
                                // rotation of the refresh glyph.
                                let t = ui.ctx().input(|i| i.time);
                                let angle = (t * 3.0) as f32 % std::f32::consts::TAU;
                                let galley = ui.painter().layout_no_wrap(
                                    icons::ARROW_CLOCKWISE.to_string(),
                                    egui::FontId::new(
                                        11.0,
                                        egui::FontFamily::Proportional,
                                    ),
                                    theme::current().accent.to_color32(),
                                );
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(14.0, 14.0),
                                    egui::Sense::hover(),
                                );
                                let center = rect.center();
                                ui.painter().add(
                                    egui::Shape::galley_with_override_text_color(
                                        center - galley.size() / 2.0,
                                        galley,
                                        theme::current().accent.to_color32(),
                                    ),
                                );
                                // egui has no rotation primitive on galley
                                // yet; fall back to the static glyph — the
                                // colour-change to accent is enough signal.
                                let _ = angle;
                            }
                        }
                        let label_resp = ui.add(
                            egui::Label::new(
                                RichText::new(label)
                                    .size(11.5)
                                    .color(theme::current().text.to_color32()),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if label_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if label_resp.clicked() {
                            to_activate = Some(idx);
                        }
                        if pane.tabs.len() > 1 {
                            let x = ui.add(
                                egui::Button::new(RichText::new(icons::X).size(10.0))
                                    .frame(false)
                                    .min_size(egui::vec2(14.0, 14.0)),
                            );
                            if x.clicked() {
                                to_close = Some(idx);
                            }
                        }
                    });
                });
            let _ = chip;
        }
        if ui
            .add(
                egui::Button::new(RichText::new(icons::PLUS).size(12.0))
                    .frame(false)
                    .min_size(egui::vec2(22.0, 22.0)),
            )
            .on_hover_text("New tab")
            .clicked()
        {
            pane.new_tab();
        }
        if let Some(idx) = to_activate {
            pane.active = idx;
        }
        if let Some(idx) = to_close {
            #[cfg(target_os = "macos")]
            if let Some(removed_tab_id) = pane.tabs.get(idx).map(|t| t.id) {
                crate::browser::queue_action(
                    composite_id(pane_id, removed_tab_id),
                    crate::browser::Action::Close,
                );
            }
            pane.close_tab(idx);
        }
    });

    let Some(tab) = pane.active_tab_mut() else {
        return;
    };
    let tab_id = tab.id;

    // URL toolbar for the active tab.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let btn = |ui: &mut egui::Ui, glyph: &'static str, tip: &str| {
            ui.add(
                egui::Button::new(RichText::new(glyph).size(13.0))
                    .frame(false)
                    .min_size(egui::vec2(24.0, 22.0)),
            )
            .on_hover_text(tip)
            .clicked()
        };
        #[cfg(target_os = "macos")]
        {
            let key = composite_id(pane_id, tab_id);
            if btn(ui, icons::ARROW_LEFT, "Back") {
                crate::browser::queue_action(key, crate::browser::Action::Back);
            }
            if btn(ui, icons::ARROW_RIGHT, "Forward") {
                crate::browser::queue_action(key, crate::browser::Action::Forward);
            }
            if btn(ui, icons::ARROW_CLOCKWISE, "Reload") {
                crate::browser::queue_action(key, crate::browser::Action::Reload);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = btn;
        }
        let desired = ui.available_width() - 90.0;
        let resp = ui.add(
            egui::TextEdit::singleline(&mut tab.input_buf)
                .hint_text("https://…")
                .desired_width(desired.max(80.0)),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let go = ui.button("Go").clicked() || enter;
        if go {
            let url = normalize_url(tab.input_buf.trim());
            if !url.is_empty() {
                tab.url = url.clone();
                tab.title = url.clone();
                *title = url.clone();
                #[cfg(target_os = "macos")]
                crate::browser::queue_action(
                    composite_id(pane_id, tab_id),
                    crate::browser::Action::Load(url),
                );
            }
        }
        if ui
            .button(icons::ARROW_SQUARE_OUT)
            .on_hover_text("Open in system browser")
            .clicked()
            && !tab.url.is_empty()
        {
            let _ = webbrowser::open(&tab.url);
        }
    });

    ui.add_space(2.0);

    // Reserve a thin footer for the WebKit memory status bar. Shrink
    // the webview rect so it doesn't render over the status line.
    const FOOTER_H: f32 = 22.0;
    let full = ui.available_rect_before_wrap();
    let rect = egui::Rect::from_min_max(
        full.min,
        egui::pos2(full.max.x, full.max.y - FOOTER_H),
    );
    let footer_rect = egui::Rect::from_min_max(
        egui::pos2(full.min.x, full.max.y - FOOTER_H),
        full.max,
    );
    ui.allocate_rect(rect, egui::Sense::hover());

    #[cfg(target_os = "macos")]
    {
        let inner = rect.shrink(1.0);
        // Report only the active tab's rect — that's the one whose
        // webview should be visible. All other tabs' webviews were
        // already reported as "alive" via report_inactive below so
        // they're retained but hidden.
        if native_hidden {
            crate::browser::report_inactive(composite_id(pane_id, tab_id), &tab.url);
        } else {
            crate::browser::report_pane(composite_id(pane_id, tab_id), inner, &tab.url);
            // Only route clipboard to the webview when the pane is
            // both focused AND visible. A focused-but-hidden pane
            // (e.g. behind a modal) should let its Cmd+C/V/X/A fall
            // through to the overlay's egui TextEdit instead.
            if is_focus {
                crate::browser::report_focused_pane(composite_id(pane_id, tab_id));
            }
        }
        ui.painter().rect_filled(
            rect,
            0.0,
            theme::current().surface.to_color32(),
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        ui.label(
            RichText::new("Embedded webview is available on macOS only.")
                .italics()
                .color(egui::Color32::from_rgb(150, 150, 160)),
        );
    }

    // Keep every other tab's webview alive (reported, but hidden). We
    // do this AFTER the active tab so it takes precedence for focus.
    #[cfg(target_os = "macos")]
    for (idx, t) in pane.tabs.iter().enumerate() {
        if idx == pane.active {
            continue;
        }
        crate::browser::report_inactive(
            composite_id(pane_id, t.id),
            &t.url,
        );
    }

    // Footer status bar — WebKit memory + tab count. Drawn last so it
    // sits above the webview's reserved rect.
    #[cfg(target_os = "macos")]
    {
        ui.painter().rect_filled(
            footer_rect,
            0.0,
            theme::current().topbar_bg.to_color32(),
        );
        ui.painter().line_segment(
            [
                footer_rect.left_top(),
                footer_rect.right_top(),
            ],
            egui::Stroke::new(1.0, theme::current().divider.to_color32()),
        );
        let snap = crate::browser::memory_snapshot();
        let tab_count = pane.tabs.len();
        let (mem_color, mem_label) = if snap.total_bytes == 0 {
            (
                theme::current().text_muted.to_color32(),
                "—".to_string(),
            )
        } else if snap.total_bytes >= crate::browser::memory::DANGER_BYTES {
            (
                theme::current().error.to_color32(),
                format!(
                    "{} (heavy — close tabs)",
                    crate::browser::memory::human_bytes(snap.total_bytes)
                ),
            )
        } else if snap.total_bytes >= crate::browser::memory::WARN_BYTES {
            (
                theme::current().warning.to_color32(),
                crate::browser::memory::human_bytes(snap.total_bytes),
            )
        } else {
            (
                theme::current().text_muted.to_color32(),
                crate::browser::memory::human_bytes(snap.total_bytes),
            )
        };
        let proc_suffix = if snap.process_count == 1 { "" } else { "es" };
        // Left: tab count.
        ui.painter().text(
            egui::pos2(footer_rect.min.x + 10.0, footer_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{tab_count} tab{}", if tab_count == 1 { "" } else { "s" }),
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
            theme::current().text_muted.to_color32(),
        );
        // Right: memory + process count.
        let right_label = if snap.total_bytes == 0 {
            "WebKit memory: —".to_string()
        } else {
            format!(
                "WebKit: {}  ·  {} process{proc_suffix}",
                mem_label, snap.process_count
            )
        };
        ui.painter().text(
            egui::pos2(footer_rect.max.x - 10.0, footer_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            right_label,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
            mem_color,
        );
        // Hitbox covering the right label for a hover tooltip.
        let hover_resp = ui.interact(
            footer_rect,
            egui::Id::new(("browser_footer_mem", pane_id)),
            egui::Sense::hover(),
        );
        if snap.total_bytes > 0 {
            hover_resp.on_hover_text(format!(
                "WebKit is using {} across {} process{proc_suffix}.\n\
                 Sum is for ALL Browser panes & tabs in Crane — wry\n\
                 doesn't expose per-tab attribution. Close tabs to free\n\
                 memory.",
                crate::browser::memory::human_bytes(snap.total_bytes),
                snap.process_count,
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = footer_rect;
        let _ = pane;
    }
}

#[cfg(target_os = "macos")]
fn composite_id(pane_id: PaneId, tab_id: u32) -> crate::browser::SlotKey {
    (pane_id, tab_id)
}

fn short_title(tab: &crate::state::layout::BrowserTab) -> String {
    if !tab.title.is_empty() && tab.title != tab.url {
        return truncate(&tab.title, 18);
    }
    if tab.url.is_empty() {
        return "New Tab".into();
    }
    let s = tab.url.trim_start_matches("https://").trim_start_matches("http://");
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

fn normalize_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("about:") {
        return raw.to_string();
    }
    // Loopback / RFC1918 private addresses always use http. A dev
    // server on :3000 has no TLS cert, so https would bounce off the
    // handshake and we'd route the user to an error page instead of
    // their app. Also short-circuits the search branch below, which
    // used to eat `localhost:3000` because it contains neither `.`
    // nor `/`.
    if is_local_host(raw) {
        return format!("http://{raw}");
    }
    // Any `host:port` with a numeric port other than 443 — treat as
    // http too. Public dev tunnels and self-hosted services commonly
    // live on a non-443 port without TLS.
    if let Some((_head, tail)) = raw.split_once(':') {
        let port_str: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(port) = port_str.parse::<u16>()
            && port != 443
        {
            return format!("http://{raw}");
        }
    }
    if !raw.contains('.') && !raw.contains('/') {
        return format!("https://duckduckgo.com/?q={}", urlencode(raw));
    }
    format!("https://{raw}")
}

/// Loopback or RFC1918 private-LAN host? Accepts raw input that may
/// carry a trailing `:port` or `/path`, so we split off just the host
/// segment before matching. Keeps the port/path attached to the URL
/// when the caller prefixes `http://`.
fn is_local_host(s: &str) -> bool {
    let host_end = s.find(|c: char| c == ':' || c == '/').unwrap_or(s.len());
    let host = &s[..host_end];
    if matches!(host, "localhost" | "0.0.0.0" | "[::1]" | "[::]") {
        return true;
    }
    if host.starts_with("127.") || host.starts_with("192.168.") || host.starts_with("10.") {
        return true;
    }
    // 172.16.0.0 – 172.31.255.255 (RFC1918 middle block).
    if let Some(rest) = host.strip_prefix("172.") {
        let octet: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = octet.parse::<u8>()
            && (16..=31).contains(&n)
        {
            return true;
        }
    }
    false
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
