use plugin_editor_api::AssetKind;

pub struct AssetComponentRegistration {
    pub asset_kind: AssetKind,
    pub class_name: &'static str,
    pub data_field: &'static str,
}

pulsar_reflection::inventory::collect!(AssetComponentRegistration);

pub fn component_class_for_asset(kind: &AssetKind) -> Option<(&'static str, &'static str)> {
    for registration in pulsar_reflection::inventory::iter::<AssetComponentRegistration> {
        if registration.asset_kind == *kind {
            return Some((registration.class_name, registration.data_field));
        }
    }
    None
}
