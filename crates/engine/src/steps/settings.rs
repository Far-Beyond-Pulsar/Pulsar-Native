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
    engine_state::settings::GlobalSettings::new().load_all();

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
