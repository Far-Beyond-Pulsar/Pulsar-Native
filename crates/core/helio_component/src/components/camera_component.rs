//! A camera the game can look through.
//!
//! `CameraComponent` is the authored projection of a viewpoint: perspective
//! (vertical field of view) or orthographic (visible height), the clip planes,
//! and a priority that decides which of several cameras is the view. Its
//! position and orientation are its owner object's world transform (the
//! camera looks down the owner's -Z with its +Y up, as every object is
//! rotated), so a camera on or under a character follows the character.
//!
//! The component carries no GPU row: a host reads it per frame to build the
//! frame's camera. The selection rules live here, behind two small entry
//! points the game runtime calls without naming any other component:
//!
//! - [`camera_on_object`]: the camera an object (a possessed pawn) looks
//!   through, found on the object first, then beneath it;
//! - [`highest_priority_camera`]: the world's own pick when nothing names one.
//!
//! The lens (`CameraLensComponent`) and the camera post-process are separate
//! components matched to a render view by `view_id`; this component does not
//! own them and does not change which view a frame renders as.
use std::cmp::Reverse;

use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::Reflectable;
use pulsar_scene_model::attachments;
use pulsar_scene_model::hierarchy_transform::descendants_of;
use pulsar_scenedb::{Entity, World};
use serde::{Deserialize, Deserializer, Serialize};

/// The class name scripts and level files use.
pub const CAMERA_CLASS_NAME: &str = "CameraComponent";

/// How the camera projects the world. Reads from its variant name or its index
/// (the reflection JSON form), like the lens enums.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Reflectable)]
pub enum CameraProjectionKind {
    /// Perspective with a vertical field of view.
    #[default]
    Perspective,
    /// Orthographic: parallel projection of a fixed visible height.
    Orthographic,
}

impl CameraProjectionKind {
    pub const ALL: &'static [Self] = &[Self::Perspective, Self::Orthographic];
}

impl<'de> Deserialize<'de> for CameraProjectionKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Index(u64),
        }
        let parsed = match Repr::deserialize(deserializer)? {
            Repr::Name(name) => Self::ALL
                .iter()
                .copied()
                .find(|v| format!("{v:?}").eq_ignore_ascii_case(&name)),
            Repr::Index(index) => Self::ALL.get(index as usize).copied(),
        };
        parsed.ok_or_else(|| serde::de::Error::custom("unknown CameraProjectionKind"))
    }
}

/// A camera (authored). Defaults are the free camera's lens (45 degrees,
/// 0.05 to 2000 m), so adding one changes the point of view and nothing else.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Camera", category_color = "#3AA0FF")]
#[category("Projection", category_color = "#E0B040")]
#[serde(default)]
pub struct CameraComponent {
    /// A disabled camera is never the view.
    #[property(category = "Camera")]
    pub enabled: bool,
    /// Among enabled cameras the highest priority is the view (ties go to the
    /// earlier object). A game that names a view target looks through that
    /// object's camera first.
    #[property(min = -1000.0, max = 1000.0, step = 1.0, category = "Camera")]
    pub priority: i32,
    #[property(category = "Projection")]
    pub projection: CameraProjectionKind,
    /// Perspective: vertical field of view in degrees.
    #[property(min = 1.0, max = 170.0, step = 0.5, category = "Projection")]
    pub fov_y_deg: f32,
    /// Orthographic: half the visible height in metres.
    #[property(min = 0.01, max = 10000.0, step = 0.1, category = "Projection")]
    pub orthographic_size: f32,
    /// Near clip plane in metres (perspective; an orthographic view reaches
    /// back and forward from the eye by the renderer's own depth range).
    #[property(min = 0.001, max = 100.0, step = 0.01, category = "Projection")]
    pub near: f32,
    /// Far clip plane in metres (perspective).
    #[property(min = 1.0, max = 100000.0, step = 1.0, category = "Projection")]
    pub far: f32,
}

impl Default for CameraComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 0,
            projection: CameraProjectionKind::Perspective,
            fov_y_deg: 45.0,
            orthographic_size: 5.0,
            near: 0.05,
            far: 2000.0,
        }
    }
}

impl CameraComponent {
    /// Vertical field of view in radians, kept inside a usable range.
    pub fn fov_y_radians(&self) -> f32 {
        let degrees = if self.fov_y_deg.is_finite() {
            self.fov_y_deg
        } else {
            45.0
        };
        degrees.clamp(1.0, 170.0).to_radians()
    }

    /// The clip planes: near positive, far beyond it.
    pub fn clip_planes(&self) -> (f32, f32) {
        let near = if self.near.is_finite() { self.near } else { 0.05 };
        let near = near.max(1.0e-3);
        let far = if self.far.is_finite() { self.far } else { 2000.0 };
        (near, far.max(near + 1.0e-3))
    }

    /// Half the visible height of an orthographic view, in metres.
    pub fn orthographic_half_height(&self) -> f32 {
        if self.orthographic_size.is_finite() {
            self.orthographic_size.max(1.0e-3)
        } else {
            5.0
        }
    }
}

impl CameraComponent {
    /// The projection with its values made usable (radians, positive clip
    /// planes), for a host to build the frame's camera from.
    pub fn resolved_projection(&self) -> ResolvedProjection {
        let (near, far) = self.clip_planes();
        match self.projection {
            CameraProjectionKind::Perspective => ResolvedProjection::Perspective {
                fov_y: self.fov_y_radians(),
                near,
                far,
            },
            CameraProjectionKind::Orthographic => ResolvedProjection::Orthographic {
                half_height: self.orthographic_half_height(),
                near,
                far,
            },
        }
    }
}

/// [`CameraComponent`]'s projection, ready to use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedProjection {
    /// `fov_y` radians vertically.
    Perspective { fov_y: f32, near: f32, far: f32 },
    /// `half_height` metres up and down from the eye. The renderer sets the
    /// depth range of an orthographic view itself, so `near` and `far` are
    /// carried for a host that wants them.
    Orthographic { half_height: f32, near: f32, far: f32 },
}

#[register_world_component]
impl CameraComponent {}

/// A camera component picked from a world: where it is and what it holds.
/// The viewpoint is `owner`'s world transform.
#[derive(Clone, Debug)]
pub struct SelectedCamera {
    /// The object the camera is attached to (its transform is the eye).
    pub owner: Entity,
    /// The component instance entity.
    pub instance: Entity,
    pub camera: CameraComponent,
}

/// The best of `candidates` (`(instance, owner, camera)`): the highest
/// priority, then the earlier owner, then the earlier instance. Disabled
/// cameras never win.
fn best<'a>(
    candidates: impl Iterator<Item = (Entity, Entity, &'a CameraComponent)>,
) -> Option<SelectedCamera> {
    candidates
        .filter(|(_, _, camera)| camera.enabled)
        .min_by_key(|(instance, owner, camera)| {
            (Reverse(camera.priority), owner.index(), instance.index())
        })
        .map(|(instance, owner, camera)| SelectedCamera {
            owner,
            instance,
            camera: camera.clone(),
        })
}

/// The camera `root` looks through: its own enabled camera of highest
/// priority, else the best enabled camera on an object beneath it (any depth).
/// `None` when neither has one.
pub fn camera_on_object(world: &World, root: Entity) -> Option<SelectedCamera> {
    if !world.is_alive(root) {
        return None;
    }
    let own = attachments::enabled_components_of::<CameraComponent>(world, root);
    if let Some(found) = best(own.into_iter().map(|(instance, camera)| (instance, root, camera))) {
        return Some(found);
    }
    let below = descendants_of(world, root).into_iter().flat_map(|owner| {
        attachments::enabled_components_of::<CameraComponent>(world, owner)
            .into_iter()
            .map(move |(instance, camera)| (instance, owner, camera))
    });
    best(below)
}

/// The world's best enabled camera when no object is named: the highest
/// priority, ties to the earlier object.
pub fn highest_priority_camera(world: &World) -> Option<SelectedCamera> {
    best(attachments::enabled_components::<CameraComponent>(world))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::attachments::NewInstance;
    use pulsar_scene_model::{SceneWorldExt, SpawnObject};

    fn object(world: &mut World, name: &str) -> Entity {
        world.spawn_object(SpawnObject::new(name)).unwrap()
    }

    fn attach(world: &mut World, owner: Entity, camera: CameraComponent) -> Entity {
        let instance =
            attachments::spawn_instance(world, owner, NewInstance::new(CAMERA_CLASS_NAME)).unwrap();
        world.insert(instance, camera);
        instance
    }

    fn with_priority(priority: i32) -> CameraComponent {
        CameraComponent {
            priority,
            ..Default::default()
        }
    }

    #[test]
    fn defaults_are_the_free_cameras_lens() {
        let camera = CameraComponent::default();
        assert!((camera.fov_y_radians() - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!(camera.clip_planes(), (0.05, 2000.0));
    }

    #[test]
    fn the_projection_resolves_by_kind() {
        let mut camera = CameraComponent::default();
        assert_eq!(
            camera.resolved_projection(),
            ResolvedProjection::Perspective {
                fov_y: std::f32::consts::FRAC_PI_4,
                near: 0.05,
                far: 2000.0
            }
        );
        camera.projection = CameraProjectionKind::Orthographic;
        camera.orthographic_size = 12.0;
        assert!(matches!(
            camera.resolved_projection(),
            ResolvedProjection::Orthographic { half_height, .. } if half_height == 12.0
        ));
    }

    #[test]
    fn bad_values_stay_inside_a_usable_projection() {
        let camera = CameraComponent {
            fov_y_deg: f32::NAN,
            near: -1.0,
            far: 0.0,
            orthographic_size: 0.0,
            ..Default::default()
        };
        assert!((camera.fov_y_radians() - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        let (near, far) = camera.clip_planes();
        assert!(near > 0.0 && far > near);
        assert!(camera.orthographic_half_height() > 0.0);
        let wide = CameraComponent {
            fov_y_deg: 400.0,
            ..Default::default()
        };
        assert!((wide.fov_y_radians() - 170f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn the_value_round_trips_and_old_data_takes_defaults() {
        let camera = CameraComponent {
            priority: 7,
            projection: CameraProjectionKind::Orthographic,
            fov_y_deg: 80.0,
            ..Default::default()
        };
        let json = serde_json::to_value(&camera).unwrap();
        let back: CameraComponent = serde_json::from_value(json).unwrap();
        assert_eq!(back.priority, 7);
        assert_eq!(back.projection, CameraProjectionKind::Orthographic);
        assert_eq!(back.fov_y_deg, 80.0);
        // Saved with fewer fields (or none): the rest is the default.
        let sparse: CameraComponent =
            serde_json::from_value(serde_json::json!({ "priority": 3 })).unwrap();
        assert_eq!(sparse.priority, 3);
        assert!(sparse.enabled);
        assert_eq!(sparse.fov_y_deg, 45.0);
        assert_eq!(sparse.far, 2000.0);
        let empty: CameraComponent = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty.projection, CameraProjectionKind::Perspective);
        let by_index: CameraProjectionKind = serde_json::from_value(serde_json::json!(1)).unwrap();
        assert_eq!(by_index, CameraProjectionKind::Orthographic);
    }

    #[test]
    fn the_class_is_registered_for_the_editor_and_blueprints() {
        assert!(
            pulsar_reflection::REGISTRY
                .get_class_names()
                .iter()
                .any(|name| *name == CAMERA_CLASS_NAME),
            "{CAMERA_CLASS_NAME} must be in the class registry (Add component list)"
        );
    }

    #[test]
    fn the_highest_priority_enabled_camera_wins_and_ties_go_to_the_earlier_object() {
        let mut world = World::new();
        let a = object(&mut world, "A");
        let b = object(&mut world, "B");
        let c = object(&mut world, "C");
        attach(&mut world, a, with_priority(1));
        attach(&mut world, b, with_priority(5));
        attach(&mut world, c, with_priority(5));
        let picked = highest_priority_camera(&world).unwrap();
        assert_eq!(picked.owner, b, "tie at 5 goes to the earlier object");

        let off = CameraComponent {
            enabled: false,
            priority: 100,
            ..Default::default()
        };
        attach(&mut world, a, off);
        assert_eq!(highest_priority_camera(&world).unwrap().owner, b);
    }

    #[test]
    fn a_disabled_instance_is_not_a_camera() {
        let mut world = World::new();
        let a = object(&mut world, "A");
        let instance = attach(&mut world, a, with_priority(0));
        attachments::set_enabled(&mut world, instance, false);
        assert!(highest_priority_camera(&world).is_none());
        assert!(camera_on_object(&world, a).is_none());
    }

    #[test]
    fn an_object_looks_through_its_own_camera_before_one_beneath_it() {
        let mut world = World::new();
        let pawn = object(&mut world, "Pawn");
        let head = object(&mut world, "Head");
        let eyes = object(&mut world, "Eyes");
        world.reparent(head, Some(pawn)).unwrap();
        world.reparent(eyes, Some(head)).unwrap();
        attach(&mut world, eyes, with_priority(9));
        // Only a deep descendant has one: found.
        assert_eq!(camera_on_object(&world, pawn).unwrap().owner, eyes);
        // Its own camera wins over a higher-priority one beneath it.
        attach(&mut world, pawn, with_priority(0));
        assert_eq!(camera_on_object(&world, pawn).unwrap().owner, pawn);
        // The head has nothing of its own: the one beneath it.
        assert_eq!(camera_on_object(&world, head).unwrap().owner, eyes);
        // A camera elsewhere is not the pawn's.
        let other = object(&mut world, "Other");
        assert!(camera_on_object(&world, other).is_none());
    }

    #[test]
    fn among_descendants_the_highest_priority_wins() {
        let mut world = World::new();
        let pawn = object(&mut world, "Pawn");
        let low = object(&mut world, "Low");
        let high = object(&mut world, "High");
        world.reparent(low, Some(pawn)).unwrap();
        world.reparent(high, Some(pawn)).unwrap();
        attach(&mut world, low, with_priority(1));
        attach(&mut world, high, with_priority(4));
        assert_eq!(camera_on_object(&world, pawn).unwrap().owner, high);
    }
}
