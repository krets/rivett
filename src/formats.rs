//! Supported image formats for Rivett.
//!
//! Format detection is extension-based only; no magic-byte sniffing is
//! performed at this layer (the `image` crate handles that during decode).

use std::path::Path;

/// Every image format Rivett can decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedFormat {
    Png,
    Jpeg,
    WebP,
    Bmp,
    Tiff,
    Gif,
    Exr,
    Svg,
}

impl SupportedFormat {
    /// Infer the format from a file path's extension (case-insensitive).
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        Self::from_extension(&ext)
    }

    /// Infer the format from a lowercase extension string.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "png"                   => Some(Self::Png),
            "jpg" | "jpeg"          => Some(Self::Jpeg),
            "webp"                  => Some(Self::WebP),
            "bmp"                   => Some(Self::Bmp),
            "tif" | "tiff"          => Some(Self::Tiff),
            "gif"                   => Some(Self::Gif),
            "exr"                   => Some(Self::Exr),
            "svg"                   => Some(Self::Svg),
            _                       => None,
        }
    }

    /// All known lowercase extensions, including aliases.
    pub fn all_extensions() -> &'static [&'static str] {
        &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "exr", "svg"]
    }

    /// `true` if this format can store rotation losslessly via metadata
    /// (EXIF orientation tag) without recompressing pixel data.
    pub fn supports_lossless_rotation_metadata(self) -> bool {
        matches!(self, Self::Jpeg | Self::Tiff)
    }

    /// Human-readable display name.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Png  => "PNG",
            Self::Jpeg => "JPEG",
            Self::WebP => "WebP",
            Self::Bmp  => "BMP",
            Self::Tiff => "TIFF",
            Self::Gif  => "GIF",
            Self::Exr  => "EXR",
            Self::Svg  => "SVG",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn every_listed_extension_is_recognised() {
        for ext in SupportedFormat::all_extensions() {
            assert!(
                SupportedFormat::from_extension(ext).is_some(),
                "extension '{ext}' should be recognised",
            );
        }
    }

    #[test]
    fn unknown_extensions_return_none() {
        for ext in ["exe", "txt", "mp4", "docx", ""] {
            assert_eq!(SupportedFormat::from_extension(ext), None, "extension '{ext}'");
        }
    }

    #[test]
    fn path_detection_is_case_insensitive() {
        assert_eq!(SupportedFormat::from_path(&PathBuf::from("photo.PNG")),  Some(SupportedFormat::Png));
        assert_eq!(SupportedFormat::from_path(&PathBuf::from("photo.JPEG")), Some(SupportedFormat::Jpeg));
        assert_eq!(SupportedFormat::from_path(&PathBuf::from("photo.Gif")),  Some(SupportedFormat::Gif));
        assert_eq!(SupportedFormat::from_path(&PathBuf::from("drawing.SVG")), Some(SupportedFormat::Svg));
    }

    #[test]
    fn path_without_extension_returns_none() {
        assert_eq!(SupportedFormat::from_path(&PathBuf::from("Makefile")), None);
    }

    #[test]
    fn jpeg_supports_lossless_rotation() {
        assert!(SupportedFormat::Jpeg.supports_lossless_rotation_metadata());
        assert!(SupportedFormat::Tiff.supports_lossless_rotation_metadata());
    }

    #[test]
    fn other_formats_do_not_support_lossless_rotation() {
        for fmt in [SupportedFormat::Png, SupportedFormat::WebP, SupportedFormat::Bmp, SupportedFormat::Gif] {
            assert!(
                !fmt.supports_lossless_rotation_metadata(),
                "{:?} should not claim lossless rotation support",
                fmt,
            );
        }
    }
}
