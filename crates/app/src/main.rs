#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("This application targets Windows 10/11 (x86_64-pc-windows-msvc).");

mod app;
mod cli;
mod fonts;
mod icon;
mod integration;
mod logging;
mod selftest;
mod settings;
mod tabs;
mod theme;
mod widgets;

use eframe::egui;

fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: no pointers; failure (no parent console) is harmless.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--embed-icon") {
        // Packaging helper: --embed-icon <exe> <ico>
        let (Some(exe), Some(ico)) = (args.get(pos + 1), args.get(pos + 2)) else {
            eprintln!("usage: --embed-icon <exe> <ico>");
            std::process::exit(2);
        };
        return match device::winsys::embed_icon(std::path::Path::new(exe), std::path::Path::new(ico)) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("embed-icon failed: {e}");
                std::process::exit(2);
            }
        };
    }
    if let Some(path) = arg_value(&args, "--write-icon") {
        if path.ends_with(".png") {
            if let Some(img) = image::RgbaImage::from_raw(256, 256, icon::rgba(256)) {
                let _ = img.save(&path);
            }
            return Ok(());
        }
        let bytes = icon::ico_bytes(&[16, 24, 32, 48, 64, 128, 256]);
        return match std::fs::write(&path, bytes) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("could not write {path}: {e}");
                std::process::exit(2);
            }
        };
    }
    if args.iter().any(|a| a == "--selftest") {
        attach_console();
        let opts = selftest::Options::from_args(&args);
        std::process::exit(selftest::run(&opts));
    }
    if args
        .iter()
        .any(|a| a == "--process" || a == "--list-presets")
    {
        attach_console();
        std::process::exit(cli::run(&args));
    }

    let _log_guard = logging::init();
    let opts = app::LaunchOptions {
        minimized: args.iter().any(|a| a == "--minimized"),
        screenshots: arg_value(&args, "--screenshots").map(std::path::PathBuf::from),
    };
    let icon = egui::IconData {
        rgba: icon::rgba(64),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AT2020USB-X Control Panel")
            .with_icon(icon)
            .with_inner_size([1180.0, 860.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AT2020USB-X Control Panel",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, opts)))),
    )
}
