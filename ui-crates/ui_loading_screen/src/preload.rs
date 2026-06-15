//! Thread-safe store for data pre-loaded by the loading-screen background thread.
//!
//! The background thread scans the project while the splash animation plays.
//! By the time the animation finishes the data is already in memory, so
//! `PulsarApp::new_internal` can consume it instantly instead of blocking the
//! GPUI main thread with disk I/O.

use std::path::PathBuf;
use std::sync::Mutex;

/// A single file entry discovered during the loading-screen's background scan.
#[derive(Clone, Debug)]
pub struct PreloadedFileEntry {
    pub path: PathBuf,
    pub name: String,
}

static PRELOADED_FILES: Mutex<Vec<PreloadedFileEntry>> = Mutex::new(Vec::new());

/// Store the result of a background file scan.
/// Called from the loading-screen's worker thread.
pub fn store_preloaded_files(files: Vec<PreloadedFileEntry>) {
    if let Ok(mut guard) = PRELOADED_FILES.lock() {
        *guard = files;
    }
}

/// Drain and return the pre-scanned file list.
/// Intended to be called exactly once from `PulsarApp::new_internal`.
/// Returns an empty `Vec` if the loading screen was skipped or the scan is not
/// yet complete (the async fallback in the constructor handles that case).
pub fn take_preloaded_files() -> Vec<PreloadedFileEntry> {
    PRELOADED_FILES
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}
