//! Which plugins a project has.
//!
//! Every object kind is a plugin (`docs/architecture/plugin-migration.md` in
//! the Helio repo). A plugin that a project does not enable is **absent**:
//! its components are not registered, its Helio features are not in the
//! render graph, its settings, editors and panels do not appear, and a game
//! built for the project does not link it.
//!
//! - A plugin describes itself in `pulsar-plugin.toml` beside its crates
//!   ([`PluginDescriptor`]: id, default, the plugins it requires, and its
//!   modules: the crates a game links and the crates the editor links). Its
//!   runtime module carries the descriptor into whatever links it
//!   ([`installed!`]), so the plugins a binary links are the plugins it has
//!   installed ([`installed`]).
//! - A project lists the plugins it enables in its `Pulsar.toml`
//!   ([`ProjectPlugins`]):
//!
//!   ```toml
//!   [plugins]
//!   "pulsar.voxel" = false
//!   "pulsar.lights" = true
//!   ```
//!
//!   A plugin the list does not name keeps its descriptor's default.
//! - [`resolve`] turns both into the enabled set: what the project enables,
//!   plus what those require; a plugin whose requirement is disabled or not
//!   installed is disabled too, with a problem saying why.
//! - The editor activates the set when it opens a project
//!   ([`activate_project`], from `engine_state::set_project_path`). Every
//!   registration a plugin submits carries its **origin**, the package that
//!   submitted it ([`origin!`]); the registries skip a registration whose
//!   origin is a disabled plugin's module ([`origin_enabled`]). Nothing
//!   activated (a game, which links only its project's plugins, or a test)
//!   means every linked plugin is enabled.
//! - A generated game links the enabled plugins' runtime modules and nothing
//!   else (`engine_backend::services::core_project_builder`).
//!
//! How others do it: Unreal's `.uplugin` descriptors (modules by type,
//! `EnabledByDefault`, plugin dependencies) and the `.uproject`'s `Plugins`
//! list; the editor loads only enabled plugins' modules and a packaged game
//! links only theirs. Bevy's plugin groups (`DefaultPlugins.build()
//! .disable::<T>()`) are chosen in code, per binary. Ours is Unreal's model
//! with the list in data: one editor binary serves every project, and the
//! project file, not code, says which kinds a project has.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

pub use inventory;

/// The file a plugin describes itself in, at its root
/// (`plugins/vendor/<kind>/pulsar-plugin.toml`).
pub const DESCRIPTOR_FILE: &str = "pulsar-plugin.toml";

/// The project file whose `[plugins]` table lists the enabled plugins.
pub const PROJECT_FILE: &str = "Pulsar.toml";

/// A plugin's descriptor (`pulsar-plugin.toml`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDescriptor {
    /// The plugin's id (`pulsar.voxel`): what a project's list names.
    pub id: String,
    /// Its display name.
    pub name: String,
    /// What it brings.
    #[serde(default)]
    pub description: String,
    /// Whether a project that does not name it has it.
    #[serde(default = "enabled_by_default")]
    pub enabled_by_default: bool,
    /// The plugins it cannot work without (its modules link theirs).
    #[serde(default)]
    pub requires: Vec<String>,
    /// Its crates, by package name.
    pub modules: Modules,
}

fn enabled_by_default() -> bool {
    true
}

/// A plugin's crates, by package name.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modules {
    /// What a game links (components, runtime, the Helio feature's
    /// registration). The first carries the descriptor ([`installed!`]).
    pub runtime: Vec<String>,
    /// What the editor links in addition (the editor plugin).
    #[serde(default)]
    pub editor: Vec<String>,
}

impl PluginDescriptor {
    /// Parse a descriptor.
    pub fn parse(source: &str) -> Result<Self, String> {
        let descriptor: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        if descriptor.id.trim().is_empty() {
            return Err("a plugin descriptor needs an `id`".into());
        }
        if descriptor.modules.runtime.is_empty() {
            return Err(format!(
                "plugin `{}` lists no runtime module (`[modules] runtime`)",
                descriptor.id
            ));
        }
        Ok(descriptor)
    }

    /// Every module, runtime then editor.
    pub fn modules(&self) -> impl Iterator<Item = &str> {
        self.modules
            .runtime
            .iter()
            .chain(&self.modules.editor)
            .map(String::as_str)
    }

    /// Whether `package` is one of its modules.
    pub fn owns(&self, package: &str) -> bool {
        self.modules().any(|module| same_package(module, package))
    }
}

/// Cargo treats `-` and `_` in package names alike.
fn same_package(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x == y || (x == b'-' || x == b'_') && (y == b'-' || y == b'_'))
}

/// A plugin's descriptor, linked with its runtime module ([`installed!`]).
pub struct InstalledPlugin {
    /// The descriptor's source.
    pub descriptor: &'static str,
}

inventory::collect!(InstalledPlugin);

/// Carry this plugin's descriptor (at `path`, relative to the calling
/// crate's manifest directory) into whatever links the crate: the plugin is
/// then installed there. Its first runtime module calls it once, at its
/// root; the crate then has `PLUGIN_DESCRIPTOR` (the source) and
/// `plugin_descriptor()` (parsed), which its editor module reads its name
/// and description from.
#[macro_export]
macro_rules! installed {
    ($path:literal) => {
        /// This plugin's descriptor (`pulsar-plugin.toml`).
        pub const PLUGIN_DESCRIPTOR: &str =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path));

        /// This plugin's descriptor, parsed.
        pub fn plugin_descriptor() -> $crate::PluginDescriptor {
            $crate::PluginDescriptor::parse(PLUGIN_DESCRIPTOR)
                .expect("the plugin's pulsar-plugin.toml parses")
        }

        $crate::inventory::submit! {
            $crate::InstalledPlugin {
                descriptor: PLUGIN_DESCRIPTOR,
            }
        }
    };
}

/// The origin of a registration: the package that submits it. Every
/// registration a plugin's crate submits names it, so the registries can
/// skip a disabled plugin's ([`origin_enabled`]).
#[macro_export]
macro_rules! origin {
    () => {
        env!("CARGO_PKG_NAME")
    };
}

/// A registration a plugin's crate can submit (`inventory`): it names its
/// origin ([`origin!`]), so its registry can skip it while its plugin is
/// disabled ([`registered`]).
pub trait Registration: inventory::Collect {
    /// The package that submitted it.
    fn origin(&self) -> &'static str;
}

/// The linked registrations of type `T` that count: those whose origin's
/// plugin is enabled ([`origin_enabled`]). What every registry iterates in
/// place of `inventory::iter`.
pub fn registered<T: Registration>() -> impl Iterator<Item = &'static T> {
    inventory::iter::<T>
        .into_iter()
        .filter(|registration| origin_enabled(registration.origin()))
}

/// The plugins this binary links, by id. A malformed descriptor is logged
/// and skipped (a test of each plugin parses its own).
pub fn installed() -> &'static [PluginDescriptor] {
    static INSTALLED: OnceLock<Vec<PluginDescriptor>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let mut plugins: BTreeMap<String, PluginDescriptor> = BTreeMap::new();
        for linked in inventory::iter::<InstalledPlugin> {
            match PluginDescriptor::parse(linked.descriptor) {
                Ok(descriptor) => {
                    plugins.insert(descriptor.id.clone(), descriptor);
                }
                Err(error) => tracing::error!("skipping a plugin descriptor: {error}"),
            }
        }
        plugins.into_values().collect()
    })
}

/// A project's `[plugins]` list: the plugins it names, enabled or not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPlugins {
    pub listed: BTreeMap<String, bool>,
}

impl ProjectPlugins {
    /// The list in a `Pulsar.toml`'s source (empty without a `[plugins]`
    /// table).
    pub fn parse(project_file: &str) -> Result<Self, String> {
        let value: toml::Table = toml::from_str(project_file).map_err(|error| error.to_string())?;
        let Some(table) = value.get("plugins") else {
            return Ok(Self::default());
        };
        let table = table
            .as_table()
            .ok_or("`plugins` in Pulsar.toml must be a table of plugin ids")?;
        let mut listed = BTreeMap::new();
        for (id, enabled) in table {
            let enabled = enabled
                .as_bool()
                .ok_or_else(|| format!("plugin `{id}` in Pulsar.toml must be true or false"))?;
            listed.insert(id.clone(), enabled);
        }
        Ok(Self { listed })
    }

    /// The list of the project at `project_root` (empty without a
    /// `Pulsar.toml`).
    pub fn read(project_root: &Path) -> Result<Self, String> {
        let path = project_root.join(PROJECT_FILE);
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                Self::parse(&source).map_err(|error| format!("{}: {error}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    /// A list naming each of `ids` with `enabled`.
    pub fn with(mut self, ids: &[&str], enabled: bool) -> Self {
        for id in ids {
            self.listed.insert((*id).to_string(), enabled);
        }
        self
    }
}

/// The plugins a project has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Enabled plugin ids.
    pub enabled: BTreeSet<String>,
    /// What did not go as the list says (a plugin it names that is not
    /// installed, one disabled because a requirement is off).
    pub problems: Vec<String>,
}

impl Resolution {
    /// Whether plugin `id` is enabled.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.contains(id)
    }

    /// The runtime modules of the enabled plugins, in `installed`'s order:
    /// what a game built for the project links.
    pub fn runtime_modules<'a>(&self, installed: &'a [PluginDescriptor]) -> Vec<&'a str> {
        installed
            .iter()
            .filter(|plugin| self.is_enabled(&plugin.id))
            .flat_map(|plugin| plugin.modules.runtime.iter().map(String::as_str))
            .collect()
    }
}

/// The plugins `project` has among `installed`. A plugin is enabled when the
/// list says so, or by its default when the list does not name it; the
/// plugins an enabled plugin requires are enabled with it (unless the list
/// turns one off); a plugin whose requirement is off or not installed is
/// disabled, and the resolution says why.
pub fn resolve(installed: &[PluginDescriptor], project: &ProjectPlugins) -> Resolution {
    let by_id: BTreeMap<&str, &PluginDescriptor> = installed
        .iter()
        .map(|plugin| (plugin.id.as_str(), plugin))
        .collect();
    let mut problems = Vec::new();
    for (id, enabled) in &project.listed {
        if *enabled && !by_id.contains_key(id.as_str()) {
            problems.push(format!(
                "plugin `{id}` is enabled by the project but not installed"
            ));
        }
    }
    let listed_off = |id: &str| project.listed.get(id) == Some(&false);
    let mut enabled: BTreeSet<String> = installed
        .iter()
        .filter(|plugin| {
            project
                .listed
                .get(&plugin.id)
                .copied()
                .unwrap_or(plugin.enabled_by_default)
        })
        .map(|plugin| plugin.id.clone())
        .collect();
    // Pull in requirements until stable, then drop what cannot have them
    // (never pulling a dropped plugin in again), until nothing drops.
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    loop {
        loop {
            let mut pulled = false;
            for plugin in installed {
                if !enabled.contains(&plugin.id) {
                    continue;
                }
                for required in &plugin.requires {
                    if by_id.contains_key(required.as_str())
                        && !listed_off(required)
                        && !dropped.contains(required)
                        && enabled.insert(required.clone())
                    {
                        pulled = true;
                    }
                }
            }
            if !pulled {
                break;
            }
        }
        let mut dropped_any = false;
        for plugin in installed {
            if !enabled.contains(&plugin.id) {
                continue;
            }
            let missing = plugin.requires.iter().find(|required| {
                !by_id.contains_key(required.as_str()) || !enabled.contains(required.as_str())
            });
            if let Some(required) = missing {
                let why = if !by_id.contains_key(required.as_str()) {
                    "which is not installed"
                } else if listed_off(required) {
                    "which the project disables"
                } else {
                    "which is disabled"
                };
                problems.push(format!(
                    "plugin `{}` is disabled: it requires `{required}`, {why}",
                    plugin.id
                ));
                enabled.remove(&plugin.id);
                dropped.insert(plugin.id.clone());
                dropped_any = true;
            }
        }
        if !dropped_any {
            break;
        }
    }
    Resolution { enabled, problems }
}

/// The activated set and the module -> plugin map the gate reads.
struct Active {
    enabled: BTreeSet<String>,
    /// Every installed plugin's modules, to its id.
    module_plugin: HashMap<String, String>,
}

static ACTIVE: RwLock<Option<Active>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn normalized(package: &str) -> String {
    package.replace('-', "_")
}

/// Make `resolution` the process's plugin set: from now on the registries
/// skip the registrations of every installed plugin it does not enable.
pub fn activate(resolution: &Resolution) {
    let module_plugin = installed()
        .iter()
        .flat_map(|plugin| {
            plugin
                .modules()
                .map(|module| (normalized(module), plugin.id.clone()))
        })
        .collect();
    *ACTIVE.write().unwrap_or_else(|poison| poison.into_inner()) = Some(Active {
        enabled: resolution.enabled.clone(),
        module_plugin,
    });
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Back to no activated set: every linked plugin enabled.
pub fn deactivate() {
    *ACTIVE.write().unwrap_or_else(|poison| poison.into_inner()) = None;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Read the project at `project_root`'s list, resolve it against the
/// installed plugins and activate the result (problems are logged and
/// returned). An unreadable list activates every installed plugin's
/// default.
pub fn activate_project(project_root: &Path) -> Resolution {
    let project = ProjectPlugins::read(project_root).unwrap_or_else(|error| {
        tracing::error!("the project's plugin list: {error}; using every plugin's default");
        ProjectPlugins::default()
    });
    let resolution = resolve(installed(), &project);
    for problem in &resolution.problems {
        tracing::warn!("{problem}");
    }
    tracing::info!(
        enabled = ?resolution.enabled,
        "plugins of {}",
        project_root.display()
    );
    activate(&resolution);
    resolution
}

/// Changes on every [`activate`]/[`deactivate`]: a registry caching what
/// passes the gate rebuilds when it changes.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Whether plugin `id` is enabled: installed and in the activated set (or
/// installed, with nothing activated).
pub fn is_enabled(id: &str) -> bool {
    let active = ACTIVE.read().unwrap_or_else(|poison| poison.into_inner());
    match active.as_ref() {
        Some(active) => active.enabled.contains(id),
        None => installed().iter().any(|plugin| plugin.id == id),
    }
}

/// Whether a registration from package `origin` ([`origin!`]) counts: its
/// plugin is enabled, or it belongs to no plugin (the engine's own), or
/// nothing is activated.
pub fn origin_enabled(origin: &str) -> bool {
    let active = ACTIVE.read().unwrap_or_else(|poison| poison.into_inner());
    let Some(active) = active.as_ref() else {
        return true;
    };
    match active.module_plugin.get(&normalized(origin)) {
        Some(plugin) => active.enabled.contains(plugin),
        None => true,
    }
}

/// The plugin package `origin` belongs to, if any.
pub fn plugin_of(origin: &str) -> Option<&'static PluginDescriptor> {
    installed().iter().find(|plugin| plugin.owns(origin))
}

#[cfg(test)]
mod tests;
