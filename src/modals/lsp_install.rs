use crate::lsp::{DownloadState, ServerKey};
use crate::state::App;
use crate::theme;
use egui::RichText;

/// Opt-in prompt shown when the user opens a file whose LSP isn't on PATH.
/// "Install" triggers an auto-download into `~/.crane/lsp/<name>/`, "Not now"
/// suppresses the prompt for this session.
pub fn render(ctx: &egui::Context, app: &mut App) {
    let Some(key) = app.lsp.prompt_install else {
        return;
    };

    let info = describe(key);
    let mut accept = false;
    let mut decline = false;

    egui::Window::new("Install language server?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size(egui::vec2(440.0, 240.0))
        .show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Crane can't find {} on your PATH — it's needed for {} diagnostics and (later) hover + formatting.",
                    info.bin, info.lang
                ))
                .size(12.5)
                .color(theme::current().text.to_color32()),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new(info.install_blurb)
                    .size(12.0)
                    .color(theme::current().text_muted.to_color32()),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("Download & use").strong())
                    .clicked()
                {
                    accept = true;
                }
                if ui.button("Not now").clicked() {
                    decline = true;
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new("Won't prompt again this session. Re-enable from Settings > Language Servers.")
                    .size(10.5)
                    .italics()
                    .color(theme::current().text_muted.to_color32()),
            );
        });

    if accept {
        app.lsp.accept_install(ctx);
    } else if decline {
        app.lsp.decline_install();
    }
}

struct Info {
    lang: &'static str,
    bin: &'static str,
    install_blurb: &'static str,
}

fn describe(key: ServerKey) -> Info {
    match key {
        ServerKey::RustAnalyzer => Info {
            lang: "Rust",
            bin: "rust-analyzer",
            install_blurb: "Download the official prebuilt binary (~15 MB) from rust-lang/rust-analyzer into ~/.crane/lsp/ ?",
        },
        ServerKey::TypeScript => Info {
            lang: "TypeScript / JavaScript",
            bin: "typescript-language-server",
            install_blurb: "Install via npm (typescript + typescript-language-server) into ~/.crane/lsp/ ?",
        },
        ServerKey::Gopls => Info {
            lang: "Go",
            bin: "gopls",
            install_blurb: "Install the Go toolchain first (go.dev/dl), then `go install golang.org/x/tools/gopls@latest`.",
        },
        ServerKey::Pyright => Info {
            lang: "Python",
            bin: "pyright-langserver",
            install_blurb: "Install via npm (pyright) into ~/.crane/lsp/ ?",
        },
        ServerKey::CssLs => Info {
            lang: "CSS",
            bin: "vscode-css-language-server",
            install_blurb: "Install via npm (vscode-langservers-extracted) into ~/.crane/lsp/ ?",
        },
        ServerKey::HtmlLs => Info {
            lang: "HTML",
            bin: "vscode-html-language-server",
            install_blurb: "Install via npm (vscode-langservers-extracted) into ~/.crane/lsp/ ?",
        },
    }
}

pub fn render_download_toast(ctx: &egui::Context, app: &App) {
    use ServerKey as K;
    for key in [
        K::RustAnalyzer,
        K::TypeScript,
        K::Gopls,
        K::Pyright,
        K::CssLs,
        K::HtmlLs,
    ] {
        if let DownloadState::Downloading { progress_bytes } = app.lsp.downloader.state(key) {
            let screen = ctx.content_rect();
            egui::Area::new(egui::Id::new(("lsp_dl_toast", key as u32)))
                .order(egui::Order::Tooltip)
                .fixed_pos(egui::pos2(screen.max.x - 280.0, screen.min.y + 60.0))
                .show(ctx, |ui| {
                    egui::Frame::default()
                        .fill(theme::current().surface.to_color32())
                        .stroke(egui::Stroke::new(
                            1.0,
                            theme::current().border.to_color32(),
                        ))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(248.0);
                            ui.label(
                                RichText::new(format!("Downloading {key:?}…"))
                                    .size(12.0)
                                    .strong()
                                    .color(theme::current().text.to_color32()),
                            );
                            ui.label(
                                RichText::new(crate::lsp::downloader::human_bytes(
                                    progress_bytes,
                                ))
                                .size(11.0)
                                .color(theme::current().text_muted.to_color32()),
                            );
                        });
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}
