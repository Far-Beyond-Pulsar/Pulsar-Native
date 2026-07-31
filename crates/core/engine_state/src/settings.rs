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

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::Context as _;
use engine_fs::{FsProvider as _, LocalFsProvider};

pub use pulsar_config::{
    ChangeEvent, Color, ConfigError, ConfigManager, ConfigStore, ConfigValue, DropdownOption,
    FieldType, ListenerId, NamespaceSchema, OwnerHandle, PersistError, SchemaEntry, SearchResult,
    SettingInfo, Validator,
};

// ─── Global manager ───────────────────────────────────────────────────────────

static GLOBAL_CONFIG: std::sync::LazyLock<ConfigManager> =
    std::sync::LazyLock::new(ConfigManager::new);
static SETTINGS_FILE_WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

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

impl Default for GlobalSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalSettings {
    pub fn new() -> Self {
        let manager = global_config().clone();
        let store = ConfigStore::new(manager, "PulsarEngine").unwrap_or_else(|_| {
            ConfigStore::with_dir(global_config().clone(), PathBuf::from("config")).unwrap()
        });
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
            if ns != NS_EDITOR {
                continue;
            }
            let owner_path = owner_segs.join("/");
            if let Some(handle) = global_config().owner_handle(&ns, &owner_path) {
                let _ = self.store.load(&handle);
            }
        }
    }

    /// Save all editor-namespace owners to disk.
    pub fn save_all(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        for owner_segs in global_config().list_owners(NS_EDITOR) {
            let owner = owner_segs.join("/");
            let handle = editor_owner_handle(&owner)?;
            let keys = handle
                .list_settings()
                .into_iter()
                .filter(|info| !info.read_only)
                .map(|info| info.key)
                .collect::<Vec<_>>();
            let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
            if let Err(error) = self.save_owner_keys(&owner, &key_refs) {
                errors.push(format!("{owner}: {error:#}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "failed to save one or more editor settings owners:\n{}",
                errors.join("\n")
            ))
        }
    }

    /// Check that an owner's persisted file can be read without applying it.
    ///
    /// This lets callers surface a damaged settings file without replaying disk
    /// values over unsaved in-memory changes.
    pub fn validate_owner_file(&self, owner: &str) -> anyhow::Result<bool> {
        let handle = editor_owner_handle(owner)?;
        let provider = self.local_provider()?;
        Ok(self.read_owner_table(&provider, &handle)?.is_some())
    }

    /// Save only `keys`, preserving all other known and unknown TOML entries.
    pub fn save_owner_keys(&self, owner: &str, keys: &[&str]) -> anyhow::Result<()> {
        let handle = editor_owner_handle(owner)?;
        let settings = handle.list_settings();

        if keys.is_empty() {
            return Ok(());
        }

        let mut selected = Vec::with_capacity(keys.len());
        for key in keys {
            let info = settings
                .iter()
                .find(|info| info.key == *key)
                .ok_or_else(|| ConfigError::UnknownKey {
                    namespace: NS_EDITOR.to_owned(),
                    owner: handle.owner().to_vec(),
                    key: (*key).to_owned(),
                })?;
            if info.read_only {
                return Err(ConfigError::ReadOnly {
                    namespace: NS_EDITOR.to_owned(),
                    owner: handle.owner().to_vec(),
                    key: (*key).to_owned(),
                }
                .into());
            }
            selected.push(info);
        }

        let _write_guard = SETTINGS_FILE_WRITE_LOCK.lock();
        let provider = self.local_provider()?;
        let mut table = self
            .read_owner_table(&provider, &handle)?
            .unwrap_or_default();
        for info in selected {
            table.insert(info.key.clone(), config_value_to_toml(&info.current_value));
        }

        let path = self.store.toml_path(NS_EDITOR, handle.owner());
        let owner_display = if handle.owner().is_empty() {
            NS_EDITOR.to_owned()
        } else {
            format!("{}/{}", NS_EDITOR, handle.owner().join("/"))
        };
        let header = format!(
            "# PulsarConfig - {owner_display}\n\
             # Edit this file to override application defaults.\n\
             # Missing keys fall back to the schema default.\n\
             # Read-only settings are managed by the application and are not listed here.\n\n"
        );
        let body = toml::to_string(&table).context("failed to serialize settings as TOML")?;
        provider
            .write_file_atomically(&path, format!("{header}{body}").as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    fn local_provider(&self) -> anyhow::Result<LocalFsProvider> {
        LocalFsProvider::with_root(self.store.config_dir().to_path_buf())
            .context("failed to access the editor settings directory")
    }

    fn read_owner_table(
        &self,
        provider: &LocalFsProvider,
        handle: &OwnerHandle,
    ) -> anyhow::Result<Option<toml::Table>> {
        let path = self.store.toml_path(NS_EDITOR, handle.owner());
        let bytes = match provider.read_file(&path) {
            Ok(bytes) => bytes,
            Err(error) if has_io_kind(&error, ErrorKind::NotFound) => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()));
            }
        };
        let content = String::from_utf8(bytes)
            .with_context(|| format!("settings file {} is not UTF-8", path.display()))?;
        let table = toml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(Some(table))
    }

    /// Return the directory where editor config files are written.
    pub fn config_dir(&self) -> &std::path::Path {
        self.store.config_dir()
    }

    pub fn get(&self, owner: &str, key: &str) -> Option<ConfigValue> {
        global_config().get(NS_EDITOR, owner, key).ok()
    }

    pub fn set(
        &self,
        owner: &str,
        key: &str,
        value: impl Into<ConfigValue>,
    ) -> Result<(), ConfigError> {
        global_config()
            .owner_handle(NS_EDITOR, owner)
            .ok_or_else(|| ConfigError::OwnerNotFound {
                namespace: NS_EDITOR.to_owned(),
                owner: owner
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
            .and_then(|h| h.set(key, value.into()))
    }
}

fn editor_owner_handle(owner: &str) -> Result<OwnerHandle, ConfigError> {
    global_config()
        .owner_handle(NS_EDITOR, owner)
        .ok_or_else(|| ConfigError::OwnerNotFound {
            namespace: NS_EDITOR.to_owned(),
            owner: owner
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect(),
        })
}

fn has_io_kind(error: &anyhow::Error, kind: ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == kind)
    })
}

fn config_value_to_toml(value: &ConfigValue) -> toml::Value {
    match value {
        ConfigValue::Bool(value) => toml::Value::Boolean(*value),
        ConfigValue::Int(value) => toml::Value::Integer(*value),
        ConfigValue::Float(value) => toml::Value::Float(*value),
        ConfigValue::String(value) => toml::Value::String(value.clone()),
        ConfigValue::Color(value) => toml::Value::Table(
            [
                ("r".to_owned(), toml::Value::Integer(value.r.into())),
                ("g".to_owned(), toml::Value::Integer(value.g.into())),
                ("b".to_owned(), toml::Value::Integer(value.b.into())),
                ("a".to_owned(), toml::Value::Integer(value.a.into())),
            ]
            .into_iter()
            .collect(),
        ),
        ConfigValue::Array(values) => {
            toml::Value::Array(values.iter().map(config_value_to_toml).collect())
        }
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
    /// Returns `None` if the project directory doesn't exist or can't be created.
    pub fn new(project_path: &Path) -> Option<Self> {
        match ConfigStore::with_dir(global_config().clone(), project_path.join(".pulsar")) {
            Ok(store) => Some(Self {
                store,
                project_path: project_path.to_owned(),
            }),
            Err(e) => {
                tracing::error!(
                    "ProjectSettings: cannot create config dir for {:?}: {:?}",
                    project_path,
                    e
                );
                None
            }
        }
    }

    pub fn load_all(&self) {
        for (ns, owner_segs) in global_config().list_all_owners() {
            if ns != NS_PROJECT {
                continue;
            }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Barrier,
        },
        thread,
    };

    use super::*;

    struct SettingsFixture {
        settings: GlobalSettings,
        handle: OwnerHandle,
        owner: String,
        directory: PathBuf,
    }

    impl SettingsFixture {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let owner = format!("tests/{label}_{}_{id}", std::process::id());
            let schema = NamespaceSchema::new("Test settings", "")
                .setting("selected", SchemaEntry::new("Selected", 0_i64))
                .setting("other", SchemaEntry::new("Other", 10_i64))
                .setting(
                    "read_only",
                    SchemaEntry::new("Read only", "fixed").read_only(),
                );
            let handle = global_config()
                .register(NS_EDITOR, &owner, schema)
                .expect("register test owner");
            let directory = std::env::temp_dir().join(format!(
                "pulsar_engine_state_{label}_{}_{id}",
                std::process::id()
            ));
            let settings = GlobalSettings::with_dir(&directory);

            Self {
                settings,
                handle,
                owner,
                directory,
            }
        }

        fn path(&self) -> PathBuf {
            self.settings
                .store
                .toml_path(NS_EDITOR, self.handle.owner())
        }
    }

    impl Drop for SettingsFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn save_owner_keys_preserves_unselected_and_unknown_entries() {
        let fixture = SettingsFixture::new("preserve");
        let path = fixture.path();
        fs::create_dir_all(path.parent().expect("owner parent")).expect("create owner parent");
        fs::write(
            &path,
            "selected = 1\nother = 41\nunknown_extension = \"keep\"\n",
        )
        .expect("write initial settings");

        fixture.handle.set("selected", 2_i64).expect("set selected");
        fixture.handle.set("other", 99_i64).expect("set other");
        fixture
            .settings
            .save_owner_keys(&fixture.owner, &["selected"])
            .expect("save selected key");

        let table: toml::Table =
            toml::from_str(&fs::read_to_string(&path).expect("read merged settings"))
                .expect("parse merged settings");
        assert_eq!(table["selected"].as_integer(), Some(2));
        assert_eq!(table["other"].as_integer(), Some(41));
        assert_eq!(table["unknown_extension"].as_str(), Some("keep"));
    }

    #[test]
    fn save_owner_keys_rejects_damaged_toml_without_overwriting_it() {
        let fixture = SettingsFixture::new("damaged");
        let path = fixture.path();
        fs::create_dir_all(path.parent().expect("owner parent")).expect("create owner parent");
        let damaged = b"selected = [\n";
        fs::write(&path, damaged).expect("write damaged settings");
        fixture.handle.set("selected", 2_i64).expect("set selected");

        assert!(fixture
            .settings
            .validate_owner_file(&fixture.owner)
            .is_err());
        assert!(fixture
            .settings
            .save_owner_keys(&fixture.owner, &["selected"])
            .is_err());
        assert_eq!(fs::read(&path).expect("read damaged settings"), damaged);
    }

    #[test]
    fn save_all_preserves_unknown_entries() {
        let fixture = SettingsFixture::new("save_all_preserve");
        let path = fixture.path();
        fs::create_dir_all(path.parent().expect("owner parent")).expect("create owner parent");
        fs::write(&path, "selected = 1\nunknown_extension = \"keep\"\n")
            .expect("write initial settings");
        fixture.handle.set("selected", 2_i64).expect("set selected");

        fixture.settings.save_all().expect("save all settings");

        let table: toml::Table =
            toml::from_str(&fs::read_to_string(&path).expect("read merged settings"))
                .expect("parse merged settings");
        assert_eq!(table["selected"].as_integer(), Some(2));
        assert_eq!(table["unknown_extension"].as_str(), Some("keep"));
    }

    #[test]
    fn save_all_rejects_damaged_toml_without_overwriting_it() {
        let fixture = SettingsFixture::new("save_all_damaged");
        let path = fixture.path();
        let healthy_owner = format!("{}_healthy", fixture.owner);
        let healthy_handle = global_config()
            .register(
                NS_EDITOR,
                &healthy_owner,
                NamespaceSchema::new("Healthy settings", "")
                    .setting("selected", SchemaEntry::new("Selected", 0_i64)),
            )
            .expect("register healthy owner");
        healthy_handle
            .set("selected", 9_i64)
            .expect("set healthy owner");
        let healthy_path = fixture
            .settings
            .store
            .toml_path(NS_EDITOR, healthy_handle.owner());
        fs::create_dir_all(path.parent().expect("owner parent")).expect("create owner parent");
        let damaged = b"selected = [\n";
        fs::write(&path, damaged).expect("write damaged settings");
        fixture.handle.set("selected", 2_i64).expect("set selected");

        assert!(fixture.settings.save_all().is_err());
        assert_eq!(fs::read(&path).expect("read damaged settings"), damaged);
        let healthy_table: toml::Table =
            toml::from_str(&fs::read_to_string(healthy_path).expect("read healthy settings"))
                .expect("parse healthy settings");
        assert_eq!(healthy_table["selected"].as_integer(), Some(9));
    }

    #[test]
    fn validate_owner_file_does_not_replay_disk_values() {
        let fixture = SettingsFixture::new("validate_only");
        let path = fixture.path();
        fs::create_dir_all(path.parent().expect("owner parent")).expect("create owner parent");
        fs::write(&path, "selected = 5\n").expect("write settings");
        fixture.handle.set("selected", 9_i64).expect("set selected");

        assert!(fixture
            .settings
            .validate_owner_file(&fixture.owner)
            .expect("validate settings"));
        assert_eq!(fixture.handle.get_int("selected"), Ok(9));
    }

    #[test]
    fn save_owner_keys_validates_requested_keys_before_writing() {
        let fixture = SettingsFixture::new("invalid_keys");

        assert!(fixture
            .settings
            .save_owner_keys(&fixture.owner, &["missing"])
            .is_err());
        assert!(fixture
            .settings
            .save_owner_keys(&fixture.owner, &["read_only"])
            .is_err());
        assert!(!fixture.path().exists());
    }

    #[test]
    fn save_owner_keys_creates_a_file_with_only_selected_keys() {
        let fixture = SettingsFixture::new("new_file");
        fixture.handle.set("selected", 7_i64).expect("set selected");
        fixture.handle.set("other", 42_i64).expect("set other");

        fixture
            .settings
            .save_owner_keys(&fixture.owner, &["selected"])
            .expect("save selected key");

        let table: toml::Table =
            toml::from_str(&fs::read_to_string(fixture.path()).expect("read settings"))
                .expect("parse settings");
        assert_eq!(table.len(), 1);
        assert_eq!(table["selected"].as_integer(), Some(7));
    }

    #[test]
    fn concurrent_key_saves_preserve_each_others_values() {
        let fixture = SettingsFixture::new("concurrent");
        fixture.handle.set("selected", 7_i64).expect("set selected");
        fixture.handle.set("other", 42_i64).expect("set other");

        let first = GlobalSettings::with_dir(&fixture.directory);
        let second = GlobalSettings::with_dir(&fixture.directory);
        let barrier = Arc::new(Barrier::new(2));
        thread::scope(|scope| {
            let first_barrier = barrier.clone();
            let owner = fixture.owner.clone();
            scope.spawn(move || {
                first_barrier.wait();
                first
                    .save_owner_keys(&owner, &["selected"])
                    .expect("save selected");
            });

            let second_barrier = barrier.clone();
            let owner = fixture.owner.clone();
            scope.spawn(move || {
                second_barrier.wait();
                second
                    .save_owner_keys(&owner, &["other"])
                    .expect("save other");
            });
        });

        let table: toml::Table =
            toml::from_str(&fs::read_to_string(fixture.path()).expect("read settings"))
                .expect("parse settings");
        assert_eq!(table["selected"].as_integer(), Some(7));
        assert_eq!(table["other"].as_integer(), Some(42));
    }
}
