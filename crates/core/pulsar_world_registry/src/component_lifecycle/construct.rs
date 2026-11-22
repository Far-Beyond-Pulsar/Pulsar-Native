//! The construct pump: delivering `construct` to new and changed instances.

use super::session::{PendingConstruct, Session};
use super::tick::lifecycles;
use pulsar_component_lifecycle::{ActorCx, ActorRegistration, ConstructInfo, ConstructReason};
use pulsar_events::EventHub;
use pulsar_scenedb::{ChangeRead, ComponentChangeKind, Entity, World};
use std::collections::HashSet;

/// The classes with a `construct`, in the frame's order.
fn constructible() -> Vec<&'static ActorRegistration> {
    lifecycles()
        .into_iter()
        .map(|lifecycle| lifecycle.0)
        .filter(|registration| registration.table.construct.is_some())
        .collect()
}

/// Every enabled instance of `registration`'s class is to construct.
fn full_scan(world: &mut World, state: &mut Session, registration: &ActorRegistration) {
    let table = &registration.table;
    let mut instances = Vec::new();
    (table.instances)(world, &mut instances);
    let pending = state
        .construct
        .pending
        .entry((table.component_type)())
        .or_default();
    pending.clear();
    for entity in instances {
        pending.insert(
            entity.bits(),
            PendingConstruct {
                entity,
                reason: ConstructReason::Rescan,
            },
        );
    }
}

/// Read the class's construct cursor into the pending set, skipping what
/// `own` names (the framework's own callbacks' writes). Returns whether the
/// journal overflowed, which needs a full scan.
pub(super) fn absorb_construct_changes(
    world: &World,
    state: &mut Session,
    registration: &ActorRegistration,
    own: Option<&HashSet<Entity>>,
) -> bool {
    let component_type = (registration.table.component_type)();
    let construct = &mut state.construct;
    let Some(cursor) = construct.cursors.get_mut(&component_type) else {
        return false;
    };
    state.scratch.clear();
    if world.read_changes(cursor, &mut state.scratch) == ChangeRead::Overflowed {
        return true;
    }
    let pending = construct.pending.entry(component_type).or_default();
    for change in &state.scratch {
        match change.kind {
            ComponentChangeKind::Removed => {
                pending.remove(&change.entity.bits());
            }
            kind => {
                if own.is_some_and(|own| own.contains(&change.entity)) {
                    continue;
                }
                let reason = if kind == ComponentChangeKind::Inserted {
                    ConstructReason::Added
                } else {
                    ConstructReason::Changed
                };
                pending
                    .entry(change.entity.bits())
                    .and_modify(|entry| entry.reason = entry.reason.max(reason))
                    .or_insert(PendingConstruct {
                        entity: change.entity,
                        reason,
                    });
            }
        }
    }
    false
}

/// Deliver `construct` for what was written since the last pump: the one
/// fixed pump point. Per instance, once: a hundred writes to one component
/// since the last pump are one call. A write a `construct` makes to its own
/// component is read past, not delivered again. The first pump of a session,
/// a requested rescan ([`Session::request_rescan`]) and an overflowed change
/// journal each scan every live instance once; that is one rescan
/// ([`ConstructStats::rescans`](crate::ConstructStats::rescans)), whatever the instance count.
///
/// Returns the `construct` calls made. Nothing is delivered while the pump is
/// suspended ([`Session::set_construct_suspended`]).
///
/// The editor drives it each frame for a [`SessionKind::Editor`](pulsar_component_lifecycle::SessionKind::Editor) session
/// (`engine_backend::scene::construct_pump`); a play session's
/// [`tick_live_components`](crate::tick_live_components) calls it at the start of the `pulsar.components`
/// step.
pub fn pump_construct(world: &mut World, events: &EventHub, state: &mut Session) -> usize {
    if state.construct.suspended {
        return 0;
    }
    profiling::profile_scope!("Lifecycle::pump_construct");
    let registrations = constructible();
    if registrations.is_empty() {
        return 0;
    }

    let mut scanned = false;
    if std::mem::take(&mut state.construct.rescan) {
        // The content changed wholesale: forget what was pending and read
        // fresh from the head of every journal.
        state.construct.cursors.clear();
        state.construct.pending.clear();
        state.construct.named.clear();
    }
    for &registration in &registrations {
        let component_type = (registration.table.component_type)();
        if !state.construct.cursors.contains_key(&component_type) {
            // Open before scanning: a write between the two is read next
            // pump (a harmless second construct), never lost.
            let cursor = world.open_change_cursor_id(component_type);
            state.construct.cursors.insert(component_type, cursor);
            full_scan(world, state, registration);
            scanned = true;
        } else if absorb_construct_changes(world, state, registration, None) {
            full_scan(world, state, registration);
            scanned = true;
        }
    }
    if scanned {
        state.construct.stats.rescans += 1;
    }

    let mut delivered = 0;
    for registration in registrations {
        let table = &registration.table;
        let Some(construct) = table.construct else {
            continue;
        };
        let component_type = (table.component_type)();
        let batch = match state.construct.pending.get_mut(&component_type) {
            Some(pending) if !pending.is_empty() => std::mem::take(pending),
            _ => continue,
        };
        let mut enabled = Vec::new();
        (table.instances)(world, &mut enabled);
        let enabled: HashSet<Entity> = enabled.into_iter().collect();
        for pending in batch.into_values() {
            let instance = pending.entity;
            if !enabled.contains(&instance) {
                continue;
            }
            let Some(link) = world
                .get::<crate::pulsar_scene_model::ComponentOwner>(instance)
                .copied()
            else {
                continue;
            };
            let changed = state
                .construct
                .named
                .remove(&instance)
                .map(|names| names.into_iter().collect())
                .unwrap_or_default();
            let mut cx = ActorCx::new(link.entity(), Some(instance), events, &mut state.scope);
            cx.construct = Some(ConstructInfo {
                reason: pending.reason,
                changed,
            });
            {
                let _scope =
                    profiling::interned_scope("Lifecycle::construct::", registration.class_name);
                construct(world, &mut cx, instance);
            }
            delivered += 1;
            state.construct.stats.delivered += 1;
            // Its own writes are not changes (D4): read them past. Others'
            // writes it made stay pending for the next pump.
            let own = HashSet::from([instance]);
            if absorb_construct_changes(world, state, registration, Some(&own)) {
                full_scan(world, state, registration);
                state.construct.stats.rescans += 1;
            }
        }
    }
    delivered
}
