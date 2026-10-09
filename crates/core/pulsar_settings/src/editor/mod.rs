pub mod advanced;
pub mod appearance;
pub mod build_notifications;
pub mod code_editor;
pub mod debugger;
pub mod file_manager;
pub mod navigation;
pub mod radial_menu;
pub mod renderer;
pub mod source_control;
pub mod viewport;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    source_control::register(cfg);
    appearance::register(cfg);
    build_notifications::register(cfg);
    code_editor::register(cfg);
    file_manager::register(cfg);
    navigation::register(cfg);
    renderer::register(cfg);
    viewport::register(cfg);
    advanced::register(cfg);
    radial_menu::register(cfg);
    debugger::register(cfg);
}
