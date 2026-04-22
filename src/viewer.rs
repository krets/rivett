//! Viewer canvas: zoom, pan, rotation application, and texture management.
//!
//! [`ViewerState`] is purely logical state; the actual egui painting happens
//! in `app.rs`. GPU textures are owned here via [`egui::TextureHandle`].

use egui::{Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use crate::image_loader::DecodedImage;
use crate::session::Rotation;

// ---------------------------------------------------------------------------
// ViewerMode
// ---------------------------------------------------------------------------

/// The current interaction mode of the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewerMode {
    /// Normal pan/zoom navigation.
    #[default]
    Navigate,
    /// User is drawing a rectangular crop/selection (`S` key).
    Selection,
    /// R/G/B/A channel inspection overlay (`K` key).
    ChannelView,
}

// ---------------------------------------------------------------------------
// ViewerState
// ---------------------------------------------------------------------------

/// All state required to render and interact with the image canvas.
pub struct ViewerState {
    /// The GPU texture for the currently displayed image (with rotation baked in).
    pub texture: Option<TextureHandle>,
    /// Current zoom level (1.0 = 100 %).
    pub zoom: f32,
    /// Canvas pan offset in logical pixels.
    pub pan: Vec2,
    /// Native (post-rotation) pixel size of the current image.
    pub image_size: Vec2,
    /// When `true`, zoom is recalculated every frame to fit the canvas.
    pub fit_to_window: bool,
    /// Zoom saved before entering fit-to-window mode, for toggling back.
    pub saved_zoom: Option<f32>,
    /// Current interaction mode.
    pub mode: ViewerMode,
    /// In-progress selection rectangle in canvas coordinates.
    pub selection: Option<Rect>,
    /// Whether the window is currently in fullscreen mode.
    pub fullscreen: bool,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            texture:        None,
            zoom:           1.0,
            pan:            Vec2::ZERO,
            image_size:     Vec2::ZERO,
            fit_to_window:  true,
            saved_zoom:     None,
            mode:           ViewerMode::default(),
            selection:      None,
            fullscreen:     false,
        }
    }
}

impl ViewerState {
    pub fn new() -> Self { Self::default() }

    /// Load a decoded image into egui, applying `rotation` before uploading.
    /// Replaces any existing texture.
    pub fn load_image(&mut self, ctx: &Context, img: &DecodedImage, rotation: Rotation, preserve_zoom: bool) {
        let (rgba, w, h) = apply_rotation(img, rotation);
        
        let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        self.image_size = Vec2::new(w as f32, h as f32);
        self.texture    = Some(ctx.load_texture(
            "current_image",
            color_image,
            TextureOptions::default(),
        ));

        if !preserve_zoom {
            self.fit_to_window = true;
            self.pan = Vec2::ZERO;
        }
    }

    /// Clear the canvas (called when no image is open, or on decode failure).
    pub fn clear(&mut self) {
        self.texture    = None;
        self.image_size = Vec2::ZERO;
        self.zoom       = 1.0;
        self.pan        = Vec2::ZERO;
        self.selection  = None;
    }

    pub fn has_image(&self) -> bool { self.texture.is_some() }

    // ── Zoom ─────────────────────────────────────────────────────────────

    /// Toggle between fit-to-window and the last manually-set zoom level.
    pub fn toggle_fit(&mut self, available: Vec2) {
        if self.fit_to_window {
            self.fit_to_window = false;
            self.zoom          = self.saved_zoom.unwrap_or(1.0);
        } else {
            self.saved_zoom    = Some(self.zoom);
            self.fit_to_window = true;
            self.zoom          = fit_zoom(self.image_size, available);
            self.pan           = Vec2::ZERO;
        }
    }

    /// Set zoom to 100 % and centre the image.
    pub fn zoom_actual_size(&mut self) {
        self.fit_to_window = false;
        self.zoom          = 1.0;
        self.pan           = Vec2::ZERO;
    }

    /// Apply a multiplicative zoom delta, clamped to [0.05, 32.0].
    ///
    /// If `anchor_screen` is supplied (e.g. the cursor position), the point
    /// under the cursor is held fixed during zoom.
    pub fn apply_zoom_delta(
        &mut self,
        delta:         f32,
        anchor_screen: Option<Pos2>,
        canvas_rect:   Rect,
    ) {
        let old_zoom   = self.zoom;
        self.zoom      = (self.zoom * delta).clamp(0.05, 32.0);
        self.fit_to_window = false;

        if let Some(anchor) = anchor_screen {
            // The image center is canvas_rect.center() + self.pan.
            // We want the point under the anchor to stay under the anchor after scaling.
            let image_centre = canvas_rect.center() + self.pan;
            let offset       = anchor - image_centre;
            let correction   = offset * (1.0 - self.zoom / old_zoom);
            self.pan        += correction;
        }
    }

    /// Recalculate the fit-to-window zoom. Must be called every frame.
    pub fn recalc_fit(&mut self, available: Vec2) {
        if self.fit_to_window && self.image_size != Vec2::ZERO {
            self.zoom = fit_zoom(self.image_size, available);
        }
    }

    /// The display size of the current image at the current zoom level.
    pub fn display_size(&self) -> Vec2 {
        self.image_size * self.zoom
    }

    /// The rectangle in which the image should be drawn, centred in `canvas`.
    pub fn image_rect(&self, canvas: Rect) -> Rect {
        let size   = self.display_size();
        let offset = (canvas.size() - size) * 0.5 + self.pan;
        Rect::from_min_size(canvas.min + offset, size)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the zoom level that makes `image_size` fit entirely within
/// `available`, preserving aspect ratio.
pub fn fit_zoom(image_size: Vec2, available: Vec2) -> f32 {
    if image_size.x == 0.0 || image_size.y == 0.0
        || available.x == 0.0 || available.y == 0.0
    {
        return 1.0;
    }
    (available.x / image_size.x).min(available.y / image_size.y)
}

/// Apply `rotation` to `img`, returning new RGBA pixels and dimensions.
fn apply_rotation(img: &DecodedImage, rotation: Rotation) -> (Vec<u8>, usize, usize) {
    use image::imageops;
    
    // Wrap raw pixels in an ImageBuffer for processing
    // NOTE: In a performance-critical app, we might want to avoid cloning
    // img.rgba here if we can rotate in-place or if we cache the rotated version.
    let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
        img.width, img.height, img.rgba.clone()
    ).expect("Invalid image buffer");

    match rotation {
        Rotation::None  => (img.rgba.clone(), img.width as usize, img.height as usize),
        Rotation::Cw90  => {
            let res = imageops::rotate90(&buffer);
            let (w, h) = (res.width() as usize, res.height() as usize);
            (res.into_raw(), w, h)
        }
        Rotation::Cw180 => {
            let res = imageops::rotate180(&buffer);
            let (w, h) = (res.width() as usize, res.height() as usize);
            (res.into_raw(), w, h)
        }
        Rotation::Cw270 => {
            let res = imageops::rotate270(&buffer);
            let (w, h) = (res.width() as usize, res.height() as usize);
            (res.into_raw(), w, h)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))
    }

    #[test]
    fn fit_zoom_respects_narrower_dimension() {
        // 1000×500 image in a 500×500 canvas → limited by width → 0.5
        let z = fit_zoom(Vec2::new(1000.0, 500.0), Vec2::new(500.0, 500.0));
        assert!((z - 0.5).abs() < 1e-6, "z = {z}");
    }

    #[test]
    fn fit_zoom_respects_shorter_dimension() {
        // 200×400 image in a 800×600 canvas → limited by height → 1.5
        let z = fit_zoom(Vec2::new(200.0, 400.0), Vec2::new(800.0, 600.0));
        assert!((z - 1.5).abs() < 1e-6, "z = {z}");
    }

    #[test]
    fn fit_zoom_handles_zero_sizes() {
        assert_eq!(fit_zoom(Vec2::ZERO, Vec2::new(800.0, 600.0)), 1.0);
        assert_eq!(fit_zoom(Vec2::new(800.0, 600.0), Vec2::ZERO), 1.0);
    }

    #[test]
    fn new_viewer_has_no_image() {
        assert!(!ViewerState::new().has_image());
    }

    #[test]
    fn zoom_is_clamped_at_maximum() {
        let mut v = ViewerState::new();
        for _ in 0..100 {
            v.apply_zoom_delta(2.0, None, canvas());
        }
        assert!(v.zoom <= 32.0, "zoom = {}", v.zoom);
    }

    #[test]
    fn zoom_is_clamped_at_minimum() {
        let mut v = ViewerState::new();
        for _ in 0..100 {
            v.apply_zoom_delta(0.1, None, canvas());
        }
        assert!(v.zoom >= 0.05, "zoom = {}", v.zoom);
    }

    #[test]
    fn zoom_actual_size_resets_to_100_percent() {
        let mut v  = ViewerState::new();
        v.apply_zoom_delta(3.0, None, canvas());
        v.zoom_actual_size();
        assert_eq!(v.zoom, 1.0);
        assert!(!v.fit_to_window);
    }

    #[test]
    fn toggle_fit_switches_modes_and_restores_zoom() {
        let available = Vec2::new(800.0, 600.0);
        let mut v     = ViewerState::new();
        v.image_size  = Vec2::new(400.0, 300.0);
        v.fit_to_window = false;
        v.zoom          = 2.5;

        v.toggle_fit(available);  // → fit
        assert!(v.fit_to_window);

        v.toggle_fit(available);  // → back to 2.5
        assert!(!v.fit_to_window);
        assert!((v.zoom - 2.5).abs() < 1e-6, "zoom = {}", v.zoom);
    }

    #[test]
    fn image_rect_is_centred_when_pan_is_zero() {
        let canvas    = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut v     = ViewerState::new();
        v.image_size  = Vec2::new(400.0, 300.0);
        v.zoom        = 1.0;
        v.pan         = Vec2::ZERO;

        let rect = v.image_rect(canvas);
        assert!((rect.min.x - 200.0).abs() < 1e-4, "min.x = {}", rect.min.x);
        assert!((rect.min.y - 150.0).abs() < 1e-4, "min.y = {}", rect.min.y);
    }
}
