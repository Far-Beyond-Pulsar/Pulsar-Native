//! The editor/runtime scene's own component types, stored directly in
//! `pulsar_scenedb::World`. There is no store wrapper around them: an object is
//! an `Entity` carrying some of these, and everything else (identity lookup,
//! hierarchy, selection) is derived from them on demand -- see
//! [`crate::world_ext::SceneWorldExt`].

use pulsar_scenedb::Entity;

/// Stable, human-readable identity for a scene object. Survives save/load
/// (unlike the raw `Entity` bits, which only mean something inside one live
/// `World`). Unique within a world; enforced at spawn time.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StableId(pub String);

impl StableId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Parent link for the outliner. Independent of transform resolution: object
/// transforms are flat world-space values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Position among an object's siblings. Lower sorts first; ties break on the
/// entity itself, so ordering is always total. Values are only comparable
/// within one parent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiblingIndex(pub u64);

/// Marks the selected object(s). Selection is ordinary world state, so the
/// renderer's gizmo/outline passes read it the same way the editor does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selected;

/// Flat per-entity world-space transform, mirrored to the GPU as one packed,
/// dirty-tracked row so renderer consumers can read an entity's current
/// transform by the same `Entity` index.
#[derive(Clone, Copy, Debug, PartialEq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed)]
#[repr(C)]
pub struct Transform {
    #[gpu]
    pub position: [f32; 3],
    #[gpu]
    pub rotation: [f32; 3],
    #[gpu]
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

/// Display name, independent of [`StableId`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name(pub String);

/// Editor visibility/lock flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub visible: bool,
    pub locked: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Self {
            visible: true,
            locked: false,
        }
    }
}

/// Renderer-facing JSON projection of an object's component data: free-form
/// scene props plus the serialized component-instance list. Dormant and
/// unregistered component payloads live here; live registered components are
/// typed World components.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderProps {
    pub props: std::collections::HashMap<String, serde_json::Value>,
    pub component_instances: Option<serde_json::Value>,
}
