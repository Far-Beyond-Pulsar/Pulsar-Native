//! Settings subsystem — backed by PulsarConfig.
//!
//! # Architecture
//!
//! All engine settings are stored in a single global [`ConfigManager`].
//! The top-level namespaces map directly to the old `SettingScope`:
//!
//! | Old scope              | PulsarConfig namespace |
//! |------------------------|------------------------|
//! | `SettingScope::Global` | `"editor"`             |
//! | `SettingScope::Project`| `"project"`            |
//!
//! Within each namespace, every subsystem or page registers its settings
//! via [`ConfigManager::register`].  The old flat `registry()` global is
//! replaced by [`global_config()`].

use std::path::{Path, PathBuf};

pub use pulsar_config::{
    ChangeEvent, Color, ConfigError, ConfigManager, ConfigStore, ConfigValue,
    DropdownOption, FieldType, ListenerId, NamespaceSchema, OwnerHandle,
    PersistError, SchemaEntry, SearchResult, SettingInfo, Validator,
};

// ─── Global manager ───────────────────────────────────────────────────────────

static GLOBAL_CONFIG: std::sync::LazyLock<ConfigManager> =
    std::sync::LazyLock::new(ConfigManager::new);

/// Return a reference to the application-wide [`ConfigManager`].
///
/// All subsystems share this instance — they register their schemas on startup
/// and write through their [`OwnerHandle`]s.
pub fn global_config() -> &'static ConfigManager {
    &GLOBAL_CONFIG
}

// ─── Namespace helpers ────────────────────────────────────────────────────────

/// The namespace used for engine / editor-wide settings.
pub const NS_EDITOR: &str = "editor";
/// The namespace used for per-project settings.
pub const NS_PROJECT: &str = "project";

// ─── Disk persistence ─────────────────────────────────────────────────────────

/// Engine-wide (editor) settings backed by PulsarConfig's [`ConfigStore`].
pub struct GlobalSettings {
    store: ConfigStore,
}

impl GlobalSettings {
    pub fn new() -> Self {
        let manager = global_config().clone();
        let store = ConfigStore::new(manager, "PulsarEngine")
            .unwrap_or_else(|_| ConfigStore::with_dir(global_config().clone(), PathBuf::from("config")).unwrap());
        Self { store }
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        let store = ConfigStore::with_dir(global_config().clone(), dir.into())
            .expect("invalid config directory");
        Self { store }
    }

    /// Load persisted values for all registered editor-namespace owners.
    pub fn load_all(&self) {
        for (ns, owner_segs) in global_config().list_all_owners() {
            if ns != NS_EDITOR { continue; }
            let owner_path = owner_segs.join("/");
            if let Some(handle) = global_config().owner_handle(&ns, &owner_path) {
                let _ = self.store.load(&handle);
            }
        }
    }

    /// Save all editor-namespace owners to disk.
    pub fn save_all(&self) -> Result<(), PersistError> {
        self.store.save_namespace(NS_EDITOR)
    }

    /// Return the directory where editor config files are written.
    pub fn config_dir(&self) -> &std::path::Path {
        self.store.config_dir()
    }

    pub fn get(&self, owner: &str, key: &str) -> Option<ConfigValue> {
        global_config().get(NS_EDITOR, owner, key).ok()
    }

    pub fn set(&self, owner: &str, key: &str, value: impl Into<ConfigValue>) -> Result<(), ConfigError> {
        global_config()
            .owner_handle(NS_EDITOR, owner)
            .ok_or_else(|| ConfigError::OwnerNotFound {
                namespace: NS_EDITOR.to_owned(),
                owner: owner.split('/').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            })
            .and_then(|h| h.set(key, value.into()))
    }
}

/// Per-project settings backed by PulsarConfig's [`ConfigStore`].
///
/// # `.pulsar` directory ownership
///
/// `ProjectSettings` writes **only** to `<project>/.pulsar/project/<owner>.toml`.
/// It never scans, deletes, or modifies any other file in `.pulsar/`, making it
/// safe to share `.pulsar/` with other subsystems (scene cache, asset manifests,
/// version control hooks, etc.).  All persistence is additive: specific TOML files
/// are written per registered owner; nothing else is touched.
pub struct ProjectSettings {
    store: ConfigStore,
    project_path: PathBuf,
}

impl ProjectSettings {
    pub fn new(project_path: &Path) -> Self {
        let store = ConfigStore::with_dir(global_config().clone(), project_path.join(".pulsar"))
            .expect("invalid project config directory");
        Self { store, project_path: project_path.to_owned() }
    }

    pub fn load_all(&self) {
        for (ns, owner_segs) in global_config().list_all_owners() {
            if ns != NS_PROJECT { continue; }
            let owner_path = owner_segs.join("/");
            if let Some(handle) = global_config().owner_handle(&ns, &owner_path) {
                let _ = self.store.load(&handle);
            }
        }
    }

    pub fn save_all(&self) -> Result<(), PersistError> {
        self.store.save_namespace(NS_PROJECT)
    }

    pub fn path(&self) -> &Path {
        &self.project_path
    }

    pub fn get(&self, owner: &str, key: &str) -> Option<ConfigValue> {
        global_config().get(NS_PROJECT, owner, key).ok()
    }
}
