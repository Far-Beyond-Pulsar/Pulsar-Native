//! Role resolution. The `*.impl.json` implementer kind is the index tests'
//! stand-in source (`index/tests.rs`).

use super::*;
use crate::builtin::{
    GAME_MODE_ROLE, GAME_MODE_TRAIT_PATH, GAME_STATE_ROLE, PLAYER_CHARACTER_ROLE,
    PLAYER_CHARACTER_TRAIT_PATH, PLAYER_CONTROLLER_ROLE,
};
use engine_fs::virtual_fs;

fn project() -> (tempfile::TempDir, TraitIndex) {
    let dir = tempfile::tempdir().unwrap();
    for (class, traits) in [
        ("Mode", vec![GAME_MODE_TRAIT_PATH]),
        ("Hero", vec![PLAYER_CHARACTER_TRAIT_PATH]),
        ("Plain", vec![]),
    ] {
        let path = dir.path().join(format!("classes/{class}/class.impl.json"));
        virtual_fs::create_dir_all(path.parent().unwrap()).unwrap();
        virtual_fs::write_file(
            &path,
            serde_json::json!({ "paths": traits }).to_string().as_bytes(),
        )
        .unwrap();
    }
    let index = TraitIndex::rebuild(dir.path()).unwrap();
    (dir, index)
}

fn selection(pairs: &[(&str, &str)]) -> RoleSelection {
    let mut selection = RoleSelection::default();
    for (role, path) in pairs {
        selection.set(role, Some((*path).to_owned()));
    }
    selection
}

#[test]
fn the_registry_has_one_distinct_key_and_trait_per_role_in_order() {
    let all = roles();
    let ids: Vec<_> = all.iter().map(|role| role.id).collect();
    assert_eq!(
        ids,
        [GAME_MODE_ROLE, GAME_STATE_ROLE, PLAYER_CONTROLLER_ROLE, PLAYER_CHARACTER_ROLE]
    );
    let keys: std::collections::BTreeSet<_> = all.iter().map(|r| r.setting_key).collect();
    let traits: std::collections::BTreeSet<_> = all.iter().map(|r| r.trait_id).collect();
    assert_eq!(keys.len(), all.len());
    assert_eq!(traits.len(), all.len());
    let index = TraitIndex::live(tempfile::tempdir().unwrap().path());
    for role in &all {
        assert!(index.resolve_trait(role.trait_id).is_some(), "{}", role.trait_id);
    }
}

#[test]
fn selections_are_checked_against_the_roles_trait() {
    let (_dir, index) = project();
    let roles = WorldRoles::resolve(
        &selection(&[
            (GAME_MODE_ROLE, "classes/Mode/class.impl.json"),
            (PLAYER_CHARACTER_ROLE, "classes/Plain/class.impl.json"),
            (PLAYER_CONTROLLER_ROLE, "classes/Gone/class.impl.json"),
        ]),
        Some(&index),
    );
    let mode = roles.get(GAME_MODE_ROLE).unwrap();
    assert_eq!(mode.status, BindingStatus::Valid);
    assert_eq!(mode.class_name(), Some("Mode"), "through the index's binding");
    assert_eq!(
        roles.get(PLAYER_CHARACTER_ROLE).unwrap().status,
        BindingStatus::MissingTrait
    );
    assert_eq!(
        roles.get(PLAYER_CONTROLLER_ROLE).unwrap().status,
        BindingStatus::NotInIndex
    );
    assert!(roles.get(GAME_STATE_ROLE).is_none());
    assert!(roles.usable(GAME_MODE_ROLE).is_some());
    assert!(roles.usable(PLAYER_CHARACTER_ROLE).is_none());
    let diagnostics = roles.diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics.iter().any(|d| d.starts_with("Player Character")));
}

#[test]
fn without_an_index_selections_pass_through_unchecked() {
    let roles = WorldRoles::resolve(
        &selection(&[(GAME_STATE_ROLE, "classes\\State\\graph_save.json")]),
        None,
    );
    let binding = roles.usable(GAME_STATE_ROLE).unwrap();
    assert_eq!(binding.status, BindingStatus::Unchecked);
    assert_eq!(binding.asset, "classes/State/graph_save.json");
    assert_eq!(binding.class_name(), Some("State"));
}

#[test]
fn levels_from_before_a_role_existed_select_nothing_for_it() {
    let level = r#"{"world_settings":{"game_mode_blueprint":"classes/Mode/graph_save.json"}}"#;
    let selection = RoleSelection::from_world_settings(&world_settings_of(level));
    assert_eq!(selection.get(GAME_MODE_ROLE), Some("classes/Mode/graph_save.json"));
    assert_eq!(selection.get(PLAYER_CONTROLLER_ROLE), None);
    assert_eq!(
        RoleSelection::from_world_settings(&world_settings_of("not json")),
        RoleSelection::default()
    );
    assert_eq!(
        RoleSelection::from_world_settings(&world_settings_of(
            r#"{"world_settings":{"game_mode_blueprint":""}}"#
        )),
        RoleSelection::default()
    );
}

#[test]
fn a_plugins_settings_ride_beside_the_roles() {
    let level = r#"{"world_settings":{"player_input_mapping":"input/Game.inputmap"}}"#;
    let roles = WorldRoles::for_level(level, tempfile::tempdir().unwrap().path());
    assert_eq!(
        roles.setting("player_input_mapping").and_then(Value::as_str),
        Some("input/Game.inputmap")
    );
    assert!(roles.is_empty(), "a setting alone is no role");
}

#[test]
fn for_level_reads_the_stored_index() {
    let (dir, _index) = project();
    let level =
        r#"{"world_settings":{"player_character_blueprint":"classes/Hero/class.impl.json"}}"#;
    let roles = WorldRoles::for_level(level, dir.path());
    assert_eq!(
        roles.get(PLAYER_CHARACTER_ROLE).unwrap().status,
        BindingStatus::Valid
    );
    let empty = tempfile::tempdir().unwrap();
    let roles = WorldRoles::for_level(level, empty.path());
    assert_eq!(
        roles.get(PLAYER_CHARACTER_ROLE).unwrap().status,
        BindingStatus::Unchecked
    );
}
