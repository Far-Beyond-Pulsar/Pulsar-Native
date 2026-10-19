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

/// Flat per-entity transform: in world space, or relative to its
/// [`Parent`]. The position is f64, so an object keeps its millimetres on
/// a planet (an f32 steps by half a metre at Earth's radius); rotation (YXZ
/// Euler degrees) and scale are f32. Mirrored to the GPU as
/// [`TransformRow`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f64; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

/// [`Transform`]'s GPU row, keyed by the object entity: the position as an
/// f32 pair (`position_hi`, the nearest f32; `position_lo`, what that
/// drops), which the renderer rebases on each frame's world origin part by
/// part, then rotation and scale. Derived on every write, removal and
/// replay, like [`ObjectHidden`].
#[derive(Clone, Copy, Debug, PartialEq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed, buffer = "object_transforms")]
#[repr(C)]
pub struct TransformRow {
    #[gpu]
    pub position_hi: [f32; 3],
    #[gpu]
    pub position_lo: [f32; 3],
    #[gpu]
    pub rotation: [f32; 3],
    #[gpu]
    pub scale: [f32; 3],
}

impl From<&Transform> for TransformRow {
    fn from(t: &Transform) -> Self {
        let hi = t.position.map(|v| v as f32);
        let lo = [0, 1, 2].map(|i| (t.position[i] - f64::from(hi[i])) as f32);
        Self {
            position_hi: hi,
            position_lo: lo,
            rotation: t.rotation,
            scale: t.scale,
        }
    }
}

fn transform_row_dispatch(
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB reaches this only through `Transform`'s own
    // `ComponentId`, with a pointer to a live `Transform`.
    let transform = unsafe { &*(data as *const Transform) };
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &TransformRow::from(transform), is_new_insert);
}

fn transform_row_clear(mirror: &pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<TransformRow>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<Transform>,
        dispatch: transform_row_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<Transform>,
        clear: transform_row_clear,
    }
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

/// [`Visibility`]'s GPU row: 1 when the object is hidden. Keyed by the
/// object entity, so an object without a `Visibility` (a zero row) draws.
/// Derived from the authored value by its own SceneDB GPU registration, on
/// every write, removal and replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed, buffer = "object_hidden")]
#[repr(C)]
pub struct ObjectHidden {
    #[gpu]
    pub hidden: u32,
}

fn object_hidden_dispatch(
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB reaches this only through `Visibility`'s own
    // `ComponentId`, with a pointer to a live `Visibility`.
    let visibility = unsafe { &*(data as *const Visibility) };
    let row_value = ObjectHidden {
        hidden: u32::from(!visibility.visible),
    };
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &row_value, is_new_insert);
}

fn object_hidden_clear(mirror: &pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<ObjectHidden>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<Visibility>,
        dispatch: object_hidden_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<Visibility>,
        clear: object_hidden_clear,
    }
}

/// [`Parent`]'s GPU row: the parent's entity index plus one, zero for an
/// object with no parent. Keyed by the child entity. The renderer composes
/// each object's world transform from its parent chain on the GPU
/// (`HierarchyResolve`), so a parent's edit is one row write however many
/// objects it holds. Derived from the authored [`Parent`] on every write,
/// removal and replay, like [`ObjectHidden`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed, buffer = "object_parent")]
#[repr(C)]
pub struct ObjectParent {
    #[gpu]
    pub parent: u32,
}

fn object_parent_dispatch(
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB reaches this only through `Parent`'s own
    // `ComponentId`, with a pointer to a live `Parent`.
    let parent = unsafe { &*(data as *const Parent) };
    let row_value = ObjectParent {
        parent: parent.0.index() + 1,
    };
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &row_value, is_new_insert);
}

fn object_parent_clear(mirror: &pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<ObjectParent>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<Parent>,
        dispatch: object_parent_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<Parent>,
        clear: object_parent_clear,
    }
}

/// [`crate::frames::SpaceSlot`]'s GPU row: the object's slot in the frame table
/// (`1..=255`), zero for an object that is not a frame. Keyed by the frame's
/// entity. The hierarchy resolve reads it to stop composing at the nearest
/// frame above each object and to tag the object with that frame's space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed, buffer = "object_frame")]
#[repr(C)]
pub struct ObjectFrame {
    #[gpu]
    pub slot: u32,
}

fn object_frame_dispatch(
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB reaches this only through `SpaceSlot`'s own `ComponentId`,
    // with a pointer to a live `SpaceSlot`.
    let frame = unsafe { &*(data as *const crate::frames::SpaceSlot) };
    let row_value = ObjectFrame { slot: frame.slot };
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &row_value, is_new_insert);
}

fn object_frame_clear(mirror: &pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<ObjectFrame>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<crate::frames::SpaceSlot>,
        dispatch: object_frame_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<crate::frames::SpaceSlot>,
        clear: object_frame_clear,
    }
}

/// Free-form scene props of an object (file-format `props`, plus the scene
/// props registered component classes project from their values). Component
/// values themselves live on component-instance entities
/// ([`crate::attachments`]); this holds no component list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderProps {
    pub props: std::collections::HashMap<String, serde_json::Value>,
}
