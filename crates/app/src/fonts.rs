//! Bundled fonts: Inter (UI text) and JetBrains Mono (numbers, code). Both are SIL OFL
//! 1.1; the licence texts ship in `assets/fonts` and are listed in the README.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("Inter".into(), FontData::from_static(INTER));
    fonts
        .font_data
        .insert("JetBrainsMono".into(), FontData::from_static(MONO));
    // Bundled faces first; egui's defaults stay as fallback for symbols and emoji.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono".into());
    ctx.set_fonts(fonts);
}
