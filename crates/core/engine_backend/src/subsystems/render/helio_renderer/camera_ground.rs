//! The ground under a camera, as the scene's content reports it.
//!
//! Any component may report ground through its methods, called by name the
//! way scripts call them: `ground_altitude(x, y, z)` (height above the
//! ground below), `ground_up_x/_y/_z(x, y, z)` (the local vertical),
//! `ground_lift(x, y, z)` (how far a point inside solid ground must rise to
//! leave it), `clip_near(x, y, z)` and `clip_far(x, y, z)` (the clip planes
//! the content certifies around an eye). A voxel terrain reports them; the
//! renderer and cameras know no kind of content. A method that fails (its
//! world not built yet) reports nothing for that content.
//!
//! Cameras use them to scale their speed with altitude and stand on the
//! local vertical (the editor viewport, the game's free camera), never
//! enter solid ground (the editor camera), and clip close to the eye
//! without clipping the ground.

use glam::DVec3;
use pulsar_scene_model::attachments::ComponentOwner;
use pulsar_scenedb::{ComponentId, Entity, World};

/// The ground under an eye.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraGround {
    /// The local vertical (a planet's radial).
    pub up: Option<DVec3>,
    /// Height above the ground directly below.
    pub altitude: Option<f64>,
}

/// Finds the scene content reporting ground and asks it. Keep one per
/// scene: the content is found again only when the scene changed.
#[derive(Default)]
pub struct GroundQueries {
    /// The instances with ground methods, and the scene revision they were
    /// found at.
    found: Option<(u64, Vec<(Entity, ComponentId)>)>,
}

impl GroundQueries {
    /// The enabled, visible component instances that report ground.
    fn providers(&mut self, world: &World) -> Vec<(Entity, ComponentId)> {
        let revision = world.revision();
        if let Some((found_at, providers)) = &self.found {
            if *found_at == revision {
                return providers.clone();
            }
        }
        let kinds = pulsar_world_registry::components_with_method("ground_altitude");
        let providers: Vec<(Entity, ComponentId)> = if kinds.is_empty() {
            Vec::new()
        } else {
            world
                .query::<&ComponentOwner>()
                .filter(|(_, owner)| owner.is_enabled())
                .filter(|(_, owner)| {
                    world
                        .get::<pulsar_scene_model::Visibility>(owner.entity())
                        .is_none_or(|v| v.visible)
                })
                .flat_map(|(instance, _)| {
                    kinds
                        .iter()
                        .filter(move |cid| world.has_component(instance, **cid))
                        .map(move |cid| (instance, *cid))
                })
                .collect()
        };
        self.found = Some((revision, providers.clone()));
        providers
    }

    /// `method(eye)` of every provider that answers.
    fn ask(&mut self, world: &mut World, method: &str, eye: DVec3) -> Vec<f64> {
        if !eye.is_finite() {
            return Vec::new();
        }
        profiling::profile_scope!("camera_ground_ask");
        self.providers(world)
            .into_iter()
            .filter_map(|(instance, cid)| {
                let mut args: Vec<Box<dyn std::any::Any>> =
                    vec![Box::new(eye.x), Box::new(eye.y), Box::new(eye.z)];
                let answer = world
                    .call_component_method(instance, cid, method, &mut args)
                    .ok()??;
                answer
                    .downcast::<f64>()
                    .ok()
                    .map(|v| *v)
                    .filter(|v| v.is_finite())
            })
            .collect()
    }

    /// The ground under `eye`: the first provider's vertical, and the
    /// lowest altitude over any of them.
    pub fn ground(&mut self, world: &mut World, eye: DVec3) -> CameraGround {
        let altitude = self
            .ask(world, "ground_altitude", eye)
            .into_iter()
            .reduce(f64::min);
        let [x, y, z] = ["ground_up_x", "ground_up_y", "ground_up_z"]
            .map(|method| self.ask(world, method, eye).first().copied());
        let up = match (x, y, z) {
            (Some(x), Some(y), Some(z)) => DVec3::new(x, y, z).try_normalize(),
            _ => None,
        };
        CameraGround { up, altitude }
    }

    /// Where a camera at `eye` should be instead, when `eye` is inside
    /// solid ground: just above it, along the local vertical.
    pub fn lift_out_of_ground(&mut self, world: &mut World, eye: DVec3) -> Option<DVec3> {
        let lift = self
            .ask(world, "ground_lift", eye)
            .into_iter()
            .reduce(f64::max)
            .filter(|lift| *lift > 0.0)?;
        let up = self.ground(world, eye).up.unwrap_or(DVec3::Y);
        Some(eye + up * lift)
    }

    /// The clip planes the content certifies around `eye`: the nearest
    /// near and the farthest far of every provider.
    pub fn clip_range(&mut self, world: &mut World, eye: DVec3) -> Option<(f32, f32)> {
        let near = self
            .ask(world, "clip_near", eye)
            .into_iter()
            .reduce(f64::min)?;
        let far = self
            .ask(world, "clip_far", eye)
            .into_iter()
            .reduce(f64::max)?;
        let (near, far) = (near as f32, far as f32);
        (near > 0.0 && far > near).then_some((near, far))
    }
}

/// How much faster than its base speed a camera flies over `ground`: the
/// base speed within 20 m of the ground, 50x at 1 km, orbit in seconds.
pub fn altitude_speed_scale(ground: &CameraGround) -> f64 {
    ground
        .altitude
        .map_or(1.0, |h| (h / 20.0).clamp(1.0, 1.0e6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_grows_with_altitude() {
        let at = |altitude| CameraGround { up: None, altitude };
        assert_eq!(altitude_speed_scale(&at(None)), 1.0);
        assert_eq!(altitude_speed_scale(&at(Some(2.0))), 1.0);
        assert_eq!(altitude_speed_scale(&at(Some(1_000.0))), 50.0);
    }
}
