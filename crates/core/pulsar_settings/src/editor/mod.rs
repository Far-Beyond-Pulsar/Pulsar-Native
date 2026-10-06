pub mod advanced;
pub mod debugger;
pub mod radial_menu;
pub mod source_control;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    source_control::register(cfg);
    advanced::register(cfg);
    radial_menu::register(cfg);
    debugger::register(cfg);
}
