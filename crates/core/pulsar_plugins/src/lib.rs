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
//!   **root crate**). The root crate is the package at the plugin's
//!   directory and the one place the plugin registers everything it
//!   contributes: it imports the plugin's own crates (`api`, `component`,
//!   `feature`, `editor`) and submits their registrations, runtime ones
//!   always and editor ones behind its `editor` feature, so a game links the
//!   root without editor dependencies. The root also carries the descriptor
//!   into whatever links it ([`installed!`]), so the plugins a binary links
//!   are the plugins it has installed ([`installed`]). Other plugins depend
//!   only on a plugin's `api` crate; no crate depends on a root but the
//!   installed set (`plugins/installed`) and a generated game.
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
//!   means every linked plugin is enabled. A plugin's root is its only
//!   submitter, so a registration's origin is its plugin's root.
//! - Every registration type summarizes itself ([`Contribution`]: stable id,
//!   label, category, origin) and declares itself with [`registration!`],
//!   which also lists it as a [`ContributionSource`]: what a plugin
//!   contributes is read from its real registrations ([`contributions`]),
//!   never from a second hand-written list. The host's catalog
//!   (`plugin_manager::catalog`) joins them with the descriptors.
//! - A generated game links the enabled plugins' roots, without their
//!   `editor` feature, and nothing else
//!   (`engine_backend::services::core_project_builder`).
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
    /// The plugins it cannot work without (its crates link theirs).
    #[serde(default)]
    pub requires: Vec<String>,
    /// Its root crate, by package name: the package at the plugin's
    /// directory, which registers everything the plugin contributes and is
    /// the origin of every registration ([`origin!`]).
    pub root: String,
    /// An editor tool (an asset editor): a game never links it, and its root
    /// has no `editor` feature (it is all editor). Otherwise a game links
    /// the root without its `editor` feature, and the editor with it.
    #[serde(default)]
    pub editor_only: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl PluginDescriptor {
    /// Parse a descriptor.
    pub fn parse(source: &str) -> Result<Self, String> {
        let descriptor: Self = toml::from_str(source).map_err(|error| error.to_string())?;
        if descriptor.id.trim().is_empty() {
            return Err("a plugin descriptor needs an `id`".into());
        }
        if descriptor.root.trim().is_empty() {
            return Err(format!(
                "plugin `{}` names no root crate (`root`)",
                descriptor.id
            ));
        }
        Ok(descriptor)
    }

    /// Whether `package` is its root crate: the package its registrations
    /// come from.
    pub fn owns(&self, package: &str) -> bool {
        same_package(&self.root, package)
    }

    /// Whether a game links it (its root, without the `editor` feature).
    pub fn runtime(&self) -> bool {
        !self.editor_only
    }
}

/// Cargo treats `-` and `_` in package names alike.
fn same_package(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x == y || (x == b'-' || x == b'_') && (y == b'-' || y == b'_'))
}

/// A plugin's descriptor, linked with its root crate ([`installed!`]).
pub struct InstalledPlugin {
    /// The descriptor's source.
    pub descriptor: &'static str,
}

inventory::collect!(InstalledPlugin);

/// Carry this plugin's descriptor (at `path`, relative to the calling
/// crate's manifest directory: `"pulsar-plugin.toml"` from the root crate)
/// into whatever links the crate: the plugin is then installed there. The
/// plugin's root crate calls it once, at its root; the crate then has
/// `PLUGIN_DESCRIPTOR` (the source) and `plugin_descriptor()` (parsed).
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

/// The origin of a registration: the package that submits it, a plugin's
/// root crate. Every registration names it, so the registries can skip a
/// disabled plugin's ([`origin_enabled`]).
#[macro_export]
macro_rules! origin {
    () => {
        env!("CARGO_PKG_NAME")
    };
}

/// The prefix of an engine origin ([`engine_origin!`]).
pub const ENGINE_ORIGIN_PREFIX: &str = "engine:";

/// The origin of a registration the engine itself submits (one of its own
/// crates, not a plugin's): `engine:<package>`. It always counts, and the
/// catalog lists it as the engine's. Any other origin that is no installed
/// plugin's root is unknown, a reported problem ([`Owner::Unknown`]), never
/// taken for the engine's.
#[macro_export]
macro_rules! engine_origin {
    () => {
        concat!("engine:", env!("CARGO_PKG_NAME"))
    };
}

/// Who a registration's origin says it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// An installed plugin: the origin is its root crate.
    Plugin(&'static PluginDescriptor),
    /// The engine ([`engine_origin!`]): the package after the prefix.
    Engine(&'static str),
    /// Neither: a crate registering outside its plugin's root, a plugin not
    /// installed here, or a typo. The origin is kept as the provenance.
    Unknown(&'static str),
}

/// The owner `origin` names: an installed plugin's root, the engine, or
/// unknown. Never guessed from a name.
pub fn owner_of(origin: &'static str) -> Owner {
    if let Some(package) = origin.strip_prefix(ENGINE_ORIGIN_PREFIX) {
        return Owner::Engine(package);
    }
    match plugin_of(origin) {
        Some(plugin) => Owner::Plugin(plugin),
        None => Owner::Unknown(origin),
    }
}

mod contribution;
pub use contribution::{
    active_contributions, active_summaries, all_summaries, contributions, Contribution,
    ContributionSource, Summary,
};

/// A registration a plugin's root crate can submit (`inventory`): it names
/// its origin and summarizes itself ([`Contribution`]), so its registry can
/// skip it while its plugin is disabled ([`registered`]) and the catalog
/// can show it under its plugin. Declared with [`registration!`], never
/// implemented by hand: the macro also lists the type as a
/// [`ContributionSource`], so no registration type is missing from the
/// catalog.
pub trait Registration: Contribution + inventory::Collect {
    #[doc(hidden)]
    fn declared_with_the_registration_macro();
}

/// Declares registration types: collects each (`inventory::collect!`),
/// implements [`Registration`] for it and lists it as a
/// [`ContributionSource`]. Each type implements [`Contribution`] itself.
///
/// ```ignore
/// pub struct SettingsGroupRegistration { pub origin: &'static str, .. }
/// impl pulsar_plugins::Contribution for SettingsGroupRegistration { .. }
/// pulsar_plugins::registration!(SettingsGroupRegistration);
/// ```
#[macro_export]
macro_rules! registration {
    ($($ty:ty),* $(,)?) => {$(
        $crate::inventory::collect!($ty);
        impl $crate::Registration for $ty {
            fn declared_with_the_registration_macro() {}
        }
        $crate::inventory::submit! {
            $crate::ContributionSource {
                category: <$ty as $crate::Contribution>::CATEGORY,
                all: $crate::all_summaries::<$ty>,
                active: $crate::active_summaries::<$ty>,
            }
        }
    )*};
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

    /// The roots of the enabled plugins a game links (not editor tools),
    /// in `installed`'s order: what a game built for the project links.
    pub fn runtime_roots<'a>(&self, installed: &'a [PluginDescriptor]) -> Vec<&'a str> {
        installed
            .iter()
            .filter(|plugin| plugin.runtime() && self.is_enabled(&plugin.id))
            .map(|plugin| plugin.root.as_str())
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

/// The activated set and the root -> plugin map the gate reads.
struct Active {
    enabled: BTreeSet<String>,
    /// Every installed plugin's root crate, to its id.
    root_plugin: HashMap<String, String>,
}

static ACTIVE: RwLock<Option<Active>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn normalized(package: &str) -> String {
    package.replace('-', "_")
}

/// Make `resolution` the process's plugin set: from now on the registries
/// skip the registrations of every installed plugin it does not enable.
pub fn activate(resolution: &Resolution) {
    let root_plugin = installed()
        .iter()
        .map(|plugin| (normalized(&plugin.root), plugin.id.clone()))
        .collect();
    *ACTIVE.write().unwrap_or_else(|poison| poison.into_inner()) = Some(Active {
        enabled: resolution.enabled.clone(),
        root_plugin,
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
    for (origin, categories) in unknown_origins() {
        tracing::error!(
            "registrations from `{origin}` ({categories:?}) name no installed plugin's root and              are not the engine's (`pulsar_plugins::engine_origin!()`); the plugin gate cannot              skip them"
        );
    }
    activate(&resolution);
    resolution
}

/// Every linked registration's origin that is neither an installed plugin's
/// root nor the engine's ([`Owner::Unknown`]), with the categories it
/// registers into: each is an error.
pub fn unknown_origins() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    let mut unknown: BTreeMap<&'static str, BTreeSet<&'static str>> = BTreeMap::new();
    for summary in contributions() {
        if let Owner::Unknown(origin) = owner_of(summary.origin) {
            unknown.entry(origin).or_default().insert(summary.category);
        }
    }
    unknown
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

/// Whether a registration from `origin` ([`origin!`]) counts: its plugin is
/// enabled, or it is the engine's ([`engine_origin!`]), or nothing is
/// activated. An unknown origin ([`Owner::Unknown`]) still counts, so a
/// mistake never silently drops a registration, but it is an error the
/// editor logs when it opens a project ([`unknown_origins`]) and the
/// installed set's tests fail on.
pub fn origin_enabled(origin: &str) -> bool {
    let active = ACTIVE.read().unwrap_or_else(|poison| poison.into_inner());
    let Some(active) = active.as_ref() else {
        return true;
    };
    match active.root_plugin.get(&normalized(origin)) {
        Some(plugin) => active.enabled.contains(plugin),
        None => true,
    }
}

/// The installed plugin `id`'s descriptor (a plugin's own crates read its
/// name and description here: only its root links the descriptor).
pub fn descriptor(id: &str) -> Option<&'static PluginDescriptor> {
    installed().iter().find(|plugin| plugin.id == id)
}

/// The installed plugin whose root crate is `origin` (the package a
/// registration names), if any. `None`: a registration that is not a
/// plugin's (the engine's own); [`contributions`] keeps its origin as its
/// provenance rather than guessing an owner from its name.
pub fn plugin_of(origin: &str) -> Option<&'static PluginDescriptor> {
    installed().iter().find(|plugin| plugin.owns(origin))
}

#[cfg(test)]
mod tests;
