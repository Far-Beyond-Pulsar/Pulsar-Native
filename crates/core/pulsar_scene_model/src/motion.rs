//! Who may move an object.
//!
//! Moving an object is a promise-checked operation: an object authored as
//! `Static` must not be moved at runtime, because renderers cache what they
//! derived from it. The scene vocabulary does not know what makes an object
//! static: the crate that owns that component (the renderer's
//! `Movability`) registers a [`MotionGate`] that reads it. Transform
//! methods consult every registered gate before they write.
//!
//! A gate sees `(&World, Entity)` and answers `Ok(())` when it has nothing
//! against the move (including when the entity lacks its component: opting
//! in must never change results).

use pulsar_scenedb::{Entity, World};

/// A check run before an object's transform changes at runtime.
pub struct MotionGate {
    /// For the error message, e.g. `"Movability"`.
    pub name: &'static str,
    /// `Err(reason)` forbids the move.
    pub check: fn(&World, Entity) -> Result<(), String>,
}

inventory::collect!(MotionGate);

/// Whether `entity` may be moved right now: every registered
/// [`MotionGate`] must agree. Fails with the first gate's reason.
pub fn ensure_can_move(world: &World, entity: Entity) -> Result<(), String> {
    if entity == Entity::DANGLING || !world.is_alive(entity) {
        return Err(format!("{entity:?} is not alive"));
    }
    for gate in inventory::iter::<MotionGate> {
        (gate.check)(world, entity).map_err(|reason| format!("{} forbids moving {entity:?}: {reason}", gate.name))?;
    }
    Ok(())
}
