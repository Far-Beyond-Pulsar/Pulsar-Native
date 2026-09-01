//! Pulsar scene file format and (legacy) Helio renderer loader.
//!
//! # Usage (runtime)
//!
//! Runtime loading does NOT go through [`SceneLoader`] anymore
//! (Pulsar-Native#637): standalone games and PIE hydrate the level file
//! into the shared `WorldSceneStore`/SceneDb world via
//! `engine_backend::scene::RuntimeLevel`, so renderer and gameplay see ONE
//! copy of scene state. `SceneLoader` remains for import/legacy conversion.
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
//! use pulsar_scene::{SceneFile, SceneObject, SceneTransform, ObjectType, MeshType};
//!
//! let file = SceneFile {
//!     version: serde_json::json!("2.1"),
//!     objects: vec![
//!         SceneObject {
//!             id: "ground".into(),
//!             name: "Ground".into(),
//!             object_type: ObjectType::Mesh(MeshType::Plane),
//!             transform: SceneTransform { scale: [10.0, 1.0, 10.0], ..Default::default() },
//!             ..Default::default()
//!         },
//!     ],
//!     ..Default::default()
//! };
//! file.save(Path::new("scenes/default_level.json"))?;
//! ```
//!
//! # Schema ownership (Pulsar-Native#557)
//!
//! The types below are **re-exports** of [`pulsar_scene_format`], the single
//! canonical definition of the scene/level wire format. The editor's
//! `LevelFile`/`SceneObjectData` are aliases of the very same types, so the
//! two sides cannot drift. Runtime-only ergonomics (the projected-prop
//! accessors) live on [`format::SceneObjectRuntimeExt`] here rather than on
//! the schema type.

pub mod format;
pub mod loader;

// Flatten the most-used types to the crate root.
pub use format::{
    BlueprintBinding, BlueprintBindings, ComponentInstance, LevelEditorCameraState,
    LevelEditorFileState, LevelMetadata, LightType, MeshType, ObjectType, SceneFile,
    SceneLoadError, SceneObject, SceneObjectRuntimeExt, SceneTransform,
};
pub use loader::{build_transform_parts, component_instances_from_props, SceneLoader};
