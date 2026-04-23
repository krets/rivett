//! Top-level application state and the [`eframe::App`] implementation.

use eframe::CreationContext;
use egui::{CentralPanel, Context, Key, Vec2};
use std::time::{Duration, Instant};

use crate::db::{Database, ImageRecord};
use crate::image_loader::{load_image, ImageCache, DirectoryListing};
use crate::metadata::{read_metadata, MetaEntry};
use crate::session::{SessionState, RatingFilter, RatingFilterOp};
use crate::settings::AppSettings;
use crate::viewer::ViewerState;

#[cfg(windows)]
extern crate windows_core;

#[cfg(windows)]
mod win_drag {
    pub use windows::Win32::Foundation::*;
    pub use windows::Win32::System::Com::*;
    pub use windows::Win32::System::Memory::*;
    pub use windows::Win32::System::Ole::*;
    pub use windows::Win32::UI::Shell::*;

    // Mouse button constants for drag and drop state
    pub const MK_LBUTTON: u32 = 0x0001;
    pub const MK_RBUTTON: u32 = 0x0002;
}

// ---------------------------------------------------------------------------
// RivettApp
// ---------------------------------------------------------------------------

pub struct RivettApp {
    db:              Option<Database>,
    viewer:          ViewerState,
    image_cache:     ImageCache,
    listing:         Option<DirectoryListing>,
    session:         SessionState,
    
    // UI state
    current_path:    Option<std::path::PathBuf>,
    current_record:  Option<ImageRecord>,
    metadata:        Vec<MetaEntry>,
    show_info_panel: bool,
    toast:           Option<Toast>,
    delete_confirm:  Option<DeleteConfirm>,
    
    #[allow(dead_code)]
    settings:        AppSettings,
}

impl RivettApp {
    pub fn new(cc: &CreationContext<'_>, settings: AppSettings, initial_image: Option<std::path::PathBuf>) -> Self {
        // Platform-specific styling
        let mut visuals = egui::Visuals::dark();
        visuals.window_rounding = 0.0.into();
        cc.egui_ctx.set_visuals(visuals);

        let db_path = settings.central_db_resolved().unwrap_or_else(|| std::path::PathBuf::from("ratings.db"));
        let db = Database::open(&db_path).map_err(|e| {
            log::error!("failed to open database at {}: {e}", db_path.display());
            e
        }).ok();

        let mut app = Self {
            db,
            viewer:          ViewerState::new(),
            image_cache:     ImageCache::new(32), // cache 32 decoded images
            listing:         None,
            session:         SessionState::new(settings.default_sort),
            current_path:    None,
            current_record:  None,
            metadata:        vec![],
            show_info_panel: settings.show_info_panel,
            toast:           None,
            delete_confirm:  None,
            settings,
        };

        if let Some(path) = initial_image {
            app.open_image(path, &cc.egui_ctx);
        }

        app
    }

    // ── Toast helper ──────────────────────────────────────────────────────

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast::new(msg.into()));
    }

    // ── Opening / Loading ─────────────────────────────────────────────────

    pub fn open_image(&mut self, path: std::path::PathBuf, ctx: &Context) {
        if !path.exists() { return; }
        
        if path.is_file() {
            if let Some(dir) = path.parent() {
                let sort   = self.session_sort_order();
                let db     = self.db.as_ref();
                match DirectoryListing::scan(dir, sort, None, db) {
                    Ok(mut listing) => {
                        listing.seek_to(&path);
                        self.listing = Some(listing);
                    }
                    Err(e) => log::warn!("failed to scan directory: {e}"),
                }
            }
        }
        self.load_current(ctx, false);
    }

    fn load_current(&mut self, ctx: &Context, preserve_zoom: bool) {
        let path = match self.listing.as_ref().and_then(|l| l.current().cloned()) {
            Some(p) => p,
            None => {
                self.viewer.clear();
                self.current_path   = None;
                self.current_record = None;
                self.metadata       = vec![];
                return;
            }
        };

        self.current_path = Some(path.clone());
        
        self.refresh_record();

        let rotation = self.current_record.as_ref()
            .map(|r| crate::session::Rotation::from_u8(r.rotation))
            .unwrap_or_default();

        if let Some(img) = self.image_cache.get(&path) {
            self.viewer.load_image(ctx, img, rotation, preserve_zoom);
        } else {
            match load_image(&path) {
                Ok(img) => {
                    self.image_cache.insert(path.clone(), img.clone());
                    self.viewer.load_image(ctx, &img, rotation, preserve_zoom);
                }
                Err(e)  => {
                    log::warn!("{e}");
                    self.viewer.set_error(e);
                }
            }
        }

        if let Some(ref listing) = self.listing {
            // Next
            let mut i = listing.current_index + 1;
            while i < listing.files.len() {
                let p = &listing.files[i];
                if !self.session_is_ignored(p) {
                    self.image_cache.prefetch(p.clone());
                    break;
                }
                i += 1;
            }
            // Prev
            let mut i = listing.current_index as i32 - 1;
            while i >= 0 {
                let p = &listing.files[i as usize];
                if !self.session_is_ignored(p) {
                    self.image_cache.prefetch(p.clone());
                    break;
                }
                i -= 1;
            }
        }

        self.metadata = read_metadata(&path);
    }

    fn refresh_record(&mut self) {
        self.current_record = self.current_path.as_ref().and_then(|path| {
            let db      = self.db.as_ref()?;
            let dir_str = path.parent()?.to_string_lossy();
            let fname   = path.file_name()?.to_str()?;
            let dir     = db.find_directory_by_path(&dir_str).ok()??;
            db.get_image(dir.id, fname).ok()?
        });
    }

    // ── Navigation ────────────────────────────────────────────────────────

    fn navigate_next(&mut self, ctx: &Context, preserve_zoom: bool) {
        let mut moved = false;
        if let Some(ref mut listing) = self.listing {
            while listing.go_next() {
                moved = true;
                if let Some(p) = listing.current() {
                    // Check ignored status without borrowing self
                    if !self.session.ignored_images.contains(p) { break; }
                }
            }
        }
        if moved { self.load_current(ctx, preserve_zoom); }
    }

    fn navigate_prev(&mut self, ctx: &Context, preserve_zoom: bool) {
        let mut moved = false;
        if let Some(ref mut listing) = self.listing {
            while listing.go_prev() {
                moved = true;
                if let Some(p) = listing.current() {
                    if !self.session.ignored_images.contains(p) { break; }
                }
            }
        }
        if moved { self.load_current(ctx, preserve_zoom); }
    }

    // ── Hide (ignore) ─────────────────────────────────────────────────────

    fn hide_current(&mut self, ctx: &Context) {
        let Some(path) = self.current_path.clone() else { return };
        self.toast(format!("Hidden: {}", path.file_name()
            .and_then(|n| n.to_str()).unwrap_or("?")));
        self.navigate_next(ctx, false);
    }

    // ── Rating ────────────────────────────────────────────────────────────

    fn set_rating(&mut self, rating: Option<u8>) {
        if let Some(path) = &self.current_path {
            if let (Some(db), Some(dir_str), Some(fname)) = (
                &self.db,
                path.parent().map(|p| p.to_string_lossy().into_owned()),
                path.file_name().and_then(|n| n.to_str()).map(str::to_string),
            ) {
                if let Ok(dir) = db.upsert_directory_by_path(&dir_str) {
                    let _ = db.set_rating(dir.id, &fname, rating);
                    self.toast(match rating {
                        Some(r) => format!("Rated: {} stars", "★".repeat(r as usize)),
                        None    => "Rating cleared".to_string(),
                    });
                    self.refresh_record();
                }
            }
        }
    }

    fn rotate_current(&mut self, cw: bool, ctx: &Context) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(db)   = &self.db              else { return };
        
        let record = self.current_record.as_ref();
        let current_rot = record.map(|r| crate::session::Rotation::from_u8(r.rotation)).unwrap_or_default();
        let new_rot = if cw { current_rot.rotate_cw() } else { current_rot.rotate_ccw() };

        if let (Some(dir_str), Some(fname)) = (
            path.parent().map(|p| p.to_string_lossy().into_owned()),
            path.file_name().and_then(|n| n.to_str()).map(str::to_string),
        ) {
            if let Ok(dir) = db.upsert_directory_by_path(&dir_str) {
                let _ = db.set_rotation(dir.id, &fname, new_rot.as_u8());
                self.load_current(ctx, true);
            }
        }
    }

    fn copy_image_to_clipboard(&mut self) {
        let Some(path) = &self.current_path else { return };
        let Some(img)  = self.image_cache.get(path) else {
            self.toast("Image not in cache, cannot copy");
            return;
        };

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                let image_data = arboard::ImageData {
                    width:  img.width as usize,
                    height: img.height as usize,
                    bytes:  std::borrow::Cow::Borrowed(&img.rgba),
                };
                if let Err(e) = clipboard.set_image(image_data) {
                    log::error!("failed to copy image to clipboard: {e}");
                    self.toast(format!("Failed to copy: {e}"));
                } else {
                    self.toast("Image copied to clipboard");
                }
            }
            Err(e) => {
                log::error!("failed to open clipboard: {e}");
                self.toast(format!("Clipboard error: {e}"));
            }
        }
    }

    // ── Delete ────────────────────────────────────────────────────────────

    fn confirm_delete(&mut self) {
        self.delete_confirm = Some(DeleteConfirm::new());
        self.toast("Press Delete again to confirm — Esc to cancel");
    }

    fn execute_delete(&mut self, ctx: &Context) {
        self.delete_confirm = None;
        let Some(path) = self.current_path.clone() else { return };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                let name = path.file_name()
                    .and_then(|n| n.to_str()).unwrap_or("?").to_string();
                
                let sort = self.session_sort_order();
                let db_ref = self.db.as_ref();
                if let Some(ref mut listing) = self.listing {
                    let _ = listing.refresh(sort, db_ref);
                }
                
                self.toast(format!("Deleted: {name}"));
                self.current_path   = None;
                self.current_record = None;
                self.metadata       = vec![];
                self.viewer.clear();
                self.load_current(ctx, false);
            }
            Err(e) => {
                self.toast(format!("Delete failed: {e}"));
            }
        }
    }

    // ── Hard refresh ─────────────────────────────────────────────────────

    fn hard_refresh(&mut self, ctx: &Context) {
        self.session.flush();
        if let Some(dir) = self.listing.as_ref().map(|l| l.dir_path.clone()) {
            let sort = self.session_sort_order();
            if let Ok(mut fresh) = DirectoryListing::scan(&dir, sort, None, self.db.as_ref()) {
                if let Some(ref cur) = self.current_path.clone() {
                    fresh.seek_to(cur);
                }
                self.listing = Some(fresh);
            }
        }
        self.load_current(ctx, false);
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn session_sort_order(&self) -> crate::settings::SortOrder {
        crate::settings::SortOrder::Name
    }

    fn session_is_ignored(&self, _path: &std::path::Path) -> bool {
        false
    }

    fn window_title(&self) -> String {
        let mut title = if let Some(ref p) = self.current_path {
            format!("{} — Rivett", p.display())
        } else {
            "Rivett".to_string()
        };

        if let Some(filter) = self.session.rating_filter {
            let scope = if self.listing.as_ref().map(|l| l.dir_path.as_os_str().is_empty()).unwrap_or(false) {
                "Library"
            } else {
                "Folder"
            };
            title = format!("{title} ({scope}: ★ {}+)", filter.value);
        }

        title
    }

    fn reveal_in_file_manager(&self) {
        if let Some(ref p) = self.current_path {
            let _ = showfile::show_path_in_file_manager(p);
        }
    }

    // ── Keyboard ─────────────────────────────────────────────────────

    fn handle_keyboard(&mut self, ctx: &Context) {
        let input = ctx.input(|i| i.clone());

        if input.key_pressed(Key::Escape) {
            if self.delete_confirm.is_some() {
                self.delete_confirm = None;
                self.toast("Delete cancelled");
            }
        }

        let shift = input.modifiers.shift;
        let preserve_zoom = shift;

        if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown) {
            self.navigate_next(ctx, preserve_zoom);
        }
        if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp) {
            self.navigate_prev(ctx, preserve_zoom);
        }

        if input.key_pressed(Key::I) { 
            self.show_info_panel = !self.show_info_panel;
            self.settings.show_info_panel = self.show_info_panel;
            let _ = self.settings.save();
        }

        for r in 0..=5 {
            let key = match r {
                0 => Key::Num0,
                1 => Key::Num1,
                2 => Key::Num2,
                3 => Key::Num3,
                4 => Key::Num4,
                5 => Key::Num5,
                _ => unreachable!(),
            };
            let rating = if r == 0 { None } else { Some(r as u8) };
            if input.key_pressed(key) { self.set_rating(rating); }
        }

        if input.key_pressed(Key::H) { self.hide_current(ctx); }

        if input.key_pressed(Key::OpenBracket) {
            self.rotate_current(false, ctx);
        }
        if input.key_pressed(Key::CloseBracket) {
            self.rotate_current(true, ctx);
        }

        let ctrl = input.modifiers.ctrl;
        if ctrl && input.key_pressed(Key::Num0) {
            self.viewer.zoom_actual_size();
        } else if input.key_pressed(Key::F) {
            self.viewer.toggle_fit(ctx.screen_rect().size());
        }

        if input.key_pressed(Key::Delete) {
            if self.delete_confirm.as_ref().map(|d| d.alive()).unwrap_or(false) {
                self.execute_delete(ctx);
            } else {
                self.confirm_delete();
            }
        }

        if ctrl && input.modifiers.shift && input.key_pressed(Key::R) {
            self.hard_refresh(ctx);
        }
    }

    // ── Info panel ────────────────────────────────────────────────────────

    fn draw_info_panel(&mut self, ctx: &Context) {
        egui::SidePanel::right("info_panel")
            .resizable(true)
            .min_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Image Info");
                    ui.separator();

                    if let Some(path) = self.current_path.clone() {
                        ui.label(format!("File: {}", path.file_name()
                            .and_then(|n| n.to_str()).unwrap_or("?")));
                        ui.label(format!("Path: {}", path.display()));

                        if let Ok(meta) = path.metadata() {
                            let kb = meta.len() as f64 / 1024.0;
                            if kb < 1024.0 {
                                ui.label(format!("Size: {kb:.1} KB"));
                            } else {
                                ui.label(format!("Size: {:.1} MB", kb / 1024.0));
                            }
                        }

                        let dim = self.viewer.image_size;
                        if dim != Vec2::ZERO {
                            ui.label(format!("Dimensions: {}×{}", dim.x as u32, dim.y as u32));
                        }

                        ui.label(format!("Zoom: {:.0}%", self.viewer.zoom * 100.0));

                        if let Some(ref listing) = self.listing {
                            ui.label(listing.position_label());
                        }

                        ui.separator();
                        ui.heading("Viewing Adjustment");
                        let mut g = self.viewer.gamma;
                        ui.horizontal(|ui| {
                            ui.label("Gamma:");
                            if ui.add(egui::Slider::new(&mut g, 0.1..=4.0)).changed() {
                                self.viewer.set_gamma(g, ctx);
                            }
                            if ui.button("Reset").clicked() {
                                self.viewer.set_gamma(1.0, ctx);
                            }
                        });

                        if let Some(img) = self.image_cache.get(&path) {
                            ui.separator();
                            ui.heading("Histogram (Luminance)");
                            let hist_height = 64.0;
                            let (rect, _) = ui.allocate_at_least(egui::vec2(ui.available_width(), hist_height), egui::Sense::hover());
                            let painter = ui.painter();
                            painter.rect_filled(rect, 2.0, egui::Color32::from_gray(30));
                            
                            let bin_width = rect.width() / 256.0;
                            for (i, &val) in img.histogram.iter().enumerate() {
                                let h = val * hist_height;
                                let x = rect.min.x + i as f32 * bin_width;
                                let bar_rect = egui::Rect::from_min_max(
                                    egui::pos2(x, rect.max.y - h),
                                    egui::pos2(x + bin_width, rect.max.y)
                                );
                                painter.rect_filled(bar_rect, 0.0, egui::Color32::from_gray(180));
                            }
                        }

                        ui.separator();
                        ui.heading("Rating");

                        let rating = self.current_record.as_ref()
                            .and_then(|r| r.rating);

                        let stars = match rating {
                            None    => "— (unrated)".to_string(),
                            Some(r) => format!("{} ({})", "★".repeat(r as usize), r),
                        };
                        ui.label(format!("Rating: {stars}"));

                        if let Some(ref rec) = self.current_record {
                            if let Some(ref note) = rec.note {
                                ui.label(format!("Note: {note}"));
                            }
                        }

                        if !self.metadata.is_empty() {
                            ui.separator();
                            ui.heading("Metadata");

                            for entry in &mut self.metadata {
                                let is_multiline = entry.value.contains('\n');
                                let is_long      = entry.value.len() > 120;

                                if is_multiline || is_long {
                                    egui::CollapsingHeader::new(
                                        egui::RichText::new(&entry.key).strong()
                                    )
                                    .id_source(egui::Id::new(&entry.key))
                                    .default_open(is_multiline && entry.key.to_lowercase() == "parameters")
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut entry.value
                                            )
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace),
                                        );
                                    });
                                } else {
                                    ui.label(egui::RichText::new(&entry.key).strong());
                                    ui.label(&entry.value);
                                }
                                ui.add_space(2.0);
                            }
                        }
                    } else {
                        ui.label("No image loaded.");
                    }
                });
            });
    }

    fn apply_global_filter(&mut self, filter: RatingFilter, ctx: &Context) {
        let Some(ref db) = self.db else { return };
        self.session.rating_filter = Some(filter);
        match DirectoryListing::scan_global(db, filter) {
            Ok(listing) => {
                self.listing = Some(listing);
                self.load_current(ctx, false);
            }
            Err(e) => log::warn!("failed to scan global ratings: {e}"),
        }
    }

    fn refresh_listing(&mut self, ctx: &Context) {
        let sort = self.session_sort_order();
        let db   = self.db.as_ref();
        if let Some(ref mut listing) = self.listing {
            if let Err(e) = listing.refresh(sort, db) {
                log::warn!("failed to refresh directory listing: {e}");
            }
            self.load_current(ctx, false);
        }
    }

    fn apply_local_filter(&mut self, filter: Option<RatingFilter>, ctx: &Context) {
        self.session.rating_filter = filter;
        if let Some(ref mut listing) = self.listing {
            listing.rating_filter = filter;
        }
        self.refresh_listing(ctx);
    }

    fn draw_context_menu(&mut self, response: &egui::Response, ctx: &Context) {
        let has_image = self.current_path.is_some();

        response.context_menu(|ui| {
            if ui.add_enabled(has_image, egui::Button::new("Next Image")).clicked() {
                self.navigate_next(ctx, true);
                ui.close_menu();
            }
            if ui.add_enabled(has_image, egui::Button::new("Previous Image")).clicked() {
                self.navigate_prev(ctx, true);
                ui.close_menu();
            }

            ui.separator();

            ui.menu_button("Set rating", |ui| {
                for (label, r, key) in [
                    ("★ 1",       Some(1u8), "1"),
                    ("★★ 2",     Some(2),   "2"),
                    ("★★★ 3",   Some(3),   "3"),
                    ("★★★★ 4", Some(4),   "4"),
                    ("★★★★★ 5", Some(5),   "5"),
                    ("Clear",      None,      "0"),
                ] {
                    if ui.add_enabled(has_image, egui::Button::new(label).shortcut_text(key)).clicked() {
                        self.set_rating(r);
                        ui.close_menu();
                    }
                }
            });

            ui.menu_button("Filter", |ui| {
                ui.menu_button("Current folder", |ui| {
                    for r in 1..=5 {
                        let filter = RatingFilter {
                            op:    RatingFilterOp::AtLeast,
                            value: r,
                        };
                        if ui.button(format!("At least ★ {r}")).clicked() {
                            self.apply_local_filter(Some(filter), ctx);
                            ui.close_menu();
                        }
                    }
                });

                let has_db = self.db.is_some();
                ui.add_enabled_ui(has_db, |ui| {
                    ui.menu_button("Library", |ui| {
                        for r in 1..=5 {
                            let filter = RatingFilter {
                                op:    RatingFilterOp::AtLeast,
                                value: r,
                            };
                            if ui.button(format!("At least ★ {r}")).clicked() {
                                self.apply_global_filter(filter, ctx);
                                ui.close_menu();
                            }
                        }
                    });
                });

                if ui.button("Clear Filter").clicked() {
                    self.apply_local_filter(None, ctx);
                    ui.close_menu();
                }
            });

            ui.separator();

            if ui.add_enabled(has_image, egui::Button::new("Hide image").shortcut_text("H")).clicked() {
                self.hide_current(ctx);
                ui.close_menu();
            }

            if ui.add_enabled(has_image, egui::Button::new("Delete").shortcut_text("Del"))
                .on_hover_text("Two-step confirmation required")
                .clicked()
            {
                self.confirm_delete();
                ui.close_menu();
            }

            ui.separator();

            if ui.add_enabled(has_image, egui::Button::new("Rotate Clockwise").shortcut_text("]")).clicked() {
                self.rotate_current(true, ctx);
                ui.close_menu();
            }
            if ui.add_enabled(has_image, egui::Button::new("Rotate Counter-Clockwise").shortcut_text("[")).clicked() {
                self.rotate_current(false, ctx);
                ui.close_menu();
            }

            ui.separator();

            if ui.add_enabled(has_image, egui::Button::new("Copy Image")).clicked() {
                self.copy_image_to_clipboard();
                ui.close_menu();
            }

            if ui.add_enabled(has_image, egui::Button::new("Copy File")).clicked() {
                if let Some(ref p) = self.current_path {
                    ctx.copy_text(p.to_string_lossy().into_owned());
                    self.toast("File path copied to clipboard");
                }
                ui.close_menu();
            }

            if ui.add_enabled(has_image, egui::Button::new("Copy path")).clicked() {
                if let Some(ref p) = self.current_path {
                    ctx.copy_text(p.to_string_lossy().into_owned());
                }
                ui.close_menu();
            }
            if ui.add_enabled(has_image, egui::Button::new("Open folder")).clicked() {
                self.reveal_in_file_manager();
                ui.close_menu();
            }

            ui.separator();

            let info_label = if self.show_info_panel { "Hide info" } else { "Show info" };
            if ui.add(egui::Button::new(info_label).shortcut_text("I")).clicked() {
                self.show_info_panel = !self.show_info_panel;
                self.settings.show_info_panel = self.show_info_panel;
                let _ = self.settings.save();
                ui.close_menu();
            }

            let fit_label = if self.viewer.fit_to_window {
                "Actual size"
            } else {
                "Fit to window"
            };
            let fit_shortcut = if self.viewer.fit_to_window { "Ctrl+0" } else { "F" };
            if ui.add(egui::Button::new(fit_label).shortcut_text(fit_shortcut)).clicked() {
                if self.viewer.fit_to_window {
                    self.viewer.zoom_actual_size();
                } else {
                    self.viewer.toggle_fit(ctx.screen_rect().size());
                }
                ui.close_menu();
            }

            ui.separator();

            if ui.add(egui::Button::new("Reset Session").shortcut_text("Ctrl+Shift+R")).clicked() {
                self.hard_refresh(ctx);
                ui.close_menu();
            }

            ui.separator();

            ui.vertical_centered(|ui| {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(format!("Rivett v{}", env!("CARGO_PKG_VERSION")))
                    .small()
                    .color(egui::Color32::from_gray(120)));
                ui.hyperlink_to(
                    egui::RichText::new("github.com/krets/rivett").small(),
                    "https://github.com/krets/rivett"
                );
            });
        });
    }
}

// ---------------------------------------------------------------------------
// eframe::App
// ---------------------------------------------------------------------------

impl eframe::App for RivettApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.image_cache.poll();
        ctx.request_repaint();

        self.handle_keyboard(ctx);

        let hovered_files = ctx.input(|i| i.raw.hovered_files.clone());
        if !hovered_files.is_empty() {
            let screen = ctx.screen_rect();
            let overlay = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground, egui::Id::new("drop_overlay"),
            ));
            overlay.rect_filled(screen, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110));
            overlay.text(
                screen.center(), egui::Align2::CENTER_CENTER,
                "Drop image to open",
                egui::FontId::proportional(28.0), egui::Color32::WHITE,
            );
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            if let Some(path) = file.path {
                self.open_image(path, ctx);
                break;
            }
        }

        if let Some(ref dc) = self.delete_confirm {
            if !dc.alive() { self.delete_confirm = None; }
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        if self.show_info_panel {
            self.draw_info_panel(ctx);
        }

        CentralPanel::default().show(ctx, |ui| {
            let canvas = ui.max_rect();
            self.viewer.recalc_fit(ui.available_size());

            let response = ui.allocate_rect(canvas, egui::Sense::click_and_drag());

            if response.dragged_by(egui::PointerButton::Primary) && !ctx.input(|i| i.modifiers.ctrl) {
                self.viewer.fit_to_window = false;
                self.viewer.pan += response.drag_delta();
            }

            #[cfg(windows)]
            {
                let is_right_drag = response.dragged_by(egui::PointerButton::Secondary);
                let is_ctrl_drag  = response.dragged_by(egui::PointerButton::Primary) && ctx.input(|i| i.modifiers.ctrl);

                if (is_right_drag || is_ctrl_drag) && self.current_path.is_some() {
                    if let Some(path) = self.current_path.clone() {
                        self.spawn_native_drag(path);
                    }
                }
            }

            if response.hovered() {
                let (scroll_y, zoom_delta) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                if zoom_delta != 1.0 {
                    let cursor = ctx.input(|i| i.pointer.latest_pos());
                    self.viewer.apply_zoom_delta(zoom_delta, cursor, canvas);
                } else if scroll_y != 0.0 {
                    let factor = if scroll_y > 0.0 { 1.1_f32 } else { 1.0 / 1.1 };
                    let cursor = ctx.input(|i| i.pointer.latest_pos());
                    self.viewer.apply_zoom_delta(factor, cursor, canvas);
                }
            }

            if response.double_clicked() {
                if let Some(ref err) = self.viewer.load_error {
                    ctx.copy_text(err.clone());
                    self.toast("Error message copied to clipboard");
                } else {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &[
                            "png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif", "gif", "exr", "svg",
                            "arw", "cr2", "cr3", "nef", "nrw", "orf", "raf", "rw2", "dng"
                        ])
                        .pick_file()
                    {
                        self.open_image(path, ctx);
                    }
                }
            }

            if response.clicked() && self.viewer.load_error.is_some() {
                if let Some(ref err) = self.viewer.load_error {
                    ctx.copy_text(err.clone());
                    self.toast("Error message copied to clipboard");
                }
            }

            self.draw_context_menu(&response, ctx);

            let painter = ui.painter();
            if let Some(ref texture) = self.viewer.texture {
                let rect = self.viewer.image_rect(canvas);
                painter.image(
                    texture.id(), rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else if let Some(ref err) = self.viewer.load_error {
                painter.text(
                    canvas.center(), egui::Align2::CENTER_CENTER,
                    format!("Error loading image:\n{err}"),
                    egui::FontId::proportional(18.0),
                    egui::Color32::LIGHT_RED,
                );
            } else {
                painter.text(
                    canvas.center(), egui::Align2::CENTER_CENTER,
                    "Drag an image here, or double-click to open",
                    egui::FontId::proportional(18.0),
                    egui::Color32::from_gray(130),
                );
            }

            if self.session.has_pending_changes() {
                let dot_pos = egui::pos2(canvas.max.x - 14.0, canvas.min.y + 14.0);
                let response = ui.interact(
                    egui::Rect::from_center_size(dot_pos, egui::vec2(12.0, 12.0)),
                    egui::Id::new("modified_badge"),
                    egui::Sense::hover(),
                );
                response.on_hover_text("Unsaved session changes (e.g. pending crops)");
                painter.circle_filled(dot_pos, 6.0, egui::Color32::from_rgb(255, 180, 0));
            }

            if self.delete_confirm.as_ref().map(|d| d.alive()).unwrap_or(false) {
                let bg = egui::Color32::from_rgba_unmultiplied(180, 30, 30, 210);
                let msg_rect = egui::Rect::from_center_size(
                    canvas.center(),
                    egui::vec2(420.0, 56.0),
                );
                painter.rect_filled(msg_rect, 6.0, bg);
                painter.text(
                    msg_rect.center(), egui::Align2::CENTER_CENTER,
                    "Press Delete to confirm — Esc to cancel",
                    egui::FontId::proportional(16.0), egui::Color32::WHITE,
                );
            }
        });

        if let Some(ref toast) = self.toast {
            let alpha = toast.alpha();
            if alpha > 0.0 {
                let screen  = ctx.screen_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip, egui::Id::new("toast"),
                ));

                let font = egui::FontId::proportional(16.0);
                let galley = ctx.fonts(|f| f.layout_no_wrap(
                    toast.message.clone(), font.clone(),
                    egui::Color32::WHITE,
                ));
                let pad    = egui::vec2(16.0, 8.0);
                let size   = galley.size() + pad * 2.0;
                let center = egui::pos2(screen.center().x, screen.max.y - 48.0);
                let rect   = egui::Rect::from_center_size(center, size);

                let a = (alpha * 200.0) as u8;
                painter.rect_filled(rect, 6.0, egui::Color32::from_rgba_unmultiplied(30, 30, 30, a));
                painter.galley(rect.min + pad, galley, egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8));
                ctx.request_repaint();
            }
        }

        if self.toast.as_ref().map(|t| !t.alive()).unwrap_or(false) {
            self.toast = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Native Windows Drag and Drop Implementation
// ---------------------------------------------------------------------------

#[cfg(windows)]
impl RivettApp {
    fn spawn_native_drag(&self, path: std::path::PathBuf) {
        use win_drag::*;
        
        std::thread::spawn(move || {
            unsafe {
                let _ = OleInitialize(None);
                
                let hdrop = match create_hdrop(&path) {
                    Ok(h) => h,
                    Err(_) => return,
                };
                
                let data_object: IDataObject = FileDataObject { hdrop }.into();
                let drop_source: IDropSource = FileDropSource.into();
                
                let mut effect = DROPEFFECT_NONE;
                let _ = DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY | DROPEFFECT_MOVE, &mut effect);
            }
        });
    }
}

#[cfg(windows)]
#[windows_core::implement(windows::Win32::System::Com::IDataObject)]
struct FileDataObject {
    hdrop: win_drag::HGLOBAL,
}

#[cfg(windows)]
impl win_drag::IDataObject_Impl for FileDataObject {
    fn GetData(&self, pformatetc: *const win_drag::FORMATETC) -> windows::core::Result<win_drag::STGMEDIUM> {
        unsafe {
            let formatetc = *pformatetc;
            if formatetc.cfFormat == win_drag::CF_HDROP.0 as u16 && (formatetc.tymed & win_drag::TYMED_HGLOBAL.0 as u32) != 0 {
                let mut medium = win_drag::STGMEDIUM::default();
                medium.tymed = win_drag::TYMED_HGLOBAL.0 as u32;
                medium.u.hGlobal = duplicate_hglobal(self.hdrop)?;
                return Ok(medium);
            }
            Err(windows::core::Error::from_hresult(win_drag::DV_E_FORMATETC))
        }
    }

    fn GetDataHere(&self, _pformatetc: *const win_drag::FORMATETC, _pmedium: *mut win_drag::STGMEDIUM) -> windows::core::Result<()> {
        Err(windows::core::Error::from_hresult(win_drag::E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const win_drag::FORMATETC) -> windows::core::HRESULT {
        unsafe {
            let formatetc = *pformatetc;
            if formatetc.cfFormat == win_drag::CF_HDROP.0 as u16 && (formatetc.tymed & win_drag::TYMED_HGLOBAL.0 as u32) != 0 {
                return win_drag::S_OK;
            }
            win_drag::DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(&self, _pformatectin: *const win_drag::FORMATETC, pformatetcout: *mut win_drag::FORMATETC) -> windows::core::HRESULT {
        unsafe {
            if !pformatetcout.is_null() {
                (*pformatetcout).ptd = std::ptr::null_mut();
            }
            win_drag::E_NOTIMPL
        }
    }

    fn SetData(&self, _pformatetc: *const win_drag::FORMATETC, _pmedium: *const win_drag::STGMEDIUM, _frelease: win_drag::BOOL) -> windows::core::Result<()> {
        Err(windows::core::Error::from_hresult(win_drag::E_NOTIMPL))
    }

    fn EnumFormatEtc(&self, _dwdirection: u32) -> windows::core::Result<win_drag::IEnumFORMATETC> {
        Err(windows::core::Error::from_hresult(win_drag::E_NOTIMPL))
    }

    fn DAdvise(&self, _pformatetc: *const win_drag::FORMATETC, _advf: u32, _padvsink: Option<&win_drag::IAdviseSink>) -> windows::core::Result<u32> {
        Err(windows::core::Error::from_hresult(win_drag::OLE_E_ADVISENOTSUPPORTED))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(windows::core::Error::from_hresult(win_drag::OLE_E_ADVISENOTSUPPORTED))
    }

    fn EnumDAdvise(&self) -> windows::core::Result<win_drag::IEnumSTATDATA> {
        Err(windows::core::Error::from_hresult(win_drag::OLE_E_ADVISENOTSUPPORTED))
    }
}

#[cfg(windows)]
impl Drop for FileDataObject {
    fn drop(&mut self) {
        unsafe {
            let _ = win_drag::GlobalFree(self.hdrop);
        }
    }
}

#[cfg(windows)]
#[windows_core::implement(windows::Win32::System::Ole::IDropSource)]
struct FileDropSource;

#[cfg(windows)]
impl win_drag::IDropSource_Impl for FileDropSource {
    fn QueryContinueDrag(&self, fescapepressed: win_drag::BOOL, grfkeystates: u32) -> windows::core::HRESULT {
        if fescapepressed.as_bool() {
            return win_drag::DRAGDROP_S_CANCEL;
        }
        // If neither left nor right mouse buttons are pressed, the drop is complete.
        if (grfkeystates & (win_drag::MK_LBUTTON | win_drag::MK_RBUTTON)) == 0 {
            return win_drag::DRAGDROP_S_DROP;
        }
        win_drag::S_OK
    }

    fn GiveFeedback(&self, _dweffect: win_drag::DROPEFFECT) -> windows::core::HRESULT {
        win_drag::DRAGDROP_S_USEDEFAULTCURSORS
    }
}

#[cfg(windows)]
fn duplicate_hglobal(hglobal: win_drag::HGLOBAL) -> windows::core::Result<win_drag::HGLOBAL> {
    unsafe {
        let size = win_drag::GlobalSize(hglobal);
        let src = win_drag::GlobalLock(hglobal);
        if src.is_null() { return Err(windows::core::Error::from_hresult(win_drag::E_FAIL)); }
        
        let dest_hglobal = win_drag::GlobalAlloc(win_drag::GMEM_MOVEABLE, size)?;
        let dest = win_drag::GlobalLock(dest_hglobal);
        if dest.is_null() {
            let _ = win_drag::GlobalFree(dest_hglobal);
            let _ = win_drag::GlobalUnlock(hglobal);
            return Err(windows::core::Error::from_hresult(win_drag::E_FAIL));
        }
        
        std::ptr::copy_nonoverlapping(src, dest, size);
        let _ = win_drag::GlobalUnlock(hglobal);
        let _ = win_drag::GlobalUnlock(dest_hglobal);
        Ok(dest_hglobal)
    }
}

#[cfg(windows)]
fn create_hdrop(path: &std::path::Path) -> windows::core::Result<win_drag::HGLOBAL> {
    use std::os::windows::ffi::OsStrExt;
    let mut path_u16: Vec<u16> = path.as_os_str().encode_wide().collect();
    path_u16.push(0);
    path_u16.push(0); 

    let size = std::mem::size_of::<win_drag::DROPFILES>() + path_u16.len() * 2;
    unsafe {
        let hglobal = win_drag::GlobalAlloc(win_drag::GMEM_MOVEABLE | win_drag::GMEM_ZEROINIT, size)?;
        let ptr = win_drag::GlobalLock(hglobal);
        if ptr.is_null() {
            let _ = win_drag::GlobalFree(hglobal);
            return Err(windows::core::Error::from_hresult(win_drag::E_FAIL));
        }
        
        let dropfiles = ptr as *mut win_drag::DROPFILES;
        (*dropfiles).pFiles = std::mem::size_of::<win_drag::DROPFILES>() as u32;
        (*dropfiles).fWide = win_drag::BOOL(1); 

        let path_ptr = (ptr as *mut u8).add(std::mem::size_of::<win_drag::DROPFILES>()) as *mut u16;
        std::ptr::copy_nonoverlapping(path_u16.as_ptr(), path_ptr, path_u16.len());

        let _ = win_drag::GlobalUnlock(hglobal);
        Ok(hglobal)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct Toast {
    message: String,
    start:   Instant,
}

impl Toast {
    fn new(message: String) -> Self {
        Self { message, start: Instant::now() }
    }
    fn alive(&self) -> bool {
        self.start.elapsed() < Duration::from_secs(3)
    }
    fn alpha(&self) -> f32 {
        let elapsed = self.start.elapsed().as_secs_f32();
        if elapsed < 0.2 { elapsed / 0.2 }
        else if elapsed > 2.5 { 1.0 - (elapsed - 2.5) / 0.5 }
        else { 1.0 }
    }
}

struct DeleteConfirm {
    start: Instant,
}

impl DeleteConfirm {
    fn new() -> Self {
        Self { start: Instant::now() }
    }
    fn alive(&self) -> bool {
        self.start.elapsed() < Duration::from_secs(4)
    }
}
