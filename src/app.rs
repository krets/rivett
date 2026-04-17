//! Top-level application state and the [`eframe::App`] implementation.

use eframe::CreationContext;
use egui::{CentralPanel, Context, Key, Vec2};

use crate::db::Database;
use crate::image_loader::{load_image, DirectoryListing};
use crate::session::SessionState;
use crate::settings::AppSettings;
use crate::viewer::ViewerState;

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
    show_info_panel: bool,
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
            show_info_panel: false,
            settings,
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

    // ── Image loading ─────────────────────────────────────────────────────

    fn open_image(&mut self, path: std::path::PathBuf, ctx: &Context) {
        if let Some(dir) = path.parent() {
            let sort = self.session.sort_order;
            match DirectoryListing::scan(dir, sort) {
                Ok(mut listing) => {
                    listing.seek_to(&path);
                    self.listing = Some(listing);
                }
                Err(e) => log::warn!("failed to scan directory: {e}"),
            }
        }
        self.load_current(ctx);
    }

    fn load_current(&mut self, ctx: &Context) {
        let path = match self.listing.as_ref().and_then(|l| l.current().cloned()) {
            Some(p) => p,
            None => {
                self.viewer.clear();
                self.current_path = None;
                return;
            }
        };

        let rotation = self.session.rotation_for(&path);
        self.current_path = Some(path.clone());

        match load_image(&path) {
            Ok(img) => self.viewer.load_image(ctx, &img, rotation),
            Err(e)  => {
                log::warn!("{e}");
                self.viewer.clear();
            }
        }
    }

    // ── Navigation ────────────────────────────────────────────────────────

    fn navigate_next(&mut self, ctx: &Context) {
        if self.listing.as_mut().map(|l| l.go_next()).unwrap_or(false) {
            self.load_current(ctx);
        }
    }

    fn navigate_prev(&mut self, ctx: &Context) {
        if self.listing.as_mut().map(|l| l.go_prev()).unwrap_or(false) {
            self.load_current(ctx);
        }
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
                Ok(dir) => { let _ = db.set_rating(dir.id, &fname, rating); }
                Err(e)  => log::warn!("set_rating: {e}"),
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
            }
        }
    }

    // ── Hard refresh ─────────────────────────────────────────────────────

    fn hard_refresh(&mut self, ctx: &Context) {
        // TODO: prompt save-or-discard when has_pending_changes()
        self.session.flush();
        if let Some(dir) = self.listing.as_ref().map(|l| l.dir_path.clone()) {
            let sort = self.session.sort_order;
            if let Ok(mut fresh) = DirectoryListing::scan(&dir, sort) {
                if let Some(ref cur) = self.current_path.clone() {
                    fresh.seek_to(cur);
                }
                self.listing = Some(fresh);
            }
        }
        self.load_current(ctx);
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

        // Navigation
        if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::PageDown) {
            self.navigate_next(ctx);
        }
        if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::PageUp) {
            self.navigate_prev(ctx);
        }

        // Info panel
        if input.key_pressed(Key::I) { self.show_info_panel = !self.show_info_panel; }

        // Bookmark
        if input.key_pressed(Key::B) { self.toggle_bookmark(); }

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
            if let Some(path) = self.current_path.clone() {
                self.session.rotate_ccw(path);
                self.load_current(ctx);
            }
        }
        if input.key_pressed(Key::CloseBracket) {
            if let Some(path) = self.current_path.clone() {
                self.session.rotate_cw(path);
                self.load_current(ctx);
            }
        }

        // Zoom via keyboard
        let ctrl = input.modifiers.ctrl;
        if ctrl && input.key_pressed(Key::Equals) {
            let r = ctx.screen_rect();
            self.viewer.apply_zoom_delta(1.25, None, r);
        }
        if ctrl && input.key_pressed(Key::Minus) {
            let r = ctx.screen_rect();
            self.viewer.apply_zoom_delta(0.8, None, r);
        }
        if ctrl && input.key_pressed(Key::Num0) {
            self.viewer.zoom_actual_size();
        }

        // Fit to window (F key) — use screen_rect as the available size;
        // recalc_fit will refine it on the next frame.
        if input.key_pressed(Key::F) {
            let avail = ctx.screen_rect().size();
            self.viewer.toggle_fit(avail);
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
            .min_width(260.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
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
                            ui.separator();
                            ui.label(listing.position_label());
                        }
                    } else {
                        ui.label("No image loaded.");
                    }
                });
            });
    }

    // ── Context menu ──────────────────────────────────────────────────────

    fn draw_context_menu(&mut self, response: &egui::Response, ctx: &Context) {
        let has_image = self.current_path.is_some();

        response.context_menu(|ui| {
            if ui.add_enabled(has_image, egui::Button::new("Bookmark  [B]")).clicked() {
                self.toggle_bookmark();
                ui.close_menu();
            }

            ui.menu_button("Set rating", |ui| {
                for (label, r) in [
                    ("★ 1",       Some(1u8)),
                    ("★★ 2",     Some(2)),
                    ("★★★ 3",   Some(3)),
                    ("★★★★ 4", Some(4)),
                    ("★★★★★ 5", Some(5)),
                    ("Clear  [0]", None),
                ] {
                    if ui.add_enabled(has_image, egui::Button::new(label)).clicked() {
                        self.set_rating(r);
                        ui.close_menu();
                    }
                }
            });

            ui.separator();

            if ui.add_enabled(has_image, egui::Button::new("Rotate CW  []]")).clicked() {
                if let Some(path) = self.current_path.clone() {
                    self.session.rotate_cw(path);
                    self.load_current(ctx);
                }
                ui.close_menu();
            }
            if ui.add_enabled(has_image, egui::Button::new("Rotate CCW  [[]")).clicked() {
                if let Some(path) = self.current_path.clone() {
                    self.session.rotate_ccw(path);
                    self.load_current(ctx);
                }
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

            let info_label = if self.show_info_panel { "Hide info  [I]" } else { "Show info  [I]" };
            if ui.button(info_label).clicked() {
                self.show_info_panel = !self.show_info_panel;
                ui.close_menu();
            }

            let fit_label = if self.viewer.fit_to_window {
                "Actual size  [Ctrl+0]"
            } else {
                "Fit to window  [F]"
            };
            if ui.button(fit_label).clicked() {
                if self.viewer.fit_to_window {
                    self.viewer.zoom_actual_size();
                } else {
                    // canvas size isn't available here; recalc_fit fixes zoom next frame
                    self.viewer.toggle_fit(ctx.screen_rect().size());
                }
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
        // ── Keyboard ─────────────────────────────────────────────────────
        self.handle_keyboard(ctx);

        // ── Drag-and-drop ─────────────────────────────────────────────────
        // Draw an overlay while dragging over the window.
        let hovered_files = ctx.input(|i| i.raw.hovered_files.clone());
        if !hovered_files.is_empty() {
            let screen = ctx.screen_rect();
            let overlay = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            overlay.rect_filled(screen, 0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110));
            overlay.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop image to open",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }

        // Open the first dropped image file.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            if let Some(path) = file.path {
                self.open_image(path, ctx);
                break;
            }
        }

        // ── Window title ─────────────────────────────────────────────────
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        // ── Side panels (must precede CentralPanel) ───────────────────────
        if self.show_info_panel {
            self.draw_info_panel(ctx);
        }

        // ── Central canvas ─────────────────────────────────────────────────
        CentralPanel::default().show(ctx, |ui| {
            let canvas = ui.max_rect();
            self.viewer.recalc_fit(ui.available_size());

            // Claim the whole canvas for interaction (pan / zoom / menus).
            let response = ui.allocate_rect(canvas, egui::Sense::click_and_drag());

            // ── Pan (left-button drag) ────────────────────────────────────
            if response.dragged_by(egui::PointerButton::Primary) {
                self.viewer.fit_to_window = false;
                self.viewer.pan += response.drag_delta();
            }

            // ── Zoom (scroll wheel & touchpad pinch) ──────────────────────
            if response.hovered() {
                let (scroll_y, zoom_delta) = ctx.input(|i| {
                    (i.smooth_scroll_delta.y, i.zoom_delta())
                });
                if zoom_delta != 1.0 {
                    let cursor = ctx.input(|i| i.pointer.latest_pos());
                    self.viewer.apply_zoom_delta(zoom_delta, cursor, canvas);
                } else if scroll_y != 0.0 {
                    let factor = if scroll_y > 0.0 { 1.1_f32 } else { 1.0 / 1.1 };
                    let cursor = ctx.input(|i| i.pointer.latest_pos());
                    self.viewer.apply_zoom_delta(factor, cursor, canvas);
                }
            }

            // ── Double-click (TODO: open file dialog via `rfd`) ───────────
            if response.double_clicked() {
                log::info!("double-click — file dialog not yet implemented (add `rfd` crate)");
            }

            // ── Right-click context menu ──────────────────────────────────
            self.draw_context_menu(&response, ctx);

            // ── Paint ─────────────────────────────────────────────────────
            let painter = ui.painter();

            if let Some(ref texture) = self.viewer.texture {
                let rect = self.viewer.image_rect(canvas);
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    canvas.center(),
                    egui::Align2::CENTER_CENTER,
                    "Drag an image here, or double-click to open",
                    egui::FontId::proportional(18.0),
                    egui::Color32::from_gray(130),
                );
            }

            // ── Pending-change badge ──────────────────────────────────────
            if self.session.has_pending_changes() {
                painter.circle_filled(
                    egui::pos2(canvas.max.x - 14.0, canvas.min.y + 14.0),
                    6.0,
                    egui::Color32::from_rgb(255, 180, 0),
                );
            }
        });
    }
}