//! Bottom-of-Files-pane status strip + scrollbar diagnostic markers.
//!
//! Extracted from `file_view.rs`. The status strip shows severity pill
//! counts (click to jump), the cursor's Ln/Col stashed off the
//! TextEdit output, the indent preference for this file's project,
//! and a language label. The scrollbar minimap is the matching strip
//! on the right edge of the editor.

use crate::views::file_util::char_idx_to_line_col;
use crate::lsp::Diagnostic;
use crate::theme;
use egui::{Color32, RichText};
use egui_phosphor::regular as icons;
use std::path::Path;

pub fn render_status_strip(
    ui: &mut egui::Ui,
    tab: &mut crate::state::layout::FileTab,
    diagnostics: &[Diagnostic],
    height: f32,
) {
    let t = theme::current();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.min.y),
            egui::pos2(rect.max.x, rect.min.y),
        ],
        egui::Stroke::new(1.0, t.divider.to_color32()),
    );

    let (e, w, i) = count_by_severity(diagnostics);

    // Read the cursor stashed off the TextEdit output during editor
    // render this same frame. Clamp to content length so a stale
    // cursor from a larger prior content doesn't show as Ln/Col past
    // the end after the file shrinks.
    let cursor_idx = tab.last_cursor_idx.min(tab.content.chars().count());
    let (cur_line, cur_col) = char_idx_to_line_col(&tab.content, cursor_idx);

    let style = crate::format::discover(Path::new(&tab.path));
    let indent_label = if style.use_tabs {
        "Tabs".to_string()
    } else {
        format!("Spaces: {}", style.tab_width)
    };
    let lang = language_label(&tab.path);

    let mut clicked_sev: Option<u8> = None;
    let pills: [(&str, usize, Color32, u8); 3] = [
        (icons::X_CIRCLE, e, t.error.to_color32(), 1),
        (icons::WARNING, w, Color32::from_rgb(226, 192, 80), 2),
        (icons::INFO, i, t.accent.to_color32(), 3),
    ];
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.horizontal_centered(|ui| {
            for (icon, count, active_color, sev) in pills {
                ui.add_space(8.0);
                if sev_button(ui, icon, count, active_color, t.text_muted.to_color32()) {
                    clicked_sev = Some(sev);
                }
            }

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.add_space(10.0);
                    if tab.read_only {
                        ui.label(
                            RichText::new(format!("{}  Read Only", icons::LOCK))
                                .size(11.0)
                                .color(t.text_muted.to_color32()),
                        );
                        ui.add_space(12.0);
                    }
                    ui.label(
                        RichText::new(lang)
                            .size(11.0)
                            .color(t.text_muted.to_color32())
                            .monospace(),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(indent_label)
                            .size(11.0)
                            .color(t.text_muted.to_color32()),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!("Ln {}, Col {}", cur_line + 1, cur_col + 1))
                            .size(11.0)
                            .color(t.text_muted.to_color32()),
                    );
                    if let Some((chars, lines)) = tab.selection_info {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(format!("({chars} sel, {lines} ln)"))
                                .size(10.5)
                                .color(t.text_muted.to_color32()),
                        );
                    }
                },
            );
        });
    });

    if let Some(sev) = clicked_sev {
        jump_to_next_diagnostic(tab, diagnostics, sev, cur_line);
    }
}

fn sev_button(
    ui: &mut egui::Ui,
    icon: &str,
    count: usize,
    active_color: Color32,
    muted_color: Color32,
) -> bool {
    let color = if count > 0 { active_color } else { muted_color };
    let resp = ui.add(
        egui::Label::new(
            RichText::new(format!("{icon}  {count}"))
                .size(11.5)
                .color(color),
        )
        .sense(egui::Sense::click()),
    );
    if resp.hovered() && count > 0 {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked() && count > 0
}

fn count_by_severity(diags: &[Diagnostic]) -> (usize, usize, usize) {
    let mut e = 0;
    let mut w = 0;
    let mut i = 0;
    for d in diags {
        match d.severity {
            1 => e += 1,
            2 => w += 1,
            _ => i += 1,
        }
    }
    (e, w, i)
}

fn jump_to_next_diagnostic(
    tab: &mut crate::state::layout::FileTab,
    diags: &[Diagnostic],
    sev: u8,
    from_line: u32,
) {
    let matching: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| match sev {
            1 => d.severity == 1,
            2 => d.severity == 2,
            _ => d.severity == 0 || d.severity >= 3,
        })
        .collect();
    if matching.is_empty() {
        return;
    }
    let next = matching
        .iter()
        .find(|d| d.line > from_line)
        .or_else(|| matching.first());
    if let Some(d) = next {
        tab.pending_cursor = Some((d.line, d.col_start));
    }
}

fn language_label(path: &str) -> String {
    // Extensionless files with a well-known basename (Dockerfile, Makefile, …).
    if let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) {
        match name.to_ascii_lowercase().as_str() {
            "dockerfile" | "containerfile" => return "Dockerfile".to_string(),
            "makefile" | "gnumakefile" => return "Makefile".to_string(),
            "cmakelists.txt" => return "CMake".to_string(),
            "jenkinsfile" => return "Groovy".to_string(),
            "rakefile" | "gemfile" | "podfile" => return "Ruby".to_string(),
            "cargo.lock" => return "TOML".to_string(),
            _ => {}
        }
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "Rust",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "py" | "pyi" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" | "sc" => "Scala",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "C++",
        "cs" => "C#",
        "swift" => "Swift",
        "m" | "mm" => "Objective-C",
        "rb" => "Ruby",
        "php" => "PHP",
        "lua" => "Lua",
        "pl" | "pm" => "Perl",
        "r" => "R",
        "dart" => "Dart",
        "zig" => "Zig",
        "nim" => "Nim",
        "hs" => "Haskell",
        "elm" => "Elm",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "clj" | "cljs" | "cljc" | "edn" => "Clojure",
        "ml" | "mli" => "OCaml",
        "fs" | "fsx" | "fsi" => "F#",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "sql" => "SQL",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "less" => "Less",
        "html" | "htm" => "HTML",
        "xml" | "xsl" | "xslt" => "XML",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "md" | "markdown" | "mdx" => "Markdown",
        "json" | "jsonc" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "ini" | "cfg" | "conf" => "INI",
        "env" => "Dotenv",
        "dockerfile" => "Dockerfile",
        "makefile" | "mk" => "Makefile",
        "graphql" | "gql" => "GraphQL",
        "proto" => "Protobuf",
        "tf" | "tfvars" => "Terraform",
        "nix" => "Nix",
        "prisma" => "Prisma",
        "tex" | "latex" => "LaTeX",
        "vim" => "Vim",
        "diff" | "patch" => "Diff",
        "log" => "Log",
        "txt" => "Plain",
        "" => "Plain",
        other => return other.to_string(),
    }
    .to_string()
}

/// Colored tick strip on the right edge of the scroll viewport, one
/// per diagnostic, proportional to line. Minimap-lite matched to the
/// diff view so both panes look like the same component family.
pub fn paint_scrollbar_diag_markers(
    ui: &egui::Ui,
    scroll_rect: egui::Rect,
    total_lines: usize,
    diagnostics: &[Diagnostic],
) {
    if diagnostics.is_empty() || total_lines == 0 {
        return;
    }
    let t = theme::current();
    let painter = ui.painter_at(scroll_rect);
    // No backdrop strip here — files pane is edit surface, a full
    // translucent band reads as padding and competes with egui's
    // scrollbar gutter. Just paint the marker dashes, pinned just
    // inside the right edge so they sit alongside the scrollbar.
    let x1 = scroll_rect.max.x - 2.0;
    let x0 = x1 - 6.0;
    let h = scroll_rect.height();
    let total = total_lines.max(1) as f32;
    for d in diagnostics {
        let color = match d.severity {
            1 => t.error.to_color32(),
            2 => Color32::from_rgb(226, 192, 80),
            _ => t.accent.to_color32(),
        };
        let y = scroll_rect.min.y + (d.line as f32 / total) * h;
        let rect = egui::Rect::from_min_max(
            egui::pos2(x0, y - 2.0),
            egui::pos2(x1, y + 2.0),
        );
        painter.rect_filled(rect, 1.0, color);
    }
}
