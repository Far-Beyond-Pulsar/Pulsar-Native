//! Generic asset seams: which component a dropped asset becomes (the
//! viewport's asset drop), who loads an asset kind into SceneDB, and which
//! world assets a changed file reloads. A plugin registers each for the
//! asset kinds it owns; the engine names none.

use std::any::Any;
use std::path::{Component, Path, PathBuf};

use plugin_editor_api::AssetKind;
use pulsar_scenedb::{AssetId, AssetPayload};

pub struct AssetComponentRegistration {
    pub asset_kind: AssetKind,
    pub class_name: &'static str,
    /// The class's value for the project-relative asset `path`, complete
    /// (a mesh's geometry loaded), ready to attach.
    pub value_for: fn(path: &str) -> Box<dyn Any + Send + Sync>,
}

pulsar_reflection::inventory::collect!(AssetComponentRegistration);

/// The component registration for assets of `kind`.
pub fn component_for_asset(kind: &AssetKind) -> Option<&'static AssetComponentRegistration> {
    pulsar_reflection::inventory::iter::<AssetComponentRegistration>
        .into_iter()
        .find(|registration| registration.asset_kind == *kind)
}

/// The owner of a SceneDB asset kind: how its assets load (on a SceneDB
/// worker thread) and how much a loaded one holds for the cache budget. The
/// owning plugin's component crate submits one with `inventory::submit!`;
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
}

inventory::collect!(AssetLoaderRegistration);

impl pulsar_plugins::Registration for AssetLoaderRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// Install every linked asset kind's loader with SceneDB. Once per
/// process; whether a kind's plugin is enabled is checked on every load, so
/// switching projects needs nothing here.
pub fn install_asset_loaders() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        for registration in inventory::iter::<AssetLoaderRegistration> {
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
        }
    });
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
    (registration.load)(source)
}

/// Which world assets a changed file of an asset kind reloads: the owner of
/// assets made from such files maps the file to their sources (a texture
/// reloads its image and every material sampling it). The engine's
/// `reload_updated_asset` asks every registration for the event's kind.
pub struct AssetReloadRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub kind: AssetKind,
    /// The sources of the world assets the file at the absolute `path` is,
    /// or feeds.
    pub sources_for_file: fn(path: &Path) -> Vec<String>,
}

inventory::collect!(AssetReloadRegistration);

impl pulsar_plugins::Registration for AssetReloadRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

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
