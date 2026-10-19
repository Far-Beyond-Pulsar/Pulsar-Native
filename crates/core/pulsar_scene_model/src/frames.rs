//! Rigid frames: parents whose contents are never rewritten when they move.
//!
//! An object carrying [`Frame`] is a **rigid frame** (a ship, a room, a placed
//! sublevel). Its [`Transform`] still places it like any parent's, but the
//! renderer treats it differently from an ordinary group: everything beneath it
//! is placed *relative to the frame* and drawn through one entry of the frame
//! table, so moving the frame is one matrix, not one row per member (see
//! `.agents/FRAMES.md`).
//!
//! * [`Frame::slot`] is the frame's index in the frame table (`1..=MAX_FRAMES`;
//!   slot 0 is the world). It is mirrored to the GPU as the derived
//!   [`ObjectFrame`](crate::ObjectFrame) row so the hierarchy resolve can find
//!   the nearest frame above each object.
//! * [`frame_table`] builds the table itself: each frame's matrix in the world,
//!   composed in f64 (frames nest, and may sit anywhere on a planet) and
//!   handed over relative to the frame's world origin as f32.
//! * A frame is authored with a prop on its object ([`FRAME_PROP`]) so it
//!   survives save and load with no file-format change; [`sync_frames_from_props`]
//!   turns the prop into the component after load.

use glam::{DMat4, DQuat, DVec3, EulerRot, Mat4};
use pulsar_scenedb::{Entity, World};

use crate::components::{Parent, RenderProps, Transform};
use crate::hierarchy_transform::MAX_DEPTH;

/// Frames the table can hold. A space id travels in 8 bits of an instance's
/// flags (`helio_pass_object_batch::INSTANCE_COORDINATE_SPACE_*`), and slot 0
/// is the world.
pub const MAX_FRAMES: u32 = 255;

/// The object prop that marks a frame in a saved scene.
pub const FRAME_PROP: &str = "rigid_frame";

/// Marks an object as a rigid frame and names its slot in the frame table.
/// Assigned by [`make_frame`]; slots are not saved (they are reassigned on
/// load).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("all {MAX_FRAMES} frame slots are in use")]
    NoFreeSlot,
    #[error("the object does not exist")]
    NoSuchObject,
}

/// The lowest unused slot, if any.
fn free_slot(world: &World) -> Option<u32> {
    let used: std::collections::HashSet<u32> =
        world.query::<&Frame>().map(|(_, frame)| frame.slot).collect();
    (1..=MAX_FRAMES).find(|slot| !used.contains(slot))
}

/// Make `entity` a rigid frame. Returns its slot (unchanged if it already is
/// one).
pub fn make_frame(world: &mut World, entity: Entity) -> Result<u32, FrameError> {
    if !world.is_alive(entity) {
        return Err(FrameError::NoSuchObject);
    }
    if let Some(frame) = world.get::<Frame>(entity) {
        return Ok(frame.slot);
    }
    let slot = free_slot(world).ok_or(FrameError::NoFreeSlot)?;
    world.insert(entity, Frame { slot });
    Ok(slot)
}

/// Make `entity` an ordinary object again.
pub fn clear_frame(world: &mut World, entity: Entity) {
    world.remove::<Frame>(entity);
}

/// Make `entity` a frame or not, keeping the saved prop in step.
pub fn set_rigid_frame(world: &mut World, entity: Entity, rigid: bool) -> Result<(), FrameError> {
    if rigid {
        make_frame(world, entity)?;
    } else {
        clear_frame(world, entity);
    }
    if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
        if rigid {
            props
                .props
                .insert(FRAME_PROP.to_string(), serde_json::Value::Bool(true));
        } else {
            props.props.remove(FRAME_PROP);
        }
    }
    Ok(())
}

/// Whether `entity` is marked a frame in its saved props.
fn prop_marks_frame(world: &World, entity: Entity) -> bool {
    world
        .get::<RenderProps>(entity)
        .and_then(|props| props.props.get(FRAME_PROP).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Make `entity`'s frame component match its saved prop (the prop is the
/// authored truth; the component is derived from it).
pub fn apply_frame_prop(world: &mut World, entity: Entity) {
    if prop_marks_frame(world, entity) {
        // Out of slots: it stays an ordinary group.
        let _ = make_frame(world, entity);
    } else if world.get::<Frame>(entity).is_some() {
        clear_frame(world, entity);
    }
}

/// Turn the saved prop into the component for each of `entities` (after a
/// scene load).
pub fn sync_frames_from_props(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        if prop_marks_frame(world, entity) {
            // Out of slots: it stays an ordinary group.
            let _ = make_frame(world, entity);
        }
    }
}

/// `t` as an f64 matrix (same Euler convention as the shaders).
fn to_dmat4(t: &Transform) -> DMat4 {
    let q = DQuat::from_euler(
        EulerRot::YXZ,
        f64::from(t.rotation[1]).to_radians(),
        f64::from(t.rotation[0]).to_radians(),
        f64::from(t.rotation[2]).to_radians(),
    );
    DMat4::from_scale_rotation_translation(
        DVec3::new(f64::from(t.scale[0]), f64::from(t.scale[1]), f64::from(t.scale[2])),
        q,
        DVec3::new(
            f64::from(t.position[0]),
            f64::from(t.position[1]),
            f64::from(t.position[2]),
        ),
    )
}

/// `entity` placed in the world, composed in f64.
pub fn world_dmat4_of(world: &World, entity: Entity) -> Option<DMat4> {
    let mut matrix = to_dmat4(&world.get::<Transform>(entity).copied()?);
    let mut at = world.get::<Parent>(entity).map(|p| p.0);
    for _ in 0..MAX_DEPTH {
        let Some(parent) = at else {
            break;
        };
        let Some(transform) = world.get::<Transform>(parent) else {
            break;
        };
        matrix = to_dmat4(&transform) * matrix;
        at = world.get::<Parent>(parent).map(|p| p.0);
    }
    Some(matrix)
}

/// The frame table: slot 0 is the identity, and each frame's slot holds the
/// matrix taking its space to the world, **relative to `origin`** (the frame's
/// camera-relative world origin; `DVec3::ZERO` outside such frames). Composed in
/// f64 and rounded once. Slots with no frame stay identity.
pub fn frame_table(world: &World, origin: DVec3) -> Vec<Mat4> {
    let frames: Vec<(Entity, u32)> = world
        .query::<&Frame>()
        .map(|(entity, frame)| (entity, frame.slot))
        .collect();
    let top = frames.iter().map(|(_, slot)| *slot).max().unwrap_or(0) as usize;
    let mut table = vec![Mat4::IDENTITY; top + 1];
    for (entity, slot) in frames {
        let Some(mut matrix) = world_dmat4_of(world, entity) else {
            continue;
        };
        matrix.w_axis -= origin.extend(0.0);
        table[slot as usize] = matrix.as_mat4();
    }
    table
}

/// The nearest frame above `entity` (not `entity` itself), if any.
pub fn frame_above(world: &World, entity: Entity) -> Option<Entity> {
    let mut at = world.get::<Parent>(entity).map(|p| p.0);
    for _ in 0..MAX_DEPTH {
        let parent = at?;
        if world.get::<Frame>(parent).is_some() {
            return Some(parent);
        }
        at = world.get::<Parent>(parent).map(|p| p.0);
    }
    None
}

/// The space id an object is drawn in: its nearest frame ancestor's slot, or 0
/// (the world). The GPU resolve derives the same value.
pub fn space_of(world: &World, entity: Entity) -> u32 {
    frame_above(world, entity)
        .and_then(|frame| world.get::<Frame>(frame).map(|f| f.slot))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_ext::{SceneWorldExt, SpawnObject};

    fn at(world: &mut World, parent: Option<Entity>, position: [f32; 3]) -> Entity {
        world
            .spawn_object(SpawnObject::new("o").with_parent(parent).with_transform(Transform {
                position,
                ..Transform::default()
            }))
            .unwrap()
    }

    #[test]
    fn slots_are_distinct_and_reused() {
        let mut world = World::new();
        let (a, b) = (at(&mut world, None, [0.0; 3]), at(&mut world, None, [0.0; 3]));
        let sa = make_frame(&mut world, a).unwrap();
        let sb = make_frame(&mut world, b).unwrap();
        assert_eq!((sa, sb), (1, 2));
        assert_eq!(make_frame(&mut world, a).unwrap(), 1, "idempotent");
        clear_frame(&mut world, a);
        let c = at(&mut world, None, [0.0; 3]);
        assert_eq!(make_frame(&mut world, c).unwrap(), 1, "the freed slot is reused");
    }

    #[test]
    fn the_table_places_frames_relative_to_the_origin_in_f64() {
        let mut world = World::new();
        let ship = at(&mut world, None, [6_400_000.0, 0.0, 0.0]);
        let room = at(&mut world, Some(ship), [0.25, 0.0, 0.0]);
        make_frame(&mut world, ship).unwrap();
        make_frame(&mut world, room).unwrap();
        let table = frame_table(&world, DVec3::new(6_400_000.0, 0.0, 0.0));
        assert_eq!(table[0], Mat4::IDENTITY);
        assert_eq!(table[1].w_axis.truncate().to_array(), [0.0, 0.0, 0.0]);
        // A nested frame composes with its parent frame, still exact.
        assert_eq!(table[2].w_axis.truncate().to_array(), [0.25, 0.0, 0.0]);
    }

    #[test]
    fn an_object_is_in_the_space_of_its_nearest_frame() {
        let mut world = World::new();
        let ship = at(&mut world, None, [0.0; 3]);
        let deck = at(&mut world, Some(ship), [0.0; 3]);
        let crate_ = at(&mut world, Some(deck), [0.0; 3]);
        let loose = at(&mut world, None, [0.0; 3]);
        make_frame(&mut world, ship).unwrap();
        assert_eq!(space_of(&world, crate_), 1);
        assert_eq!(space_of(&world, ship), 0, "a frame is itself placed in its parent's space");
        assert_eq!(space_of(&world, loose), 0);
        make_frame(&mut world, deck).unwrap();
        assert_eq!(space_of(&world, crate_), 2, "the nearest frame wins");
    }

    #[test]
    fn the_saved_prop_round_trips() {
        let mut world = World::new();
        let ship = at(&mut world, None, [0.0; 3]);
        set_rigid_frame(&mut world, ship, true).unwrap();
        assert!(prop_marks_frame(&world, ship));
        clear_frame(&mut world, ship);
        sync_frames_from_props(&mut world, &[ship]);
        assert!(world.get::<Frame>(ship).is_some(), "the prop makes it a frame again");
        set_rigid_frame(&mut world, ship, false).unwrap();
        assert!(!prop_marks_frame(&world, ship));
        assert!(world.get::<Frame>(ship).is_none());
    }
}
