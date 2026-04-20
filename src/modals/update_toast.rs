use crate::state::App;

/// Candidate release-asset URLs for this build, in the order the
/// updater should try them. Arch-specific DMG first (matches how we
/// actually publish today), universal second (what cargo-bundle's
/// Makefile target names them when run with release-universal), and
/// a bare arm64 fallback for older builds. The first URL that returns
/// 200 wins.
fn dmg_urls_for(version: &str) -> Vec<String> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let base = format!(
        "https://github.com/rajpootathar/Crane/releases/download/v{version}/Crane-{version}"
    );
    vec![
        format!("{base}-{arch}.dmg"),
        format!("{base}-universal.dmg"),
    ]
}

fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.0} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

pub fn render(ctx: &egui::Context, app: &mut App) {
    if !app.update_check.should_show() {
        return;
    }
    let version = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.version.clone())
        .unwrap_or_default();
    let url = app
        .update_check
        .available
        .as_ref()
        .map(|u| u.url.clone())
        .unwrap_or_default();

    let theme = crate::theme::current();
    let screen = ctx.content_rect();
    let toast_w = 440.0_f32.min(screen.width() - 40.0);
    egui::Area::new(egui::Id::new("update_toast"))
        .order(egui::Order::Tooltip)
        .fixed_pos(egui::pos2(
            screen.max.x - toast_w - 20.0,
            screen.max.y - 140.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(theme.surface.to_color32())
                .stroke(egui::Stroke::new(1.0, theme.border.to_color32()))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(toast_w - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(egui_phosphor::regular::ARROW_CIRCLE_UP)
                                .size(18.0)
                                .color(theme.accent.to_color32()),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Crane v{version} is available"))
                                    .size(13.0)
                                    .color(theme.text.to_color32())
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "You're on v{}. Grab the new build?",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .size(11.5)
                                .color(theme.text_muted.to_color32()),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    let dmg_urls = dmg_urls_for(&version);
                    let supports_in_app = crate::update::apply::Updater::is_supported_platform()
                        && !dmg_urls.is_empty();
                    use crate::update::apply::UpdateState;
                    match app.updater.state() {
                        UpdateState::Downloading { bytes } => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  Downloading… {}",
                                    egui_phosphor::regular::DOWNLOAD_SIMPLE,
                                    human_bytes(bytes),
                                ))
                                .size(12.0)
                                .italics(),
                            );
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                        }
                        UpdateState::Installing => {
                            ui.label(
                                egui::RichText::new("Installing…")
                                    .size(12.0)
                                    .italics(),
                            );
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                        }
                        UpdateState::Ready { .. } => {
                            if ui
                                .button(
                                    egui::RichText::new(format!(
                                        "{}  Restart now",
                                        egui_phosphor::regular::ARROW_COUNTER_CLOCKWISE
                                    ))
                                    .size(12.0)
                                    .strong(),
                                )
                                .clicked()
                            {
                                app.updater.apply_and_exit();
                            }
                            if ui.button(egui::RichText::new("Later").size(12.0)).clicked() {
                                app.update_check.dismiss_session();
                            }
                        }
                        UpdateState::Failed(err) => {
                            ui.label(
                                egui::RichText::new(format!("Install failed: {err}"))
                                    .size(11.0)
                                    .color(theme.error.to_color32()),
                            );
                            if ui.button(egui::RichText::new("Open in browser").size(12.0)).clicked() {
                                let _ = webbrowser::open(&url);
                            }
                        }
                        UpdateState::Idle => {
                            ui.horizontal(|ui| {
                                if supports_in_app {
                                    if ui
                                        .button(
                                            egui::RichText::new(format!(
                                                "{}  Install update",
                                                egui_phosphor::regular::DOWNLOAD_SIMPLE
                                            ))
                                            .size(12.0)
                                            .strong(),
                                        )
                                        .clicked()
                                        && !dmg_urls.is_empty()
                                    {
                                        app.updater.start(dmg_urls.clone(), ctx.clone());
                                    }
                                } else if ui
                                    .button(
                                        egui::RichText::new(format!(
                                            "{}  Download",
                                            egui_phosphor::regular::DOWNLOAD_SIMPLE
                                        ))
                                        .size(12.0)
                                        .strong(),
                                    )
                                    .clicked()
                                {
                                    let _ = webbrowser::open(&url);
                                    app.update_check.dismiss_forever();
                                }
                                if ui.button(egui::RichText::new("Not now").size(12.0)).clicked() {
                                    app.update_check.dismiss_session();
                                }
                                if ui.button(egui::RichText::new("Remind in 7 days").size(12.0)).clicked() {
                                    app.update_check.remind_later();
                                }
                            });
                        }
                    }
                });
        });
}
