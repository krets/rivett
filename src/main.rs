// On Windows this is a GUI application — suppress the console window entirely.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod db;
mod formats;
mod image_loader;
mod metadata;
mod session;
mod settings;
mod viewer;

use app::RivettApp;
use settings::AppSettings;
use std::path::PathBuf;

fn parse_args() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.get(1).map(PathBuf::from)
}

fn main() -> eframe::Result<()> {
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