//! Project scripting config and the id / file-name conventions of script
//! instances and compiled modules. Entry points: [`ScriptingConfig::load`],
//! [`instance_id_for`], [`module_file`].

use std::path::{Path, PathBuf};

use pulsar_class::ClassId;
use serde::{Deserialize, Serialize};

/// The project's scripting config file, relative to the project root.
pub const SCRIPTING_CONFIG_FILE: &str = "Pulsar/scripting.json";

/// `<project>/Pulsar/scripting.json`.
pub fn scripting_config_path(project_root: &Path) -> PathBuf {
    project_root.join(SCRIPTING_CONFIG_FILE)
}

/// Project scripting settings (`Pulsar/scripting.json`, next to the native
/// prefab config `Pulsar/level.json`). A missing file means the defaults.
///
/// ```json
/// { "global_scripts": ["GameRules", "5b0c…-class-guid"] }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptingConfig {
    /// Classes (GUID or name) that get exactly one instance at startup,
    /// bound to no entity: logic that belongs to the game, not to an
    /// object. Default: none.
    #[serde(default)]
    pub global_scripts: Vec<String>,
}

impl ScriptingConfig {
    /// Read the project's config. Missing: defaults. Unreadable: defaults,
    /// with a warning.
    pub fn load(project_root: &Path) -> Self {
        let path = scripting_config_path(project_root);
        let Ok(bytes) = engine_fs::virtual_fs::read_file(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), "Unreadable scripting config; using defaults: {error}");
            Self::default()
        })
    }
}

/// Id of the script instance of class `class` on the object `stable_id`.
pub fn instance_id_for(stable_id: &str, class: &ClassId) -> String {
    format!("{stable_id}::{class}")
}

/// Id of the global script instance of class `class`.
pub fn global_instance_id(class: &ClassId) -> String {
    format!("global::{class}")
}

/// Where compiled modules live is the class model's
/// (`pulsar_class::modules`): the project's module store, or cooked content.
pub use pulsar_class::modules::{module_file, MODULE_BINARY_FILE, MODULE_JSON_FILE};

/// Read a compiled module file (JSON or binary) through the virtual
/// filesystem. `None` when it does not exist.
pub(super) fn read_module_bytes(path: &Path) -> Option<std::io::Result<Vec<u8>>> {
    if !engine_fs::virtual_fs::exists(path).unwrap_or(false) {
        return None;
    }
    Some(engine_fs::virtual_fs::read_file(path).map_err(|e| std::io::Error::other(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_name_the_object_or_mark_a_global() {
        let class = ClassId::from("guid-1");
        assert_eq!(instance_id_for("obj-7", &class), "obj-7::guid-1");
        assert_eq!(global_instance_id(&class), "global::guid-1");
    }

    #[test]
    fn config_path_sits_under_the_project_pulsar_folder() {
        let path = scripting_config_path(Path::new("proj"));
        assert!(path.ends_with(SCRIPTING_CONFIG_FILE));
        assert!(path.starts_with("proj"));
    }
}
