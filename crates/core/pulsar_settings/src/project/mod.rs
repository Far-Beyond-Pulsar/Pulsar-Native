pub mod graphics;
pub mod rendering;
pub mod streaming;
pub mod virtual_geometry;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    graphics::register(cfg);
    rendering::register(cfg);
    streaming::register(cfg);
    virtual_geometry::register(cfg);
}
