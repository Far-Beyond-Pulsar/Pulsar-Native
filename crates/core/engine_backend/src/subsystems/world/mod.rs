//! # World Subsystem
//!
//! This module manages the game world, including loading levels, managing actors, and handling world state.
//! It utilizes the PebbleVault crate for efficient spatial storage and retrieval of game objects.
//! The World struct provides methods to add, remove, and query actors within the game world.
//! It is designed to be extensible for future features such as dynamic world generation and environmental effects.
//!
//! # Design decisions
//! - **PebbleVault Integration**: Chosen for its efficient spatial storage capabilities, allowing for quick access
//!   to game objects based on their location and for levels to be partitioned into regions by default.
//! - **Modular Structure**: The World subsystem is designed to be modular, making it easy to extend with additional
//!
//! # Features
//! - Load and manage game levels
//! - Add, remove, and query actors
//! - Spatial partitioning of the world for performance optimization
//! - Future support for dynamic world generation and environmental effects
//! - Integration with other subsystems like rendering and physics
//! - Designed for scalability to handle large game worlds
//! - Error handling and logging for robust operation
//! - Unit tests for core functionalities

use pebble::spacial_store::sqlite_backend::SqliteDatabase;

/// Minimal actor stored in the PebbleVault spatial database.
/// (Formerly in `subsystems::classes`.)
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Actor {
    pub id: u64,
    pub name: String,
}

// NOTE: World cannot implement the Subsystem trait due to PebbleVault::VaultManager
// not implementing Send + Sync (it contains Box<dyn PersistenceBackend> without bounds).
// This is a limitation of the PebbleVault crate.

pub struct World {
    // World data and methods
    vault: pebble::VaultManager<Actor>,
}
impl World {
    pub fn new(level_name: &str) -> Self {
        let db = SqliteDatabase::new(level_name)
            .expect("Failed to create SqliteDatabase, probably FS permission issues");
        let vault = pebble::VaultManager::new(Box::new(db)).expect("Failed to create VaultManager");

        Self { vault }
    }

    /// Persists all in-memory databases to disk.
    ///
    /// This function saves all objects from all regions to the persistent database.
    /// It's important to call this method periodically to ensure data is not lost in case of unexpected shutdowns.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - An empty result if successful, or an error message if not.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// # use your_crate::{VaultManager, CustomData};
    /// # let mut vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// vault_manager.persist_to_disk().expect("Failed to persist data to disk");
    /// ```
    ///
    /// # Notes
    ///
    /// - This operation can be time-consuming for large datasets. Consider running it in a separate thread.
    /// - The method provides progress feedback using a progress bar.
    /// - All existing points in the database are cleared before persisting the current state.
    pub fn save_world(&self) -> Result<(), String> {
        profiling::profile_scope!("Subsystem::World::save_world");
        self.vault.persist_to_disk()
    }
}
