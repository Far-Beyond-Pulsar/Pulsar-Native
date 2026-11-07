//! The engine's scene vocabulary.
//!
//! A scene is a `pulsar_scenedb::SceneDb`. Objects are entities carrying the
//! plain components in [`components`]; this crate adds no store around them.
//! [`world_ext::SceneWorldExt`] gives stateless helpers (identity lookup,
//! hierarchy, selection) that are derived from those components on demand, and
//! [`attachments`] holds every attached component instance as its own entity
//! linked to its owner (Pulsar-Native#1035, D1).

pub mod attachments;
pub mod components;
pub mod frames;
pub mod hierarchy_transform;
pub mod instance;
pub mod level_instances;
pub mod motion;
pub mod partitions;
pub mod payload_catalog;
pub mod runtime;
mod transform_script;
pub mod world_ext;

use serde::{Deserialize, Serialize};

pub use attachments::{
    AmbiguousComponent, ClassSlot, ComponentAttachments, ComponentInstanceId, ComponentMeta,
    ComponentOwner, InstanceError, NewInstance, UnresolvedComponent,
};
pub use components::{
    Name, ObjectFrame, ObjectHidden, ObjectParent, Parent, RenderProps, Selected, SiblingIndex,
    StableId, Transform, TransformRow, Visibility,
};
pub use frames::{RigidFrame, SpaceSlot};
pub use instance::{ComponentInstance, EditorObjectId};
pub use level_instances::{LevelInstanceMember, LevelInstanceRoot};
pub use partitions::ContentSpace;
pub use payload_catalog::{
    ContentId, PayloadCatalog, PayloadCatalogError, PayloadMutation, PayloadRef,
};
pub use world_ext::{SceneError, SceneWorldExt, SpawnObject};

/// Same type as [`EditorObjectId`]; the alias the editor spells it as.
pub type ObjectId = EditorObjectId;

/// What an object is structurally. What it draws, lights or simulates is its
/// components' (a mesh, a light, an emitter: each kind's plugin owns its
/// component), never a variant here, so the scene model names no kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Blueprint,
}

/// Levels saved before object types were structural name a kind
/// (`{"Light": "Point"}`, `{"Mesh": "Cube"}`, `"ParticleSystem"`,
/// `"AudioSource"`): such objects load as `Empty`, their components saying
/// what they are.
impl<'de> Deserialize<'de> for ObjectType {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Tagged(std::collections::BTreeMap<String, serde::de::IgnoredAny>),
        }
        Ok(match Repr::deserialize(de)? {
            Repr::Name(name) => match name.as_str() {
                "Folder" => ObjectType::Folder,
                "Camera" => ObjectType::Camera,
                "Blueprint" => ObjectType::Blueprint,
                _ => ObjectType::Empty,
            },
            Repr::Tagged(_) => ObjectType::Empty,
        })
    }
}
