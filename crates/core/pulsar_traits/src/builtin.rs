//! What the engine itself brings: its trait files, served read-only into
//! every project from the `pulsar/` mount, and its four gameplay roles.
//!
//! The traits are ordinary `*.trait.json` files (`content/traits/`), embedded
//! and registered as the engine's [`MountedContent`](engine_fs::MountedContent),
//! so a project sees them at `pulsar/traits/<name>.trait.json` and cannot
//! change them (a write fails with `engine_fs::ReadOnlyMount`). Their ids are
//! fixed, readable strings in the engine's namespace (`pulsar.GameMode`), equal
//! in every project. To change a contract, edit the file and bump its
//! `meta.contractVersion`.
//!
//! Role traits take plain ids (`u32` player, `u64` actor handle) until the
//! spawn path hands Blueprints typed actor references; changing a parameter
//! type is a contract version bump.

use crate::roles::{RoleRegistration, RoleStart};

/// Id of the `IntoGraph` trait: Helio builds a renderer from anything that
/// implements it (a built-in pipeline, a render graph asset).
pub const INTO_GRAPH_TRAIT_ID: &str = "pulsar.IntoGraph";
pub const GAME_MODE_TRAIT_ID: &str = "pulsar.GameMode";
pub const PLAYER_CONTROLLER_TRAIT_ID: &str = "pulsar.PlayerController";
pub const PLAYER_CHARACTER_TRAIT_ID: &str = "pulsar.PlayerCharacter";
pub const GAME_STATE_TRAIT_ID: &str = "pulsar.GameState";

/// Where the engine's trait files are in a project.
pub const GAME_MODE_TRAIT_PATH: &str = "pulsar/traits/game_mode.trait.json";
pub const PLAYER_CONTROLLER_TRAIT_PATH: &str = "pulsar/traits/player_controller.trait.json";
pub const PLAYER_CHARACTER_TRAIT_PATH: &str = "pulsar/traits/player_character.trait.json";
pub const GAME_STATE_TRAIT_PATH: &str = "pulsar/traits/game_state.trait.json";
pub const INTO_GRAPH_TRAIT_PATH: &str = "pulsar/traits/into_graph.trait.json";

/// The engine's roles: the rules of a level, its shared state, and the local
/// player's controller and body. The game runtime gives these four their
/// session behaviour (possession, input); any plugin role is spawned and
/// called the same way and reacts to the role events on the hub.
pub const GAME_MODE_ROLE: &str = "pulsar.game_mode";
pub const GAME_STATE_ROLE: &str = "pulsar.game_state";
pub const PLAYER_CONTROLLER_ROLE: &str = "pulsar.player_controller";
pub const PLAYER_CHARACTER_ROLE: &str = "pulsar.player_character";

/// The engine's trait files, as mounted.
pub const TRAIT_FILES: &[engine_fs::MountedFile] = engine_fs::mounted_files!("content":
    "traits/into_graph.trait.json",
    "traits/game_mode.trait.json",
    "traits/game_state.trait.json",
    "traits/player_character.trait.json",
    "traits/player_controller.trait.json",
);

inventory::submit! {
    engine_fs::MountedContent {
        origin: pulsar_plugins::engine_origin!(),
        id: "pulsar.traits",
        files: TRAIT_FILES,
    }
}

// Spawned in this order when a session starts: the game mode, the game
// state and the controller begin within the first script phase (so they
// `begin_play` before the level's own objects); the character's spawn is
// queued like a script's and possession follows when it runs.
inventory::submit! {
    RoleRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: GAME_MODE_ROLE,
        label: "Game Mode",
        trait_id: GAME_MODE_TRAIT_ID,
        setting_key: "game_mode_blueprint",
        order: 0,
        start: RoleStart::Now,
    }
}

inventory::submit! {
    RoleRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: GAME_STATE_ROLE,
        label: "Game State",
        trait_id: GAME_STATE_TRAIT_ID,
        setting_key: "game_state_blueprint",
        order: 10,
        start: RoleStart::Now,
    }
}

inventory::submit! {
    RoleRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: PLAYER_CONTROLLER_ROLE,
        label: "Player Controller",
        trait_id: PLAYER_CONTROLLER_TRAIT_ID,
        setting_key: "player_controller_blueprint",
        order: 20,
        start: RoleStart::Now,
    }
}

inventory::submit! {
    RoleRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: PLAYER_CHARACTER_ROLE,
        label: "Player Character",
        trait_id: PLAYER_CHARACTER_TRAIT_ID,
        setting_key: "player_character_blueprint",
        order: 30,
        start: RoleStart::EndOfFrame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::parse_trait_record;

    #[test]
    fn every_engine_trait_file_is_a_versioned_declaration_in_the_engine_namespace() {
        let mut ids = std::collections::BTreeSet::new();
        for file in TRAIT_FILES {
            let value: serde_json::Value = serde_json::from_slice(file.bytes).unwrap();
            let id = value["id"].as_str().unwrap().to_owned();
            assert!(ids.insert(id.clone()), "duplicate id {id}");
            let name = id.strip_prefix("pulsar.").expect("the engine's namespace");
            assert!(!name.contains('.'), "{id}");
            let path = format!("pulsar/{}", file.path);
            let record = parse_trait_record(&value, &path, id.clone()).unwrap();
            assert!(record.contract_version >= 1, "{id} versions its contract");
            assert!(!record.methods.is_empty());
            let mut names = std::collections::BTreeSet::new();
            assert!(record.methods.iter().all(|m| names.insert(m.name.clone())));
        }
        for path in [
            GAME_MODE_TRAIT_PATH,
            PLAYER_CONTROLLER_TRAIT_PATH,
            PLAYER_CHARACTER_TRAIT_PATH,
            GAME_STATE_TRAIT_PATH,
            INTO_GRAPH_TRAIT_PATH,
        ] {
            assert!(
                TRAIT_FILES
                    .iter()
                    .any(|file| format!("pulsar/{}", file.path) == path),
                "{path}"
            );
        }
    }
}
