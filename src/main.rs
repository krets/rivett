// Hide the console window on Windows in release builds.
// In debug builds we keep it so log output is visible.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod db;
mod formats;
mod image_loader;
mod session;
mod settings;
mod viewer;

use app::RivettApp;
use settings::AppSettings;
use std::path::PathBuf;

/// Minimal CLI argument parsing — no dep needed for this.
/// Usage:
///   rivett [image_path]
fn parse_args() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    // args[0] is the binary; args[1] (if present) is the image path
    args.get(1).map(PathBuf::from)
}

fn main() -> eframe::Result<()> {
    // Initialise logging.  RUST_LOG=debug rivett will show all log output.
    env_logger::init();

    let initial_image = parse_args();
    let settings = AppSettings::load();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([400.0, 300.0])
            .with_drag_and_drop(true)
            .with_title("Rivett"),
        ..Default::default()
    };

    eframe::run_native(
        "Rivett",
        native_options,
        Box::new(move |cc| Ok(Box::new(RivettApp::new(cc, settings, initial_image)))),
    )
}
