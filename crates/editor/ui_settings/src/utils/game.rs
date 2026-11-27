//! The Project Settings "Game" page: the default level and the game instance
//! class.
//!
//! These two live in the project's `Pulsar/project.json`, the file a
//! packaged game reads, not in the settings database: the window edits the
//! two keys there and keeps every other key as written
//! (`pulsar_content::edit_project_file`). Edits are staged until Save, as
//! every other setting's are.
//!
//! `startup_level` has three states: absent (the default: the level the game
//! would pick on its own), the empty string (explicitly none: no level until
//! a script loads one) and a level path. `game_instance` is absent (none) or
//! a class folder.
//!
//! Nothing here touches GPUI.

use std::path::{Path, PathBuf};

use engine_state::{ConfigValue, FieldType, SettingInfo};
use serde_json::{Map, Value};

/// The namespace the page's two rows use: not the settings database, so the
/// window reads, writes, resets and saves them through [`GameSettings`].
pub const NAMESPACE: &str = "project_file";
/// The owner of the page's rows.
pub const OWNER: &str = "game";
pub const STARTUP_LEVEL: &str = "startup_level";
pub const GAME_INSTANCE: &str = "game_instance";

/// What the default level row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultLevel {
    /// The project names none: the level the game would load on its own, if
    /// the project has one.
    Automatic(Option<String>),
    /// No level: nothing loads until a script asks.
    None,
    Level(String),
}

/// The two keys, as saved and as edited.
#[derive(Debug)]
pub struct GameSettings {
    root: PathBuf,
    saved: Map<String, Value>,
    edited: Map<String, Value>,
}

impl GameSettings {
    /// Read the project at `root`. A file that cannot be read shows no keys
    /// (saving it then fails and leaves it as it is).
    pub fn load(root: &Path) -> Self {
        let keys = |map: Map<String, Value>| {
            map.into_iter()
                .filter(|(key, _)| key == STARTUP_LEVEL || key == GAME_INSTANCE)
                .collect::<Map<_, _>>()
        };
        let saved = pulsar_content::read_project_file(root)
            .map(keys)
            .unwrap_or_else(|error| {
                tracing::warn!("{error}");
                Map::new()
            });
        Self {
            root: root.to_path_buf(),
            edited: saved.clone(),
            saved,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.edited.get(key).and_then(Value::as_str)
    }

    /// The default level, as edited. `default_map` is the editor's
    /// `project.default_map` setting, which the game also consults when the
    /// project names no level.
    pub fn default_level(&self, default_map: Option<&str>) -> DefaultLevel {
        match self.text(STARTUP_LEVEL).map(str::trim) {
            None => DefaultLevel::Automatic(pulsar_content::default_level(default_map, |rel| {
                engine_fs::virtual_fs::is_file(&self.root.join(rel))
            })),
            Some("") => DefaultLevel::None,
            Some(level) => DefaultLevel::Level(level.to_owned()),
        }
    }

    /// The game instance class folder, as edited.
    pub fn game_instance(&self) -> Option<&str> {
        self.text(GAME_INSTANCE).filter(|class| !class.is_empty())
    }

    /// Pick `level` as the default level.
    pub fn set_level(&mut self, level: &str) {
        self.edited.insert(
            STARTUP_LEVEL.into(),
            Value::String(level.replace('\\', "/")),
        );
    }

    /// No default level.
    pub fn set_no_level(&mut self) {
        self.edited
            .insert(STARTUP_LEVEL.into(), Value::String(String::new()));
    }

    /// Pick `class` (a class folder) as the game instance.
    pub fn set_game_instance(&mut self, class: &str) {
        self.edited.insert(
            GAME_INSTANCE.into(),
            Value::String(class.replace('\\', "/")),
        );
    }

    /// Put the key back to its default: absent.
    pub fn reset(&mut self, key: &str) {
        self.edited.shift_remove(key);
    }

    /// Whether the key differs from its default (is set at all).
    pub fn is_changed(&self, key: &str) -> bool {
        self.edited.contains_key(key)
    }

    /// Whether there is anything to save.
    pub fn is_dirty(&self) -> bool {
        self.edited != self.saved
    }

    /// Write the keys edited since the last save into the project file,
    /// leaving every other key (and a key not edited here) as the file has it.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.is_dirty() {
            return Ok(());
        }
        let (edited, saved) = (&self.edited, &self.saved);
        pulsar_content::edit_project_file(&self.root, |file| {
            for key in [STARTUP_LEVEL, GAME_INSTANCE] {
                if edited.get(key) == saved.get(key) {
                    continue;
                }
                match edited.get(key) {
                    Some(value) => {
                        file.insert(key.into(), value.clone());
                    }
                    None => {
                        file.shift_remove(key);
                    }
                }
            }
        })?;
        self.saved = self.edited.clone();
        Ok(())
    }
}

/// The class folders of the project that implement the Game Instance trait
/// (what the game instance picker offers).
pub fn game_instance_classes(root: &Path) -> Vec<String> {
    let index = pulsar_traits::TraitIndex::load(root).or_else(|_| {
        // A project never indexed yet.
        pulsar_traits::TraitIndex::rebuild(root)
    });
    let index = match index {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!("The project's traits could not be read: {error:#}");
            return Vec::new();
        }
    };
    let mut classes: Vec<String> = index
        .implementers_of(pulsar_traits::builtin::GAME_INSTANCE_TRAIT_ID)
        .iter()
        .filter_map(|implementer| match &implementer.binding {
            Some(pulsar_traits::Binding::ScriptClass { dir }) => {
                Some(dir.trim_matches('/').to_owned())
            }
            _ => None,
        })
        .collect();
    classes.sort();
    classes.dedup();
    classes
}

/// The two rows, as the window's model lists settings.
pub fn rows() -> Vec<SettingInfo> {
    let row = |key: &str, label: &str, description: &str, order| SettingInfo {
        key: key.into(),
        namespace: NAMESPACE.into(),
        owner: OWNER.into(),
        label: Some(label.into()),
        page: Some("Startup".into()),
        description: description.into(),
        current_value: ConfigValue::String(String::new()),
        default_value: ConfigValue::String(String::new()),
        tags: Vec::new(),
        read_only: false,
        field_type: Some(FieldType::TextInput {
            placeholder: None,
            multiline: false,
        }),
        order,
    };
    vec![
        row(
            STARTUP_LEVEL,
            "Default Level",
            "The level the game starts in, after the game instance. None: no level loads until \
             a script's Load Level; widgets the game instance shows are drawn meanwhile. \
             Play-in-Editor plays the level open in the editor instead.",
            0,
        ),
        row(
            GAME_INSTANCE,
            "Game Instance",
            "A Blueprint class implementing the Game Instance trait. It is spawned when the \
             game starts, before any level, and stays across level loads: main menus and \
             state that outlives a level belong to it.",
            1,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(json: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Pulsar")).unwrap();
        std::fs::write(dir.path().join("Pulsar/project.json"), json).unwrap();
        std::fs::create_dir_all(dir.path().join("scene")).unwrap();
        std::fs::write(dir.path().join("scene/default.level"), "{}").unwrap();
        dir
    }

    fn file(dir: &tempfile::TempDir) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("Pulsar/project.json")).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn the_default_level_is_automatic_none_or_a_level() {
        let dir = project(r#"{ "name": "G" }"#);
        let mut game = GameSettings::load(dir.path());
        assert_eq!(
            game.default_level(None),
            DefaultLevel::Automatic(Some("scene/default.level".into()))
        );
        assert!(!game.is_changed(STARTUP_LEVEL) && !game.is_dirty());
        game.set_no_level();
        assert_eq!(game.default_level(None), DefaultLevel::None);
        assert!(game.is_changed(STARTUP_LEVEL) && game.is_dirty());
        game.save().unwrap();
        assert_eq!(file(&dir)["startup_level"], serde_json::json!(""));
        game.set_level("scene\\a.level");
        game.save().unwrap();
        assert_eq!(
            file(&dir)["startup_level"],
            serde_json::json!("scene/a.level")
        );
        game.reset(STARTUP_LEVEL);
        game.save().unwrap();
        assert!(file(&dir).get("startup_level").is_none());
        assert_eq!(file(&dir)["name"], serde_json::json!("G"));
    }

    #[test]
    fn saving_keeps_other_keys_and_edits_made_elsewhere() {
        let dir = project(r#"{ "window": { "width": 800 }, "startup_level": "a.level" }"#);
        let mut game = GameSettings::load(dir.path());
        // Another tool changes the file after the window opened.
        std::fs::write(
            dir.path().join("Pulsar/project.json"),
            r#"{ "window": { "width": 900 }, "startup_level": "b.level", "extra": 1 }"#,
        )
        .unwrap();
        game.set_game_instance("classes/Main.class");
        game.save().unwrap();
        let saved = file(&dir);
        assert_eq!(saved["window"]["width"], serde_json::json!(900));
        assert_eq!(saved["extra"], serde_json::json!(1));
        assert_eq!(
            saved["startup_level"],
            serde_json::json!("b.level"),
            "not edited here"
        );
        assert_eq!(
            saved["game_instance"],
            serde_json::json!("classes/Main.class")
        );
        assert_eq!(game.game_instance(), Some("classes/Main.class"));
    }

    #[test]
    fn a_file_that_is_not_an_object_is_left_alone() {
        let dir = project("[1]");
        let mut game = GameSettings::load(dir.path());
        game.set_no_level();
        assert!(game.save().is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Pulsar/project.json")).unwrap(),
            "[1]"
        );
    }
}
