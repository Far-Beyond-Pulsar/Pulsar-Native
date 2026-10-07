//! `Transform` as scripts see it.
//!
//! The methods below are the whole script surface of an object's transform:
//! the VM lists them as `Transform::position`, `Transform::set_position`,
//! and so on, grouped under the component's reference type, and Blueprint
//! palettes and TypeScript declarations are generated from the same
//! signatures. Getters read the component. Setters are world methods: they
//! consult the registered [`MotionGate`](crate::motion::MotionGate)s (so a
//! `Static` object cannot be moved at runtime) and then write through
//! `World::get_mut`, whose guard reports the change to subscribers and the
//! GPU mirror when it drops.
//!
//! Rotation is Euler angles in degrees (applied Y, X, Z), as stored.

use glam::Vec3;
use pulsar_scenedb::{component_methods, Entity, World};

use crate::components::Transform;
use crate::motion::ensure_can_move;

/// Write `change` to `entity`'s transform if it may move.
fn write(
    world: &mut World,
    entity: Entity,
    change: impl FnOnce(&mut Transform),
) -> Result<(), String> {
    ensure_can_move(world, entity)?;
    let mut transform = world
        .get_mut::<Transform>(entity)
        .ok_or_else(|| format!("{entity:?} has no Transform"))?;
    change(&mut transform);
    Ok(())
}

#[component_methods]
impl Transform {
    /// World-space position.
    #[reflect_method(side_effect_free, category = "Transform")]
    fn position(&self) -> Vec3 {
        Vec3::from(self.position)
    }

    /// Euler rotation in degrees.
    #[reflect_method(side_effect_free, category = "Transform")]
    fn rotation_degrees(&self) -> Vec3 {
        Vec3::from(self.rotation)
    }

    #[reflect_method(side_effect_free, category = "Transform")]
    fn scale(&self) -> Vec3 {
        Vec3::from(self.scale)
    }

    /// Fails if the object is not allowed to move.
    #[world_method(category = "Transform")]
    fn set_position(world: &mut World, entity: Entity, position: Vec3) -> Result<(), String> {
        write(world, entity, |t| t.position = position.to_array())
    }

    /// Move by `delta`. Fails if the object is not allowed to move.
    #[world_method(category = "Transform")]
    fn translate(world: &mut World, entity: Entity, delta: Vec3) -> Result<(), String> {
        write(world, entity, |t| {
            t.position = (Vec3::from(t.position) + delta).to_array()
        })
    }

    /// Set the Euler rotation (degrees). Fails if the object is not allowed
    /// to move.
    #[world_method(category = "Transform")]
    fn set_rotation_degrees(
        world: &mut World,
        entity: Entity,
        rotation: Vec3,
    ) -> Result<(), String> {
        write(world, entity, |t| t.rotation = rotation.to_array())
    }

    /// Fails if the object is not allowed to move.
    #[world_method(category = "Transform")]
    fn set_scale(world: &mut World, entity: Entity, scale: Vec3) -> Result<(), String> {
        write(world, entity, |t| t.scale = scale.to_array())
    }
}
