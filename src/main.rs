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
mod selftest;
mod settings;
mod shared;
mod widgets;

use eframe::egui;

fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        attach_console();
        let opts = selftest::Options::from_args(&args);
        std::process::exit(selftest::run(&opts));
    }
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
