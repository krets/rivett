// On Windows this is a GUI application — suppress the console window entirely.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use rivett::app::RivettApp;
use rivett::settings::AppSettings;
use std::path::PathBuf;

fn parse_args() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.get(1).map(PathBuf::from)
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    let initial_image = parse_args();
    let settings = AppSettings::load();

    // Load icon
    let icon_data = include_bytes!("../resources/icon.png");
    let icon = image::load_from_memory(icon_data)
        .expect("Failed to load embedded icon")
        .to_rgba8();
    let (width, height) = icon.dimensions();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([400.0, 300.0])
            .with_drag_and_drop(true)
            .with_title("Rivett")
            .with_icon(egui::IconData {
                rgba: icon.into_raw(),
                width,
                height,
            }),
        ..Default::default()
    };

    eframe::run_native(
        "Rivett",
        native_options,
        Box::new(move |cc| Ok(Box::new(RivettApp::new(cc, settings, initial_image)))),
    )
}
