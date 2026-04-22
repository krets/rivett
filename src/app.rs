//! Top-level application state and the [`eframe::App`] implementation.

use eframe::CreationContext;
use egui::{CentralPanel, Context, Key, Vec2};
use std::time::{Duration, Instant};

use crate::db::{Database, ImageRecord};
use crate::image_loader::{load_image, DirectoryListing, ImageCache};
use crate::metadata::{read_metadata, MetaEntry};
use crate::session::SessionState;
use crate::settings::AppSettings;
use crate::viewer::ViewerState;

// ---------------------------------------------------------------------------
// Toast
// ---------------------------------------------------------------------------

/// A brief on-screen notification that fades out automatically.
struct Toast {
    message: String,
    born:    Instant,
    ttl:     Duration,
}

impl Toast {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), born: Instant::now(), ttl: Duration::from_millis(1800) }
    }

    /// 0.0 = invisible, 1.0 = fully opaque.
    fn alpha(&self) -> f32 {
        let elapsed  = self.born.elapsed().as_secs_f32();
        let total    = self.ttl.as_secs_f32();
        let fade_for = 0.4_f32; // last N seconds are a fade-out
        if elapsed >= total { return 0.0; }
        let remaining = total - elapsed;
        (remaining / fade_for).min(1.0)
    }

    fn alive(&self) -> bool {
        self.born.elapsed() < self.ttl
    }
}

// ---------------------------------------------------------------------------
// DeleteConfirm
// ---------------------------------------------------------------------------

struct DeleteConfirm {
    born: Instant,
}

impl DeleteConfirm {
    fn new() -> Self { Self { born: Instant::now() } }
    /// Confirmation expires after 3 s of inaction.
    fn alive(&self) -> bool { self.born.elapsed() < Duration::from_secs(3) }
}

// ---------------------------------------------------------------------------
// RivettApp
// ---------------------------------------------------------------------------

pub struct RivettApp {
    settings:        AppSettings,
    viewer:          ViewerState,
    listing:         Option<DirectoryListing>,
    session:         SessionState,
    db:              Option<Database>,
    current_path:    Option<std::path::PathBuf>,
    current_record:  Option<ImageRecord>,
    metadata:        Vec<MetaEntry>,
    show_info_panel: bool,
    toast:           Option<Toast>,
    delete_confirm:  Option<DeleteConfirm>,
    image_cache:     ImageCache,
}

impl RivettApp {
    pub fn new(
        _cc:           &CreationContext<'_>,
        settings:      AppSettings,
        initial_image: Option<std::path::PathBuf>,
    ) -> Self {
        let session = SessionState::new(settings.default_sort);

        let mut app = Self {
            session,
            viewer:          ViewerState::new(),
            listing:         None,
            db:              None,
            current_path:    None,
            current_record:  None,
            metadata:        vec![],
            show_info_panel: false,
            toast:           None,
            delete_confirm:  None,
            settings,
            image_cache:     ImageCache::new(24),
        };

        app.db = app.settings.central_db_resolved().and_then(|p| {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok()?;
            }
            Database::open(&p).map_err(|e| log::warn!("DB open failed: {e}")).ok()
        });

        if let Some(path) = initial_image {
            app.open_image(path, &_cc.egui_ctx);
        }

        app
    }

    // ── Toast helper ──────────────────────────────────────────────────────

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast::new(msg));
    }

    // ── Image loading ─────────────────────────────────────────────────────

    fn open_image(&mut self, path: std::path::PathBuf, ctx: &Context) {
        // Reset filter when explicitly opening a new file
        self.session.rating_filter = None;

        if let Some(dir) = path.parent() {
            let sort   = self.session.sort_order;
            let db     = self.db.as_ref();
            match DirectoryListing::scan(dir, sort, None, db) {
                Ok(mut listing) => {
                    listing.seek_to(&path);
                    self.listing = Some(listing);
                }
                Err(e) => log::warn!("failed to scan directory: {e}"),
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
        
        // Fetch the DB record (ratings, bookmark, note, rotation).
        self.refresh_record();

        let rotation = self.current_record.as_ref()
            .map(|r| crate::session::Rotation::from_u8(r.rotation))
            .unwrap_or_default();

        // 1. Try cache
        if let Some(img) = self.image_cache.get(&path) {
            self.viewer.load_image(ctx, img, rotation, preserve_zoom);
        } else {
            // 2. Load from disk
            match load_image(&path) {
                Ok(img) => {
                    self.image_cache.insert(path.clone(), img.clone());
                    self.viewer.load_image(ctx, &img, rotation, preserve_zoom);
                }
                Err(e)  => {
                    log::warn!("{e}");
                    self.viewer.clear();
                }
            }
        }

        // 3. Prefetch neighbors
        if let Some(ref listing) = self.listing {
            // Next
            let mut i = listing.current_index + 1;
            while i < listing.files.len() {
                let p = &listing.files[i];
                if !self.session.is_ignored(p) {
                    self.image_cache.prefetch(p.clone());
                    break;
                }
                i += 1;
            }
            // Prev
            let mut i = listing.current_index as i32 - 1;
            while i >= 0 {
                let p = &listing.files[i as usize];
                if !self.session.is_ignored(p) {
                    self.image_cache.prefetch(p.clone());
                    break;
                }
                i -= 1;
            }
        }

        // Read PNG/EXIF metadata for the info panel.
        self.metadata = read_metadata(&path);
    }

    fn refresh_record(&mut self) {
        self.current_record = self.current_path.as_ref().and_then(|path| {
            let db      = self.db.as_ref()?;
            let dir_str = path.parent()?.to_string_lossy().into_owned();
            let fname   = path.file_name()?.to_str()?.to_string();
            let dir     = db.find_directory_by_path(&dir_str).ok()??;
            db.get_image(dir.id, &fname).ok().flatten()
        });
    }

    // ── Navigation ────────────────────────────────────────────────────────

    fn navigate_next(&mut self, ctx: &Context, preserve_zoom: bool) {
        let Some(ref mut listing) = self.listing else { return };
        // Skip past ignored images.
        let mut moved = false;
        loop {
            if !listing.go_next() { break; }
            moved = true;
            if let Some(p) = listing.current() {
                if !self.session.is_ignored(p) { break; }
            }
        }
        if moved { self.load_current(ctx, preserve_zoom); }
    }

    fn navigate_prev(&mut self, ctx: &Context, preserve_zoom: bool) {
        let Some(ref mut listing) = self.listing else { return };
        let mut moved = false;
        loop {
            if !listing.go_prev() { break; }
            moved = true;
            if let Some(p) = listing.current() {
                if !self.session.is_ignored(p) { break; }
            }
        }
        if moved { self.load_current(ctx, preserve_zoom); }
    }

    // ── Hide (ignore) ─────────────────────────────────────────────────────

    fn hide_current(&mut self, ctx: &Context) {
        let Some(path) = self.current_path.clone() else { return };
        self.session.ignore_image(path.clone());
        self.toast(format!("Hidden: {}", path.file_name()
            .and_then(|n| n.to_str()).unwrap_or("?")));
        self.navigate_next(ctx, false);
    }

    // ── Rating / bookmarks ────────────────────────────────────────────────

    fn set_rating(&mut self, rating: Option<u8>) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(db)   = &self.db              else { return };
        if let (Some(dir_str), Some(fname)) = (
            path.parent().map(|p| p.to_string_lossy().into_owned()),
            path.file_name().and_then(|n| n.to_str()).map(str::to_string),
        ) {
            match db.upsert_directory_by_path(&dir_str) {
                Ok(dir) => {
                    let _ = db.set_rating(dir.id, &fname, rating);
                    let msg = match rating {
                        None    => "Rating cleared".to_string(),
                        Some(r) => format!("Rated {}", "★".repeat(r as usize)),
                    };
                    self.toast(msg);
                    self.refresh_record();
                }
                Err(e) => log::warn!("set_rating: {e}"),
            }
        }
    }

    fn toggle_bookmark(&mut self) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(db)   = &self.db              else { return };
        if let (Some(dir_str), Some(fname)) = (
            path.parent().map(|p| p.to_string_lossy().into_owned()),
            path.file_name().and_then(|n| n.to_str()).map(str::to_string),
        ) {
            if let Ok(dir) = db.upsert_directory_by_path(&dir_str) {
                let current = db.get_image(dir.id, &fname)
                    .ok().flatten().map(|r| r.bookmarked).unwrap_or(false);
                let _ = db.set_bookmark(dir.id, &fname, !current);
                self.toast(if current { "Bookmark removed" } else { "Bookmarked ★" });
                self.refresh_record();
            }
        }
    }

    fn rotate_current(&mut self, cw: bool, ctx: &Context) {
        let Some(path) = self.current_path.clone() else { return };
        let Some(db)   = &self.db              else { return };
        
        let new_rot = if cw {
            self.session.rotate_cw(path.clone())
        } else {
            self.session.rotate_ccw(path.clone())
        };

        if let (Some(dir_str), Some(fname)) = (
            path.parent().map(|p| p.to_string_lossy().into_owned()),
            path.file_name().and_then(|n| n.to_str()).map(str::to_string),
        ) {
            if let Ok(dir) = db.upsert_directory_by_path(&dir_str) {
                let _ = db.set_rotation(dir.id, &fname, new_rot.as_u8());
                self.load_current(ctx, false);
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
                // Remove from listing so we don't see it again.
                if let Some(ref mut listing) = self.listing {
                    let _ = listing.refresh(self.session.sort_order, self.db.as_ref());
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
        // TODO: prompt save-or-discard when has_pending_changes()
        self.session.flush();
        if let Some(dir) = self.listing.as_ref().map(|l| l.dir_path.clone()) {
            let sort = self.session.sort_order;
            if let Ok(mut fresh) = DirectoryListing::scan(&dir, sort, None, self.db.as_ref()) {
                if let Some(ref cur) = self.current_path.clone() {
                    fresh.seek_to(cur);
                }
                self.listing = Some(fresh);
            }
        }
        self.load_current(ctx, false);
    }

    // ── Open in file manager ──────────────────────────────────────────────

    fn reveal_in_file_manager(&self) {
        let Some(ref path) = self.current_path else { return };
        let dir = path.parent().unwrap_or(path.as_path());
        #[cfg(target_os = "windows")]
        { let _ = std::process::Command::new("explorer").arg(dir).spawn(); }
        #[cfg(target_os = "macos")]
        { let _ = std::process::Command::new("open").arg(dir).spawn(); }
        #[cfg(target_os = "linux")]
        { let _ = std::process::Command::new("xdg-open").arg(dir).spawn(); }
    }

    // ── Window title ─────────────────────────────────────────────────────

    fn window_title(&self) -> String {
        let Some(ref path) = self.current_path else { return "Rivett".to_string() };
        let name   = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let prefix = if self.session.has_pending_changes() { "*" } else { "" };
        format!("{prefix}{name} — Rivett")
    }

    // ── Keyboard input ────────────────────────────────────────────────────

    fn handle_keyboard(&mut self, ctx: &Context) {
        let input = ctx.input(|i| i.clone());

        // Cancel delete confirm with Esc
        if input.key_pressed(Key::Escape) {
            if self.delete_confirm.is_some() {
                self.delete_confirm = None;
                self.toast("Delete cancelled");
                return;
            }
        }

        // Navigation
        let shift = input.modifiers.shift;
        let preserve_zoom = shift;

        if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown) {
            self.navigate_next(ctx, preserve_zoom);
        }
        if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp) {
            self.navigate_prev(ctx, preserve_zoom);
        }

        // Info panel
        if input.key_pressed(Key::I) { self.show_info_panel = !self.show_info_panel; }

        // Bookmark
        if input.key_pressed(Key::B) { self.toggle_bookmark(); }

        // Hide
        if input.key_pressed(Key::H) { self.hide_current(ctx); }

        // Ratings
        let rating_keys = [
            (Key::Num0, None),
            (Key::Num1, Some(1u8)),
            (Key::Num2, Some(2)),
            (Key::Num3, Some(3)),
            (Key::Num4, Some(4)),
            (Key::Num5, Some(5)),
        ];
        for (key, rating) in rating_keys {
            if input.key_pressed(key) { self.set_rating(rating); }
        }

        // Rotation
        if input.key_pressed(Key::OpenBracket) {
            self.rotate_current(false, ctx);
        }
        if input.key_pressed(Key::CloseBracket) {
            self.rotate_current(true, ctx);
        }

        // Zoom via keyboard
        let ctrl = input.modifiers.ctrl;
        if ctrl && input.key_pressed(Key::Equals) || input.key_pressed(Key::ArrowUp) {
            let r = ctx.screen_rect();
            self.viewer.apply_zoom_delta(1.25, Some(r.center()), r);
        }
        if ctrl && input.key_pressed(Key::Minus) || input.key_pressed(Key::ArrowDown) {
            let r = ctx.screen_rect();
            self.viewer.apply_zoom_delta(0.8, Some(r.center()), r);
        }
        if ctrl && input.key_pressed(Key::Num0) {
            self.viewer.zoom_actual_size();
        }

        // Fit to window
        if input.key_pressed(Key::F) {
            let avail = ctx.screen_rect().size();
            self.viewer.toggle_fit(avail);
        }

        // Delete (two-step) / Shift+Delete (immediate)
        if input.key_pressed(Key::Delete) {
            if input.modifiers.shift {
                self.execute_delete(ctx);
            } else if self.delete_confirm.as_ref().map(|d| d.alive()).unwrap_or(false) {
                self.execute_delete(ctx);
            } else {
                self.confirm_delete();
            }
        }

        // Hard refresh (Ctrl+Shift+R)
        if ctrl && input.modifiers.shift && input.key_pressed(Key::R) {
            self.hard_refresh(ctx);
        }
    }

    // ── Info panel ────────────────────────────────────────────────────────

    fn draw_info_panel(&self, ctx: &Context) {
        egui::SidePanel::right("info_panel")
            .resizable(true)
            .min_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // ── File info ─────────────────────────────────────────
                    ui.heading("Image Info");
                    ui.separator();

                    if let Some(ref path) = self.current_path {
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

                        // ── Rating & bookmark ─────────────────────────────
                        ui.separator();
                        ui.heading("Rating & Bookmark");

                        let (rating, bookmarked) = self.current_record.as_ref()
                            .map(|r| (r.rating, r.bookmarked))
                            .unwrap_or((None, false));

                        let stars = match rating {
                            None    => "— (unrated)".to_string(),
                            Some(r) => format!("{} ({})", "★".repeat(r as usize), r),
                        };
                        ui.label(format!("Rating: {stars}"));
                        ui.label(if bookmarked { "Bookmarked: ✓" } else { "Bookmarked: ✗" });

                        if let Some(ref rec) = self.current_record {
                            if let Some(ref note) = rec.note {
                                ui.label(format!("Note: {note}"));
                            }
                        }

                        // ── Image metadata ───────────────────────────────
                        if !self.metadata.is_empty() {
                            ui.separator();
                            ui.heading("Metadata");

                            for entry in &self.metadata {
                                // Large values (JSON, etc.) get a collapsing section.
                                if entry.value.len() > 120 {
                                    let preview = &entry.value[..entry.value.char_indices()
                                        .nth(80).map(|(i, _)| i).unwrap_or(entry.value.len())];
                                    egui::CollapsingHeader::new(
                                        egui::RichText::new(&entry.key).strong()
                                    )
                                    .id_source(egui::Id::new(&entry.key))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::TextEdit::multiline(
                                                &mut entry.value.as_str()
                                            )
                                            .desired_width(f32::INFINITY)
                                            .font(egui::TextStyle::Monospace),
                                        );
                                    });
                                    let _ = preview; // shown in header implicitly
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

    fn refresh_listing(&mut self, ctx: &Context) {
        if let Some(ref mut listing) = self.listing {
            let sort = self.session.sort_order;
            let db   = self.db.as_ref();
            if let Err(e) = listing.refresh(sort, db) {
                log::warn!("failed to refresh directory listing: {e}");
            }
            self.load_current(ctx, false);
        }
    }

    fn apply_local_filter(&mut self, filter: Option<crate::session::RatingFilter>, ctx: &Context) {
        self.session.rating_filter = filter;
        if let Some(ref mut listing) = self.listing {
            listing.rating_filter = filter;
            self.refresh_listing(ctx);
        }
    }

    fn apply_global_filter(&mut self, filter: crate::session::RatingFilter, ctx: &Context) {
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

    // ── Context menu ──────────────────────────────────────────────────────

    fn draw_context_menu(&mut self, response: &egui::Response, ctx: &Context) {
        let has_image = self.current_path.is_some();
        let has_db    = self.db.is_some();

        response.context_menu(|ui| {
            let next_shortcut = "Shift+Right";
            let prev_shortcut = "Shift+Left";
            let zoom_hint = "(Shift to preserve zoom)";

            if ui.add_enabled(has_image, egui::Button::new(format!("Next Image {zoom_hint}"))
                .shortcut_text(next_shortcut)).clicked() 
            {
                self.navigate_next(ctx, true);
                ui.close_menu();
            }
            if ui.add_enabled(has_image, egui::Button::new(format!("Previous Image {zoom_hint}"))
                .shortcut_text(prev_shortcut)).clicked() 
            {
                self.navigate_prev(ctx, true);
                ui.close_menu();
            }

            ui.separator();

            if ui.add_enabled(has_image, egui::Button::new("Bookmark").shortcut_text("B")).clicked() {
                self.toggle_bookmark();
                ui.close_menu();
            }

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

            ui.separator();

            ui.menu_button("Filter", |ui| {
                ui.menu_button("Local Filter (current folder)", |ui| {
                    for r in 1..=5 {
                        let filter = crate::session::RatingFilter {
                            op:    crate::session::RatingFilterOp::AtLeast,
                            value: r,
                        };
                        if ui.button(format!("At least ★ {r}")).clicked() {
                            self.apply_local_filter(Some(filter), ctx);
                            ui.close_menu();
                        }
                    }
                });

                ui.add_enabled_ui(has_db, |ui| {
                    ui.menu_button("Global Filter (entire library)", |ui| {
                        for r in 1..=5 {
                            let filter = crate::session::RatingFilter {
                                op:    crate::session::RatingFilterOp::AtLeast,
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

            ui.menu_button("Sort by", |ui| {
                for (label, order) in [
                    ("Name",          crate::settings::SortOrder::Name),
                    ("Date Modified", crate::settings::SortOrder::DateModified),
                    ("File Size",     crate::settings::SortOrder::FileSize),
                ] {
                    let is_selected = self.session.sort_order == order;
                    if ui.selectable_label(is_selected, label).clicked() {
                        self.session.sort_order = order;
                        self.refresh_listing(ctx);
                        ui.close_menu();
                    }
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
        });
    }
}

// ---------------------------------------------------------------------------
// eframe::App
// ---------------------------------------------------------------------------

impl eframe::App for RivettApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // ── Background loading ───────────────────────────────────────────
        self.image_cache.poll();
        // Keep the GUI thread alive if we're waiting for background loads.
        // In a real app we might only request_repaint when we know we're pending,
        // but for a smooth viewer experience, egui's default often suffices.
        // However, if nothing is moving, egui might go to sleep.
        // Let's request repaint to be sure we pick up finished loads quickly.
        ctx.request_repaint();

        // ── Keyboard ─────────────────────────────────────────────────────
        self.handle_keyboard(ctx);

        // ── Drag-and-drop ─────────────────────────────────────────────────
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

        // ── Expire delete confirm ─────────────────────────────────────────
        if let Some(ref dc) = self.delete_confirm {
            if !dc.alive() { self.delete_confirm = None; }
        }

        // ── Window title ─────────────────────────────────────────────────
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        // ── Side panels ───────────────────────────────────────────────────
        if self.show_info_panel {
            self.draw_info_panel(ctx);
        }

        // ── Central canvas ─────────────────────────────────────────────────
        CentralPanel::default().show(ctx, |ui| {
            let canvas = ui.max_rect();
            self.viewer.recalc_fit(ui.available_size());

            let response = ui.allocate_rect(canvas, egui::Sense::click_and_drag());

            // Pan
            if response.dragged_by(egui::PointerButton::Primary) {
                self.viewer.fit_to_window = false;
                self.viewer.pan += response.drag_delta();
            }

            // Zoom
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

            // Double-click: native file open dialog
            if response.double_clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "tiff", "tif", "gif", "exr", "svg"])
                    .pick_file()
                {
                    self.open_image(path, ctx);
                }
            }

            // Right-click context menu
            self.draw_context_menu(&response, ctx);

            // ── Paint ─────────────────────────────────────────────────────
            let painter = ui.painter();

            if let Some(ref texture) = self.viewer.texture {
                let rect = self.viewer.image_rect(canvas);
                painter.image(
                    texture.id(), rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    canvas.center(), egui::Align2::CENTER_CENTER,
                    "Drag an image here, or double-click to open",
                    egui::FontId::proportional(18.0),
                    egui::Color32::from_gray(130),
                );
            }

            // Pending-change badge
            if self.session.has_pending_changes() {
                painter.circle_filled(
                    egui::pos2(canvas.max.x - 14.0, canvas.min.y + 14.0),
                    6.0, egui::Color32::from_rgb(255, 180, 0),
                );
            }

            // ── Delete confirm overlay ────────────────────────────────────
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

        // ── Toast overlay (drawn after everything else) ───────────────────
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

                // Keep repainting until the toast expires.
                ctx.request_repaint();
            } else {
                // Let the borrow end, then clear — we can't mutate self here,
                // so we just let `update` clear it on the next frame via the
                // alive() check below.  We request one more repaint to get there.
                ctx.request_repaint();
            }
        }

        // Clear expired toast (outside the borrow above).
        if self.toast.as_ref().map(|t| !t.alive()).unwrap_or(false) {
            self.toast = None;
        }
    }
}
