//! Where the tools find the scene they act on.

use std::sync::Arc;

use pulsar_scenedb::{Entity, SceneDb, World};

/// The scene handle the editor, renderer and game tick share.
pub type SharedScene = Arc<parking_lot::RwLock<SceneDb>>;

/// Supplies the live scene and the editor's rules for it. The host (the
/// editor) implements this; tests use [`FixedScene`].
pub trait SceneSource: Send + Sync {
    /// The scene to edit now, or the reason there is none (no level open,
    /// Play-In-Editor running).
    fn scene(&self) -> Result<SharedScene, String>;

    /// Level rules a component instance must satisfy right after it was
    /// attached or edited (for example "a level has one sky"). `Err` carries
    /// the message for the model; the edit is rolled back.
    fn check_instance(&self, _world: &World, _instance: Entity) -> Result<(), String> {
        Ok(())
    }

    /// The live scene was changed by an agent (a committed write, an applied
    /// sandbox, a revert) and the scene lock is released again. The host
    /// marks the level dirty here. `summary` is a one-line description.
    fn scene_edited(&self, _summary: &str) {}
}

/// A source over one fixed scene.
pub struct FixedScene(pub SharedScene);

impl FixedScene {
    pub fn new() -> Self {
        Self(Arc::new(parking_lot::RwLock::new(SceneDb::new())))
    }

    /// The shared scene, for tests that inspect the world directly.
    pub fn handle(&self) -> SharedScene {
        self.0.clone()
    }
}

impl Default for FixedScene {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneSource for FixedScene {
    fn scene(&self) -> Result<SharedScene, String> {
        Ok(self.0.clone())
    }
}
