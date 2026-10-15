#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("This application targets Windows 10/11 (x86_64-pc-windows-msvc).");

mod app;
mod denoise;
mod devctl;
mod dsp;
mod engine;
mod params;
mod resample;
mod ring;
mod settings;
mod shared;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AT2020USB-X Control Panel")
            .with_inner_size([1180.0, 860.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AT2020USB-X Control Panel",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
