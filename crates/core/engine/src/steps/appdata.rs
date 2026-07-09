//! App data step: config directory initialization.

use crate::appdata;
use crate::init::{InitContext, InitError};

pub fn run(_ctx: &mut InitContext) -> Result<(), InitError> {
    let appdata = appdata::setup_appdata();
    tracing::debug!("App data directory: {:?}", appdata.appdata_dir);
    tracing::debug!("Themes directory: {:?}", appdata.themes_dir);
    tracing::debug!("Config directory: {:?}", appdata.config_dir);
    tracing::debug!("Config file: {:?}", appdata.config_file);
    Ok(())
}
