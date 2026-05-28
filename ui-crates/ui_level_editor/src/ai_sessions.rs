//! AI tool session registry — thread-safe scene access for background AI tools.
//!
//! AI tools run synchronously off the GPUI thread.  They access scene data
//! through `SceneDatabase` (which is Arc-based and internally lock-free for
//! reads) and signal mutations by incrementing `scene_revision`.  The GPUI
//! main-thread watcher propagates the increment into the `Entity<LevelEditorState>`
//! so panels re-render.

use crate::level_editor::scene_database::SceneDatabase;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64};

/// Per-scene data exposed to AI tools.
#[derive(Clone)]
pub struct OpenSceneHandle {
    /// Scene data — Arc-based, safe to clone and read from any thread.
    pub scene_db: SceneDatabase,
    /// Monotonic counter AI tools increment after mutating the scene.
    /// The GPUI watcher task picks this up and triggers a panel re-render.
    pub revision: Arc<AtomicU64>,
    /// Set to `true` by AI tools after any mutation so the editor title bar
    /// shows the unsaved-changes indicator.
    pub has_unsaved_changes: Arc<AtomicBool>,
}

static OPEN_SCENES: LazyLock<RwLock<HashMap<PathBuf, OpenSceneHandle>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn register_open_scene(
    path: &Path,
    scene_db: &SceneDatabase,
    revision: &Arc<AtomicU64>,
    has_unsaved_changes: &Arc<AtomicBool>,
) {
    if let Ok(mut map) = OPEN_SCENES.write() {
        map.insert(normalize(path), OpenSceneHandle {
            scene_db: scene_db.clone(),
            revision: revision.clone(),
            has_unsaved_changes: has_unsaved_changes.clone(),
        });
    }
}

pub fn unregister_open_scene(path: &Path) {
    if let Ok(mut map) = OPEN_SCENES.write() {
        map.remove(&normalize(path));
    }
}

pub fn get_open_scene(path: &Path) -> Option<OpenSceneHandle> {
    if let Ok(map) = OPEN_SCENES.read() {
        map.get(&normalize(path)).cloned()
    } else {
        None
    }
}
