//! Data and skeleton-driven defaults for skeletal physics assets.
//!
//! The editor stores these definitions in `.physasset` files. Keeping the
//! schema here lets runtime systems consume the same body, shape and joint
//! data without depending on the editor plugin.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use skeletal_assets::ids::SkeletalMeshId;
use skeletal_assets::skeleton::Skeleton;

/// A complete physics rig for one skeletal mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkeletalPhysicsAsset {
    pub version: u32,
    /// Stable mesh identity; paths and names can change independently.
    pub skeletal_mesh: SkeletalMeshId,
    pub bodies: Vec<SkeletalBody>,
    pub constraints: Vec<SkeletalConstraint>,
    /// Fields authored by newer versions survive saves from this version.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for SkeletalPhysicsAsset {
    fn default() -> Self {
        Self {
            version: 1,
            skeletal_mesh: SkeletalMeshId::new(),
            bodies: Vec::new(),
            constraints: Vec::new(),
            extra: Map::new(),
        }
    }
}

impl SkeletalPhysicsAsset {
    /// Build a usable first pass from a skeleton's bind hierarchy. Each bone
    /// gets a dynamic capsule and every non-root bone is constrained to its
    /// parent with a limited ball joint. These values are authoring defaults,
    /// intended to be refined in the physics editor.
    pub fn from_skeleton(skeletal_mesh: SkeletalMeshId, skeleton: &Skeleton) -> Self {
        let mut bodies = Vec::with_capacity(skeleton.bones.len());
        let mut constraints = Vec::with_capacity(skeleton.bones.len().saturating_sub(1));

        for bone in &skeleton.bones {
            let child_length = skeleton
                .children_of(&bone.id)
                .iter()
                .map(|child| child.bind_transform.translation.length())
                .filter(|length| length.is_finite() && *length > 1e-4)
                .fold(f32::INFINITY, f32::min);
            let parent_length = bone.bind_transform.translation.length();
            let length = if child_length.is_finite() {
                child_length
            } else if parent_length.is_finite() && parent_length > 1e-4 {
                parent_length
            } else {
                0.25
            };
            let radius = (length * 0.18).clamp(0.025, 0.25);
            let height = (length - radius * 2.0).max(0.0);

            bodies.push(SkeletalBody {
                bone_id: bone.id.clone(),
                motion: SkeletalMotion::Dynamic,
                mass: 1.0,
                shape: SkeletalShape::Capsule,
                center: [0.0, length * 0.5, 0.0],
                dimensions: [radius, height, 0.0],
            });

            if let Some(parent_bone_id) = bone.parent.as_ref() {
                constraints.push(SkeletalConstraint {
                    bone_id: bone.id.clone(),
                    connected_bone_id: parent_bone_id.clone(),
                    kind: SkeletalJointKind::BallSocket,
                    anchor: [0.0, -length * 0.5, 0.0],
                    connected_anchor: [0.0; 3],
                    axis: [1.0, 0.0, 0.0],
                    limits_enabled: true,
                    limits_degrees: [-45.0, 45.0],
                });
            }
        }

        Self {
            version: 1,
            skeletal_mesh,
            bodies,
            constraints,
            extra: Map::new(),
        }
    }

    /// Remove entries that refer to bones no longer present in `skeleton`.
    pub fn retain_skeleton_bones(&mut self, skeleton: &Skeleton) {
        self.bodies
            .retain(|body| skeleton.bone(&body.bone_id).is_some());
        self.constraints.retain(|joint| {
            skeleton.bone(&joint.bone_id).is_some()
                && skeleton.bone(&joint.connected_bone_id).is_some()
        });
    }
}

/// Motion mode used by a bone body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkeletalMotion {
    #[default]
    Dynamic,
    Kinematic,
    Fixed,
}

/// Convex collider shape authored for a bone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkeletalShape {
    Box,
    Sphere,
    #[default]
    Capsule,
    Cylinder,
}

/// One rigid body attached to a bone. `dimensions` is half-extents for boxes,
/// radius in `x` for spheres, and radius/half-height in `x`/`y` for round rods.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkeletalBody {
    pub bone_id: String,
    pub motion: SkeletalMotion,
    pub mass: f32,
    pub shape: SkeletalShape,
    pub center: [f32; 3],
    pub dimensions: [f32; 3],
}

impl Default for SkeletalBody {
    fn default() -> Self {
        Self {
            bone_id: String::new(),
            motion: SkeletalMotion::Dynamic,
            mass: 1.0,
            shape: SkeletalShape::Capsule,
            center: [0.0; 3],
            dimensions: [0.15, 0.25, 0.0],
        }
    }
}

/// Joint type connecting this bone body to `connected_bone_id`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkeletalJointKind {
    Fixed,
    Hinge,
    #[default]
    BallSocket,
}

/// Constraint between two bone bodies. Angular limits are in degrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkeletalConstraint {
    pub bone_id: String,
    pub connected_bone_id: String,
    pub kind: SkeletalJointKind,
    pub anchor: [f32; 3],
    pub connected_anchor: [f32; 3],
    pub axis: [f32; 3],
    pub limits_enabled: bool,
    pub limits_degrees: [f32; 2],
}

impl Default for SkeletalConstraint {
    fn default() -> Self {
        Self {
            bone_id: String::new(),
            connected_bone_id: String::new(),
            kind: SkeletalJointKind::BallSocket,
            anchor: [0.0; 3],
            connected_anchor: [0.0; 3],
            axis: [1.0, 0.0, 0.0],
            limits_enabled: true,
            limits_degrees: [-45.0, 45.0],
        }
    }
}
