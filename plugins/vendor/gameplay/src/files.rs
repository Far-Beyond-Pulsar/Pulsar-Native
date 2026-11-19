//! The plugin's folder, `plugins/Gameplay/`: the default player character as a
//! `.class` folder, served read-only from the files embedded here.
//!
//! A level selects it in World Settings > Player Character like any other
//! implementer of the role's trait; the roles system spawns it. Nothing is
//! spawned for a level that does not select a character: the free camera is
//! the only fallback.

use std::sync::Arc;

use engine_fs::{FolderSource, InGame, MemoryFolder, PluginFolder};

/// The default player character's class folder, embedded; each path is where
/// it sits in the plugin's folder.
pub const FILES: &[engine_fs::EmbeddedFile] = engine_fs::embedded_files!("assets":
    "classes/PlayerCharacter.class/class.json",
    "classes/PlayerCharacter.class/prefab.json",
    "classes/PlayerCharacter.class/graph_save.json",
);

/// Where the class's graph is in a project: what a level's World Settings
/// store for the role when this character is selected.
pub const PLAYER_CHARACTER_GRAPH: &str =
    "plugins/Gameplay/classes/PlayerCharacter.class/graph_save.json";

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(MemoryFolder::with_files(folder, FILES))
}

/// The plugin's one file provider.
pub(crate) fn provider(origin: &'static str) -> engine_fs::FileProvider {
    engine_fs::FileProvider {
        origin,
        open,
        // A game does not ask the plugin: it serves the packager's snapshot.
        in_game: InGame::Snapshot,
    }
}
