//! A skinned mesh as another kind drives it: the pose a physics rig reads
//! and the bones it takes over, without linking the meshes' component
//! crate. The meshes' component crate submits the one
//! [`SkinnedMeshAccess`]; while the meshes plugin is disabled there is none
//! and a rig has nothing to drive.
//!
//! As the step 1 audit split it: the animation plugin writes joint
//! palettes, the meshes own the skinned mesh and its drawing, and a physics
//! rig reads and drives the skeletal pose through this contract.

use pulsar_scenedb::{ComponentId, Entity, World};

/// What a skinned mesh instance shows: the skeletal mesh asset it draws
/// (its id), its skeleton's bone ids and each bone's model-space pose.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinnedPose {
    pub mesh: Option<String>,
    pub bone_ids: Vec<String>,
    pub bone_world_pose: Vec<[[f32; 4]; 4]>,
}

/// How to read and drive skinned mesh instances.
pub struct SkinnedMeshAccess {
    /// The package that submitted it (the meshes' component crate): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// The skinned mesh component's id (its rows' change journal).
    pub component: fn() -> ComponentId,
    /// The pose of the skinned mesh instance `instance`.
    pub pose: fn(world: &World, instance: Entity) -> Option<SkinnedPose>,
    /// Take over `bones` (`(bone index, model-space matrix)`) of `instance`;
    /// the animation keeps the rest. An empty list returns every bone to
    /// the animation. False when the instance is not ready or an entry is
    /// bad.
    pub drive_bones: fn(world: &mut World, instance: Entity, bones: Vec<(usize, [[f32; 4]; 4])>) -> bool,
    /// Return every bone of `instance` to its animation.
    pub release_bones: fn(world: &mut World, instance: Entity),
}

inventory::collect!(SkinnedMeshAccess);

impl pulsar_plugins::Registration for SkinnedMeshAccess {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// The enabled skinned mesh access, if the meshes plugin is enabled.
pub fn skinned_mesh_access() -> Option<&'static SkinnedMeshAccess> {
    pulsar_plugins::registered::<SkinnedMeshAccess>().next()
}
