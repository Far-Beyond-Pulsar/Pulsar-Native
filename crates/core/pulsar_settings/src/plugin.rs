//! Plugin settings: what a plugin (or an engine crate that stands in for one)
//! lets a person configure, registered from its root `build`.
//!
//! ```ignore
//! p.add(PluginSettings { origin: p.origin(), id: "", schema: my_schema });
//! ```
//!
//! The host registers each contribution with the settings database under the
//! namespace [`NS`] (`plugin`), owner `[plugin id]` (`[plugin id, group id]`
//! for a plugin's second and later groups), while its plugin is enabled, and
//! withdraws it when the plugin is disabled ([`sync`]); the Project Settings
//! window lists what is registered under the plugin. A plugin reads and observes its
//! values through a [`PluginSettingsHandle`]; the database stays the host's
//! (a plugin never creates a second `ConfigManager`).
//!
//! Persistence (the project's overrides) belongs to `engine_state`, which
//! owns the files; this module is the registration and the read side.

use std::sync::{Arc, OnceLock};

use pulsar_config::{
    ChangeEvent, ConfigError, ConfigManager, ConfigValue, ListenerId, NamespaceSchema, OwnerHandle,
};
use pulsar_plugins::Owner;

/// The settings database namespace of every plugin setting.
pub const NS: &str = "plugin";

/// A settings group a plugin contributes. `schema` builds the schema afresh
/// each time the plugin is enabled (a schema holds closures and is consumed
/// by registration).
pub struct PluginSettings {
    /// The package that submitted it (`pulsar_plugins::origin!()`, or
    /// `engine_origin!()` for an engine crate): the group is registered only
    /// while that package's plugin is enabled.
    pub origin: &'static str,
    /// Names the group within its plugin: `""` for the plugin's main group,
    /// otherwise a stable key (`"rust-analyzer"`).
    pub id: &'static str,
    pub schema: fn() -> NamespaceSchema,
}

impl pulsar_plugins::Contribution for PluginSettings {
    const CATEGORY: &'static str = "plugin.settings";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        if self.id.is_empty() {
            "main".to_string()
        } else {
            self.id.to_string()
        }
    }

    fn label(&self) -> String {
        (self.schema)().display_name
    }
}

pulsar_plugins::registration!(PluginSettings);

/// A group whose saved values live under another owner path: the plugin took
/// over a group an engine crate used to register (the Rust plugin's server
/// group was `engine.pulsar_lsp/rust-analyzer`). `engine_state` moves the
/// saved project files from `from` to the group's own owner path the
/// first time it finds the old file and no new one, so no value is lost.
pub struct SettingsMove {
    /// The plugin that now owns the group (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// The group's id within the plugin (as [`PluginSettings::id`]).
    pub id: &'static str,
    /// The owner path the values were saved under before.
    pub from: &'static str,
}

impl pulsar_plugins::Contribution for SettingsMove {
    const CATEGORY: &'static str = "plugin.settings.move";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.from.to_string()
    }
}

pulsar_plugins::registration!(SettingsMove);

/// The settings page key of `origin`: the plugin's id, or `engine.<package>`
/// for an engine crate.
pub fn page_key(origin: &'static str) -> String {
    match pulsar_plugins::owner_of(origin) {
        Owner::Plugin(plugin) => plugin.id.clone(),
        Owner::Engine(package) => format!("engine.{package}"),
        Owner::Unknown(origin) => origin.to_string(),
    }
}

/// The owner path of the group `origin`/`id` in the database: `<page key>`
/// or `<page key>/<group id>`.
pub fn owner_of(origin: &'static str, id: &str) -> String {
    let page = page_key(origin);
    if id.is_empty() {
        page
    } else {
        format!("{page}/{id}")
    }
}

/// The owner path of a contribution in the database.
pub fn owner_path(settings: &PluginSettings) -> String {
    owner_of(settings.origin, settings.id)
}

/// What [`sync`] changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Owner paths registered by this pass.
    pub registered: Vec<String>,
    /// Owner paths withdrawn by this pass.
    pub withdrawn: Vec<String>,
}

impl SyncReport {
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty() && self.withdrawn.is_empty()
    }
}

/// A settings group derived at runtime by the host from another registration
/// (the standard group of every language server, from its language's
/// registration). Registered and withdrawn like a [`PluginSettings`] of the
/// same `origin`.
pub struct SettingsGroup {
    pub origin: &'static str,
    pub id: &'static str,
    pub schema: Arc<dyn Fn() -> NamespaceSchema + Send + Sync>,
}

/// Where derived groups come from: a registration whose `groups` lists them
/// (called on every [`sync`], over every registration whatever its plugin's
/// state, so a disabled plugin's are withdrawn).
pub struct SettingsGroups {
    /// The package that submitted it (`engine_origin!()`).
    pub origin: &'static str,
    /// Names the source (`"language.servers"`).
    pub id: &'static str,
    pub groups: fn() -> Vec<SettingsGroup>,
}

impl pulsar_plugins::Contribution for SettingsGroups {
    const CATEGORY: &'static str = "plugin.settings.groups";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }
}

pulsar_plugins::registration!(SettingsGroups);

/// Bring the database in line with the plugins that are enabled now: register
/// the groups of enabled plugins that are missing, withdraw those of disabled
/// ones. Call after the plugin set changes (`pulsar_plugins::generation`).
pub fn sync(cfg: &ConfigManager) -> SyncReport {
    let mut groups: Vec<SettingsGroup> = pulsar_plugins::all_registered::<PluginSettings>()
        .map(SettingsGroup::from)
        .collect();
    groups.extend(
        pulsar_plugins::all_registered::<SettingsGroups>().flat_map(|source| (source.groups)()),
    );
    sync_groups(cfg, groups, pulsar_plugins::origin_enabled)
}

impl From<&PluginSettings> for SettingsGroup {
    fn from(settings: &PluginSettings) -> Self {
        let schema = settings.schema;
        Self {
            origin: settings.origin,
            id: settings.id,
            schema: Arc::new(move || schema()),
        }
    }
}

/// [`sync`] over an explicit set of contributions and a gate (what the tests
/// drive).
pub fn sync_with<'a>(
    cfg: &ConfigManager,
    contributions: impl IntoIterator<Item = &'a PluginSettings>,
    enabled: impl Fn(&str) -> bool,
) -> SyncReport {
    sync_groups(
        cfg,
        contributions.into_iter().map(SettingsGroup::from),
        enabled,
    )
}

/// [`sync`] over explicit groups and a gate.
pub fn sync_groups(
    cfg: &ConfigManager,
    groups: impl IntoIterator<Item = SettingsGroup>,
    enabled: impl Fn(&str) -> bool,
) -> SyncReport {
    let mut report = SyncReport::default();
    let mut seen = std::collections::HashSet::new();
    for group in groups {
        let owner = owner_of(group.origin, group.id);
        if !seen.insert(owner.clone()) {
            tracing::error!(
                "plugin settings `{owner}` are contributed twice; the second is ignored"
            );
            continue;
        }
        let registered = cfg.owner_handle(NS, &owner).is_some();
        match (enabled(group.origin), registered) {
            (true, false) => match cfg.register(NS, &owner, (group.schema)()) {
                Ok(_) => report.registered.push(owner),
                Err(error) => tracing::error!("plugin settings `{owner}`: {error}"),
            },
            (false, true) => {
                cfg.unregister(NS, &owner);
                report.withdrawn.push(owner);
            }
            _ => {}
        }
    }
    report
}

/// The title of the Settings page of `page_key` (see [`page_key`]): the
/// plugin's name, or, for an engine crate, the display name of its first
/// group.
pub fn page_title(cfg: &ConfigManager, page_key: &str) -> String {
    if let Some(plugin) = pulsar_plugins::descriptor(page_key) {
        return plugin.name.clone();
    }
    let mut owners: Vec<Vec<String>> = cfg
        .list_owners(NS)
        .into_iter()
        .filter(|owner| owner.first().map(String::as_str) == Some(page_key))
        .collect();
    owners.sort();
    owners
        .first()
        .and_then(|owner| {
            cfg.list_all_owners()
                .into_iter()
                .find(|(ns, segments)| ns == NS && segments == owner)
        })
        .and_then(|(ns, segments)| {
            cfg.search("")
                .into_iter()
                .find(|result| result.namespace == ns && result.owner == segments)
                .map(|result| result.owner_display_name)
        })
        .unwrap_or_else(|| page_key.to_string())
}

static MANAGER: OnceLock<&'static ConfigManager> = OnceLock::new();

/// Remember the host's database (`register_all_settings` does): handles and
/// [`handle`] resolve against it.
pub(crate) fn bind(cfg: &'static ConfigManager) {
    let _ = MANAGER.set(cfg);
}

/// The database this copy of the crate is bound to: the host's, in the host and in
/// every plugin library it attached ([`attach_exported`]); `None` before either.
pub fn bound() -> Option<&'static ConfigManager> {
    MANAGER.get().copied()
}

/// The entry point a plugin library exports for [`attach_exported`]
/// (`export_settings_attach!`, which `plugin_editor_api::export_plugin!`
/// invokes).
pub const ATTACH_SYMBOL: &str = "_plugin_attach_settings";

/// Bumped on any change to [`HostSettings`] or to how a copy reads it.
const ATTACH_VERSION: u64 = 1;

/// The layout this copy of the crate was built with: a library built from
/// other `pulsar-config` sources refuses the host's database.
const fn layout() -> u64 {
    let parts = [
        ATTACH_VERSION as usize,
        size_of::<HostSettings>(),
        size_of::<ConfigManager>(),
        size_of::<OwnerHandle>(),
        size_of::<ConfigValue>(),
    ];
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < parts.len() {
        hash ^= parts[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

/// What the host hands a plugin library: its settings database, with the
/// layout it was built with.
#[repr(C)]
pub struct HostSettings {
    layout: u64,
    manager: *const ConfigManager,
}

// SAFETY: `manager` points to the process's `ConfigManager` (`MANAGER` holds
// it as a `&'static`, so it is Sync), and is never written through.
unsafe impl Send for HostSettings {}
// SAFETY: as above.
unsafe impl Sync for HostSettings {}

/// The host's settings database for a plugin library, or null before
/// [`register_all_settings`](crate::register_all_settings) ran. It lives for
/// the process (plugins are never unloaded).
pub fn export_host_settings() -> *const std::ffi::c_void {
    static EXPORTED: OnceLock<HostSettings> = OnceLock::new();
    let Some(cfg) = MANAGER.get() else {
        return std::ptr::null();
    };
    let exported = EXPORTED.get_or_init(|| HostSettings {
        layout: layout(),
        manager: *cfg,
    });
    (exported as *const HostSettings).cast()
}

/// Bind this copy of the crate (a plugin library's) to the host's settings
/// database, so [`handle`] answers inside the library (#1932). `false` when
/// `host` is null or was built with another layout.
///
/// # Safety
///
/// `host` is null or a pointer from the host's [`export_host_settings`],
/// alive for the process.
#[doc(hidden)]
pub unsafe fn attach_exported(host: *const std::ffi::c_void) -> bool {
    // SAFETY: the caller's contract.
    let Some(host) = (unsafe { host.cast::<HostSettings>().as_ref() }) else {
        return false;
    };
    if host.layout != layout() || host.manager.is_null() {
        tracing::error!("refused the host's settings: built with another layout");
        return false;
    }
    // SAFETY: the host's database lives for the process, and the layout
    // matches this copy's.
    bind(unsafe { &*host.manager });
    true
}

/// Export [`ATTACH_SYMBOL`] from a plugin library, so the editor's plugin
/// manager binds the library's settings handles to the editor's database
/// before the plugin runs any code. `plugin_editor_api::export_plugin!`
/// invokes it.
#[macro_export]
macro_rules! export_settings_attach {
    () => {
        /// Bind this library's plugin settings to the host's database.
        /// Called by the plugin manager before `_plugin_create`.
        ///
        /// # Safety
        ///
        /// `host` is the host's exported settings, alive for the process.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_attach_settings(host: *const ::std::ffi::c_void) -> bool {
            // A panic must not unwind across the FFI boundary: the host reads
            // it as a refusal.
            ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: the caller's contract.
                unsafe { $crate::plugin::attach_exported(host) }
            }))
            .unwrap_or(false)
        }
    };
}

/// A plugin's read access to its own settings, plus change notification.
/// Cheap to clone. A value is `None` while the group is withdrawn (its plugin
/// is disabled).
#[derive(Clone)]
pub struct PluginSettingsHandle {
    owner: OwnerHandle,
}

/// Keeps a change listener alive; dropping it unsubscribes.
pub struct Subscription(#[allow(dead_code)] ListenerId);

/// The handle of the group `origin`/`id` contributes, or `None` while it is
/// not registered (the plugin is disabled, or [`sync`] has not run).
pub fn handle(origin: &'static str, id: &str) -> Option<PluginSettingsHandle> {
    let cfg = MANAGER.get()?;
    cfg.owner_handle(NS, &owner_of(origin, id))
        .map(|owner| PluginSettingsHandle { owner })
}

impl PluginSettingsHandle {
    pub fn value(&self, key: &str) -> Option<ConfigValue> {
        self.owner.get(key).ok()
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.owner.get_bool(key).ok()
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.owner.get_int(key).ok()
    }

    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.owner.get_float(key).ok()
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.owner.get_string(key).ok()
    }

    /// Call `callback` when `key` changes (a person edits it, or a scope
    /// layer changes its effective value). It runs on the thread that made
    /// the change, with no database lock held.
    pub fn on_change(
        &self,
        key: &str,
        callback: impl Fn(&ChangeEvent) + Send + Sync + 'static,
    ) -> Result<Subscription, ConfigError> {
        self.owner.on_change(key, callback).map(Subscription)
    }

    /// Call `callback` when any key of the group changes.
    pub fn on_any_change(
        &self,
        callback: impl Fn(&ChangeEvent) + Send + Sync + 'static,
    ) -> Subscription {
        Subscription(self.owner.on_any_change(callback))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_config::{FieldType, SchemaEntry};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn demo_schema() -> NamespaceSchema {
        NamespaceSchema::new("Demo", "A demo plugin").setting(
            "verbose",
            SchemaEntry::new("Verbose", false)
                .page("General")
                .field_type(FieldType::Checkbox),
        )
    }

    fn other_schema() -> NamespaceSchema {
        NamespaceSchema::new("Other", "Second group").setting("n", SchemaEntry::new("N", 3_i64))
    }

    const MAIN: PluginSettings = PluginSettings {
        origin: "engine:demo",
        id: "",
        schema: demo_schema,
    };
    const SECOND: PluginSettings = PluginSettings {
        origin: "engine:demo",
        id: "second",
        schema: other_schema,
    };
    const GONE: PluginSettings = PluginSettings {
        origin: "engine:gone",
        id: "",
        schema: demo_schema,
    };

    #[test]
    fn an_enabled_plugins_groups_register_under_its_page() {
        let cfg = ConfigManager::new();
        let report = sync_with(&cfg, [&MAIN, &SECOND], |_| true);
        assert_eq!(report.registered, ["engine.demo", "engine.demo/second"]);
        assert!(cfg.owner_handle(NS, "engine.demo").is_some());
        assert_eq!(
            cfg.get(NS, "engine.demo/second", "n").unwrap(),
            ConfigValue::Int(3)
        );
        assert!(
            sync_with(&cfg, [&MAIN, &SECOND], |_| true).is_empty(),
            "idempotent"
        );
    }

    #[test]
    fn disabling_a_plugin_withdraws_its_settings_and_enabling_restores_them() {
        let cfg = ConfigManager::new();
        sync_with(&cfg, [&MAIN, &GONE], |_| true);
        let report = sync_with(&cfg, [&MAIN, &GONE], |origin| origin != "engine:gone");
        assert_eq!(report.withdrawn, ["engine.gone"]);
        assert!(cfg.owner_handle(NS, "engine.gone").is_none());
        assert!(
            cfg.owner_handle(NS, "engine.demo").is_some(),
            "the other plugin is untouched"
        );
        let report = sync_with(&cfg, [&MAIN, &GONE], |_| true);
        assert_eq!(report.registered, ["engine.gone"]);
    }

    #[test]
    fn change_listeners_survive_a_disable_and_hear_later_changes() {
        let cfg = ConfigManager::new();
        sync_with(&cfg, [&MAIN], |_| true);
        let handle = PluginSettingsHandle {
            owner: cfg.owner_handle(NS, "engine.demo").unwrap(),
        };
        let heard = Arc::new(AtomicUsize::new(0));
        let counter = heard.clone();
        let _subscription = handle
            .on_change("verbose", move |event| {
                assert_eq!(event.new_value, ConfigValue::Bool(true));
                counter.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();
        assert_eq!(handle.get_bool("verbose"), Some(false));
        sync_with(&cfg, [&MAIN], |_| false);
        assert_eq!(handle.get_bool("verbose"), None, "withdrawn: no value");
        sync_with(&cfg, [&MAIN], |_| true);
        handle.owner.set("verbose", true).unwrap();
        assert_eq!(heard.load(Ordering::Relaxed), 1);
        assert_eq!(handle.get_bool("verbose"), Some(true));
    }

    #[test]
    fn the_same_group_contributed_twice_registers_once() {
        let cfg = ConfigManager::new();
        let report = sync_with(&cfg, [&MAIN, &MAIN], |_| true);
        assert_eq!(report.registered, ["engine.demo"]);
    }

    #[test]
    fn an_engine_page_is_titled_by_its_first_group() {
        let cfg = ConfigManager::new();
        sync_with(&cfg, [&MAIN, &SECOND], |_| true);
        assert_eq!(page_title(&cfg, "engine.demo"), "Demo");
    }
}

#[cfg(test)]
mod derived_tests {
    use super::*;
    use pulsar_config::SchemaEntry;

    fn derived(origin: &'static str, id: &'static str) -> SettingsGroup {
        SettingsGroup {
            origin,
            id,
            schema: Arc::new(|| {
                NamespaceSchema::new("Derived", "a derived group")
                    .setting("n", SchemaEntry::new("N", 1_i64))
            }),
        }
    }

    #[test]
    fn a_derived_group_follows_its_origins_plugin() {
        let cfg = ConfigManager::new();
        let report = sync_groups(&cfg, [derived("engine:lang", "server")], |_| true);
        assert_eq!(report.registered, ["engine.lang/server"]);
        let report = sync_groups(&cfg, [derived("engine:lang", "server")], |_| false);
        assert_eq!(report.withdrawn, ["engine.lang/server"]);
        assert!(cfg.owner_handle(NS, "engine.lang/server").is_none());
    }
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    // Only the refusals: a successful attach binds this process's MANAGER,
    // which the other tests must not see.
    #[test]
    fn a_null_host_is_refused() {
        // SAFETY: null is allowed by the contract.
        assert!(!unsafe { attach_exported(std::ptr::null()) });
    }

    #[test]
    fn a_host_of_another_layout_is_refused() {
        let cfg: &'static ConfigManager = Box::leak(Box::new(ConfigManager::new()));
        let host = HostSettings {
            layout: layout() ^ 1,
            manager: cfg,
        };
        // SAFETY: `host` and the database it names outlive the call.
        assert!(!unsafe { attach_exported((&host as *const HostSettings).cast()) });
        assert!(MANAGER.get().is_none_or(|bound| !std::ptr::eq(*bound, cfg)));
    }
}
