//! The plugin's example files: TypeScript classes (`<Class>/class.ts`),
//! provided read-only into every project under the plugin's folder at
//! `examples/`. TypeScript registers no file type of its own, so these are
//! the class directories the language compiles.

use std::sync::Arc;

use engine_fs::{EmbeddedFile, FolderSource, MemoryFolder, PluginFolder};

/// Every embedded example, at its path inside the plugin's folder.
pub const EXAMPLES: &[EmbeddedFile] = engine_fs::embedded_files!("examples_assets":
    "examples/README.md",
    "examples/Door/class.ts",
    "examples/Counter/class.ts",
    "examples/Patrol/class.ts",
    "examples/Hero/class.ts",
);

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(MemoryFolder::with_files(folder, EXAMPLES))
}

/// Register the plugin's file provider (the plugin's `build` calls this).
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(engine_fs::FileProvider {
        origin: p.origin(),
        open,
        // Embedded in the plugin; a game serves the packager's snapshot.
        in_game: engine_fs::InGame::Snapshot,
    });
}
