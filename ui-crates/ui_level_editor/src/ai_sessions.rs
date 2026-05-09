use crate::level_editor::LevelEditorState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock, Weak};

static OPEN_SCENES: LazyLock<RwLock<HashMap<PathBuf, Weak<parking_lot::RwLock<LevelEditorState>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn register_open_scene(path: &Path, state: &Arc<parking_lot::RwLock<LevelEditorState>>) {
    let key = normalize_path(path);
    if let Ok(mut map) = OPEN_SCENES.write() {
        map.insert(key, Arc::downgrade(state));
    }
}

pub fn unregister_open_scene(path: &Path) {
    let key = normalize_path(path);
    if let Ok(mut map) = OPEN_SCENES.write() {
        map.remove(&key);
    }
}

pub fn get_open_scene_state(path: &Path) -> Option<Arc<parking_lot::RwLock<LevelEditorState>>> {
    let key = normalize_path(path);
    let mut remove_key = false;

    let state = if let Ok(map) = OPEN_SCENES.read() {
        map.get(&key).and_then(|weak| {
            let upgraded = weak.upgrade();
            if upgraded.is_none() {
                remove_key = true;
            }
            upgraded
        })
    } else {
        None
    };

    if remove_key {
        if let Ok(mut map) = OPEN_SCENES.write() {
            map.remove(&key);
        }
    }

    state
}
