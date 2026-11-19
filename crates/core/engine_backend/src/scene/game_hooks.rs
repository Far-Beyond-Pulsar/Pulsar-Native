//! What a plugin's runtime adds to the game's tick, its session and its
//! headless report: a phase before the game's systems (the skeletal meshes'
//! animation), a publication after the scripts (the animation notifies they
//! collected), its part in a session and a possession (the player's input),
//! and a line in the headless run's report (how many meshes bound). The
//! plugin's root crate submits a [`TickPhaseRegistration`], a
//! [`GameSessionRegistration`], a [`ContentReportRegistration`] or an
//! [`AssetChangedRegistration`]; the game loop names no kind.
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

impl pulsar_plugins::Contribution for TickPhaseRegistration {
    const CATEGORY: &'static str = "game.tick_phase";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(TickPhaseRegistration);

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

impl pulsar_plugins::Contribution for ContentReportRegistration {
    const CATEGORY: &'static str = "game.content_report";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.key.to_string()
    }
}

pulsar_plugins::registration!(ContentReportRegistration);

/// Every enabled content report about `world`, by key.
pub fn content_reports(world: &World) -> std::collections::BTreeMap<String, serde_json::Value> {
    pulsar_plugins::registered::<ContentReportRegistration>()
        .map(|registration| (registration.key.to_string(), (registration.report)(world)))
        .collect()
}

/// A plugin's reaction to a changed asset file that is not itself a world
/// asset: the animation plugin recompiles every running graph that depends on
/// the file (a sequence, Blend Space, rig or Montage the graph names), which no
/// per-file world asset reload reaches. `reload_updated_asset` runs every
/// enabled hook after the world assets the file is or feeds were queued.
pub struct AssetChangedRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Its stable id in the plugin catalog (`<kind>.<what>`).
    pub name: &'static str,
    /// The file at the absolute `path` changed; returns how many things the
    /// hook updated.
    pub on_changed: fn(world: &mut World, path: &std::path::Path) -> usize,
}

impl pulsar_plugins::Contribution for AssetChangedRegistration {
    const CATEGORY: &'static str = "game.asset_changed";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(AssetChangedRegistration);

/// Tell every enabled [`AssetChangedRegistration`] that the file at `path`
/// changed. Returns the sum of what they updated.
pub fn notify_asset_changed(world: &mut World, path: &std::path::Path) -> usize {
    pulsar_plugins::registered::<AssetChangedRegistration>()
        .map(|registration| (registration.on_changed)(world, path))
        .sum()
}

/// What a game session needs to know about a possession: the project it
/// plays and the roles its level chose.
pub struct PossessionContext<'a> {
    /// The project's root directory.
    pub project_root: &'a std::path::Path,
    /// The level's roles and its `world_settings` (`pulsar_traits::WorldRoles`),
    /// where a plugin reads its own settings (`WorldRoles::setting`).
    pub roles: &'a pulsar_traits::WorldRoles,
}

/// A plugin's part in a game session and in the local player's possession of
/// a pawn: the input plugin declares the project's action events when the
/// session starts, puts the player's mapping to work on the possessed pawn
/// and releases its devices when the session ends. The game loop
/// (`pulsar_game`'s script driver) runs every enabled one and names none.
///
/// How others do it: Unreal's `APlayerController::SetupInputComponent`, which
/// the Enhanced Input plugin implements; Bevy's `OnEnter`/`OnExit` state
/// schedules plugins add systems to.
pub struct GameSessionRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// Its stable id in the plugin catalog (`<kind>.<what>`); hooks run in
    /// name order.
    pub name: &'static str,
    /// The session's scripts go on `hub` for the project at the path, before
    /// any class links: declare the events scripts may handle.
    pub begin: Option<fn(hub: &EventHub, project_root: &std::path::Path)>,
    /// The local player's controller possessed `pawn`. Returns the component
    /// instance the plugin attached to the pawn for the possession, if any;
    /// the driver detaches it when the possession ends (or, left by a session
    /// that never shut down, when the next one possesses the pawn). An error
    /// is reported once and the possession carries on without it.
    pub possess: Option<
        fn(
            world: &mut World,
            pawn: Entity,
            context: &PossessionContext<'_>,
        ) -> Result<Option<Entity>, String>,
    >,
    /// The session over `world` ended: release what it held.
    pub end: Option<fn(world: &World)>,
}

impl pulsar_plugins::Contribution for GameSessionRegistration {
    const CATEGORY: &'static str = "game.session";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(GameSessionRegistration);

/// Every enabled game session hook, in name order.
pub fn game_sessions() -> Vec<&'static GameSessionRegistration> {
    let mut hooks: Vec<_> = pulsar_plugins::registered::<GameSessionRegistration>().collect();
    hooks.sort_by_key(|hook| hook.name);
    hooks
}
