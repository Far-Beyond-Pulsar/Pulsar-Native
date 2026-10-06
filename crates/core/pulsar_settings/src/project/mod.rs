pub mod graphics;
pub mod streaming;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    graphics::register(cfg);
    streaming::register(cfg);
}
