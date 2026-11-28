//! Generic asset seams: which component a dropped asset becomes (the
//! viewport's asset drop), who loads an asset kind into SceneDB, and which
//! world assets a changed file reloads. A plugin registers each for the
//! asset kinds it owns; the engine names none.

use std::any::Any;
use std::path::{Component, Path, PathBuf};

use pulsar_scenedb::{AssetDependency, AssetId, AssetPayload};
use ui_types_common::AssetKind;

/// The component an asset of a kind becomes when dropped in a level.
/// Skipped while its plugin is disabled.
pub struct AssetComponentRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub asset_kind: AssetKind,
    pub class_name: &'static str,
    /// The class's value for the project-relative asset `path`, complete
    /// (a mesh's geometry loaded), ready to attach.
    pub value_for: fn(path: &str) -> Box<dyn Any + Send + Sync>,
}

impl pulsar_plugins::Contribution for AssetComponentRegistration {
    const CATEGORY: &'static str = "asset.component";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        format!("{:?}", self.asset_kind)
    }

    fn label(&self) -> String {
        format!("{:?} assets become {}", self.asset_kind, self.class_name)
    }
}

pulsar_plugins::registration!(AssetComponentRegistration);

/// The component registration for assets of `kind`, from an enabled plugin.
pub fn component_for_asset(kind: &AssetKind) -> Option<&'static AssetComponentRegistration> {
    pulsar_plugins::registered::<AssetComponentRegistration>()
        .find(|registration| registration.asset_kind == *kind)
}

/// What a plugin does to a component of `class_name` that is being made new:
/// it copies the project's defaults into the component's own fields. Only
/// for new components; a loaded, copied or rebuilt one keeps what it has.
pub struct NewComponentRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub class_name: &'static str,
    /// Given the component's value, of `class_name`'s type.
    pub seed: fn(&mut dyn Any),
}

impl pulsar_plugins::Contribution for NewComponentRegistration {
    const CATEGORY: &'static str = "component.new_defaults";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.class_name.to_string()
    }
}

pulsar_plugins::registration!(NewComponentRegistration);

fn new_component_registration(class_name: &str) -> Option<&'static NewComponentRegistration> {
    pulsar_plugins::registered::<NewComponentRegistration>()
        .find(|registration| registration.class_name == class_name)
}

/// Give `value`, a new component of `class_name` about to be authored, the
/// project's defaults. Call it before applying anything the author asked for.
pub fn seed_new_component(class_name: &str, value: &mut dyn Any) {
    if let Some(registration) = new_component_registration(class_name) {
        (registration.seed)(value);
    }
}

/// A new component of `class_name` with the project's defaults, for a class
/// that has any; `None` when its class default is all there is.
pub fn new_component_value(class_name: &str) -> Option<Box<dyn Any + Send + Sync>> {
    new_component_registration(class_name)?;
    match pulsar_world_registry::decode_world_component_value(class_name, &serde_json::json!({}))? {
        Ok(mut value) => {
            seed_new_component(class_name, &mut *value);
            Some(value)
        }
        Err(error) => {
            tracing::warn!("no default value of '{class_name}': {error}");
            None
        }
    }
}

/// The owner of a SceneDB asset kind: how its assets load (on a SceneDB
/// worker thread) and how much a loaded one holds for the cache budget. The
/// owning plugin's root crate submits one with `inventory::submit!`;
/// a component naming assets of the kind declares them with
/// [`declared_asset_id`] and binds the payload the loader produced, without
/// linking the owner. While the owner's plugin is disabled its assets fail
/// to load, and whoever names them keeps its fallback.
pub struct AssetLoaderRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// The SceneDB asset kind.
    pub kind: &'static str,
    pub load: fn(source: &str) -> Result<AssetPayload, String>,
    /// Bytes a payload holds outside SceneDB's residency cache.
    pub size: Option<pulsar_scenedb::SizeFn>,
    /// For assets made from other assets: what `load` made is composed with
    /// them (`pulsar_scenedb::register_composer`).
    pub compose: Option<AssetCompose>,
}

/// The two halves of a composed asset kind.
#[derive(Clone, Copy)]
pub struct AssetCompose {
    /// The assets a loaded payload needs, by declared ids.
    pub dependencies: fn(&AssetPayload) -> Vec<AssetId>,
    /// The payload components bind, from the loader's and what it needs.
    pub compose: fn(&AssetPayload, &[AssetDependency]) -> Result<AssetPayload, String>,
}

impl pulsar_plugins::Contribution for AssetLoaderRegistration {
    const CATEGORY: &'static str = "asset.loader";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.kind.to_string()
    }
}

pulsar_plugins::registration!(AssetLoaderRegistration);

/// Install every asset kind's loader with SceneDB: each registration once
/// (by origin and kind), including those a plugin library brings after an
/// earlier call (`pulsar_plugins::Handled`: a `Once` would have been spent
/// before the library was imported, and a library's own copy of this crate
/// has its own). Whether a kind's plugin is enabled is checked on every load,
/// so switching projects needs nothing here.
pub fn install_asset_loaders() {
    static HANDLED: pulsar_plugins::Handled = pulsar_plugins::Handled::new();
    for registration in HANDLED.take_new_ungated::<AssetLoaderRegistration>() {
        let (origin, load) = (registration.origin, registration.load);
        pulsar_scenedb::register_loader(registration.kind, move |source| {
            if !pulsar_plugins::origin_enabled(origin) {
                return Err(format!("the plugin of '{origin}' is not enabled"));
            }
            load(source)
        });
        if let Some(size) = registration.size {
            pulsar_scenedb::register_payload_size(registration.kind, size);
        }
        if let Some(compose) = registration.compose {
            pulsar_scenedb::register_composer(
                registration.kind,
                compose.dependencies,
                compose.compose,
            );
        }
    }
}

/// The id of the asset of `kind` at `source`, declared so a world can load
/// it (with every kind's loader installed). [`AssetId::ZERO`] for an empty
/// source.
pub fn declared_asset_id(kind: &'static str, source: &str) -> AssetId {
    if source.is_empty() {
        return AssetId::ZERO;
    }
    install_asset_loaders();
    let id = AssetId::of(kind, source);
    if id.source().is_none() {
        AssetId::declare(kind, source);
    }
    id
}

/// Load the asset of `kind` at `source` now, on the caller's thread, with
/// its owner's loader (for tools that show an asset outside a world).
pub fn load_asset_now(kind: &str, source: &str) -> Result<AssetPayload, String> {
    let registration = pulsar_plugins::registered::<AssetLoaderRegistration>()
        .find(|registration| registration.kind == kind)
        .ok_or_else(|| format!("no enabled plugin loads '{kind}' assets"))?;
    let payload = (registration.load)(source)?;
    // What it needs is loaded and composed the way a world would.
    install_asset_loaders();
    pulsar_scenedb::compose_asset_now(registration.kind, payload)
}

/// Which world assets a changed file of an asset kind reloads: the owner of
/// assets made from such files maps the file to their sources (a texture
/// reloads its image and every material sampling it). The engine's
/// `reload_updated_asset` asks every registration for the event's kind.
pub struct AssetReloadRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Stable id of the registration within its plugin (`"materials.reload"`): what
    /// the plugin catalog lists it by.
    pub name: &'static str,
    pub kind: AssetKind,
    /// The sources of the world assets the file at the absolute `path` is,
    /// or feeds.
    pub sources_for_file: fn(path: &Path) -> Vec<String>,
}

impl pulsar_plugins::Contribution for AssetReloadRegistration {
    const CATEGORY: &'static str = "asset.reload";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.name.to_string()
    }
}

pulsar_plugins::registration!(AssetReloadRegistration);

/// The sources of every world asset a change to the file at `path` (a file
/// of `kind`) reloads, sorted, without duplicates.
pub fn asset_sources_for_file(kind: &AssetKind, path: &Path) -> Vec<String> {
    let mut sources: Vec<String> = pulsar_plugins::registered::<AssetReloadRegistration>()
        .filter(|registration| registration.kind == *kind)
        .flat_map(|registration| (registration.sources_for_file)(path))
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

/// Lexically normalized `path`: `\` read as `/`, `.` dropped, `..` applied.
fn normalize(path: &str) -> PathBuf {
    let path = path.trim().replace('\\', "/");
    let mut normal = PathBuf::new();
    for component in Path::new(&path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    normal.push(component);
                }
            }
            other => normal.push(other),
        }
    }
    normal
}

/// The source of the asset an authored `path` names: normalized lexically
/// (`\` as `/`, `.` and `..` applied) and made project-relative when it is
/// inside the project. Loaders resolve it against the project, the working
/// directory and the engine's assets. `None` for an empty path. Does not
/// touch the file system.
pub fn asset_source(path: &str) -> Option<String> {
    let normal = normalize(path);
    if normal.as_os_str().is_empty() {
        return None;
    }
    if normal.is_absolute() {
        if let Some(root) = engine_state::get_project_path() {
            if let Ok(relative) = normal.strip_prefix(normalize(&root)) {
                return Some(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Some(normal.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_source_is_normalized() {
        assert_eq!(asset_source(""), None);
        assert_eq!(asset_source("  "), None);
        assert_eq!(
            asset_source(r"materials\.\stone\..\Brick.mat").as_deref(),
            Some("materials/Brick.mat")
        );
    }
}
