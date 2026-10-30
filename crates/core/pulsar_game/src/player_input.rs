//! Player input at the edge of the game runtime.
//!
//! Possession (`ScriptDriver::advance_possession`) is what routes the local
//! player's keys, mouse and pad to their character: once the controller
//! possesses the pawn, a `PlayerInputComponent` carrying the controller's
//! mapping is attached to the pawn, and the input runtime publishes
//! `input::<Action>.triggered` (and its other phases) on the pawn's own event
//! channel, where a `Self`-scoped handler in the character's Blueprint hears
//! its player. Ending the possession, or the session, detaches it.
//!
//! This module is the whole of the runtime's knowledge of the input plugin:
//! its class name, how to attach and detach it by that name (no component type
//! is named here), and how to declare the project's action events. The
//! component, the mappings and the evaluation live in the plugin
//! (`plugins/vendor/input_plugin`), which this crate links without its GPUI
//! editors, and whose per-frame `tick` runs in the tick loop's component
//! phase like any native component's.
//!
//! [`PossessionGate`] is the one piece of shared state the rest of the
//! runtime reads: whether a possessed pawn exists, which is what keeps the
//! built-in free camera off the keys the pawn is listening to.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::pulsar_scene_model::attachments::{self, NewInstance};
use pulsar_world_registry::{attach_component, AttachError, ComponentPayload};

/// The class name of the component that puts a mapping to work on an object.
pub use input_plugin::PLAYER_INPUT_CLASS_NAME;

/// Whether the local player's controller has a possessed pawn.
///
/// Owned by the [`TickLoop`](crate::tick::TickLoop), set by its script driver
/// as possession begins and ends, and cloned to whoever must act differently
/// while a game owns the player's input (the built-in [`FreeCam`](crate::freecam::FreeCam)
/// in the windowed and Play-in-Editor hosts, which run on other threads than
/// the tick loop). Clones share one flag.
#[derive(Clone, Debug, Default)]
pub struct PossessionGate(Arc<AtomicBool>);

impl PossessionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pawn is possessed: the game owns the player's input.
    pub fn is_possessed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn set(&self, possessed: bool) {
        self.0.store(possessed, Ordering::Release);
    }
}

/// Declare the events of every input action of the project on `hub`.
///
/// A mapping registers its actions' events only once it is active, which is
/// after a Blueprint that handles `input::Jump.triggered` has linked and
/// started; declaring them up front lets any class subscribe whichever loads
/// first.
pub(crate) fn declare_events(hub: &EventHub, project_root: &Path) {
    let actions = input_plugin::runtime::events::declare_project_events(hub, project_root);
    tracing::debug!(actions, "Declared the project's input action events");
}

/// Make `mapping` (a project-relative `.inputmap` path in the project at
/// `project_root`) active on `pawn`. Returns the component instance, for
/// [`detach`].
pub(crate) fn attach(
    world: &mut World,
    pawn: Entity,
    project_root: &Path,
    mapping: &str,
) -> Result<Entity, AttachError> {
    // The runtime otherwise finds the project from the environment or the
    // working directory; this session knows better, and it is per world.
    input_plugin::runtime::set_world_project_root(world, Some(project_root.to_path_buf()));
    attach_component(
        world,
        pawn,
        NewInstance::new(PLAYER_INPUT_CLASS_NAME),
        ComponentPayload::Json(serde_json::json!({
            "enabled": true,
            "mapping": mapping,
            "priority": 0,
        })),
    )
}

/// Remove an input component [`attach`] made. Its mapping ends at the input
/// runtime's next frame, completing the actions that were running.
pub(crate) fn detach(world: &mut World, instance: Entity) -> bool {
    attachments::detach(world, instance)
}

/// The game over `world` ended: release the input session (its listeners on
/// the hub and its controller thread), so a stopped session leaves nothing
/// behind.
pub(crate) fn end_session(world: &World) {
    input_plugin::runtime::end_session(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_of_the_gate_share_one_flag() {
        let gate = PossessionGate::new();
        let seen_elsewhere = gate.clone();
        assert!(!gate.is_possessed());
        gate.set(true);
        assert!(seen_elsewhere.is_possessed());
        gate.set(false);
        assert!(!seen_elsewhere.is_possessed());
    }
}
