//! Gameplay (`pulsar.gameplay`): the gameplay roles as scene defaults.
//!
//! Its one job. It registers the four roles a level assigns classes to
//! (Game Mode, Game State, Player Controller, Player Character: `roles`),
//! and, for the player character, what plays when the level assigns none
//! (`default_character`): a scene object with a `CharacterComponent` (the
//! character plugin's facade over its controller and input) and a camera,
//! driven by the input plugin's `Move` and `Jump` actions. A level that
//! assigns the role keeps its own class; the default is never layered on it.
//!
//! The plugin owns no object kind and links no other plugin's crate: it
//! requires the input and character `class` contracts (`pulsar-plugin.toml`)
//! and reaches their classes by name, as a script does. The role ids, trait
//! ids and the possession that routes the mapping to the pawn are the
//! engine's (`pulsar_traits::builtin`, `pulsar_game`, the input plugin's
//! session hook); a level's World Settings still pick its own classes.
//!
//! With the plugin disabled there are no roles: nothing is spawned and
//! World Settings lists no role pickers.
//!
//! This crate is the plugin's root: [`build`] registers everything.

pub mod default_character;
pub mod roles;

pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// Everything the plugin registers: the four roles, the default player
/// character, and the bridge from input actions to it.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    roles::build(p);
    default_character::build(p);
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
        assert!(engine_backend::scene::game_hooks::role_default(
            pulsar_traits::builtin::PLAYER_CHARACTER_ROLE
        )
        .is_some_and(|default| default.origin == origin));
        let _ = build;
    }
}
