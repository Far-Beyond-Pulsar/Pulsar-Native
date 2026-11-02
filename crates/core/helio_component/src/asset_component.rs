//! Which component a dropped asset becomes (the viewport's asset drop).

use std::any::Any;

use plugin_editor_api::AssetKind;

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
