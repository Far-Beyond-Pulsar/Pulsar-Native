//! `<project>/.pulsar/keybindings.json`: the user's keybinding changes.
//!
//! ```json
//! {
//!   "version": 1,
//!   "bindings": [
//!     { "keystrokes": "ctrl-j", "action": "app::ToggleFileManager" },
//!     { "keystrokes": "ctrl-space", "action": null },
//!     { "keystrokes": "ctrl-e", "action": "app::Run", "context": "Editor" }
//!   ]
//! }
//! ```
//!
//! `action: null` unbinds the keystrokes in that context (no `context` means the
//! bindings that have none; ones in other contexts are not touched). Entries
//! apply in order, after the
//! built-in bindings, and later ones win.
//!
//! Reading is forgiving, writing is careful:
//! - a missing file is an empty list;
//! - a file that is not valid JSON is copied to `keybindings.json.bak` and
//!   treated as empty (the next save replaces it);
//! - an entry that does not parse is skipped, with a warning and the same
//!   `.bak` copy, so nothing a person wrote is lost silently;
//! - a file of a newer `version` is read-only: it is never overwritten;
//! - writes go to a sibling file that is then renamed over the target.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::overrides::Override;

pub const FILE_NAME: &str = "keybindings.json";
pub const VERSION: u64 = 1;

pub fn keybindings_path(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join(FILE_NAME)
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub overrides: Vec<Override>,
    /// Things the person should know about the file they wrote.
    pub warnings: Vec<String>,
    /// Why the file must not be written, if so.
    pub read_only: Option<String>,
}

pub fn keep_copy(path: &Path) {
    let backup = path.with_extension("json.bak");
    if let Err(error) = engine_fs::virtual_fs::copy(path, &backup) {
        tracing::warn!("could not keep a copy of {}: {error}", path.display());
    }
}

pub fn load(project_root: &Path) -> Loaded {
    let path = keybindings_path(project_root);
    if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
        return Loaded::default();
    }
    let text = match engine_fs::virtual_fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return Loaded {
                read_only: Some(format!("{} could not be read: {error}", path.display())),
                ..Loaded::default()
            };
        }
    };
    parse(&text, &path)
}

fn parse(text: &str, path: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            keep_copy(path);
            loaded.warnings.push(format!(
                "{FILE_NAME} is not valid JSON ({error}); a copy was kept as {FILE_NAME}.bak and the file will be replaced on the next change"
            ));
            return loaded;
        }
    };
    let Some(object) = value.as_object() else {
        keep_copy(path);
        loaded.warnings.push(format!(
            "{FILE_NAME} is not a JSON object; a copy was kept as {FILE_NAME}.bak"
        ));
        return loaded;
    };
    let version = object.get("version").and_then(Value::as_u64).unwrap_or(1);
    if version > VERSION {
        loaded.read_only = Some(format!(
            "{FILE_NAME} is version {version}, newer than this editor understands ({VERSION}); it is not modified"
        ));
        return loaded;
    }
    let Some(list) = object.get("bindings").and_then(Value::as_array) else {
        return loaded;
    };
    let mut skipped = false;
    for (ix, item) in list.iter().enumerate() {
        match serde_json::from_value::<Override>(item.clone()) {
            Ok(entry) if !entry.keystrokes.trim().is_empty() => loaded.overrides.push(entry),
            Ok(_) => {
                skipped = true;
                loaded.warnings.push(format!(
                    "entry {} has no keystrokes and was skipped",
                    ix + 1
                ));
            }
            Err(error) => {
                skipped = true;
                loaded
                    .warnings
                    .push(format!("entry {} was skipped: {error}", ix + 1));
            }
        }
    }
    if skipped {
        keep_copy(path);
    }
    loaded
}

pub fn save(project_root: &Path, overrides: &[Override]) -> io::Result<()> {
    let path = keybindings_path(project_root);
    let body = json!({ "version": VERSION, "bindings": overrides });
    let text = serde_json::to_string_pretty(&body).map_err(io::Error::other)?;
    engine_fs::virtual_fs::write_file_atomically(&path, (text + "\n").as_bytes())
        .map_err(|error| io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(keys: &str, action: Option<&str>) -> Override {
        Override {
            keystrokes: keys.into(),
            action: action.map(str::to_owned),
            context: None,
            params: None,
        }
    }

    #[test]
    fn a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(dir.path());
        assert!(loaded.overrides.is_empty());
        assert!(loaded.warnings.is_empty());
        assert!(loaded.read_only.is_none());
    }

    #[test]
    fn saved_overrides_load_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = vec![
            entry("ctrl-space", None),
            entry("ctrl-j", Some("a::Toggle")),
        ];
        save(dir.path(), &overrides).unwrap();
        assert!(keybindings_path(dir.path()).is_file());
        assert!(
            !keybindings_path(dir.path())
                .with_extension("json.tmp")
                .exists()
        );
        assert_eq!(load(dir.path()).overrides, overrides);
    }

    #[test]
    fn invalid_json_is_kept_as_a_backup_and_read_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = keybindings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let loaded = load(dir.path());
        assert!(loaded.overrides.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.read_only.is_none());
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn a_bad_entry_is_skipped_and_the_rest_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = keybindings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{ "version": 1, "bindings": [
                { "keystrokes": "f5", "action": "a::Run" },
                { "action": "a::NoKeys" },
                { "keystrokes": "", "action": "a::Empty" },
                { "keystrokes": "f6", "action": null }
            ] }"#,
        )
        .unwrap();
        let loaded = load(dir.path());
        assert_eq!(
            loaded.overrides,
            vec![entry("f5", Some("a::Run")), entry("f6", None)]
        );
        assert_eq!(loaded.warnings.len(), 2);
        assert!(path.with_extension("json.bak").is_file());
    }

    #[test]
    fn a_newer_version_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = keybindings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{ "version": 9, "bindings": [] }"#).unwrap();
        let loaded = load(dir.path());
        assert!(loaded.read_only.is_some());
        assert!(loaded.overrides.is_empty());
    }

    #[test]
    fn a_file_without_a_version_is_read_as_version_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = keybindings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{ "bindings": [ { "keystrokes": "f5", "action": "a::Run" } ] }"#,
        )
        .unwrap();
        assert_eq!(
            load(dir.path()).overrides,
            vec![entry("f5", Some("a::Run"))]
        );
    }
}
