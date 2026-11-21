//! Object pivots: where an object's origin, the point it moves, rotates and
//! scales about, sits relative to its stored [`Transform`].
//!
//! The stored transform places the object's *geometry* (it is what the
//! renderer draws), so moving the pivot never moves what is drawn. The pivot
//! is an offset in the object's local space (before its scale and rotation),
//! which keeps it attached to the geometry as the object is moved, turned
//! and scaled. The zero offset is the default: the pivot sits at the
//! transform's position.
//!
//! The offset is authored as an object prop ([`PIVOT_OFFSET_PROP`]), so it
//! survives save, load and undo with no file-format change, and has this one
//! owner: nothing else reads or writes that prop.

use glam::{DQuat, DVec3, EulerRot};
use pulsar_scenedb::{Entity, World};

use crate::components::{RenderProps, Transform};

/// The object prop that carries the pivot offset (`[x, y, z]`) in a saved
/// scene.
pub const PIVOT_OFFSET_PROP: &str = "pivot_offset";

/// `entity`'s pivot offset in its local space (zero when it has none).
pub fn pivot_offset_of(world: &World, entity: Entity) -> [f32; 3] {
    world
        .get::<RenderProps>(entity)
        .map_or([0.0; 3], |props| pivot_offset_from_props(&props.props))
}

/// The pivot offset in an object's `props` (zero when absent or malformed).
pub fn pivot_offset_from_props(
    props: &std::collections::HashMap<String, serde_json::Value>,
) -> [f32; 3] {
    let Some(values) = props.get(PIVOT_OFFSET_PROP).and_then(|v| v.as_array()) else {
        return [0.0; 3];
    };
    let mut offset = [0.0; 3];
    for (slot, value) in offset.iter_mut().zip(values) {
        match value.as_f64().filter(|v| v.is_finite()) {
            Some(v) => *slot = v as f32,
            None => return [0.0; 3],
        }
    }
    offset
}

/// Set `entity`'s pivot offset (the zero offset clears the prop). Returns
/// whether anything changed.
pub fn set_pivot_offset(world: &mut World, entity: Entity, offset: [f32; 3]) -> bool {
    if !offset.iter().all(|v| v.is_finite()) {
        return false;
    }
    let Some(mut props) = world.get_mut::<RenderProps>(entity) else {
        return false;
    };
    if pivot_offset_from_props(&props.props) == offset {
        return false;
    }
    if offset == [0.0; 3] {
        props.props.remove(PIVOT_OFFSET_PROP);
    } else {
        props
            .props
            .insert(PIVOT_OFFSET_PROP.to_string(), serde_json::json!(offset));
    }
    true
}

/// The orientation of `transform` (YXZ Euler degrees, as everywhere).
pub fn orientation(transform: &Transform) -> DQuat {
    DQuat::from_euler(
        EulerRot::YXZ,
        f64::from(transform.rotation[1]).to_radians(),
        f64::from(transform.rotation[0]).to_radians(),
        f64::from(transform.rotation[2]).to_radians(),
    )
}

/// The world vector from `transform`'s position to its pivot: `offset`
/// scaled and turned by the transform.
pub fn pivot_vector(transform: &Transform, offset: [f32; 3]) -> DVec3 {
    let scaled = DVec3::new(
        f64::from(offset[0]) * f64::from(transform.scale[0]),
        f64::from(offset[1]) * f64::from(transform.scale[1]),
        f64::from(offset[2]) * f64::from(transform.scale[2]),
    );
    orientation(transform) * scaled
}

/// `transform`'s pivot in the world (`transform` being the object's world
/// transform).
pub fn pivot_point(transform: &Transform, offset: [f32; 3]) -> [f64; 3] {
    (DVec3::from_array(transform.position) + pivot_vector(transform, offset)).to_array()
}

/// The position that puts `transform`'s pivot at `pivot` (its rotation and
/// scale as given).
pub fn position_for_pivot(transform: &Transform, offset: [f32; 3], pivot: [f64; 3]) -> [f64; 3] {
    (DVec3::from_array(pivot) - pivot_vector(transform, offset)).to_array()
}

/// The offset that puts `transform`'s pivot at the world point `point`: the
/// inverse of [`pivot_point`].
pub fn offset_for_point(transform: &Transform, point: [f64; 3]) -> [f32; 3] {
    let local = orientation(transform).inverse()
        * (DVec3::from_array(point) - DVec3::from_array(transform.position));
    let axis = |i: usize| {
        let scale = f64::from(transform.scale[i]);
        if scale.abs() < 1e-9 {
            0.0
        } else {
            (local[i] / scale) as f32
        }
    };
    [axis(0), axis(1), axis(2)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_ext::{SceneWorldExt, SpawnObject};

    fn placed() -> Transform {
        Transform {
            position: [10.0, 2.0, -3.0],
            rotation: [10.0, 70.0, -20.0],
            scale: [2.0, 0.5, 3.0],
        }
    }

    #[test]
    fn the_pivot_point_and_offset_invert_each_other() {
        let t = placed();
        let point = [11.5, 4.0, -1.0];
        let offset = offset_for_point(&t, point);
        let back = pivot_point(&t, offset);
        for i in 0..3 {
            assert!((back[i] - point[i]).abs() < 1e-4, "axis {i}: {back:?}");
        }
    }

    #[test]
    fn moving_the_pivot_to_a_point_keeps_the_pivot_there() {
        let mut t = placed();
        let offset = [0.25, -1.0, 0.5];
        let pivot = pivot_point(&t, offset);
        t.rotation = [0.0, 33.0, 5.0];
        t.scale = [1.0, 4.0, 2.0];
        t.position = position_for_pivot(&t, offset, pivot);
        let after = pivot_point(&t, offset);
        for i in 0..3 {
            assert!((after[i] - pivot[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn the_offset_round_trips_through_the_prop() {
        let mut world = World::new();
        let object = world.spawn_object(SpawnObject::new("o")).unwrap();
        assert_eq!(pivot_offset_of(&world, object), [0.0; 3]);
        assert!(set_pivot_offset(&mut world, object, [1.0, 2.0, 3.0]));
        assert_eq!(pivot_offset_of(&world, object), [1.0, 2.0, 3.0]);
        assert!(!set_pivot_offset(&mut world, object, [1.0, 2.0, 3.0]));
        assert!(set_pivot_offset(&mut world, object, [0.0; 3]));
        let props = world.get::<RenderProps>(object).unwrap();
        assert!(!props.props.contains_key(PIVOT_OFFSET_PROP), "cleared for good");
    }
}
