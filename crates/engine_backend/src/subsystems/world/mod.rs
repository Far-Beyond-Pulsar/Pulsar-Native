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

use std::sync::Arc;
use PebbleVault as pebble;
use PebbleVault::spacial_store::sqlite_backend::SqliteDatabase;
use PebbleVault::SpatialObject;
use super::classes::actor::Actor;
use uuid::Uuid;

// NOTE: World cannot implement the Subsystem trait due to PebbleVault::VaultManager
// not implementing Send + Sync (it contains Box<dyn PersistenceBackend> without bounds).
// This is a limitation of the PebbleVault crate.

pub struct World {
    // World data and methods
    vault: pebble::VaultManager<Actor>,
}
impl World {
    pub fn new(level_name: &str) -> Self {
        let db = SqliteDatabase::new(level_name).expect(
            "Failed to create SqliteDatabase, probably FS permission issues"
        );
        let vault = pebble::VaultManager::new(Box::new(db)).expect("Failed to create VaultManager");

        Self {
            vault,
        }
    }

    /// Adds an object to a specific region.
    ///
    /// This function creates a new SpatialObject and adds it to both the in-memory RTree
    /// and the persistent database. It's used to populate your world with entities.
    ///
    /// # Arguments
    ///
    /// * `region_id` - The UUID of the region to add the object to.
    /// * `uuid` - The UUID of the object being added.
    /// * `object_type` - The type of the object being added (e.g., "player", "building", "resource").
    /// * `x` - The x-coordinate of the object.
    /// * `y` - The y-coordinate of the object.
    /// * `z` - The z-coordinate of the object.
    /// * `size_x` - The width of the object along the X-axis.
    /// * `size_y` - The height of the object along the Y-axis.
    /// * `size_z` - The depth of the object along the Z-axis.
    ///
    /// * `custom_data` - The custom data associated with the object, wrapped in an `Arc`.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - An empty result if successful, or an error message if not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use your_crate::{VaultManager, CustomData};
    /// # use uuid::Uuid;
    /// # use std::sync::Arc;
    /// # let mut vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// # let region_id = vault_manager.create_or_load_region([0.0, 0.0, 0.0], 100.0).unwrap();
    /// let object_id = Uuid::new_v4();
    /// let custom_data = Arc::new(CustomData { /* ... */ });
    /// vault_manager.add_object(region_id, object_id, "player", 1.0, 2.0, 3.0, custom_data).expect("Failed to add object");
    /// ```
    ///
    /// # Notes
    ///
    /// - The object is added to the specified region regardless of its coordinates. Ensure the coordinates fall within the region's cubic bounds for consistent behavior.
    /// - If an object with the same UUID already exists, it will be overwritten.
    /// - The `custom_data` is stored as an `Arc<T>` to allow efficient sharing of data between objects.
    fn add_actor_private(
        &self,
        region_id: Uuid,
        uuid: Uuid,
        object_type: &str,
        x: f64,
        y: f64,
        z: f64,
        size_x: f64,
        size_y: f64,
        size_z: f64,
        custom_data: Arc<Actor>
    ) -> Result<(), String> {
        self.vault.add_object(region_id, uuid, object_type, x, y, z, size_x, size_y, size_z, custom_data)
    }

    /// Gets a reference to an object by its ID.
    ///
    /// This method searches for an object with the given UUID across all regions.
    ///
    /// # Arguments
    ///
    /// * `object_id` - The UUID of the object to retrieve.
    ///
    /// # Returns
    ///
    /// * `Result<Option<SpatialObject<T>>, String>` - An `Option` containing a clone of the object if found, or `None` if not found.
    ///
    /// # Examples
    ///
    /// ```
    /// # use your_crate::{VaultManager, CustomData};
    /// # use uuid::Uuid;
    /// # let vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// # let object_id = Uuid::new_v4();
    /// if let Ok(Some(object)) = vault_manager.get_object(object_id) {
    ///     tracing::debug!("Found object: {:?}", object);
    /// } else {
    ///     tracing::debug!("Object not found");
    /// }
    /// ```
    ///
    /// # Notes
    ///
    /// - This method returns a clone of the `SpatialObject`, including the `Arc<T>` custom data.
    /// - The search is performed across all regions, which may be slow for a large number of regions or objects.
    fn get_actor_private(&self, object_id: Uuid) -> Option<Arc<Actor>> {
        match self.vault.get_object(object_id) {
            Ok(Some(spatial_obj)) => Some(spatial_obj.custom_data.clone()),
            Ok(None) => None,
            Err(_) => None,
        }
    }

    /// Removes an object from its region and the persistent database.
    ///
    /// # Arguments
    ///
    /// * `object_id` - The UUID of the object to remove.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - An empty result if successful, or an error message if not.
    fn remove_actor_private(&mut self, id: Uuid) -> Option<Actor> {
            let actor = self.get_actor_private(id).map(|arc| (*arc).clone());
            match self.vault.remove_object(id) {
                Ok(_) => actor,
                Err(_) => None,
            }
        }

    /// Creates a new region or loads an existing one from the persistent database.
    ///
    /// This function is used to define spatial partitions in your world. If a region with the given
    /// center and size already exists, it returns the existing region's ID. Otherwise, it creates a new region.
    ///
    /// # Arguments
    ///
    /// * `center` - An array of 3 f64 values representing the x, y, z coordinates of the region's center.
    /// * `size` - The size (length of each side) of the cubic region.
    ///
    /// # Returns
    ///
    /// * `Result<Uuid, String>` - The UUID of the created or loaded region if successful, or an error message if not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use your_crate::{VaultManager, CustomData};
    /// # let mut vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// let center = [0.0, 0.0, 0.0];
    /// let size = 100.0;  // Creates a 100x100x100 cubic region
    /// let region_id = vault_manager.create_or_load_region(center, size).expect("Failed to create region");
    /// ```
    ///
    /// # Notes
    ///
    /// - Regions are cubic, defined by a center point and a size (length of each side).
    /// - Overlapping regions are allowed, but may impact performance for objects in the overlapped areas.
    fn create_or_load_region_private(&mut self, center: [f64; 3], size: f64) -> Result<Uuid, String> {
        self.vault.create_or_load_region(center, size)
    }

    /// Transfers a player (object) from one region to another.
    ///
    /// This function moves a player object from its current region to a new region,
    /// updating the in-memory RTree structure. The object's size and custom data are preserved.
    ///
    /// # Arguments
    ///
    /// * `player_uuid` - The UUID of the player to transfer.
    /// * `from_region_id` - The UUID of the source region.
    /// * `to_region_id` - The UUID of the destination region.
    ///
    /// # Returns
    ///
    /// * `Result<(), String>` - Ok if successful, or an error string if the transfer fails.
    ///
    /// # Examples
    ///
    /// ```
    /// # use your_crate::{VaultManager, CustomData};
    /// # use uuid::Uuid;
    /// # let mut vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// # let from_region_id = vault_manager.create_or_load_region([0.0, 0.0, 0.0], 100.0).unwrap();
    /// # let to_region_id = vault_manager.create_or_load_region([200.0, 200.0, 200.0], 100.0).unwrap();
    /// # let player_id = Uuid::new_v4();
    /// # let custom_data = Arc::new(CustomData { /* ... */ });
    /// # vault_manager.add_object(from_region_id, player_id, "player", 1.0, 2.0, 3.0, 1.0, 1.0, 1.0, custom_data).unwrap();
    /// vault_manager.transfer_player(player_id, from_region_id, to_region_id).expect("Failed to transfer player");
    /// ```
    ///
    /// # Notes
    ///
    /// - The player’s position is set to the center of the destination region.
    /// - The player's size and custom data are preserved.
    /// - This does **not** persist the change to the database; call `persist_to_disk()` to flush to disk.
    fn transfer_player_private(
        &self,
        player_uuid: Uuid,
        from_region_id: Uuid,
        to_region_id: Uuid
    ) -> Result<(), String> {
        self.vault.transfer_player(player_uuid, from_region_id, to_region_id)
    }

    /// Queries objects within a specific region.
    ///
    /// This function searches for objects within a given cubic bounding box in a specified region.
    /// It's useful for finding all objects in a particular area, such as for rendering or game logic.
    ///
    /// # Arguments
    ///
    /// * `region_id` - The UUID of the region to query.
    /// * `min_x`, `min_y`, `min_z` - The minimum coordinates of the bounding box.
    /// * `max_x`, `max_y`, `max_z` - The maximum coordinates of the bounding box.
    ///
    /// # Returns
    ///
    /// * `Result<Vec<SpatialObject<T>>, String>` - A vector of `SpatialObject`s within the bounding box if successful, or an error message if not.
    ///
    /// # Examples
    ///
    /// ```
    /// # use your_crate::{VaultManager, CustomData};
    /// # use uuid::Uuid;
    /// # let vault_manager: VaultManager<CustomData> = VaultManager::new("path/to/database.db").unwrap();
    /// # let region_id = Uuid::new_v4();
    /// // Query a 10x10x10 cubic area
    /// let objects = vault_manager.query_region(region_id, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0).expect("Failed to query region");
    /// for object in objects {
    ///     tracing::debug!("Found object: {:?}", object.uuid);
    /// }
    /// ```
    ///
    /// # Notes
    ///
    /// - The query is performed using an R-tree, which provides efficient spatial searching.
    /// - Objects intersecting the cubic bounding box are included in the results.
    /// - The query box does not need to align with region boundaries.
    fn query_region_private(
        &self,
        region_id: Uuid,
        min_x: f64,
        min_y: f64,
        min_z: f64,
        max_x: f64,
        max_y: f64,
        max_z: f64
    ) -> Result<Vec<SpatialObject<Actor>>, String> {
        self.vault.query_region(region_id, min_x, min_y, min_z, max_x, max_y, max_z)
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
    /// ```
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
        self.vault.persist_to_disk()
    }
}
