//! The roles a level assigns to implementers.
//!
//! A role is a job a level hands to one of its classes: the game mode, the
//! player controller, ... Each is a [`RoleRegistration`] (an origin-gated
//! contribution): its id, the trait an implementer must have, and the key in
//! the level's `world_settings` that stores the choice. The engine registers
//! its four ([`crate::builtin`]); a plugin registers its own from its root
//! crate, and a disabled plugin's roles are absent. The editor's World
//! Settings builds one picker per registered role, listing the implementers
//! of its trait; the runtime spawns each selected one when play starts, in
//! `order`, and calls its trait methods through the trait.
//!
//! [`WorldRoles`] is what the runtime reads: the selections resolved against
//! the project's trait index and checked, and the level's `world_settings`
//! for the plugins that keep their own settings there. Building it never
//! spawns anything.
//!
//! How others do it: Unreal's `AGameModeBase` names its classes
//! (`PlayerControllerClass`, `DefaultPawnClass`, `GameStateClass`), each a
//! `TSubclassOf` the World Settings override per level, and a plugin adds a
//! class slot by subclassing the game mode. Ours makes the slots data: a
//! plugin adds a role by registering it, not by replacing the game mode, and
//! the slot names a trait rather than a base class, so any implementer kind
//! fills it.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::index::TraitIndex;
use crate::sources::Binding;

/// When a role's entity is spawned at the start of play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleStart {
    /// In the session's first script phase, its script started there, before
    /// the level's own objects begin play.
    Now,
    /// Queued like a script's `world::spawn`: it runs from the next frame.
    EndOfFrame,
}

/// One role (see the module docs).
pub struct RoleRegistration {
    pub origin: &'static str,
    /// Stable, prefixed by its owner (`pulsar.game_mode`).
    pub id: &'static str,
    /// Shown in the editor.
    pub label: &'static str,
    /// The trait an implementer must have.
    pub trait_id: &'static str,
    /// Key in the level's `world_settings` holding the implementer's
    /// project-relative path.
    pub setting_key: &'static str,
    /// Where it is listed and when it spawns, lowest first.
    pub order: i32,
    pub start: RoleStart,
}

impl pulsar_plugins::Contribution for RoleRegistration {
    const CATEGORY: &'static str = "gameplay.roles";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_owned()
    }

    fn label(&self) -> String {
        self.label.to_owned()
    }
}

pulsar_plugins::registration!(RoleRegistration);

/// The enabled roles, in `order`.
pub fn roles() -> Vec<&'static RoleRegistration> {
    let mut all: Vec<_> = pulsar_plugins::registered::<RoleRegistration>().collect();
    all.sort_by_key(|role| (role.order, role.id));
    all
}

/// The enabled role `id`.
pub fn role(id: &str) -> Option<&'static RoleRegistration> {
    pulsar_plugins::registered::<RoleRegistration>().find(|role| role.id == id)
}

/// The implementers a level selected, by role id, before any checking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleSelection {
    selected: BTreeMap<String, String>,
}

impl RoleSelection {
    pub fn set(&mut self, role: &str, implementer: Option<String>) {
        match implementer.filter(|path| !path.is_empty()) {
            Some(path) => self.selected.insert(role.to_owned(), path),
            None => self.selected.remove(role),
        };
    }

    pub fn get(&self, role: &str) -> Option<&str> {
        self.selected.get(role).map(String::as_str)
    }

    /// Read the selections of every enabled role from a level's
    /// `world_settings` object. Levels from before a role existed have no key
    /// for it, which is no selection.
    pub fn from_world_settings(settings: &Value) -> Self {
        let mut selection = Self::default();
        for role in roles() {
            if let Some(path) = settings.get(role.setting_key).and_then(Value::as_str) {
                selection.set(role.id, Some(path.to_owned()));
            }
        }
        selection
    }
}

/// The level's `world_settings` object in a level file's JSON text; empty for
/// unreadable text.
pub fn world_settings_of(level_json: &str) -> Value {
    serde_json::from_str::<Value>(level_json)
        .ok()
        .and_then(|mut level| level.get_mut("world_settings").map(Value::take))
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// How far a selection could be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingStatus {
    /// The implementer exists in the index and implements the role's trait.
    Valid,
    /// No index was available (shipped content), so the selection is passed
    /// on as written.
    Unchecked,
    /// The implementer is not in the project's index: moved, deleted or
    /// never indexed.
    NotInIndex,
    /// The implementer exists but does not implement the role's trait.
    MissingTrait,
}

/// One role's implementer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleBinding {
    pub role: String,
    /// Project-relative path of the implementing asset.
    pub asset: String,
    /// How the runtime reaches it, when the index says.
    pub binding: Option<Binding>,
    pub status: BindingStatus,
}

impl RoleBinding {
    /// The script class to spawn: the index's binding, else (no index) the
    /// asset's folder name for `<Class>/graph_save.json`, else its file stem.
    pub fn class_name(&self) -> Option<&str> {
        if let Some(Binding::ScriptClass { dir }) = &self.binding {
            return Path::new(dir).file_name().and_then(|name| name.to_str());
        }
        let path = Path::new(&self.asset);
        if path.file_name().and_then(|name| name.to_str()) == Some("graph_save.json") {
            return path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str());
        }
        self.asset
            .strip_suffix(".blueprint.json")
            .or_else(|| path.file_stem().and_then(|stem| stem.to_str()))
    }

    /// Safe to instantiate: checked and valid, or not checkable.
    pub fn is_usable(&self) -> bool {
        matches!(self.status, BindingStatus::Valid | BindingStatus::Unchecked)
    }

    /// The role's label (its id when the role is not enabled now).
    pub fn label(&self) -> &str {
        role(&self.role).map_or(self.role.as_str(), |role| role.label)
    }
}

/// A level's roles, resolved: the runtime's single view of "which class is
/// the game mode / controller / ...", plus the level's settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldRoles {
    bindings: BTreeMap<String, RoleBinding>,
    settings: Value,
}

impl WorldRoles {
    /// Resolve `selection` against `index`, or pass it on unchecked when
    /// there is no index.
    pub fn resolve(selection: &RoleSelection, index: Option<&TraitIndex>) -> Self {
        let mut bindings = BTreeMap::new();
        for role in roles() {
            let Some(asset) = selection.get(role.id) else {
                continue;
            };
            let entry = index.and_then(|index| index.entry(asset));
            let status = match index {
                None => BindingStatus::Unchecked,
                Some(_) if entry.is_none() => BindingStatus::NotInIndex,
                Some(index) if index.db().implements(asset, role.trait_id) => BindingStatus::Valid,
                Some(_) => BindingStatus::MissingTrait,
            };
            bindings.insert(
                role.id.to_owned(),
                RoleBinding {
                    role: role.id.to_owned(),
                    asset: asset.replace('\\', "/"),
                    binding: entry.and_then(|entry| entry.binding.clone()),
                    status,
                },
            );
        }
        Self {
            bindings,
            settings: Value::Object(Default::default()),
        }
    }

    /// Resolve the roles a level file selects against the project at
    /// `project_root`, using its stored trait index. This only reads: a
    /// shipped game has no editor to have built the index, and then the roles
    /// come back [`Unchecked`](BindingStatus::Unchecked).
    pub fn for_level(level_json: &str, project_root: &Path) -> Self {
        let settings = world_settings_of(level_json);
        let selection = RoleSelection::from_world_settings(&settings);
        let index = TraitIndex::load(project_root).ok();
        let mut roles = Self::resolve(&selection, index.as_ref());
        roles.settings = settings;
        roles
    }

    /// The binding for `role`, valid or not.
    pub fn get(&self, role: &str) -> Option<&RoleBinding> {
        self.bindings.get(role)
    }

    /// The binding for `role` if it can be instantiated.
    pub fn usable(&self, role: &str) -> Option<&RoleBinding> {
        self.bindings.get(role).filter(|binding| binding.is_usable())
    }

    pub fn iter(&self) -> impl Iterator<Item = &RoleBinding> {
        self.bindings.values()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// One of the level's `world_settings` values, by key: what a plugin
    /// keeps there (the input plugin's player input mapping).
    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key)
    }

    /// Replace the level's settings (tests, or a host that read them).
    pub fn with_settings(mut self, settings: Value) -> Self {
        self.settings = settings;
        self
    }

    /// One message per selection that cannot be used.
    pub fn diagnostics(&self) -> Vec<String> {
        self.iter()
            .filter(|binding| !binding.is_usable())
            .map(|binding| match binding.status {
                BindingStatus::NotInIndex => format!(
                    "{} '{}' is not in the project's trait index",
                    binding.label(),
                    binding.asset
                ),
                _ => format!(
                    "{} '{}' does not implement the {} trait",
                    binding.label(),
                    binding.asset,
                    role(&binding.role).map_or("role's", |role| role.trait_id)
                ),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
