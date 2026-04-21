//! Directory scanning, sort management, and image decoding.
//!
//! [`DirectoryListing`] owns the ordered list of image paths and a cursor.
//! Navigation never wraps: the list has a hard start and end, matching the
//! spec.
//!
//! [`load_image`] is a thin wrapper around `image::open` that returns a
//! descriptive error string instead of an `image::ImageError`.

use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::formats::SupportedFormat;
use crate::settings::SortOrder;

// ---------------------------------------------------------------------------
// DirectoryListing
// ---------------------------------------------------------------------------

/// Sorted list of supported image files in a single directory, with a cursor.
#[derive(Debug, Default)]
pub struct DirectoryListing {
    pub dir_path:      PathBuf,
    pub files:         Vec<PathBuf>,
    pub current_index: usize,
    /// When set, only images matching this filter (as found in the database) are shown.
    pub rating_filter: Option<crate::session::RatingFilter>,
}

impl DirectoryListing {
    /// Scan `dir` for supported image files and sort according to `order`.
    pub fn scan(
        dir:    &Path,
        order:  SortOrder,
        filter: Option<crate::session::RatingFilter>,
        db:     Option<&crate::db::Database>,
    ) -> std::io::Result<Self> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && SupportedFormat::from_path(p).is_some())
            .collect();

        if let Some(db) = db {
            let dir_str = dir.to_string_lossy();
            if let Ok(Some(d_rec)) = db.find_directory_by_path(&dir_str) {
                // Prune records for files that no longer exist
                let _ = Self::prune_missing_images(db, d_rec.id, &files);

                if let Some(f) = filter {
                    files.retain(|p| {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if let Ok(Some(img_rec)) = db.get_image(d_rec.id, fname) {
                            f.matches(img_rec.rating)
                        } else {
                            false
                        }
                    });
                }
            } else if filter.is_some() {
                files.clear();
            }
        }

        sort_paths(&mut files, order);

        Ok(Self {
            dir_path: dir.to_path_buf(),
            files,
            current_index: 0,
            rating_filter: filter,
        })
    }

    /// Delete DB records for images in `directory_id` that are NOT in `current_files`.
    fn prune_missing_images(
        db:            &crate::db::Database,
        directory_id:  i64,
        current_files: &[PathBuf],
    ) -> rusqlite::Result<()> {
        let db_images = db.get_images(directory_id)?;
        let disk_names: std::collections::HashSet<_> = current_files.iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();

        for img in db_images {
            if !disk_names.contains(img.filename.as_str()) {
                db.delete_image(directory_id, &img.filename)?;
            }
        }
        Ok(())
    }


    /// Create a listing of all rated images across the library that match `filter`.
    pub fn scan_global(
        db:     &crate::db::Database,
        filter: crate::session::RatingFilter,
    ) -> std::io::Result<Self> {
        let records = db.get_rated_filtered(filter)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        
        let mut files = Vec::new();
        for (dir, rec) in records {
            let path = dir.join(&rec.filename);
            if path.exists() {
                files.push(path);
            } else {
                // Quietly prune missing file from DB
                let _ = db.delete_image(rec.directory_id, &rec.filename);
            }
        }

        Ok(Self {
            dir_path: PathBuf::new(), // Special empty path for global view
            files,
            current_index: 0,
            rating_filter: Some(filter),
        })
    }

    /// Move the cursor to `target`. Returns `false` if it is not in the list.
    pub fn seek_to(&mut self, target: &Path) -> bool {
        if let Some(idx) = self.files.iter().position(|p| p == target) {
            self.current_index = idx;
            true
        } else {
            false
        }
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.current_index)
    }

    pub fn can_go_next(&self) -> bool {
        !self.files.is_empty() && self.current_index + 1 < self.files.len()
    }

    pub fn can_go_prev(&self) -> bool {
        self.current_index > 0
    }

    /// Advance cursor by one. Returns `false` (and does not move) at the end.
    pub fn go_next(&mut self) -> bool {
        if self.can_go_next() {
            self.current_index += 1;
            true
        } else {
            false
        }
    }

    /// Move cursor back by one. Returns `false` (and does not move) at the start.
    pub fn go_prev(&mut self) -> bool {
        if self.can_go_prev() {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize { self.files.len() }
    pub fn is_empty(&self) -> bool { self.files.is_empty() }

    /// 1-based position string, e.g. "7 / 42".
    pub fn position_label(&self) -> String {
        if self.files.is_empty() {
            "0 / 0".to_string()
        } else {
            format!("{} / {}", self.current_index + 1, self.files.len())
        }
    }

    /// Re-scan the directory in-place, preserving cursor position where possible.
    pub fn refresh(
        &mut self,
        order: SortOrder,
        db:    Option<&crate::db::Database>,
    ) -> std::io::Result<()> {
        let current = self.current().cloned();

        let fresh = if self.dir_path.as_os_str().is_empty() {
            if let (Some(db), Some(filter)) = (db, self.rating_filter) {
                Self::scan_global(db, filter)?
            } else {
                return Ok(()); // Should not happen
            }
        } else {
            Self::scan(&self.dir_path, order, self.rating_filter, db)?
        };

        self.files  = fresh.files;
        match current {
            Some(ref p) if !self.seek_to(p) => {
                // Image was removed or no longer matches filter; clamp cursor.
                self.current_index = self.current_index
                    .min(self.files.len().saturating_sub(1));
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Image loading
// ---------------------------------------------------------------------------

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::{self, Receiver, Sender};

// ... (DirectoryListing stays as is until load_image)

/// Decode `path` into a [`DynamicImage`].
///
/// Returns `Err(String)` with a human-readable message on failure. The caller
/// is responsible for displaying a placeholder and offering the ignore options
/// described in the spec.
pub fn load_image(path: &Path) -> Result<DynamicImage, String> {
    image::open(path)
        .map_err(|e| format!("could not decode {}: {e}", path.display()))
}

/// Simple LRU cache for decoded images.
pub struct ImageCache {
    /// Maps path to decoded image.
    images: HashMap<PathBuf, DynamicImage>,
    /// Tracks insertion order for LRU eviction.
    order:  VecDeque<PathBuf>,
    /// Maximum number of images to keep in memory.
    capacity: usize,
    
    /// Background loader channel to receive images.
    rx: Receiver<(PathBuf, DynamicImage)>,
    /// Sender for the background thread to use.
    tx: Sender<(PathBuf, DynamicImage)>,
    /// Set of paths currently being loaded in the background.
    pending: Arc<Mutex<HashMap<PathBuf, thread::JoinHandle<()>>>>,
}

impl ImageCache {
    pub fn new(capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            images: HashMap::new(),
            order:  VecDeque::new(),
            capacity,
            rx,
            tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&mut self, path: &Path) -> Option<&DynamicImage> {
        if self.images.contains_key(path) {
            // Move to back of LRU (most recently used)
            if let Some(pos) = self.order.iter().position(|p| p == path) {
                let p = self.order.remove(pos).unwrap();
                self.order.push_back(p);
            }
            self.images.get(path)
        } else {
            None
        }
    }

    pub fn insert(&mut self, path: PathBuf, image: DynamicImage) {
        if self.images.contains_key(&path) {
            return;
        }
        if self.images.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.images.remove(&oldest);
            }
        }
        self.order.push_back(path.clone());
        self.images.insert(path, image);
    }

    /// Check for any images that finished loading in the background.
    pub fn poll(&mut self) {
        while let Ok((path, img)) = self.rx.try_recv() {
            self.insert(path.clone(), img);
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&path);
            }
        }
    }

    /// Start loading an image in a background thread if it's not already cached or pending.
    pub fn prefetch(&self, path: PathBuf) {
        if self.images.contains_key(&path) {
            return;
        }

        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(&path) {
            return;
        }

        let tx = self.tx.clone();
        let p = path.clone();
        let handle = thread::spawn(move || {
            if let Ok(img) = load_image(&p) {
                let _ = tx.send((p, img));
            }
        });
        pending.insert(path, handle);
    }
}


// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

fn sort_paths(files: &mut [PathBuf], order: SortOrder) {
    match order {
        SortOrder::Name => {
            files.sort_by(|a, b| {
                let a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
                a.cmp(b)
            });
        }
        SortOrder::DateModified => {
            files.sort_by_key(|p| {
                p.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH)
            });
        }
        SortOrder::FileSize => {
            files.sort_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0));
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Create a temp directory with known image filenames.
    fn make_dir(names: &[&str]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        dir
    }

    #[test]
    fn scan_finds_supported_extensions() {
        let dir     = make_dir(&["b.png", "a.jpg", "c.bmp", "skip.txt"]);
        let listing = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        assert_eq!(listing.len(), 3, "txt should be excluded");
    }

    #[test]
    fn scan_sorts_by_name_ascending() {
        let dir     = make_dir(&["c.gif", "a.png", "b.jpg"]);
        let listing = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        let names: Vec<_> = listing.files.iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["a.png", "b.jpg", "c.gif"]);
    }

    #[test]
    fn navigation_does_not_wrap_at_end() {
        let dir     = make_dir(&["a.png", "b.png", "c.png"]);
        let mut l   = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        while l.go_next() {}
        assert!(!l.can_go_next());
        assert!(l.can_go_prev());
        assert!(!l.go_next(), "go_next at end must return false");
    }

    #[test]
    fn navigation_does_not_wrap_at_start() {
        let dir   = make_dir(&["a.png", "b.png"]);
        let mut l = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        assert!(!l.can_go_prev());
        assert!(!l.go_prev(), "go_prev at start must return false");
        assert_eq!(l.current_index, 0);
    }

    #[test]
    fn seek_to_positions_cursor_correctly() {
        let dir     = make_dir(&["a.png", "b.png", "c.png"]);
        let mut l   = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        let target  = dir.path().join("b.png");
        assert!(l.seek_to(&target));
        assert_eq!(l.current_index, 1);
    }

    #[test]
    fn seek_to_unknown_returns_false() {
        let dir   = make_dir(&["a.png"]);
        let mut l = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        assert!(!l.seek_to(&dir.path().join("nonexistent.png")));
        assert_eq!(l.current_index, 0, "cursor should be unchanged");
    }

    #[test]
    fn empty_directory_listing() {
        let dir   = make_dir(&["readme.txt"]);
        let l     = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        assert!(l.is_empty());
        assert!(l.current().is_none());
        assert!(!l.can_go_next());
        assert!(!l.can_go_prev());
    }

    #[test]
    fn position_label_is_1_based() {
        let dir   = make_dir(&["a.png", "b.png", "c.png"]);
        let mut l = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        assert_eq!(l.position_label(), "1 / 3");
        l.go_next();
        assert_eq!(l.position_label(), "2 / 3");
    }

    #[test]
    fn refresh_restores_cursor_to_same_file() {
        let dir    = make_dir(&["a.png", "b.png", "c.png"]);
        let mut l  = DirectoryListing::scan(dir.path(), SortOrder::Name).unwrap();
        l.seek_to(&dir.path().join("b.png"));
        l.refresh(SortOrder::Name).unwrap();
        assert_eq!(
            l.current().unwrap().file_name().unwrap(),
            "b.png",
        );
    }
}
