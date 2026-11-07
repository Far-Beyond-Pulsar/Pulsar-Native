//! What a plugin's runtime adds to the game's tick and its headless report:
//! a phase before the game's systems (the skeletal meshes' animation), a
//! publication after the scripts (the animation notifies they collected),
//! and a line in the headless run's report (how many meshes bound). The
//! plugin's component crate submits a [`TickPhaseRegistration`] or a
//! [`ContentReportRegistration`]; the game loop names no kind.
//!
//! How others do it: Unreal's tick groups (`TG_PrePhysics`, ...) that
//! components register into; Bevy's schedule labels plugins add systems
//! to. Ours names the two points the game loop has.

use pulsar_events::EventHub;
use pulsar_scenedb::{Entity, World};

/// Maps a script instance's entity variable: the entity `owner`'s script
/// holds in its variable `variable`, if any.
pub type ScriptVariableResolver<'a> = &'a dyn Fn(Entity, &str) -> Option<Entity>;

/// A plugin's work at the game tick's fixed points.
pub struct TickPhaseRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// For diagnostics; phases run in name order.
    pub name: &'static str,
    /// Before the game's systems (and so before physics), with the frame's
    /// delta in seconds.
    pub before_systems: Option<fn(world: &mut World, delta_seconds: f32)>,
    /// After the scripts released the world: publish what the tick
    /// collected on `hub`.
    pub after_scripts:
        Option<fn(world: &mut World, hub: &EventHub, resolve: ScriptVariableResolver)>,
}

inventory::collect!(TickPhaseRegistration);

impl pulsar_plugins::Registration for TickPhaseRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// Every enabled tick phase, in name order.
pub fn tick_phases() -> Vec<&'static TickPhaseRegistration> {
    let mut phases: Vec<_> = pulsar_plugins::registered::<TickPhaseRegistration>().collect();
    phases.sort_by_key(|phase| phase.name);
    phases
}

/// A line of a headless run's report: `key` with what `report` says about
/// the world (the meshes': how many mesh components, how many bound).
pub struct ContentReportRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub key: &'static str,
    pub report: fn(world: &World) -> serde_json::Value,
}

inventory::collect!(ContentReportRegistration);

impl pulsar_plugins::Registration for ContentReportRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// Every enabled content report about `world`, by key.
pub fn content_reports(world: &World) -> std::collections::BTreeMap<String, serde_json::Value> {
    pulsar_plugins::registered::<ContentReportRegistration>()
        .map(|registration| (registration.key.to_string(), (registration.report)(world)))
        .collect()
}
