//! Pulsar scene file format and shared scene-record helpers.
//!
//! # Usage (runtime)
//!
//! Standalone games and PIE hydrate the level file into the shared
//! `WorldSceneStore`/SceneDb world via `engine_backend::scene::RuntimeLevel`
//! (Pulsar-Native#637), so renderer and gameplay see ONE copy of scene
//! state. The legacy loader that dispatched records straight into a Helio
//! scene was removed in Pulsar-Native#1035 (Phase 4).
//!
//! ```rust,ignore
//! use engine_backend::scene::RuntimeLevel;
//!
//! let level = RuntimeLevel::load(&project_root.join("scenes/default_level.json"))?;
//! let store = level.store(); // Arc<RwLock<WorldSceneStore>> -- the one world
//! ```
//!
//! # Usage (editor / save)
//!
//! ```rust,ignore
//! use pulsar_scene::{SceneFile, SceneObject, ObjectType, MeshType};
//!
//! let file = SceneFile {
//!     version: 1,
//!     objects: vec![
//!         SceneObject {
//!             id: "ground".into(),
//!             name: "Ground".into(),
//!             object_type: ObjectType::Mesh(MeshType::Plane),
//!             scale: [10.0, 1.0, 10.0],
//!             ..Default::default()
//!         },
//!     ],
//! };
//! file.save(Path::new("scenes/default_level.json"))?;
//! ```

pub mod format;
pub mod loader;

// Flatten the most-used types to the crate root.
pub use format::{
    BlueprintBinding, BlueprintBindings, LightType, MeshType, ObjectType, SceneFile,
    SceneLoadError, SceneObject,
};
pub use loader::{
    build_transform_parts, component_instances_from_props,
    migrate_legacy_material_override_records,
};
