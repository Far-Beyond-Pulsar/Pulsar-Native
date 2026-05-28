//! Pulsar scene file format and Helio renderer loader.
//!
//! # Usage (runtime)
//!
//! ```rust,ignore
//! use pulsar_scene::{SceneFile, SceneLoader};
//!
//! // Load a scene into an existing helio::Renderer
//! let loaded = SceneLoader::load_file(
//!     &project_root.join("scenes/default_level.json"),
//!     &project_root,
//!     &mut renderer,
//! )?;
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
    LightType, MeshType, ObjectType, SceneFile, SceneLoadError, SceneObject,
};
pub use loader::{
    SceneLoader,
    component_instances_from_props, build_transform_parts,
    load_mesh_upload,
};
