//! Trait assets every project has without authoring them.
//!
//! The engine owns these contracts. They are defined once, here, and
//! materialized into each project as ordinary `*.trait.json` assets so
//! editors and Blueprints see them like any other trait. [`ensure`] runs at
//! the start of every [`BlueprintTraitIndex`](crate::BlueprintTraitIndex)
//! rebuild and keeps the project copy in step with this definition:
//!
//! - a missing file is written;
//! - an existing copy keeps its `description` (a project may document a
//!   built-in) but its `id`, `meta.builtin`, `meta.contractVersion` and its
//!   methods are always the engine's. A copy whose methods drifted (edited by
//!   hand, or left over from an older engine) is rewritten and a warning is
//!   logged, so a project can document a role but not redefine it;
//! - a file already at a built-in's path that was authored by the project
//!   (no `meta.builtin`) is adopted the same way. The role traits were
//!   expected at these paths before the engine shipped them.
//!
//! Built-in ids are fixed, readable strings (`pulsar.GameMode`), not UUIDs:
//! they must be equal in every project. When a project cannot be written
//! (read-only, remote without write access) [`TraitDb`](crate::TraitDb) still
//! serves the engine's definition under the canonical path.
//!
//! To change a contract, edit the definition and bump its `contract_version`.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::trait_records::{parse_trait_record, TraitRecord};
use crate::virtual_fs;

/// Primitive names the trait type system spells as `{"kind":"Primitive"}`.
const PRIMITIVES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
    "f64", "bool", "char", "str", "String", "()",
];

/// One method of a built-in contract: `(name, [(param, type)], return, doc)`.
pub struct BuiltinMethod {
    pub name: &'static str,
    pub params: &'static [(&'static str, &'static str)],
    pub returns: &'static str,
    pub doc: &'static str,
}

/// An engine-owned trait.
pub struct BuiltinTrait {
    /// Fixed id, equal in every project.
    pub id: &'static str,
    pub name: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Where the asset lives in a project.
    pub path: &'static str,
    /// Bump when `methods` change.
    pub contract_version: u32,
    pub methods: &'static [BuiltinMethod],
}

/// Project-relative path of the `IntoGraph` trait.
///
/// Helio builds a renderer from anything that implements `IntoGraph` (a
/// built-in pipeline, a render graph asset). A Blueprint declaring this trait
/// states that it describes a render pipeline.
pub const INTO_GRAPH_TRAIT_PATH: &str = "types/traits/into_graph.trait.json";
/// Project-relative path of the `GameMode` role trait.
pub const GAME_MODE_TRAIT_PATH: &str = "types/traits/game_mode.trait.json";
/// Project-relative path of the `PlayerController` role trait.
pub const PLAYER_CONTROLLER_TRAIT_PATH: &str = "types/traits/player_controller.trait.json";
/// Project-relative path of the `PlayerCharacter` role trait.
pub const PLAYER_CHARACTER_TRAIT_PATH: &str = "types/traits/player_character.trait.json";
/// Project-relative path of the `GameState` role trait.
pub const GAME_STATE_TRAIT_PATH: &str = "types/traits/game_state.trait.json";

pub const INTO_GRAPH_TRAIT_ID: &str = "pulsar.IntoGraph";
pub const GAME_MODE_TRAIT_ID: &str = "pulsar.GameMode";
pub const PLAYER_CONTROLLER_TRAIT_ID: &str = "pulsar.PlayerController";
pub const PLAYER_CHARACTER_TRAIT_ID: &str = "pulsar.PlayerCharacter";
pub const GAME_STATE_TRAIT_ID: &str = "pulsar.GameState";

// Role traits take plain ids (`u32` player, `u64` actor handle) until the
// spawn path hands Blueprints typed actor references; changing a parameter
// type is a contract version bump.
static BUILTINS: &[BuiltinTrait] = &[
    BuiltinTrait {
        id: INTO_GRAPH_TRAIT_ID,
        name: "IntoGraph",
        display_name: "Into Graph",
        description: "Describes a render pipeline: builds a Helio render graph from the renderer's construction context. Render graph assets (.rendergraph) implement it implicitly.",
        path: INTO_GRAPH_TRAIT_PATH,
        contract_version: 1,
        methods: &[BuiltinMethod {
            name: "into_graph",
            params: &[],
            returns: "helio::RenderGraph",
            doc: "Builds the render graph. Called when the renderer is created and again whenever it is rebuilt (resize, shader reload).",
        }],
    },
    BuiltinTrait {
        id: GAME_MODE_TRAIT_ID,
        name: "GameMode",
        display_name: "Game Mode",
        description: "The rules of a level: the one object that decides how play starts, who joins and when it ends. A level selects its game mode in World Settings.",
        path: GAME_MODE_TRAIT_PATH,
        contract_version: 1,
        methods: &[
            BuiltinMethod {
                name: "on_game_start",
                params: &[],
                returns: "()",
                doc: "Called once when play begins, before any actor's begin_play.",
            },
            BuiltinMethod {
                name: "on_player_joined",
                params: &[("player_id", "u32")],
                returns: "()",
                doc: "Called when a player enters the game, before their controller and character are spawned.",
            },
            BuiltinMethod {
                name: "on_game_end",
                params: &[],
                returns: "()",
                doc: "Called once when play ends.",
            },
        ],
    },
    BuiltinTrait {
        id: PLAYER_CONTROLLER_TRAIT_ID,
        name: "PlayerController",
        display_name: "Player Controller",
        description: "Turns a player's input into intent. One exists per player and outlives the character it drives. A level selects its player controller in World Settings.",
        path: PLAYER_CONTROLLER_TRAIT_PATH,
        contract_version: 1,
        methods: &[
            BuiltinMethod {
                name: "on_possess",
                params: &[("pawn_id", "u64")],
                returns: "()",
                doc: "Called when the controller takes control of a character.",
            },
            BuiltinMethod {
                name: "on_unpossess",
                params: &[],
                returns: "()",
                doc: "Called when the controller releases its character.",
            },
            BuiltinMethod {
                name: "on_input",
                params: &[("delta_seconds", "f32")],
                returns: "()",
                doc: "Called every frame so the controller can read input and steer its character.",
            },
        ],
    },
    BuiltinTrait {
        id: PLAYER_CHARACTER_TRAIT_ID,
        name: "PlayerCharacter",
        display_name: "Player Character",
        description: "The body a player controls in the world. A level selects the character spawned for each player in World Settings.",
        path: PLAYER_CHARACTER_TRAIT_PATH,
        contract_version: 1,
        methods: &[
            BuiltinMethod {
                name: "on_spawned",
                params: &[],
                returns: "()",
                doc: "Called when the character is placed in the world.",
            },
            BuiltinMethod {
                name: "on_possessed",
                params: &[("controller_id", "u64")],
                returns: "()",
                doc: "Called when a controller takes control of this character.",
            },
            BuiltinMethod {
                name: "on_unpossessed",
                params: &[],
                returns: "()",
                doc: "Called when its controller lets go.",
            },
        ],
    },
    BuiltinTrait {
        id: GAME_STATE_TRAIT_ID,
        name: "GameState",
        display_name: "Game State",
        description: "The shared state of a running game that every player can observe, such as scores and the roster. A level selects its game state in World Settings.",
        path: GAME_STATE_TRAIT_PATH,
        contract_version: 1,
        methods: &[
            BuiltinMethod {
                name: "on_init",
                params: &[],
                returns: "()",
                doc: "Called once when the game state is created, before play begins.",
            },
            BuiltinMethod {
                name: "on_player_added",
                params: &[("player_id", "u32")],
                returns: "()",
                doc: "Called when a player joins the roster.",
            },
            BuiltinMethod {
                name: "on_player_removed",
                params: &[("player_id", "u32")],
                returns: "()",
                doc: "Called when a player leaves the roster.",
            },
        ],
    },
];

/// Every engine-owned trait.
pub fn all() -> &'static [BuiltinTrait] {
    BUILTINS
}

/// The built-in with this id.
pub fn by_id(id: &str) -> Option<&'static BuiltinTrait> {
    BUILTINS.iter().find(|builtin| builtin.id == id)
}

/// The built-in whose canonical path is `path`.
pub fn by_path(path: &str) -> Option<&'static BuiltinTrait> {
    BUILTINS.iter().find(|builtin| builtin.path == path)
}

fn type_ref(text: &str) -> Value {
    if PRIMITIVES.contains(&text) {
        json!({ "kind": "Primitive", "name": text })
    } else {
        json!({ "kind": "Path", "path": text })
    }
}

impl BuiltinTrait {
    /// The asset as written to a project, with the project's own
    /// `description` if it has one.
    pub fn to_json(&self, description: Option<&str>) -> Value {
        let methods: Vec<Value> = self
            .methods
            .iter()
            .map(|method| {
                json!({
                    "name": method.name,
                    "signature": {
                        "params": method.params.iter().map(|(name, ty)| json!({
                            "name": name,
                            "type": type_ref(ty),
                        })).collect::<Vec<_>>(),
                        "return": type_ref(method.returns),
                    },
                    "doc": method.doc,
                })
            })
            .collect();
        json!({
            "schemaVersion": 1,
            "typeKind": "trait",
            "id": self.id,
            "name": self.name,
            "displayName": self.display_name,
            "description": description.unwrap_or(self.description),
            "methods": methods,
            "meta": { "builtin": true, "contractVersion": self.contract_version },
        })
    }

    /// The engine's definition as an index record under the canonical path.
    pub fn to_record(&self) -> TraitRecord {
        parse_trait_record(&self.to_json(None), self.path, self.id.to_owned())
            .expect("built-in traits are valid declarations")
    }
}

/// Whether the project's copy matches the engine's definition: same id,
/// stamped builtin, current contract version and identical methods.
fn in_sync(builtin: &BuiltinTrait, existing: &Value, path: &str) -> bool {
    let Some(record) = parse_trait_record(existing, path, builtin.id.to_owned()) else {
        return false;
    };
    let expected = builtin.to_record();
    crate::trait_records::embedded_id(existing).as_deref() == Some(builtin.id)
        && record.builtin
        && record.contract_version == builtin.contract_version
        && record.name == builtin.name
        && record.same_contract(&expected)
}

/// What [`sync_one`] did to a project's copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSync {
    InSync,
    Created,
    /// Rewritten; `contract_drift` is set when the methods differed while
    /// the version stamp claimed they were current.
    Restamped { contract_drift: bool },
}

/// Bring one built-in's file in `project_root` in step with the engine.
pub fn sync_one(project_root: &Path, builtin: &BuiltinTrait) -> Result<BuiltinSync> {
    let path = project_root.join(builtin.path);
    let existing = if virtual_fs::exists(&path).unwrap_or(false) {
        virtual_fs::read_file(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    } else {
        None
    };
    let (outcome, description) = match &existing {
        None => (BuiltinSync::Created, None),
        Some(value) if in_sync(builtin, value, builtin.path) => return Ok(BuiltinSync::InSync),
        Some(value) => {
            let current_stamp = value
                .pointer("/meta/contractVersion")
                .and_then(Value::as_u64)
                == Some(u64::from(builtin.contract_version));
            let drift = current_stamp
                && parse_trait_record(value, builtin.path, builtin.id.to_owned())
                    .is_some_and(|record| !record.same_contract(&builtin.to_record()));
            let description = value
                .get("description")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned);
            (BuiltinSync::Restamped { contract_drift: drift }, description)
        }
    };
    if let BuiltinSync::Restamped { contract_drift } = outcome {
        if contract_drift {
            tracing::warn!(
                path = builtin.path,
                "Built-in trait '{}' was edited; its methods are engine-owned and were restored",
                builtin.name
            );
        } else {
            tracing::info!(path = builtin.path, "Updated built-in trait '{}'", builtin.name);
        }
    }
    if let Some(parent) = path.parent() {
        virtual_fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(&builtin.to_json(description.as_deref()))
        .context("Failed to serialize built-in trait")?;
    bytes.push(b'\n');
    virtual_fs::write_file(&path, &bytes)
        .with_context(|| format!("Failed to write built-in trait '{}'", path.display()))?;
    Ok(outcome)
}

/// Keep every built-in in step with the engine (see the module docs). The
/// first failure is returned after every built-in has been tried.
pub fn ensure(project_root: &Path) -> Result<()> {
    let mut first_error = None;
    for builtin in BUILTINS {
        if let Err(error) = sync_one(project_root, builtin) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_definition_is_a_valid_canonical_declaration() {
        let mut ids = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        for builtin in all() {
            assert!(ids.insert(builtin.id), "duplicate id {}", builtin.id);
            assert!(paths.insert(builtin.path), "duplicate path {}", builtin.path);
            let value = builtin.to_json(None);
            assert_eq!(value["typeKind"], "trait");
            let record = parse_trait_record(&value, builtin.path, builtin.id.into()).unwrap();
            assert!(record.builtin);
            assert!(!record.methods.is_empty());
            let mut names = std::collections::BTreeSet::new();
            assert!(record.methods.iter().all(|m| names.insert(m.name.clone())));
        }
    }

    #[test]
    fn a_missing_copy_is_created_and_then_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = by_id(GAME_MODE_TRAIT_ID).unwrap();
        assert_eq!(sync_one(dir.path(), builtin).unwrap(), BuiltinSync::Created);
        assert_eq!(sync_one(dir.path(), builtin).unwrap(), BuiltinSync::InSync);
    }

    #[test]
    fn a_project_may_document_but_not_redefine_a_role() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = by_id(GAME_MODE_TRAIT_ID).unwrap();
        sync_one(dir.path(), builtin).unwrap();
        let path = dir.path().join(builtin.path);

        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["description"] = json!("Our rules.");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(sync_one(dir.path(), builtin).unwrap(), BuiltinSync::InSync);

        value["methods"] = json!([]);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            sync_one(dir.path(), builtin).unwrap(),
            BuiltinSync::Restamped { contract_drift: true }
        );
        let healed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(healed["description"], "Our rules.");
        assert_eq!(healed["methods"].as_array().unwrap().len(), builtin.methods.len());
    }

    #[test]
    fn a_hand_authored_file_at_a_built_in_path_is_adopted() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = by_id(GAME_STATE_TRAIT_ID).unwrap();
        let path = dir.path().join(builtin.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"name":"GameState","display_name":"Mine","visibility":"Public","methods":[]}"#,
        )
        .unwrap();
        assert_eq!(
            sync_one(dir.path(), builtin).unwrap(),
            BuiltinSync::Restamped { contract_drift: false }
        );
        let adopted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(adopted["id"], GAME_STATE_TRAIT_ID);
        assert_eq!(adopted["meta"]["builtin"], true);
    }
}
