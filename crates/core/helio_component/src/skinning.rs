//! Skinned meshes and the poses that drive them, between kinds that do not
//! link each other: the meshes own the skinned mesh (its asset, skin
//! weights, inverse binds and the joint palette it draws with), the
//! animation plugin owns the skeletal pose (graphs, Montages, root motion,
//! the physics blend), and a physics rig reads and drives that pose.
//!
//! - [`SkinnedMeshAccess`]: submitted by the meshes. An animator finds the
//!   skinned mesh it drives, reads which skeleton the mesh is bound to and
//!   shows a pose on it; the mesh turns the pose into its palette.
//! - [`SkeletalPoseAccess`]: submitted by the animation plugin. A physics
//!   rig reads the animated pose and takes over bones.
//!
//! As Unity splits the Animator from the SkinnedMeshRenderer: the pose is
//! the animator's, the bind poses and the palette are the renderer's.
//! While a plugin is disabled its access is absent and the other side has
//! nothing to drive.

use pulsar_scenedb::{ComponentId, Entity, World};

/// What a skinned mesh instance is bound to: its skeletal mesh asset and the
/// skeleton the mesh's header names (stable asset ids).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkinBinding {
    pub mesh: Option<String>,
    pub skeleton: Option<String>,
}

/// How an animator reaches skinned meshes (the meshes submit the one).
pub struct SkinnedMeshAccess {
    /// The package that submitted it: skipped while its plugin is disabled.
    pub origin: &'static str,
    /// `owner`'s `ordinal`-th skinned mesh instance (or `owner` itself when
    /// it holds the mesh), the one the owner's `ordinal`-th animator drives.
    pub instance: fn(world: &World, owner: Entity, ordinal: u32) -> Option<Entity>,
    /// What `instance` is bound to; `None` when it is not a skinned mesh.
    pub binding: fn(world: &World, instance: Entity) -> Option<SkinBinding>,
    /// Show `pose` on `instance`: one model-space matrix per bone, `bones`
    /// naming each (skeleton bone ids, in the same order; shared, so a pose
    /// a frame copies no names). An empty pose returns the mesh to its bind
    /// pose. False when `instance` is not a skinned mesh.
    pub show_pose: fn(
        world: &mut World,
        instance: Entity,
        bones: std::sync::Arc<[String]>,
        pose: Vec<[[f32; 4]; 4]>,
    ) -> bool,
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

/// An animated skeleton as another kind reads it: the skeletal mesh it
/// drives (its asset id), its skeleton's bone ids and each bone's
/// model-space pose as the animation evaluated it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkeletalPose {
    pub mesh: Option<String>,
    pub bone_ids: Vec<String>,
    pub bone_world_pose: Vec<[[f32; 4]; 4]>,
}

/// How a physics rig reads and drives animated skeletons (the animation
/// plugin submits the one).
pub struct SkeletalPoseAccess {
    /// The package that submitted it: skipped while its plugin is disabled.
    pub origin: &'static str,
    /// The animator component's id (its rows' change journal).
    pub component: fn() -> ComponentId,
    /// The pose of animator instance `instance`.
    pub pose: fn(world: &World, instance: Entity) -> Option<SkeletalPose>,
    /// Take over `bones` (`(bone index, model-space matrix)`) of `instance`;
    /// the animation keeps the rest. An empty list returns every bone to
    /// the animation. False when the instance is not ready or an entry is
    /// bad.
    pub drive_bones:
        fn(world: &mut World, instance: Entity, bones: Vec<(usize, [[f32; 4]; 4])>) -> bool,
    /// Return every bone of `instance` to its animation.
    pub release_bones: fn(world: &mut World, instance: Entity),
}

inventory::collect!(SkeletalPoseAccess);

impl pulsar_plugins::Registration for SkeletalPoseAccess {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// The enabled skeletal pose access, if the animation plugin is enabled.
pub fn skeletal_pose_access() -> Option<&'static SkeletalPoseAccess> {
    pulsar_plugins::registered::<SkeletalPoseAccess>().next()
}
