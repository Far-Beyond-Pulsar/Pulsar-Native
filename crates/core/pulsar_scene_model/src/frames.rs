//! Spaces: groups whose contents are never rewritten when they move.
//!
//! Several things define a **space** the renderer draws a group of objects
//! through, with one matrix in the coordinate-space table instead of one row per
//! member (see `.agents/FRAMES.md`): a **rigid frame** an author makes of a
//! folder ([`RigidFrame`]), a placed sublevel, a ship or room. They are
//! different authored things with one runtime need, so none of them knows about
//! the others and none numbers the table itself:
//!
//! * Anything that defines a space **registers** a [`SpaceDefiner`] saying
//!   which objects it applies to. Registrations live in the crate that owns the
//!   component (this crate registers [`RigidFrame`]; the engine registers
//!   sublevel placements), so the scene model needs no knowledge of them.
//! * [`sync_spaces`] is the one allocator. It gives every defining object a
//!   [`SpaceSlot`] (`1..=255`; 0 is the world), takes it away when the object
//!   stops defining one, and runs each definer's normalizer.
//! * [`SpaceSlot`] is mirrored to the GPU as the derived
//!   [`ObjectFrame`](crate::ObjectFrame) row, so the hierarchy resolve can find
//!   the nearest space above each object.
//! * [`frame_table`] builds the table's matrices: each space's matrix in the
//!   world, composed in f64 (spaces nest, and may sit anywhere on a planet) and
//!   handed over relative to the frame's world origin as f32.
//!
//! A rigid frame is authored with a prop on its object ([`FRAME_PROP`]) so it
//! survives save and load with no file-format change.

use glam::{DVec3, Mat4};
use pulsar_scenedb::{Entity, World};
use std::collections::HashSet;

use crate::components::{Parent, RenderProps};
use crate::hierarchy_transform::{world_matrix_of, MAX_DEPTH};

/// Spaces the table can hold. A space id travels in 8 bits of an instance's
/// flags (`helio_pass_object_batch::INSTANCE_COORDINATE_SPACE_*`), and slot 0 is
/// the world.
pub const MAX_SPACES: u32 = 255;

/// The object prop that marks a rigid frame in a saved scene.
pub const FRAME_PROP: &str = "rigid_frame";

/// An author made this object a rigid frame: its contents are drawn through one
/// matrix. The component is derived from the saved prop ([`FRAME_PROP`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RigidFrame;

/// The object's slot in the coordinate-space table. Assigned and removed by
/// [`sync_spaces`] for objects a [`SpaceDefiner`] claims; never authored and not
/// saved (slots are reassigned on load).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceSlot {
    pub slot: u32,
}

/// Something that defines spaces. Register one with `inventory::submit!` from
/// the crate that owns the component.
pub struct SpaceDefiner {
    /// For diagnostics.
    pub name: &'static str,
    /// Every object this definer makes a space of.
    pub entities: fn(&World) -> Vec<Entity>,
    /// Whether an object is already in the form the allocator expects (a
    /// read-only check, so a frame can ask whether a sync is needed without
    /// taking a write lock). `None`: always.
    pub is_normalized: Option<fn(&World, Entity) -> bool>,
    /// Put an object into that form (for example fold a sublevel placement's
    /// matrix into the object's own transform, so a space has exactly one
    /// placement). `None`: nothing to do.
    pub normalize: Option<fn(&mut World, Entity)>,
}

inventory::collect!(SpaceDefiner);

fn rigid_frames(world: &World) -> Vec<Entity> {
    world
        .query::<&RigidFrame>()
        .map(|(entity, _)| entity)
        .collect()
}

inventory::submit! {
    SpaceDefiner {
        name: "rigid frame",
        entities: rigid_frames,
        is_normalized: None,
        normalize: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("all {MAX_SPACES} space slots are in use")]
    NoFreeSlot,
    #[error("the object does not exist")]
    NoSuchObject,
}

/// Every object any definer makes a space of, in a stable order.
pub fn defining_entities(world: &World) -> Vec<Entity> {
    let mut found: Vec<Entity> = inventory::iter::<SpaceDefiner>()
        .flat_map(|definer| (definer.entities)(world))
        .filter(|entity| world.is_alive(*entity))
        .collect();
    found.sort_by_key(|entity| entity.bits());
    found.dedup();
    found
}

/// Whether [`sync_spaces`] has anything to do: a defining object without a slot,
/// a slot on an object that no longer defines one, or a definer's object not yet
/// normalized. Read-only and cheap, so a renderer can check every frame.
pub fn needs_space_sync(world: &World) -> bool {
    let defining = defining_entities(world);
    let wanted: HashSet<Entity> = defining.iter().copied().collect();
    if defining
        .iter()
        .any(|entity| world.get::<SpaceSlot>(*entity).is_none())
    {
        return true;
    }
    if world
        .query::<&SpaceSlot>()
        .any(|(entity, _)| !wanted.contains(&entity))
    {
        return true;
    }
    inventory::iter::<SpaceDefiner>().any(|definer| {
        definer.is_normalized.is_some_and(|is_normalized| {
            (definer.entities)(world)
                .into_iter()
                .any(|entity| world.is_alive(entity) && !is_normalized(world, entity))
        })
    })
}

/// The allocator: bring every object's [`SpaceSlot`] in line with what the
/// registered definers claim. Returns whether anything changed. Idempotent.
pub fn sync_spaces(world: &mut World) -> bool {
    let mut changed = false;
    for definer in inventory::iter::<SpaceDefiner>() {
        let Some(normalize) = definer.normalize else {
            continue;
        };
        for entity in (definer.entities)(world) {
            let normalized = definer
                .is_normalized
                .is_some_and(|is_normalized| is_normalized(world, entity));
            if world.is_alive(entity) && !normalized {
                normalize(world, entity);
                changed = true;
            }
        }
    }
    let defining = defining_entities(world);
    let wanted: HashSet<Entity> = defining.iter().copied().collect();
    let stale: Vec<Entity> = world
        .query::<&SpaceSlot>()
        .map(|(entity, _)| entity)
        .filter(|entity| !wanted.contains(entity))
        .collect();
    for entity in stale {
        world.remove::<SpaceSlot>(entity);
        changed = true;
    }
    let mut used: HashSet<u32> = world.query::<&SpaceSlot>().map(|(_, s)| s.slot).collect();
    for entity in defining {
        if world.get::<SpaceSlot>(entity).is_some() {
            continue;
        }
        // Out of slots: it stays an ordinary group.
        let Some(slot) = (1..=MAX_SPACES).find(|slot| !used.contains(slot)) else {
            continue;
        };
        used.insert(slot);
        world.insert(entity, SpaceSlot { slot });
        changed = true;
    }
    changed
}

/// Make `entity` a rigid frame. Returns its slot (unchanged if it already is
/// one).
pub fn make_frame(world: &mut World, entity: Entity) -> Result<u32, FrameError> {
    if !world.is_alive(entity) {
        return Err(FrameError::NoSuchObject);
    }
    if world.get::<RigidFrame>(entity).is_none() {
        world.insert(entity, RigidFrame);
    }
    sync_spaces(world);
    world
        .get::<SpaceSlot>(entity)
        .map(|s| s.slot)
        .ok_or(FrameError::NoFreeSlot)
}

/// Make `entity` an ordinary object again (a sublevel placement stays a space:
/// that is its own definer's business).
pub fn clear_frame(world: &mut World, entity: Entity) {
    world.remove::<RigidFrame>(entity);
    sync_spaces(world);
}

/// Make `entity` a rigid frame or not, keeping the saved prop in step.
pub fn set_rigid_frame(world: &mut World, entity: Entity, rigid: bool) -> Result<(), FrameError> {
    if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
        if rigid {
            props
                .props
                .insert(FRAME_PROP.to_string(), serde_json::Value::Bool(true));
        } else {
            props.props.remove(FRAME_PROP);
        }
    }
    if rigid {
        make_frame(world, entity).map(|_| ())
    } else {
        clear_frame(world, entity);
        Ok(())
    }
}

/// Whether `entity` is marked a rigid frame in its saved props.
fn prop_marks_frame(world: &World, entity: Entity) -> bool {
    world
        .get::<RenderProps>(entity)
        .and_then(|props| props.props.get(FRAME_PROP).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Make `entity`'s [`RigidFrame`] match its saved prop (the prop is the
/// authored truth; the component is derived from it) and sync the slots.
pub fn apply_frame_prop(world: &mut World, entity: Entity) {
    let marked = prop_marks_frame(world, entity);
    let is = world.get::<RigidFrame>(entity).is_some();
    if marked && !is {
        world.insert(entity, RigidFrame);
    } else if !marked && is {
        world.remove::<RigidFrame>(entity);
    }
    sync_spaces(world);
}

/// [`apply_frame_prop`] for each of `entities` (after a scene load), with one
/// slot sync at the end.
pub fn sync_frames_from_props(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        if prop_marks_frame(world, entity) && world.get::<RigidFrame>(entity).is_none() {
            world.insert(entity, RigidFrame);
        }
    }
    sync_spaces(world);
}

/// The matrices of the spaces objects define: slot 0 is the identity, and each
/// space's slot holds the matrix taking its space to the world, **relative to
/// `origin`** (the frame's camera-relative world origin; `DVec3::ZERO` outside
/// such frames). Composed in f64 and rounded once. Slots with no space stay
/// identity. Spaces from other producers (portal projections) go after these
/// through the host's allocator.
pub fn frame_table(world: &World, origin: DVec3) -> Vec<Mat4> {
    let spaces: Vec<(Entity, u32)> = world
        .query::<&SpaceSlot>()
        .map(|(entity, space)| (entity, space.slot))
        .collect();
    let top = spaces.iter().map(|(_, slot)| *slot).max().unwrap_or(0) as usize;
    let mut table = vec![Mat4::IDENTITY; top + 1];
    for (entity, slot) in spaces {
        let Some(mut matrix) = world_matrix_of(world, entity) else {
            continue;
        };
        matrix.w_axis -= origin.extend(0.0);
        table[slot as usize] = matrix.as_mat4();
    }
    table
}

/// The nearest space-defining object above `entity` (not `entity` itself).
pub fn frame_above(world: &World, entity: Entity) -> Option<Entity> {
    let mut at = world.get::<Parent>(entity).map(|p| p.0);
    for _ in 0..MAX_DEPTH {
        let parent = at?;
        if world.get::<SpaceSlot>(parent).is_some() {
            return Some(parent);
        }
        at = world.get::<Parent>(parent).map(|p| p.0);
    }
    None
}

/// The space id an object is drawn in: its nearest space-defining ancestor's
/// slot, or 0 (the world). The GPU resolve derives the same value.
pub fn space_of(world: &World, entity: Entity) -> u32 {
    frame_above(world, entity)
        .and_then(|frame| world.get::<SpaceSlot>(frame).map(|s| s.slot))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Transform;
    use crate::world_ext::{SceneWorldExt, SpawnObject};

    fn at(world: &mut World, parent: Option<Entity>, position: [f64; 3]) -> Entity {
        world
            .spawn_object(
                SpawnObject::new("o")
                    .with_parent(parent)
                    .with_transform(Transform {
                        position,
                        ..Transform::default()
                    }),
            )
            .unwrap()
    }

    #[test]
    fn slots_are_distinct_and_reused() {
        let mut world = World::new();
        let (a, b) = (
            at(&mut world, None, [0.0; 3]),
            at(&mut world, None, [0.0; 3]),
        );
        let sa = make_frame(&mut world, a).unwrap();
        let sb = make_frame(&mut world, b).unwrap();
        assert_eq!((sa, sb), (1, 2));
        assert_eq!(make_frame(&mut world, a).unwrap(), 1, "idempotent");
        clear_frame(&mut world, a);
        let c = at(&mut world, None, [0.0; 3]);
        assert_eq!(
            make_frame(&mut world, c).unwrap(),
            1,
            "the freed slot is reused"
        );
    }

    #[test]
    fn the_table_places_spaces_relative_to_the_origin_in_f64() {
        let mut world = World::new();
        let ship = at(&mut world, None, [6_400_000.0, 0.0, 0.0]);
        let room = at(&mut world, Some(ship), [0.25, 0.0, 0.0]);
        make_frame(&mut world, ship).unwrap();
        make_frame(&mut world, room).unwrap();
        let table = frame_table(&world, DVec3::new(6_400_000.0, 0.0, 0.0));
        assert_eq!(table[0], Mat4::IDENTITY);
        assert_eq!(table[1].w_axis.truncate().to_array(), [0.0, 0.0, 0.0]);
        // A nested space composes with its parent space, still exact.
        assert_eq!(table[2].w_axis.truncate().to_array(), [0.25, 0.0, 0.0]);
    }

    #[test]
    fn an_object_is_in_the_space_of_its_nearest_definer() {
        let mut world = World::new();
        let ship = at(&mut world, None, [0.0; 3]);
        let deck = at(&mut world, Some(ship), [0.0; 3]);
        let crate_ = at(&mut world, Some(deck), [0.0; 3]);
        let loose = at(&mut world, None, [0.0; 3]);
        make_frame(&mut world, ship).unwrap();
        assert_eq!(space_of(&world, crate_), 1);
        assert_eq!(
            space_of(&world, ship),
            0,
            "a space is itself placed in its parent's space"
        );
        assert_eq!(space_of(&world, loose), 0);
        make_frame(&mut world, deck).unwrap();
        assert_eq!(space_of(&world, crate_), 2, "the nearest one wins");
    }

    #[test]
    fn the_saved_prop_round_trips() {
        let mut world = World::new();
        let ship = at(&mut world, None, [0.0; 3]);
        set_rigid_frame(&mut world, ship, true).unwrap();
        assert!(prop_marks_frame(&world, ship));
        world.remove::<RigidFrame>(ship);
        sync_frames_from_props(&mut world, &[ship]);
        assert!(
            world.get::<SpaceSlot>(ship).is_some(),
            "the prop makes it a space again"
        );
        set_rigid_frame(&mut world, ship, false).unwrap();
        assert!(!prop_marks_frame(&world, ship));
        assert!(world.get::<SpaceSlot>(ship).is_none());
    }

    // A second kind of space, registered from outside the module the way the
    // engine registers sublevel placements: it needs no knowledge of frames and
    // frames need none of it.
    #[derive(Clone, Copy)]
    struct TestPlacement {
        offset: f64,
    }

    fn placements(world: &World) -> Vec<Entity> {
        world.query::<&TestPlacement>().map(|(e, _)| e).collect()
    }

    fn placement_is_folded(world: &World, entity: Entity) -> bool {
        world
            .get::<TestPlacement>(entity)
            .is_none_or(|p| p.offset == 0.0)
    }

    fn fold_placement(world: &mut World, entity: Entity) {
        let Some(placement) = world.get::<TestPlacement>(entity).copied() else {
            return;
        };
        if let Some(mut t) = world.get_mut::<Transform>(entity) {
            t.position[0] += placement.offset;
        }
        world.insert(entity, TestPlacement { offset: 0.0 });
    }

    inventory::submit! {
        SpaceDefiner {
            name: "test placement",
            entities: placements,
            is_normalized: Some(placement_is_folded),
            normalize: Some(fold_placement),
        }
    }

    #[test]
    fn independent_definers_share_one_allocator() {
        let mut world = World::new();
        let folder = at(&mut world, None, [0.0; 3]);
        let placed = at(&mut world, None, [10.0, 0.0, 0.0]);
        world.insert(placed, TestPlacement { offset: 5.0 });
        assert!(needs_space_sync(&world), "a placement without a slot");
        make_frame(&mut world, folder).unwrap();

        // Both got distinct slots from the one allocator; the placement was
        // normalized once (its offset folded into the object's transform).
        let slots = (
            world.get::<SpaceSlot>(folder).unwrap().slot,
            world.get::<SpaceSlot>(placed).unwrap().slot,
        );
        assert_ne!(slots.0, slots.1);
        assert_eq!(world.get::<Transform>(placed).unwrap().position[0], 15.0);
        assert!(!needs_space_sync(&world));
        assert!(!sync_spaces(&mut world), "idempotent");
        assert_eq!(world.get::<Transform>(placed).unwrap().position[0], 15.0);

        // Removing a definer's component takes its slot away.
        world.remove::<TestPlacement>(placed);
        assert!(needs_space_sync(&world));
        sync_spaces(&mut world);
        assert!(world.get::<SpaceSlot>(placed).is_none());
        assert!(world.get::<SpaceSlot>(folder).is_some());
    }
}
