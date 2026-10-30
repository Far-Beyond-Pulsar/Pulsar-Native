//! The roles a level assigns to Blueprints.
//!
//! A level names, in its world settings, which Blueprint plays each engine
//! role: the game mode, the player controller, the player character and the
//! game state. Every role is a built-in trait (see
//! [`builtin_traits`](crate::builtin_traits)); a Blueprint can fill a role
//! only if it implements that trait, which is what the World Settings picker
//! lists.
//!
//! [`ROLES`] is the one table tying a role to its trait and to the setting
//! that stores the choice. The editor builds its pickers from it and the
//! runtime reads the same keys, so adding a role is one row here, a field in
//! the editor's `WorldSettingsData`, and a built-in trait.
//!
//! [`WorldRoles`] is what the runtime reads: the selections resolved against
//! the project's trait index and checked. Building it never spawns anything.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::blueprint_trait_index::BlueprintTraitIndex;
use crate::builtin_traits::{
    GAME_MODE_TRAIT_ID, GAME_MODE_TRAIT_PATH, GAME_STATE_TRAIT_ID, GAME_STATE_TRAIT_PATH,
    PLAYER_CHARACTER_TRAIT_ID, PLAYER_CHARACTER_TRAIT_PATH, PLAYER_CONTROLLER_TRAIT_ID,
    PLAYER_CONTROLLER_TRAIT_PATH,
};

/// A job a level hands to a Blueprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EngineRole {
    GameMode,
    PlayerController,
    PlayerCharacter,
    GameState,
}

/// Where a role's trait and setting are.
#[derive(Debug, Clone, Copy)]
pub struct RoleSpec {
    pub role: EngineRole,
    /// Shown in the editor.
    pub label: &'static str,
    /// Id of the built-in trait a Blueprint must implement.
    pub trait_id: &'static str,
    /// Canonical project path of that trait.
    pub trait_path: &'static str,
    /// Key in the level's `world_settings` holding the Blueprint path.
    pub setting_key: &'static str,
}

/// Every role, in the order the editor lists them.
pub const ROLES: [RoleSpec; 4] = [
    RoleSpec {
        role: EngineRole::GameMode,
        label: "Game Mode",
        trait_id: GAME_MODE_TRAIT_ID,
        trait_path: GAME_MODE_TRAIT_PATH,
        setting_key: "game_mode_blueprint",
    },
    RoleSpec {
        role: EngineRole::PlayerController,
        label: "Player Controller",
        trait_id: PLAYER_CONTROLLER_TRAIT_ID,
        trait_path: PLAYER_CONTROLLER_TRAIT_PATH,
        setting_key: "player_controller_blueprint",
    },
    RoleSpec {
        role: EngineRole::PlayerCharacter,
        label: "Player Character",
        trait_id: PLAYER_CHARACTER_TRAIT_ID,
        trait_path: PLAYER_CHARACTER_TRAIT_PATH,
        setting_key: "player_character_blueprint",
    },
    RoleSpec {
        role: EngineRole::GameState,
        label: "Game State",
        trait_id: GAME_STATE_TRAIT_ID,
        trait_path: GAME_STATE_TRAIT_PATH,
        setting_key: "game_state_blueprint",
    },
];

impl EngineRole {
    pub const ALL: [EngineRole; 4] = [
        EngineRole::GameMode,
        EngineRole::PlayerController,
        EngineRole::PlayerCharacter,
        EngineRole::GameState,
    ];

    pub fn spec(self) -> &'static RoleSpec {
        ROLES
            .iter()
            .find(|spec| spec.role == self)
            .expect("every role has a row in ROLES")
    }
}

/// Key in the level's `world_settings` holding the project-relative path of
/// the `.inputmap` the local player's controller brings to its pawn.
pub const PLAYER_INPUT_MAPPING_KEY: &str = "player_input_mapping";

/// The Blueprint paths a level selected, before any checking, and the input
/// mapping the player controller carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleSelection {
    blueprints: BTreeMap<EngineRole, String>,
    input_mapping: Option<String>,
}

impl RoleSelection {
    pub fn set(&mut self, role: EngineRole, blueprint: Option<String>) {
        match blueprint.filter(|path| !path.is_empty()) {
            Some(path) => self.blueprints.insert(role, path),
            None => self.blueprints.remove(&role),
        };
    }

    pub fn get(&self, role: EngineRole) -> Option<&str> {
        self.blueprints.get(&role).map(String::as_str)
    }

    /// Choose the `.inputmap` the player controller possesses its pawn with.
    pub fn set_input_mapping(&mut self, mapping: Option<String>) {
        self.input_mapping = mapping
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty());
    }

    pub fn input_mapping(&self) -> Option<&str> {
        self.input_mapping.as_deref()
    }

    /// Read the selections from a level's `world_settings` object. Levels
    /// from before a role existed have no key for it, which is no selection.
    pub fn from_world_settings(settings: &Value) -> Self {
        let mut selection = Self::default();
        for spec in &ROLES {
            if let Some(path) = settings.get(spec.setting_key).and_then(Value::as_str) {
                selection.set(spec.role, Some(path.to_owned()));
            }
        }
        selection.set_input_mapping(
            settings
                .get(PLAYER_INPUT_MAPPING_KEY)
                .and_then(Value::as_str)
                .map(str::to_owned),
        );
        selection
    }

    /// Read the selections from a level file's JSON text; unreadable text
    /// selects nothing.
    pub fn from_level_json(json: &str) -> Self {
        serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|level| level.get("world_settings").map(Self::from_world_settings))
            .unwrap_or_default()
    }
}

/// How far a selection could be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingStatus {
    /// The Blueprint exists in the index and implements the role's trait.
    Valid,
    /// No index was available (shipped content), so the selection is passed
    /// on as written.
    Unchecked,
    /// The Blueprint is not in the project's index: moved, deleted or never
    /// indexed.
    NotInIndex,
    /// The Blueprint exists but does not implement the role's trait.
    MissingTrait,
}

/// One role's Blueprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleBinding {
    pub role: EngineRole,
    /// Project-relative path of the Blueprint asset.
    pub blueprint: String,
    pub status: BindingStatus,
}

impl RoleBinding {
    /// The Blueprint class: its folder name for `<Class>/graph_save.json`,
    /// else the file stem.
    pub fn class_name(&self) -> Option<&str> {
        let path = Path::new(&self.blueprint);
        if path.file_name().and_then(|name| name.to_str()) == Some("graph_save.json") {
            return path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str());
        }
        self.blueprint
            .strip_suffix(".blueprint.json")
            .or_else(|| path.file_stem().and_then(|stem| stem.to_str()))
    }

    /// Safe to instantiate: checked and valid, or not checkable.
    pub fn is_usable(&self) -> bool {
        matches!(self.status, BindingStatus::Valid | BindingStatus::Unchecked)
    }
}

/// A level's roles, resolved. The runtime's single view of "which Blueprint
/// is the game mode / controller / character / game state".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldRoles {
    bindings: BTreeMap<EngineRole, RoleBinding>,
    input_mapping: Option<String>,
}

impl WorldRoles {
    /// Resolve `selection` against `index`, or pass it on unchecked when
    /// there is no index.
    pub fn resolve(selection: &RoleSelection, index: Option<&BlueprintTraitIndex>) -> Self {
        let mut bindings = BTreeMap::new();
        for spec in &ROLES {
            let Some(blueprint) = selection.get(spec.role) else {
                continue;
            };
            let status = match index {
                None => BindingStatus::Unchecked,
                Some(index) => {
                    if index.entry(blueprint).is_none() {
                        BindingStatus::NotInIndex
                    } else if index.db().implements(blueprint, spec.trait_id) {
                        BindingStatus::Valid
                    } else {
                        BindingStatus::MissingTrait
                    }
                }
            };
            bindings.insert(
                spec.role,
                RoleBinding {
                    role: spec.role,
                    blueprint: blueprint.replace('\\', "/"),
                    status,
                },
            );
        }
        Self {
            bindings,
            input_mapping: selection
                .input_mapping()
                .map(|path| path.replace('\\', "/")),
        }
    }

    /// Resolve the roles a level file selects against the project at
    /// `project_root`, using its persisted trait index. This only reads: a
    /// shipped game has no editor to have built the index, and then the
    /// roles come back [`Unchecked`](BindingStatus::Unchecked).
    pub fn for_level(level_json: &str, project_root: &Path) -> Self {
        let selection = RoleSelection::from_level_json(level_json);
        let index = BlueprintTraitIndex::load(project_root).ok();
        Self::resolve(&selection, index.as_ref())
    }

    /// The binding for `role`, valid or not.
    pub fn get(&self, role: EngineRole) -> Option<&RoleBinding> {
        self.bindings.get(&role)
    }

    /// The binding for `role` if it can be instantiated.
    pub fn usable(&self, role: EngineRole) -> Option<&RoleBinding> {
        self.bindings.get(&role).filter(|binding| binding.is_usable())
    }

    pub fn iter(&self) -> impl Iterator<Item = &RoleBinding> {
        self.bindings.values()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Project-relative path of the `.inputmap` the player controller
    /// possesses its pawn with, when the level names one. Passed on as
    /// written: the input runtime finds the asset (or reports it missing).
    pub fn input_mapping(&self) -> Option<&str> {
        self.input_mapping.as_deref()
    }

    /// One message per selection that cannot be used.
    pub fn diagnostics(&self) -> Vec<String> {
        self.iter()
            .filter(|binding| !binding.is_usable())
            .map(|binding| {
                let spec = binding.role.spec();
                match binding.status {
                    BindingStatus::NotInIndex => format!(
                        "{} Blueprint '{}' is not in the project's trait index",
                        spec.label, binding.blueprint
                    ),
                    _ => format!(
                        "{} Blueprint '{}' does not implement the {} trait",
                        spec.label, binding.blueprint, spec.trait_id
                    ),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, BlueprintTraitIndex) {
        let dir = tempfile::tempdir().unwrap();
        let bp = |traits: &[&str]| {
            serde_json::json!({
                "format_version": 1, "main_graph": {}, "variables": [],
                "blueprint_metadata": {"implemented_traits": traits}
            })
            .to_string()
        };
        for (class, traits) in [
            ("Mode", vec![GAME_MODE_TRAIT_PATH]),
            ("Hero", vec![PLAYER_CHARACTER_TRAIT_PATH]),
            ("Plain", vec![]),
        ] {
            let path = dir.path().join(format!("classes/{class}/graph_save.json"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bp(&traits)).unwrap();
        }
        let index = BlueprintTraitIndex::rebuild(dir.path()).unwrap();
        (dir, index)
    }

    fn selection(pairs: &[(EngineRole, &str)]) -> RoleSelection {
        let mut selection = RoleSelection::default();
        for (role, path) in pairs {
            selection.set(*role, Some((*path).to_owned()));
        }
        selection
    }

    #[test]
    fn the_table_has_one_distinct_row_per_role() {
        for role in EngineRole::ALL {
            assert_eq!(role.spec().role, role);
        }
        let keys: std::collections::BTreeSet<_> = ROLES.iter().map(|s| s.setting_key).collect();
        let traits: std::collections::BTreeSet<_> = ROLES.iter().map(|s| s.trait_id).collect();
        assert_eq!(keys.len(), ROLES.len());
        assert_eq!(traits.len(), ROLES.len());
        for spec in &ROLES {
            assert_eq!(
                crate::builtin_traits::by_id(spec.trait_id).unwrap().path,
                spec.trait_path
            );
        }
    }

    #[test]
    fn selections_are_checked_against_the_roles_trait() {
        let (_dir, index) = project();
        let roles = WorldRoles::resolve(
            &selection(&[
                (EngineRole::GameMode, "classes/Mode/graph_save.json"),
                (EngineRole::PlayerCharacter, "classes/Plain/graph_save.json"),
                (EngineRole::PlayerController, "classes/Gone/graph_save.json"),
            ]),
            Some(&index),
        );
        assert_eq!(roles.get(EngineRole::GameMode).unwrap().status, BindingStatus::Valid);
        assert_eq!(roles.get(EngineRole::GameMode).unwrap().class_name(), Some("Mode"));
        assert_eq!(roles.get(EngineRole::PlayerCharacter).unwrap().status, BindingStatus::MissingTrait);
        assert_eq!(roles.get(EngineRole::PlayerController).unwrap().status, BindingStatus::NotInIndex);
        assert!(roles.get(EngineRole::GameState).is_none());
        assert!(roles.usable(EngineRole::GameMode).is_some());
        assert!(roles.usable(EngineRole::PlayerCharacter).is_none());
        assert_eq!(roles.diagnostics().len(), 2);
    }

    #[test]
    fn without_an_index_selections_pass_through_unchecked() {
        let roles = WorldRoles::resolve(
            &selection(&[(EngineRole::GameState, "classes\\State\\graph_save.json")]),
            None,
        );
        let binding = roles.usable(EngineRole::GameState).unwrap();
        assert_eq!(binding.status, BindingStatus::Unchecked);
        assert_eq!(binding.blueprint, "classes/State/graph_save.json");
    }

    #[test]
    fn levels_from_before_a_role_existed_select_nothing_for_it() {
        let level = r#"{"world_settings":{"game_mode_blueprint":"classes/Mode/graph_save.json"}}"#;
        let selection = RoleSelection::from_level_json(level);
        assert_eq!(selection.get(EngineRole::GameMode), Some("classes/Mode/graph_save.json"));
        assert_eq!(selection.get(EngineRole::PlayerController), None);
        assert_eq!(RoleSelection::from_level_json("not json"), RoleSelection::default());
        assert_eq!(RoleSelection::from_level_json(r#"{"world_settings":{"game_mode_blueprint":""}}"#), RoleSelection::default());
    }

    #[test]
    fn the_input_mapping_rides_beside_the_roles_and_old_levels_have_none() {
        let level = r#"{"world_settings":{"player_input_mapping":"input\\Game.inputmap"}}"#;
        let roles = WorldRoles::for_level(level, tempfile::tempdir().unwrap().path());
        assert_eq!(roles.input_mapping(), Some("input/Game.inputmap"));
        assert!(roles.is_empty(), "a mapping alone is no role");
        for old in [
            r#"{"world_settings":{"game_mode_blueprint":"a"}}"#,
            r#"{"world_settings":{"player_input_mapping":"  "}}"#,
            r#"{"world_settings":{"player_input_mapping":3}}"#,
            r#"{}"#,
        ] {
            assert_eq!(RoleSelection::from_level_json(old).input_mapping(), None, "{old}");
        }
    }

    #[test]
    fn for_level_reads_the_persisted_index() {
        let (dir, _index) = project();
        let level = r#"{"world_settings":{"player_character_blueprint":"classes/Hero/graph_save.json"}}"#;
        let roles = WorldRoles::for_level(level, dir.path());
        assert_eq!(roles.get(EngineRole::PlayerCharacter).unwrap().status, BindingStatus::Valid);
        let empty = tempfile::tempdir().unwrap();
        let roles = WorldRoles::for_level(level, empty.path());
        assert_eq!(roles.get(EngineRole::PlayerCharacter).unwrap().status, BindingStatus::Unchecked);
    }
}
