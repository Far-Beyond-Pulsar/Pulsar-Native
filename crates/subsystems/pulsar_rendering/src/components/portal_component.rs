//! Portal component — a real-time portal surface linked to exactly one other
//! `PortalComponent` sharing the same `portal_id`.
//!
//! Unlike every other component in this crate (one scene object → one helio
//! actor), a portal is inherently a *pair*: on its own, a single
//! `PortalComponent` has nothing to connect to and creates nothing in the
//! helio scene. Placing a second `PortalComponent` elsewhere with the same
//! `portal_id` completes the pair — [`PortalLinkCache`] (in
//! `crate::subsystems`) tracks both sides across sync passes (which one gets
//! synced first is not guaranteed) and creates/updates/tears down the real
//! `helio::Scene` portal as sides come and go. See that type's doc for the
//! full mechanism.
//!
//! Movable the same way `LightComponent`/`StaticMeshComponent` are: this
//! component reads no transform of its own — `sync_component` is handed the
//! owning scene object's current position/rotation every sync pass (via
//! `RuntimeComponentOwner`), so animating, dragging, or otherwise moving the
//! parent object moves its side of the portal automatically.

use engine_class_derive::{engine_class, register_runtime_behavior};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet, RuntimeComponentOwner,
    get_subsystem,
};

use crate::subsystems::{PortalLinkCache, PortalSide, apply_portal_pair_action, portal_link_key};

pub const PORTAL_CLASS_NAME: &str = "PortalComponent";

/// One end of a linked portal pair.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("General", category_color = "#6EC5FF")]
pub struct PortalComponent {
    #[property]
    pub enabled: bool,
    /// Link ID — exactly **two** `PortalComponent`s sharing this ID become a
    /// linked pair, forming the portal: each shows what's on the other's
    /// side. A third component claiming an already-paired ID is a
    /// configuration error (reported, and ignored until the count drops back
    /// to two).
    #[property]
    pub portal_id: i32,
    /// Opening width, in world units, centered on the owning object.
    ///
    /// Only one side's size actually governs the portal's clip window —
    /// which of the two linked components "wins" is unspecified — so keep
    /// both ends of a pair set to the same width/height; a mismatch won't
    /// error, but which one applies isn't something to rely on.
    #[property(min = 0.1, max = 1000.0, step = 0.1, category = "General")]
    pub width: f32,
    /// Opening height, in world units, centered on the owning object. See
    /// `width` for why both ends of a pair should match.
    #[property(min = 0.1, max = 1000.0, step = 0.1, category = "General")]
    pub height: f32,
}

impl Default for PortalComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            portal_id: 0,
            width: 2.0,
            height: 3.0,
        }
    }
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PortalComponent {
    const CLASS_NAME: &'static str = PORTAL_CLASS_NAME;

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        // Phase 1 — pure bookkeeping against `PortalLinkCache`: record (or
        // drop) this object's side and get back whatever action, if any, is
        // now needed to bring the real helio portal in sync. No `Renderer`
        // access here — see `apply_portal_pair_action`'s doc for why this is
        // its own phase (a `ComponentRuntimeContext` can't hand out two
        // simultaneous mutable subsystem borrows).
        let action = if component.enabled {
            let side = PortalSide {
                position: owner.position,
                rotation: owner.rotation,
                half_extent: [component.width * 0.5, component.height * 0.5],
            };
            match get_subsystem!(context, PortalLinkCache).record_side(
                component.portal_id,
                owner.scene_object_id,
                side,
            ) {
                Ok(action) => action,
                Err(message) => {
                    context.report_error(format!(
                        "{PORTAL_CLASS_NAME} on '{}': {message}",
                        owner.scene_object_id
                    ));
                    return;
                }
            }
        } else {
            get_subsystem!(context, PortalLinkCache)
                .record_absent(component.portal_id, owner.scene_object_id)
        };

        // A disabled side is simply absent from `PortalLinkCache` — same as
        // if the object had been deleted — so only a live, enabled side
        // needs to survive this pass's stale sweep.
        if component.enabled {
            if let Some(live_keys) = context.subsystems_mut().get_mut::<LiveKeySet>() {
                live_keys.insert(portal_link_key(component.portal_id, owner.scene_object_id));
            }
        }

        // Phase 2 — apply the action to the real scene, then (phase 3, if
        // the action created or removed a portal) feed the result back into
        // the cache. Three separate subsystem fetches, none overlapping.
        let scene = get_subsystem!(context, helio::Renderer).scene_mut();
        if let Some((portal_id, id)) = apply_portal_pair_action(scene, action) {
            let cache = get_subsystem!(context, PortalLinkCache);
            match id {
                Some(id) => cache.set_active(portal_id, id),
                None => cache.clear_active(portal_id),
            }
        }
    }
}
