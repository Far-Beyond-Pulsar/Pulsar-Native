//! Gameplay (`pulsar.gameplay`): the gameplay roles as scene defaults.
//!
//! Its one job. It registers the four roles a level assigns classes to (Game
//! Mode, Game State, Player Controller, Player Character: `roles`) and ships
//! the default player character as a class (`files`): `PlayerCharacter.class`,
//! mounted read-only at `plugins/Gameplay/classes/`, an implementer of the
//! Player Character trait that a level selects in World Settings like any
//! other. The roles system spawns it; nothing is spawned for a level that
//! selects no character (the free camera is the only fallback).
//!
//! `player_input` follows the possessed pawn the driver announces
//! (`PossessionChanged`) and drives its `CharacterComponent` from the input
//! plugin's `Move` and `Jump` events, for the shipped class and for any class
//! of a project that has a character on it.
//!
//! The plugin owns no object kind and links no other plugin's crate: it
//! requires the input and character `class` contracts (`pulsar-plugin.toml`)
//! and reaches their classes by name. The role ids, trait ids and the
//! possession that routes the mapping to the pawn are the engine's
//! (`pulsar_traits::builtin`, `pulsar_game`, the input plugin's session hook).
//!
//! With the plugin disabled there are no roles: World Settings lists no role
//! pickers and nothing is spawned.
//!
//! This crate is the plugin's root: [`build`] registers everything.

pub mod files;
pub mod player_input;
pub mod roles;

pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// Everything the plugin registers: the four roles, its folder with the default
/// player character class, and the bridge from input actions to the pawn.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    roles::build(p);
    p.add(files::provider(p.origin()));
    player_input::build(p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registration_resolves_to_the_plugin() {
        let plugin = pulsar_plugins::installed()
            .iter()
            .find(|plugin| plugin.id == "pulsar.gameplay")
            .expect("the plugin is installed");
        let origin = plugin.root.as_str();
        let roles: Vec<_> = pulsar_traits::roles::roles()
            .into_iter()
            .filter(|role| role.origin == origin)
            .map(|role| role.id)
            .collect();
        assert_eq!(roles.len(), 4, "{roles:?}");
        let _ = build;
    }
}
