//! Image metadata extraction.
//!
//! Currently reads PNG `tEXt` and `iTXt` chunks, which is where ComfyUI,
//! Automatic1111, and InvokeAI embed their workflow/prompt data.
//! EXIF (JPEG/TIFF/WebP) support is implemented via the `exif` crate.

use std::path::Path;
use std::fs::File;
use std::io::BufReader;

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
    let Ok(file) = File::open(path) else { return vec![] };
    let reader = BufReader::new(file);
    let img_reader = image::ImageReader::new(reader).with_guessed_format();
    
    let mut entries = if let Ok(reader) = img_reader {
        match reader.format() {
            Some(image::ImageFormat::Png)  => read_png(path),
            Some(image::ImageFormat::Jpeg) => read_exif(path),
            Some(image::ImageFormat::Tiff) => read_exif(path),
            Some(image::ImageFormat::WebP) => read_exif(path),
            _                              => vec![],
        }
    } else if is_raw_extension(path) {
        read_exif(path)
    } else {
        vec![]
    };

    // Post-process entries for known AI formats (JSON pretty-printing, etc.)
    for entry in &mut entries {
        // 1. Try JSON pretty-print (ComfyUI workflow/prompt, InvokeAI metadata)
        if entry.value.trim().starts_with('{') || entry.value.trim().starts_with('[') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&entry.value) {
                if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                    entry.value = pretty;
                }
            }
        }
        
        // 2. A1111 "parameters" — usually a prompt block followed by "Steps: 20, Sampler: ..."
        // We'll leave it as-is but ensure newlines are preserved.
    }

    entries
}

fn is_raw_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else { return false };
    matches!(
        ext.to_lowercase().as_str(),
        "arw" | "cr2" | "cr3" | "nef" | "nrw" | "orf" | "raf" | "rw2" | "dng"
    )
}

/// Returns the EXIF orientation tag (1-8) if present.
pub fn get_orientation(path: &Path) -> Option<u32> {
    let Ok(file) = File::open(path) else { return None };
    let reader = BufReader::new(file);
    let img_reader = image::ImageReader::new(reader).with_guessed_format();
    
    let is_raw = is_raw_extension(path);

    if let Ok(reader) = img_reader {
        match reader.format() {
            Some(image::ImageFormat::Jpeg) | Some(image::ImageFormat::Tiff) | Some(image::ImageFormat::WebP) => {
                let file = File::open(path).ok()?;
                let mut reader = BufReader::new(file);
                let exifreader = exif::Reader::new();
                let exif = exifreader.read_from_container(&mut reader).ok()?;
                return exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
                    .value.get_uint(0);
            }
            _ => {}
        }
    }

    if is_raw {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let exifreader = exif::Reader::new();
        let exif = exifreader.read_from_container(&mut reader).ok()?;
        return exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
            .value.get_uint(0);
    }

    None
}

// ---------------------------------------------------------------------------
// PNG — tEXt and iTXt chunks
// ---------------------------------------------------------------------------

fn read_png(path: &Path) -> Vec<MetaEntry> {
    let Ok(file) = File::open(path) else { return vec![] };
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
// JPEG/TIFF/WebP — EXIF
// ---------------------------------------------------------------------------

fn read_exif(path: &Path) -> Vec<MetaEntry> {
    let Ok(file) = File::open(path) else { return vec![] };
    let mut reader = BufReader::new(file);
    let exifreader = exif::Reader::new();
    let Ok(exif) = exifreader.read_from_container(&mut reader) else { return vec![] };
    
    let mut entries = Vec::new();
    for field in exif.fields() {
        entries.push(MetaEntry {
            key:   field.tag.to_string(),
            value: field.display_value().with_unit(&exif).to_string(),
        });
    }
    entries
}
