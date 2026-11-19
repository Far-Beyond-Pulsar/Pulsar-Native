//! Plugin settings: scopes and persistence.
//!
//! `pulsar_settings::plugin` registers every enabled plugin's groups in the
//! settings database (namespace `plugin`); this module gives each value two
//! scopes and keeps them on disk:
//!
//! | Scope | File |
//! |---|---|
//! | user | `<editor config dir>/plugin/<owner>.toml` |
//! | project | `<project>/.pulsar/plugin/<owner>.toml` |
//!
//! A file holds only the keys a person overrode in that scope. The value a
//! plugin reads is the user override, else the project override, else the
//! schema default (a person's own preference beats the team's default).
//! The database holds that effective value, so `PluginSettingsHandle`
//! listeners fire when either layer changes it.
//!
//! Headless tools (`pulsar package`, CI) read the project scope without the
//! editor through [`read_project_layer`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use engine_fs::{FsProvider as _, LocalFsProvider};
use parking_lot::Mutex;
use pulsar_settings::plugin::NS;

use crate::settings::{global_config, ConfigValue, GlobalSettings};

/// Where a person's override of a plugin setting lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingScope {
    /// The person's own editor configuration, for every project.
    User,
    /// The open project's `.pulsar` folder, shared with the project.
    Project,
}

type Layer = BTreeMap<String, BTreeMap<String, ConfigValue>>;

#[derive(Default)]
struct State {
    user: Layer,
    project: Layer,
    project_root: Option<PathBuf>,
    user_root: Option<PathBuf>,
    /// Owners whose layer changed since the last save, per scope.
    dirty_user: Vec<String>,
    dirty_project: Vec<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    user: BTreeMap::new(),
    project: BTreeMap::new(),
    project_root: None,
    user_root: None,
    dirty_user: Vec::new(),
    dirty_project: Vec::new(),
});

fn layer_file(root: &Path, owner: &str) -> PathBuf {
    let mut path = root.join(NS);
    let segments: Vec<&str> = owner.split('/').filter(|s| !s.is_empty()).collect();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        path.push(segment);
    }
    path.push(format!("{}.toml", segments.last().copied().unwrap_or("_root")));
    path
}

/// Read one scope's file for `owner` into raw TOML values (missing: empty).
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
    let text = String::from_utf8(bytes)
        .with_context(|| format!("{} is not UTF-8", path.display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

/// The project scope of plugin settings `owner` straight from the project's
/// files, with no editor and no database: the keys the project overrides, as
/// raw TOML. For `pulsar package`, CI and anything else that runs headless.
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

fn user_root() -> PathBuf {
    GlobalSettings::new().config_dir().to_path_buf()
}

/// Move the saved values of groups a plugin took over from another owner path
/// (`pulsar_settings::plugin::SettingsMove`) to the group's own: the old file
/// is renamed when it exists and the new one does not. Both scopes' roots are
/// passed (a root that does not exist has nothing to move).
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
/// value: the user override, else the project override, else the default.
/// Planned under the lock; [`commit`] applies it after the lock is released,
/// because applying fires change listeners, which may call back in here.
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
                .user
                .get(&owner)
                .and_then(|layer| layer.get(&info.key))
                .or_else(|| state.project.get(&owner).and_then(|layer| layer.get(&info.key)))
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
/// withdraw the disabled ones, reload both scopes from disk and apply the
/// effective values. Called when a project is set; call again after the
/// plugin set changes.
pub fn activate(project: Option<&Path>) {
    pulsar_settings::plugin::sync(global_config());
    let user = user_root();
    let project_dir = project.map(project_root_of);
    apply_moves(&user, pulsar_plugins::registered::<pulsar_settings::plugin::SettingsMove>());
    if let Some(root) = &project_dir {
        apply_moves(root, pulsar_plugins::registered::<pulsar_settings::plugin::SettingsMove>());
    }
    let mut state = STATE.lock();
    state.user.clear();
    state.project.clear();
    state.dirty_user.clear();
    state.dirty_project.clear();
    for owner in owners() {
        let user_layer = typed_layer(&user, &owner);
        if !user_layer.is_empty() {
            state.user.insert(owner.clone(), user_layer);
        }
        if let Some(root) = &project_dir {
            let project_layer = typed_layer(root, &owner);
            if !project_layer.is_empty() {
                state.project.insert(owner.clone(), project_layer);
            }
        }
    }
    state.user_root = Some(user);
    state.project_root = project_dir;
    let changes = plan_effective(&state);
    drop(state);
    commit(changes);
}

/// Re-register after the plugin set changed (without touching the open
/// project): newly enabled groups appear with their persisted values, the
/// groups of disabled plugins are withdrawn.
pub fn resync() {
    let project = STATE.lock().project_root.clone().and_then(|root| {
        root.parent().map(Path::to_path_buf)
    });
    activate(project.as_deref());
}

/// Whether a project scope exists (a project is open).
pub fn has_project_scope() -> bool {
    STATE.lock().project_root.is_some()
}

/// Which scope provides `owner.key`'s effective value: `None` when neither
/// overrides it (the default is in effect).
pub fn scope_of(owner: &str, key: &str) -> Option<SettingScope> {
    let state = STATE.lock();
    if state.user.get(owner).is_some_and(|layer| layer.contains_key(key)) {
        Some(SettingScope::User)
    } else if state.project.get(owner).is_some_and(|layer| layer.contains_key(key)) {
        Some(SettingScope::Project)
    } else {
        None
    }
}

/// Whether `scope` overrides `owner.key` (even when a higher scope hides it).
pub fn overrides(owner: &str, key: &str, scope: SettingScope) -> bool {
    let state = STATE.lock();
    let layer = match scope {
        SettingScope::User => &state.user,
        SettingScope::Project => &state.project,
    };
    layer.get(owner).is_some_and(|layer| layer.contains_key(key))
}

/// Override `owner.key` in `scope` (validated against the schema), update the
/// effective value and notify listeners. The change is on disk after
/// [`save`].
pub fn set(owner: &str, key: &str, scope: SettingScope, value: ConfigValue) -> anyhow::Result<()> {
    let handle = global_config()
        .owner_handle(NS, owner)
        .with_context(|| format!("plugin settings `{owner}` are not registered"))?;
    anyhow::ensure!(
        scope != SettingScope::Project || has_project_scope(),
        "no project is open"
    );
    // The schema decides what is legal, before any layer changes.
    handle.validate(key, &value)?;
    let mut guard = STATE.lock();
    let state = &mut *guard;
    let (layer, dirty) = match scope {
        SettingScope::User => (&mut state.user, &mut state.dirty_user),
        SettingScope::Project => (&mut state.project, &mut state.dirty_project),
    };
    layer
        .entry(owner.to_string())
        .or_default()
        .insert(key.to_string(), value);
    if !dirty.iter().any(|d| d == owner) {
        dirty.push(owner.to_string());
    }
    let changes = plan_effective(state);
    drop(guard);
    commit(changes);
    Ok(())
}

/// Remove `scope`'s override of `owner.key` (the setting falls back to the
/// other scope, then the default).
pub fn reset(owner: &str, key: &str, scope: SettingScope) {
    let mut guard = STATE.lock();
    let state = &mut *guard;
    let (layer, dirty) = match scope {
        SettingScope::User => (&mut state.user, &mut state.dirty_user),
        SettingScope::Project => (&mut state.project, &mut state.dirty_project),
    };
    let removed = layer
        .get_mut(owner)
        .map(|layer| layer.remove(key).is_some())
        .unwrap_or(false);
    if let Some(empty) = layer.get(owner).map(BTreeMap::is_empty) {
        if empty {
            layer.remove(owner);
        }
    }
    if removed && !dirty.iter().any(|d| d == owner) {
        dirty.push(owner.to_string());
    }
    let changes = plan_effective(state);
    drop(guard);
    commit(changes);
}

fn write_layer(root: &Path, owner: &str, layer: Option<&BTreeMap<String, ConfigValue>>) -> anyhow::Result<()> {
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
                "# PulsarConfig - {NS}/{owner}\n# Only the settings overridden in this scope are listed.\n\n"
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

/// Write every layer that changed since the last save. Returns the failures
/// (one line each) joined, so the caller can show them.
pub fn save() -> anyhow::Result<()> {
    let mut state = STATE.lock();
    let mut errors = Vec::new();
    let user_root = state.user_root.clone().unwrap_or_else(user_root);
    for owner in std::mem::take(&mut state.dirty_user) {
        if let Err(error) = write_layer(&user_root, &owner, state.user.get(&owner)) {
            errors.push(format!("{owner}: {error:#}"));
        }
    }
    let dirty_project = std::mem::take(&mut state.dirty_project);
    if let Some(root) = state.project_root.clone() {
        for owner in dirty_project {
            if let Err(error) = write_layer(&root, &owner, state.project.get(&owner)) {
                errors.push(format!("{owner}: {error:#}"));
            }
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
        NamespaceSchema::new("Scoped", "scope test")
            .setting(
                "level",
                SchemaEntry::new("Level", 1_i64)
                    .page("General")
                    .field_type(FieldType::NumberInput { min: None, max: None, step: None })
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

    /// The tests share the process-wide database and layers, so they run as
    /// one sequence.
    #[test]
    fn user_overrides_project_overrides_default_and_both_persist() {
        let user_dir = temp("user");
        let project_dir = temp("project");
        let owner = "engine.scoped_test";
        sync_with(global_config(), [&SETTINGS], |_| true);
        {
            let mut state = STATE.lock();
            *state = State::default();
            state.user_root = Some(user_dir.clone());
            state.project_root = Some(project_dir.join(".pulsar"));
        }
        let level = || {
            global_config()
                .get(NS, owner, "level")
                .unwrap()
                .as_int()
                .unwrap()
        };
        assert_eq!(level(), 1);
        assert_eq!(scope_of(owner, "level"), None);

        // A listener may call back into the layers (it runs with no lock held).
        let heard = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = heard.clone();
        let _listener = global_config()
            .owner_handle(NS, owner)
            .unwrap()
            .on_change("level", move |_| {
                let _ = scope_of("engine.scoped_test", "level");
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .unwrap();

        set(owner, "level", SettingScope::Project, ConfigValue::Int(4)).unwrap();
        assert_eq!(level(), 4);
        assert_eq!(heard.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(scope_of(owner, "level"), Some(SettingScope::Project));

        set(owner, "level", SettingScope::User, ConfigValue::Int(7)).unwrap();
        assert_eq!(level(), 7, "the user's value beats the project's");
        assert!(overrides(owner, "level", SettingScope::Project), "the project value is kept underneath");

        assert!(set(owner, "level", SettingScope::User, ConfigValue::Int(99)).is_err());
        assert_eq!(level(), 7, "a rejected value changes nothing");

        save().unwrap();
        let user_file = layer_file(&user_dir, owner);
        let project_file = layer_file(&project_dir.join(".pulsar"), owner);
        assert!(user_file.exists() && project_file.exists());
        let table: toml::Table = toml::from_str(&std::fs::read_to_string(&user_file).unwrap()).unwrap();
        assert_eq!(table.len(), 1, "only the overridden key is written");
        assert_eq!(table["level"].as_integer(), Some(7));

        reset(owner, "level", SettingScope::User);
        assert_eq!(level(), 4, "reset falls back to the project scope");
        reset(owner, "level", SettingScope::Project);
        assert_eq!(level(), 1, "and then to the default");
        save().unwrap();
        assert!(!user_file.exists() && !project_file.exists(), "an empty layer removes its file");

        // The project scope is readable headlessly.
        set(owner, "name", SettingScope::Project, ConfigValue::String("team".into())).unwrap();
        save().unwrap();
        let headless = read_project_layer(&project_dir, owner).unwrap();
        assert_eq!(headless["name"].as_str(), Some("team"));

        // Reloading from disk restores the layers.
        {
            let mut state = STATE.lock();
            state.user.clear();
            state.project.clear();
            state.user_root = Some(user_dir.clone());
            state.project_root = Some(project_dir.join(".pulsar"));
        }
        let user_layer = typed_layer(&user_dir, owner);
        let project_layer = typed_layer(&project_dir.join(".pulsar"), owner);
        assert!(user_layer.is_empty());
        assert_eq!(project_layer["name"], ConfigValue::String("team".into()));

        let _ = std::fs::remove_dir_all(user_dir);
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
        let new = layer_file(&root, &pulsar_settings::plugin::owner_of(moved.origin, moved.id));
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
        assert!(old.exists(), "an existing new file is not overwritten, the old stays");
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "path = \"C:/tools/server.exe\"\n");
        let _ = std::fs::remove_dir_all(root);
    }
}
