use crate::settings::global_config;

/// Register all default engine and project settings with the global [`ConfigManager`].
/// Delegates entirely to the `pulsar_settings` crate which organises settings
/// into fine-grained modules under `src/editor/` and `src/project/`.
pub fn register_default_settings() {
    pulsar_settings::register_all_settings(global_config());
}
