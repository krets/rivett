//! Image metadata extraction.
//!
//! Currently reads PNG `tEXt` and `iTXt` chunks, which is where ComfyUI,
//! Automatic1111, and InvokeAI embed their workflow/prompt data.
//! EXIF (JPEG/TIFF) support is a TODO.

use std::path::Path;

/// A single key/value metadata entry.
#[derive(Debug, Clone)]
pub struct MetaEntry {
    pub key:   String,
    /// Raw value string, potentially very long (ComfyUI JSON can be MBs).
    pub value: String,
}

/// Extract all readable metadata from a file.
/// Returns an empty vec for unsupported formats or unreadable files.
pub fn read_metadata(path: &Path) -> Vec<MetaEntry> {
    match path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png")                    => read_png(path),
        Some("jpg") | Some("jpeg")     => read_jpeg_exif(path),
        _                              => vec![],
    }
}

// ---------------------------------------------------------------------------
// PNG — tEXt and iTXt chunks
// ---------------------------------------------------------------------------

fn read_png(path: &Path) -> Vec<MetaEntry> {
    let Ok(file) = std::fs::File::open(path) else { return vec![] };
    let decoder = png::Decoder::new(file);
    let Ok(reader) = decoder.read_info() else { return vec![] };
    let info = reader.info();
    let mut entries = Vec::new();

    for chunk in &info.uncompressed_latin1_text {
        entries.push(MetaEntry {
            key:   chunk.keyword.clone(),
            value: chunk.text.clone(),
        });
    }

    for chunk in &info.utf8_text {
        // `text` may be None if the chunk is compressed — use the raw bytes
        // as a fallback.  In practice ComfyUI always uses uncompressed iTXt.
        let value = chunk.get_text().unwrap_or_default();
        if !value.is_empty() {
            entries.push(MetaEntry {
                key:   chunk.keyword.clone(),
                value,
            });
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// JPEG — minimal EXIF via raw scan (no external crate required)
// ---------------------------------------------------------------------------
// TODO: replace with kamadak-exif for full EXIF parsing.

fn read_jpeg_exif(_path: &Path) -> Vec<MetaEntry> {
    // Placeholder — EXIF parsing not yet implemented.
    vec![]
}