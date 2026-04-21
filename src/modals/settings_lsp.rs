//! Settings → Language Servers section. Extracted from
//! `settings.rs` to keep that file focused on chrome (sidebar,
//! appearance, shortcuts). Owns the LSP list rendering + per-server
//! row + status chip + PATH lookup helper.

use super::settings::section_title;
use crate::state::App;
use crate::theme;
use egui::RichText;

pub fn render(ui: &mut egui::Ui, app: &mut App) {
    section_title(ui, "Language Servers");
    ui.add_space(6.0);
    ui.label(
        RichText::new(
            "Crane speaks LSP to external servers for diagnostics, hover, and formatting. It prefers anything already on your PATH; otherwise you can download a vetted binary here."
        )
        .size(11.5)
        .color(theme::current().text_muted.to_color32()),
    );
    ui.add_space(6.0);

    egui::ScrollArea::vertical()
        .id_salt("lsp_list")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let path_val = std::env::var("PATH").unwrap_or_default();
            let short = if path_val.len() > 90 {
                format!("{}…", &path_val[..90])
            } else {
                path_val.clone()
            };
            ui.label(
                RichText::new(format!("PATH: {short}"))
                    .size(10.5)
                    .monospace()
                    .color(theme::current().text_muted.to_color32()),
            );
            ui.add_space(10.0);

            use crate::lsp::ServerKey as K;
            let all = [
                K::RustAnalyzer,
                K::TypeScript,
                K::Eslint,
                K::Gopls,
                K::Pyright,
                K::CssLs,
                K::HtmlLs,
            ];
            let statuses = app.lsp.statuses();

            for key in all {
                render_lsp_row(ui, app, key, &statuses);
                ui.add_space(8.0);
            }
        });
}

fn render_lsp_row(
    ui: &mut egui::Ui,
    app: &mut App,
    key: crate::lsp::ServerKey,
    statuses: &[(crate::lsp::ServerKey, crate::lsp::server::Status)],
) {
    use crate::lsp::DownloadState;
    let status = statuses
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, s)| *s);
    let (cmd, _) = key.command();
    let found = which_on_path(cmd);
    let dl_state = app.lsp.downloader.state(key);

    egui::Frame::default()
        .fill(theme::current().surface.to_color32())
        .stroke(egui::Stroke::new(1.0, theme::current().border.to_color32()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{key:?}"))
                        .size(13.0)
                        .color(theme::current().text.to_color32())
                        .strong(),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let (label, color) = status_chip(status, &dl_state, &found);
                        ui.label(RichText::new(label).size(11.0).color(color).strong());
                    },
                );
            });
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("$ {cmd}"))
                    .size(10.5)
                    .monospace()
                    .color(theme::current().text_muted.to_color32()),
            );
            match &found {
                Some(p) => {
                    ui.label(
                        RichText::new(format!("PATH → {p}"))
                            .size(10.5)
                            .monospace()
                            .color(theme::current().success.to_color32()),
                    );
                }
                None => {
                    ui.label(
                        RichText::new("PATH → not found")
                            .size(10.5)
                            .color(theme::current().text_muted.to_color32()),
                    );
                }
            }
            if let DownloadState::Ready(p) = &dl_state {
                ui.label(
                    RichText::new(format!("Crane → {}", p.display()))
                        .size(10.5)
                        .monospace()
                        .color(theme::current().success.to_color32()),
                );
            }

            // Per-language toggles — configure diagnostics behavior without
            // a binary rebuild. Persisted in session.
            ui.add_space(6.0);
            let mut cfg = app.language_configs.get_or_default(key);
            let mut changed = false;
            if ui
                .checkbox(&mut cfg.enabled, "Enable language server")
                .changed()
            {
                changed = true;
            }
            if matches!(key, crate::lsp::ServerKey::RustAnalyzer) {
                if ui
                    .add_enabled(
                        cfg.enabled,
                        egui::Checkbox::new(
                            &mut cfg.check_on_save,
                            "Run cargo check on save (real compile errors)",
                        ),
                    )
                    .changed()
                {
                    changed = true;
                }
            } else if matches!(key, crate::lsp::ServerKey::Gopls) {
                if ui
                    .add_enabled(
                        cfg.enabled,
                        egui::Checkbox::new(
                            &mut cfg.check_on_save,
                            "Run go vet / build on save",
                        ),
                    )
                    .changed()
                {
                    changed = true;
                }
            } else if ui
                .add_enabled(
                    cfg.enabled,
                    egui::Checkbox::new(
                        &mut cfg.check_on_save,
                        "Notify server on save",
                    ),
                )
                .changed()
            {
                changed = true;
            }
            let fmt_label = match key {
                crate::lsp::ServerKey::RustAnalyzer => "Format on save (rustfmt)",
                crate::lsp::ServerKey::TypeScript
                | crate::lsp::ServerKey::CssLs
                | crate::lsp::ServerKey::HtmlLs => "Format on save (prettier)",
                crate::lsp::ServerKey::Pyright => "Format on save (ruff)",
                crate::lsp::ServerKey::Gopls => "Format on save (gofmt)",
                crate::lsp::ServerKey::Eslint => "(ESLint fixes via Prettier)",
            };
            if ui
                .add_enabled(
                    cfg.enabled,
                    egui::Checkbox::new(&mut cfg.format_on_save, fmt_label),
                )
                .changed()
            {
                changed = true;
            }
            if changed {
                app.language_configs.set(key, cfg);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if crate::lsp::Downloader::is_supported(key) {
                    match dl_state.clone() {
                        DownloadState::Downloading { progress_bytes } => {
                            ui.label(
                                RichText::new(format!(
                                    "⬇ downloading… {}",
                                    crate::lsp::downloader::human_bytes(progress_bytes)
                                ))
                                .size(11.0)
                                .italics()
                                .color(theme::current().warning.to_color32()),
                            );
                        }
                        DownloadState::Ready(_) => {
                            ui.label(
                                RichText::new("✓ downloaded by Crane")
                                    .size(11.0)
                                    .color(theme::current().success.to_color32()),
                            );
                            if ui.small_button("Re-download").clicked() {
                                app.lsp
                                    .downloader
                                    .start_download(key, ui.ctx().clone());
                            }
                        }
                        DownloadState::Failed(e) => {
                            ui.label(
                                RichText::new(format!("✗ {e}"))
                                    .size(10.5)
                                    .color(theme::current().error.to_color32()),
                            );
                            if ui.small_button("Retry").clicked() {
                                app.lsp
                                    .downloader
                                    .start_download(key, ui.ctx().clone());
                            }
                        }
                        DownloadState::NotStarted => {
                            if ui
                                .button(
                                    RichText::new("⬇ Download & use Crane's copy").strong(),
                                )
                                .clicked()
                            {
                                app.lsp.declined.remove(&key);
                                app.lsp
                                    .downloader
                                    .start_download(key, ui.ctx().clone());
                            }
                        }
                    }
                } else if let Some(hint) = crate::lsp::Downloader::runtime_missing_hint(key) {
                    ui.label(
                        RichText::new(hint)
                            .size(11.0)
                            .italics()
                            .color(theme::current().warning.to_color32()),
                    );
                } else {
                    ui.label(
                        RichText::new(format!("install yourself: {}", key.install_hint()))
                            .size(10.5)
                            .monospace()
                            .color(theme::current().accent.to_color32()),
                    );
                }
            });
        });
}

fn status_chip(
    status: Option<crate::lsp::server::Status>,
    dl: &crate::lsp::DownloadState,
    found_on_path: &Option<String>,
) -> (String, egui::Color32) {
    use crate::lsp::server::Status;
    if let Some(s) = status {
        let (label, color) = match s {
            Status::Ready => ("ready", theme::current().success.to_color32()),
            Status::Initializing => ("initializing", theme::current().warning.to_color32()),
            Status::Spawned => ("starting", theme::current().warning.to_color32()),
            Status::Dead => ("dead", theme::current().error.to_color32()),
        };
        return (label.to_string(), color);
    }
    if matches!(dl, crate::lsp::DownloadState::Downloading { .. }) {
        return ("downloading".to_string(), theme::current().warning.to_color32());
    }
    if matches!(dl, crate::lsp::DownloadState::Ready(_)) || found_on_path.is_some() {
        return (
            "installed (not started)".to_string(),
            theme::current().text_muted.to_color32(),
        );
    }
    (
        "not installed".to_string(),
        theme::current().text_muted.to_color32(),
    )
}

fn which_on_path(bin: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let full = std::path::Path::new(dir).join(bin);
        if full.is_file() {
            return Some(full.to_string_lossy().to_string());
        }
    }
    None
}
