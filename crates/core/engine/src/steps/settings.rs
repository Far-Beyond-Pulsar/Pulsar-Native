//! Settings step: load engine configuration.

use crate::appdata;
use crate::init::{InitContext, InitError};
use crate::settings::EngineSettings;

pub fn run(_ctx: &mut InitContext) -> Result<(), InitError> {
    let appdata = appdata::setup_appdata();
    tracing::debug!("Loading engine settings from {:?}", appdata.config_file);
    let engine_settings = EngineSettings::load(&appdata.config_file);

    // Initialize modern ConfigManager Global Settings
    engine_state::register_default_settings();
    let global_settings = engine_state::settings::GlobalSettings::new();
    global_settings.load_all();

    if !global_settings
        .has_saved_key("advanced", "allow_unsafe_process")
        .unwrap_or(false)
    {
        if let Some(legacy_value) = legacy_unsafe_process_override(&appdata.config_file) {
            if let Err(error) = global_settings.set_and_save(
                "advanced",
                "allow_unsafe_process",
                engine_state::settings::ConfigValue::Bool(legacy_value),
            ) {
                tracing::warn!(%error, "Could not migrate legacy unsafe-process preference");
            }
        }
    }

    let allow_unsafe = engine_state::settings::global_config()
        .get(
            engine_state::settings::NS_EDITOR,
            "advanced",
            "allow_unsafe_process",
        )
        .ok()
        .and_then(|v| v.as_bool().ok())
        .unwrap_or(engine_settings.advanced.allow_unsafe_process);

    pulsar_std::set_unsafe_process_allowed(allow_unsafe);

    Ok(())
}

fn legacy_unsafe_process_override(path: &std::path::Path) -> Option<bool> {
    let content = std::fs::read_to_string(path).ok()?;
    let document: toml::Value = toml::from_str(&content).ok()?;
    document
        .get("advanced")?
        .get("allow_unsafe_process")?
        .as_bool()
}
