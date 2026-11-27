//! The live scenes a tool may look at (the SceneDB Inspector).
//!
//! Whoever owns a [`SharedScene`] [publishes](publish) it and keeps the
//! returned [`PublishedScene`] for as long as the scene should be listed.
//! The directory holds only weak handles: it owns no scene, copies nothing
//! and never locks one. A tool reads the scene through the handle itself,
//! under the scene's own lock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::scene::SharedScene;

type SceneLock = parking_lot::RwLock<pulsar_scenedb::SceneDb>;

/// Names a scene at the time it is listed (its level may change).
pub type SceneLabel = Box<dyn Fn() -> String + Send + Sync>;

struct Entry {
    id: u64,
    label: Arc<dyn Fn() -> String + Send + Sync>,
    scene: Weak<SceneLock>,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn entries() -> std::sync::MutexGuard<'static, Vec<Entry>> {
    ENTRIES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scene's entry in the directory; dropping it removes it.
#[must_use = "the scene is listed only while this is kept"]
#[derive(Debug)]
pub struct PublishedScene(u64);

impl Drop for PublishedScene {
    fn drop(&mut self) {
        entries().retain(|entry| entry.id != self.0);
    }
}

/// List `scene` under what `label` returns.
pub fn publish(scene: &SharedScene, label: SceneLabel) -> PublishedScene {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    entries().push(Entry {
        id,
        label: label.into(),
        scene: Arc::downgrade(scene),
    });
    PublishedScene(id)
}

/// A listed scene, alive while this is held.
#[derive(Clone)]
pub struct ListedScene {
    /// Stable while the publication is kept.
    pub id: u64,
    pub label: String,
    pub scene: SharedScene,
}

/// Every published scene that is still alive, oldest first.
pub fn list() -> Vec<ListedScene> {
    // The labels run outside the directory's lock: one may take its owner's
    // lock, and the owner may be publishing.
    let live: Vec<_> = entries()
        .iter()
        .filter_map(|entry| Some((entry.id, entry.label.clone(), entry.scene.upgrade()?)))
        .collect();
    live.into_iter()
        .map(|(id, label, scene)| ListedScene {
            id,
            label: label(),
            scene,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_is_listed_while_published_and_alive() {
        let scene: SharedScene = Arc::new(parking_lot::RwLock::new(crate::scene::new_scene()));
        let published = publish(&scene, Box::new(|| "listed-test".to_string()));
        let find = || list().into_iter().any(|s| s.label == "listed-test");
        assert!(find());
        drop(published);
        assert!(!find());

        let published = publish(&scene, Box::new(|| "listed-test".to_string()));
        drop(scene);
        assert!(!find(), "a dropped scene is not listed or kept alive");
        drop(published);
    }
}
