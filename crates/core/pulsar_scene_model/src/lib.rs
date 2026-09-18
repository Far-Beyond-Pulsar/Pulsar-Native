//! The engine's scene vocabulary.
//!
//! A scene is a `pulsar_scenedb::SceneDb`. Objects are entities carrying the
//! plain components in [`components`]; this crate adds no store around them.
//! [`world_ext::SceneWorldExt`] gives stateless helpers (identity lookup,
//! hierarchy, selection) that are derived from those components on demand, and
//! [`attachments`] records which component instances an object has.

pub mod attachments;
pub mod components;
pub mod instance;
pub mod world_ext;

use serde::{Deserialize, Serialize};

pub use attachments::ComponentAttachments;
pub use components::{
    Name, Parent, RenderProps, Selected, SiblingIndex, StableId, Transform, Visibility,
};
pub use instance::{ComponentInstance, EditorObjectId};
pub use world_ext::{SceneError, SceneWorldExt, SpawnObject};

/// Same type as [`EditorObjectId`]; the alias the editor spells it as.
pub type ObjectId = EditorObjectId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Light(LightType),
    Mesh(MeshType),
    ParticleSystem,
    AudioSource,
    Blueprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
    Custom,
}
