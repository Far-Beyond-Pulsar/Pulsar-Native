//! Scene subsystems plugins add to every scene.
//!
//! A plugin whose components need a world-wide step (building their worlds
//! off the game thread, answering what the renderer sent back) registers a
//! SceneDB [`Subsystem`](pulsar_scenedb::Subsystem) here; a host installs
//! every registered one into each scene it steps ([`install_scene_subsystems`]),
//! and `SceneDb::step` runs them. The host knows no subsystem by name.

use pulsar_scenedb::SceneDb;

/// A subsystem every scene gets: `name` is its SceneDB name (one instance
/// per scene), `install` registers a new one on a scene.
pub struct SceneSubsystemRegistration {
    pub name: &'static str,
    pub install: fn(&mut SceneDb),
}

inventory::collect!(SceneSubsystemRegistration);

/// Install every registered scene subsystem `scene` lacks. Cheap when it
/// has them all; call it before each `SceneDb::step`.
pub fn install_scene_subsystems(scene: &mut SceneDb) {
    for registration in inventory::iter::<SceneSubsystemRegistration> {
        if scene.subsystem_by_name_mut(registration.name).is_none() {
            (registration.install)(scene);
        }
    }
}
