// Quick standalone smoke test for the pdfium binding path. Compile and
// run against the same vendor/pdfium dylib the app uses.
use pdfium_render::prelude::*;

fn main() {
    let dylib = std::env::var("PDFIUM_DYLIB")
        .unwrap_or_else(|_| {
            let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
            format!("vendor/pdfium/{arch}/libpdfium.dylib")
        });
    println!("binding to: {dylib}");
    let bindings = Pdfium::bind_to_library(&dylib).expect("bind_to_library");
    let pdfium = Pdfium::new(bindings);
    let path = std::env::args().nth(1).expect("pass a pdf path");
    let doc = pdfium.load_pdf_from_file(&path, None).expect("load_pdf_from_file");
    let pages = doc.pages();
    println!("opened {path}: {} pages", pages.len());
    let first = pages.get(0).expect("first page");
    println!("page 0 size pts: {}x{}", first.width().value, first.height().value);
    let cfg = PdfRenderConfig::new()
        .set_target_width(800)
        .set_target_height(1000);
    let bitmap = first.render_with_config(&cfg).expect("render");
    let img = bitmap.as_image().expect("as_image");
    let rgba = img.to_rgba8();
    println!("rendered {}x{} px", rgba.width(), rgba.height());

    // Text extract smoke
    if let Ok(text) = first.text() {
        let all_chars = text.chars();
        let preview: String = all_chars
            .iter()
            .filter_map(|c| c.unicode_char())
            .take(80)
            .collect();
        println!("first chars: {preview:?}");
    }
}
