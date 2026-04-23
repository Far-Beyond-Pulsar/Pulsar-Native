pub mod advanced;
pub mod appearance;
pub mod code_editor;
pub mod debugger;
pub mod extensions;
pub mod keybindings;
pub mod localization;
pub mod performance;
pub mod source_control;
pub mod terminal;
pub mod tooling;
pub mod viewport;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    appearance::register(cfg);
    code_editor::register(cfg);
    viewport::register(cfg);
    tooling::register(cfg);
    source_control::register(cfg);
    performance::register(cfg);
    advanced::register(cfg);
    keybindings::register(cfg);
    terminal::register(cfg);
    debugger::register(cfg);
    extensions::register(cfg);
    localization::register(cfg);
}
