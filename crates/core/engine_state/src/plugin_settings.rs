//! Plugin settings: persistence.
//!
//! `pulsar_settings::plugin` registers every enabled plugin's groups in the
//! settings database (namespace `plugin`); this module keeps the values a
//! project overrides on disk. Plugins are project relative, so their settings
//! are project settings: one layer, the open project's
//! `<project>/.pulsar/plugin/<owner>.toml`, over the schema defaults.
//!
//! A file holds only the keys the project overrides. The database holds the
//! effective value (the override, else the default), so
//! `PluginSettingsHandle` listeners fire when the layer changes it.
//!
//! Headless tools (`pulsar package`, CI) read the layer without the editor
//! through [`read_project_layer`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use engine_fs::{FsProvider as _, LocalFsProvider};
use parking_lot::Mutex;
use pulsar_settings::plugin::NS;

use crate::settings::{global_config, ConfigValue, GlobalSettings};

type Layer = BTreeMap<String, BTreeMap<String, ConfigValue>>;

#[derive(Default)]
struct State {
    project: Layer,
    /// `<project>/.pulsar`, while a project is open.
    project_root: Option<PathBuf>,
    /// Owners whose layer changed since the last save.
    dirty: Vec<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    project: BTreeMap::new(),
    project_root: None,
    dirty: Vec::new(),
});

fn layer_file(root: &Path, owner: &str) -> PathBuf {
    let mut path = root.join(NS);
    let segments: Vec<&str> = owner.split('/').filter(|s| !s.is_empty()).collect();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        path.push(segment);
    }
    path.push(format!(
        "{}.toml",
        segments.last().copied().unwrap_or("_root")
    ));
    path
}

/// Read one owner's file into raw TOML values (missing: empty).
fn read_table(root: &Path, owner: &str) -> anyhow::Result<toml::Table> {
    let provider = LocalFsProvider::with_root(root.to_path_buf())
        .with_context(|| format!("cannot access {}", root.display()))?;
    let path = layer_file(root, owner);
    match provider.exists(&path) {
        Ok(true) => {}
        _ => return Ok(toml::Table::new()),
    }
    let bytes = provider
        .read_file(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let text =
        String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", path.display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

/// The plugin settings `owner` overrides, straight from the project's files,
/// with no editor and no database: the keys the project overrides, as raw
/// TOML. For `pulsar package`, CI and anything else that runs headless.
pub fn read_project_layer(project_root: &Path, owner: &str) -> anyhow::Result<toml::Table> {
    read_table(&project_root.join(".pulsar"), owner)
}

fn typed_layer(root: &Path, owner: &str) -> BTreeMap<String, ConfigValue> {
    let table = match read_table(root, owner) {
        Ok(table) => table,
        Err(error) => {
            tracing::error!("plugin settings `{owner}`: {error:#}");
            return BTreeMap::new();
        }
    };
    let Some(settings) = global_config().list_settings(NS, owner) else {
        return BTreeMap::new();
    };
    let mut layer = BTreeMap::new();
    for (key, raw) in table {
        let Some(info) = settings.iter().find(|info| info.key == key) else {
            continue;
        };
        if info.read_only {
            continue;
        }
        if let Some(value) = crate::settings::toml_to_config_value(&raw, &info.default_value) {
            layer.insert(key, value);
        }
    }
    layer
}

/// Move the saved values of groups a plugin took over from another owner path
/// (`pulsar_settings::plugin::SettingsMove`) to the group's own: the old file
/// is renamed when it exists and the new one does not.
fn apply_moves<'a>(
    root: &Path,
    moves: impl IntoIterator<Item = &'a pulsar_settings::plugin::SettingsMove>,
) {
    let Ok(provider) = LocalFsProvider::with_root(root.to_path_buf()) else {
        return;
    };
    for moved in moves {
        let to = pulsar_settings::plugin::owner_of(moved.origin, moved.id);
        let (from_file, to_file) = (layer_file(root, moved.from), layer_file(root, &to));
        if !provider.exists(&from_file).unwrap_or(false) {
            continue;
        }
        if provider.exists(&to_file).unwrap_or(false) {
            tracing::warn!(
                "plugin settings: {} and {} both exist; keeping {}",
                from_file.display(),
                to_file.display(),
                to_file.display()
            );
            continue;
        }
        let moved_to = match to_file.parent() {
            Some(dir) => engine_fs::virtual_fs::create_dir_all(dir),
            None => Ok(()),
        }
        .and_then(|()| provider.rename(&from_file, &to_file));
        match moved_to {
            Ok(()) => tracing::info!(
                "plugin settings: moved {} to {}",
                from_file.display(),
                to_file.display()
            ),
            Err(error) => tracing::error!(
                "plugin settings: cannot move {} to {}: {error:#}",
                from_file.display(),
                to_file.display()
            ),
        }
    }
}

/// Plugin settings used to have a per-user layer in the editor's config
/// directory. They are project settings now: say so once for any such file
/// still there, so a value that stopped applying is not a mystery.
fn report_retired_user_layer(owners: &[String]) {
    static REPORTED: std::sync::Once = std::sync::Once::new();
    REPORTED.call_once(|| {
        let user_root = GlobalSettings::new().config_dir().to_path_buf();
        for owner in owners {
            let file = layer_file(&user_root, owner);
            if engine_fs::virtual_fs::exists(&file).unwrap_or(false) {
                tracing::warn!(
                    "plugin settings are project settings now: {} is no longer read; set its values in Project Settings",
                    file.display()
                );
            }
        }
    });
}

fn project_root_of(project: &Path) -> PathBuf {
    project.join(".pulsar")
}

/// The owners registered under the plugin namespace.
fn owners() -> Vec<String> {
    let mut owners: Vec<String> = global_config()
        .list_owners(NS)
        .into_iter()
        .map(|segments| segments.join("/"))
        .collect();
    owners.sort();
    owners
}

/// A value the database must take to be the effective one.
struct Change {
    owner: String,
    key: String,
    value: ConfigValue,
}

/// What the database must change to hold every plugin setting's effective
/// value: the project's override, else the default. Planned under the lock;
/// [`commit`] applies it after the lock is released, because applying fires
/// change listeners, which may call back in here.
fn plan_effective(state: &State) -> Vec<Change> {
    let mut changes = Vec::new();
    for owner in owners() {
        let Some(handle) = global_config().owner_handle(NS, &owner) else {
            continue;
        };
        for info in handle.list_settings() {
            if info.read_only {
                continue;
            }
            let wanted = state
                .project
                .get(&owner)
                .and_then(|layer| layer.get(&info.key))
                .cloned()
                .unwrap_or_else(|| info.default_value.clone());
            if wanted != info.current_value {
                changes.push(Change {
                    owner: owner.clone(),
                    key: info.key,
                    value: wanted,
                });
            }
        }
    }
    changes
}

/// Apply planned changes to the database, firing change listeners (no lock
/// is held).
fn commit(changes: Vec<Change>) {
    for change in changes {
        let Some(handle) = global_config().owner_handle(NS, &change.owner) else {
            continue;
        };
        if let Err(error) = handle.set(&change.key, change.value) {
            tracing::warn!("plugin setting `{}.{}`: {error}", change.owner, change.key);
            let _ = handle.reset_to_default(&change.key);
        }
    }
}

/// Bring plugin settings in line with the enabled plugins and `project`
/// (the open project, if any): register the enabled plugins' groups and
/// withdraw the disabled ones, reload the project's overrides and apply the
/// effective values. Called when a project is set; call again after the
/// plugin set changes.
pub fn activate(project: Option<&Path>) {
    pulsar_settings::plugin::sync(global_config());
    let project_dir = project.map(project_root_of);
    if let Some(root) = &project_dir {
        apply_moves(
            root,
            pulsar_plugins::registered::<pulsar_settings::plugin::SettingsMove>(),
        );
    }
    let owners = owners();
    report_retired_user_layer(&owners);
    let mut state = STATE.lock();
    state.project.clear();
    state.dirty.clear();
    if let Some(root) = &project_dir {
        for owner in &owners {
            let layer = typed_layer(root, owner);
            if !layer.is_empty() {
                state.project.insert(owner.clone(), layer);
            }
        }
    }
    state.project_root = project_dir;
    let changes = plan_effective(&state);
    drop(state);
    commit(changes);
}

/// Re-register after the plugin set changed (without touching the open
/// project): newly enabled groups appear with their persisted values, the
/// groups of disabled plugins are withdrawn.
pub fn resync() {
    let project = STATE
        .lock()
        .project_root
        .clone()
        .and_then(|root| root.parent().map(Path::to_path_buf));
    activate(project.as_deref());
}

/// Whether a project is open (plugin settings can be changed).
pub fn has_project() -> bool {
    STATE.lock().project_root.is_some()
}

/// Whether the project overrides `owner.key` (otherwise the default is in
/// effect).
pub fn is_overridden(owner: &str, key: &str) -> bool {
    STATE
        .lock()
        .project
        .get(owner)
        .is_some_and(|layer| layer.contains_key(key))
}

/// Override `owner.key` in the project (validated against the schema),
/// update the effective value and notify listeners. The change is on disk
/// after [`save`].
pub fn set(owner: &str, key: &str, value: ConfigValue) -> anyhow::Result<()> {
    let handle = global_config()
        .owner_handle(NS, owner)
        .with_context(|| format!("plugin settings `{owner}` are not registered"))?;
    anyhow::ensure!(has_project(), "no project is open");
    // The schema decides what is legal, before the layer changes.
    handle.validate(key, &value)?;
    let mut guard = STATE.lock();
    let state = &mut *guard;
    state
        .project
        .entry(owner.to_string())
        .or_default()
        .insert(key.to_string(), value);
    if !state.dirty.iter().any(|d| d == owner) {
        state.dirty.push(owner.to_string());
    }
    let changes = plan_effective(state);
    drop(guard);
    commit(changes);
    Ok(())
}

/// Remove the project's override of `owner.key` (the setting falls back to
/// its default).
pub fn reset(owner: &str, key: &str) {
    let mut guard = STATE.lock();
    let state = &mut *guard;
    let removed = state
        .project
        .get_mut(owner)
        .map(|layer| layer.remove(key).is_some())
        .unwrap_or(false);
    if state.project.get(owner).is_some_and(BTreeMap::is_empty) {
        state.project.remove(owner);
    }
    if removed && !state.dirty.iter().any(|d| d == owner) {
        state.dirty.push(owner.to_string());
    }
    let changes = plan_effective(state);
    drop(guard);
    commit(changes);
}

fn write_layer(
    root: &Path,
    owner: &str,
    layer: Option<&BTreeMap<String, ConfigValue>>,
) -> anyhow::Result<()> {
    if layer.is_some_and(|layer| !layer.is_empty()) {
        engine_fs::virtual_fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    } else if !engine_fs::virtual_fs::exists(root).unwrap_or(false) {
        return Ok(());
    }
    let provider = LocalFsProvider::with_root(root.to_path_buf())
        .with_context(|| format!("cannot access {}", root.display()))?;
    let path = layer_file(root, owner);
    match layer {
        Some(layer) if !layer.is_empty() => {
            let mut table = toml::Table::new();
            for (key, value) in layer {
                table.insert(key.clone(), crate::settings::config_value_to_toml(value));
            }
            let header = format!(
                "# PulsarConfig - {NS}/{owner}\n# Only the settings this project overrides are listed.\n\n"
            );
            let body = toml::to_string(&table).context("failed to serialize plugin settings")?;
            provider
                .write_file_atomically(&path, format!("{header}{body}").as_bytes())
                .with_context(|| format!("failed to write {}", path.display()))
        }
        _ => match provider.exists(&path) {
            Ok(true) => provider
                .delete_path(&path)
                .with_context(|| format!("failed to remove {}", path.display())),
            _ => Ok(()),
        },
    }
}

/// Write every owner whose overrides changed since the last save.
pub fn save() -> anyhow::Result<()> {
    let mut state = STATE.lock();
    let dirty = std::mem::take(&mut state.dirty);
    let Some(root) = state.project_root.clone() else {
        return Ok(());
    };
    let mut errors = Vec::new();
    for owner in dirty {
        if let Err(error) = write_layer(&root, &owner, state.project.get(&owner)) {
            errors.push(format!("{owner}: {error:#}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "failed to save plugin settings:\n{}",
            errors.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_config::{FieldType, NamespaceSchema, SchemaEntry, Validator};
    use pulsar_settings::plugin::{sync_with, PluginSettings};

    fn schema() -> NamespaceSchema {
        NamespaceSchema::new("Scoped", "project layer test")
            .setting(
                "level",
                SchemaEntry::new("Level", 1_i64)
                    .page("General")
                    .field_type(FieldType::NumberInput {
                        min: None,
                        max: None,
                        step: None,
                    })
                    .validator(Validator::int_range(0, 10)),
            )
            .setting("name", SchemaEntry::new("Name", "base"))
    }

    const SETTINGS: PluginSettings = PluginSettings {
        origin: "engine:scoped_test",
        id: "",
        schema,
    };

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pulsar_plugin_settings_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The tests share the process-wide database and layer, so they run as
    /// one sequence.
    #[test]
    fn the_project_overrides_the_default_and_persists() {
        let project_dir = temp("project");
        let owner = "engine.scoped_test";
        sync_with(global_config(), [&SETTINGS], |_| true);
        {
            let mut state = STATE.lock();
            *state = State::default();
        }
        assert!(
            set(owner, "level", ConfigValue::Int(4)).is_err(),
            "no project is open"
        );
        STATE.lock().project_root = Some(project_dir.join(".pulsar"));
        let level = || {
            global_config()
                .get(NS, owner, "level")
                .unwrap()
                .as_int()
                .unwrap()
        };
        assert_eq!(level(), 1);
        assert!(!is_overridden(owner, "level"));

        // A listener may call back into the layer (it runs with no lock held).
        let heard = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = heard.clone();
        let _listener = global_config()
            .owner_handle(NS, owner)
            .unwrap()
            .on_change("level", move |_| {
                let _ = is_overridden("engine.scoped_test", "level");
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .unwrap();

        set(owner, "level", ConfigValue::Int(4)).unwrap();
        assert_eq!(level(), 4);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(is_overridden(owner, "level"));

        assert!(set(owner, "level", ConfigValue::Int(99)).is_err());
        assert_eq!(level(), 4, "a rejected value changes nothing");

        save().unwrap();
        let project_file = layer_file(&project_dir.join(".pulsar"), owner);
        let table: toml::Table =
            toml::from_str(&std::fs::read_to_string(&project_file).unwrap()).unwrap();
        assert_eq!(table.len(), 1, "only the overridden key is written");
        assert_eq!(table["level"].as_integer(), Some(4));

        reset(owner, "level");
        assert_eq!(level(), 1, "reset falls back to the default");
        save().unwrap();
        assert!(!project_file.exists(), "an empty layer removes its file");

        // The project layer is readable headlessly.
        set(owner, "name", ConfigValue::String("team".into())).unwrap();
        save().unwrap();
        let headless = read_project_layer(&project_dir, owner).unwrap();
        assert_eq!(headless["name"].as_str(), Some("team"));
        let layer = typed_layer(&project_dir.join(".pulsar"), owner);
        assert_eq!(layer["name"], ConfigValue::String("team".into()));

        let _ = std::fs::remove_dir_all(project_dir);
    }

    /// A group a plugin took over keeps the values saved under its old owner
    /// path: the file moves, an existing new file is never overwritten.
    #[test]
    fn a_moved_group_keeps_its_saved_values() {
        use pulsar_settings::plugin::SettingsMove;
        let moved = SettingsMove {
            origin: "engine:moved_test",
            id: "server",
            from: "engine.old_home/server",
        };
        let root = temp("moved");
        let old = layer_file(&root, moved.from);
        let new = layer_file(
            &root,
            &pulsar_settings::plugin::owner_of(moved.origin, moved.id),
        );
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "path = \"C:/tools/server.exe\"\n").unwrap();

        apply_moves(&root, [&moved]);
        assert!(!old.exists(), "the old file moved");
        assert_eq!(
            std::fs::read_to_string(&new).unwrap(),
            "path = \"C:/tools/server.exe\"\n",
            "with its values"
        );

        // A second pass, or a new file already there, changes nothing.
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "path = \"other\"\n").unwrap();
        apply_moves(&root, [&moved]);
        assert!(
            old.exists(),
            "an existing new file is not overwritten, the old stays"
        );
        assert_eq!(
            std::fs::read_to_string(&new).unwrap(),
            "path = \"C:/tools/server.exe\"\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
