//! The four gameplay roles. The engine owns their traits and ids
//! (`pulsar_traits::builtin`); this plugin registers the roles themselves, so
//! the roles exist when the plugin is enabled and not otherwise.
//!
//! Spawned in `order` when a session starts: the game mode, the game state and
//! the controller begin within the first script phase (so they `begin_play`
//! before the level's own objects); the character's spawn is queued like a
//! script's and possession follows when it runs.

use pulsar_traits::builtin::{
    GAME_MODE_ROLE, GAME_MODE_TRAIT_ID, GAME_STATE_ROLE, GAME_STATE_TRAIT_ID,
    PLAYER_CHARACTER_ROLE, PLAYER_CHARACTER_TRAIT_ID, PLAYER_CONTROLLER_ROLE,
    PLAYER_CONTROLLER_TRAIT_ID,
};
use pulsar_traits::roles::{RoleRegistration, RoleStart};

/// The key in the level's `world_settings` of each role, as levels have always
/// stored them.
pub const GAME_MODE_SETTING: &str = "game_mode_blueprint";
pub const GAME_STATE_SETTING: &str = "game_state_blueprint";
pub const PLAYER_CONTROLLER_SETTING: &str = "player_controller_blueprint";
pub const PLAYER_CHARACTER_SETTING: &str = "player_character_blueprint";

/// The roles as data: `(id, label, trait, setting key, order, start)`.
pub const ROLES: [(&str, &str, &str, &str, i32, RoleStart); 4] = [
    (
        GAME_MODE_ROLE,
        "Game Mode",
        GAME_MODE_TRAIT_ID,
        GAME_MODE_SETTING,
        0,
        RoleStart::Now,
    ),
    (
        GAME_STATE_ROLE,
        "Game State",
        GAME_STATE_TRAIT_ID,
        GAME_STATE_SETTING,
        10,
        RoleStart::Now,
    ),
    (
        PLAYER_CONTROLLER_ROLE,
        "Player Controller",
        PLAYER_CONTROLLER_TRAIT_ID,
        PLAYER_CONTROLLER_SETTING,
        20,
        RoleStart::Now,
    ),
    (
        PLAYER_CHARACTER_ROLE,
        "Player Character",
        PLAYER_CHARACTER_TRAIT_ID,
        PLAYER_CHARACTER_SETTING,
        30,
        RoleStart::EndOfFrame,
    ),
];

pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    for (id, label, trait_id, setting_key, order, start) in ROLES {
        p.add(RoleRegistration {
            origin: p.origin(),
            id,
            label,
            trait_id,
            setting_key,
            order,
            start,
        });
    }
}
