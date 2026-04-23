pub mod accessibility;
pub mod ai;
pub mod animation;
pub mod audio;
pub mod build;
pub mod gameplay;
pub mod graphics;
pub mod input;
pub mod localization;
pub mod network;
pub mod packaging;
pub mod paths;
pub mod physics;
pub mod plugins;
pub mod project_info;
pub mod rendering;
pub mod scripting;
pub mod streaming;
pub mod vr;
pub mod window;
pub mod world;

use pulsar_config::ConfigManager;

pub fn register_all(cfg: &'static ConfigManager) {
    project_info::register(cfg);
    window::register(cfg);
    graphics::register(cfg);
    rendering::register(cfg);
    gameplay::register(cfg);
    world::register(cfg);
    physics::register(cfg);
    network::register(cfg);
    audio::register(cfg);
    input::register(cfg);
    paths::register(cfg);
    build::register(cfg);
    packaging::register(cfg);
    ai::register(cfg);
    accessibility::register(cfg);
    localization::register(cfg);
    plugins::register(cfg);
    scripting::register(cfg);
    animation::register(cfg);
    streaming::register(cfg);
    vr::register(cfg);
}
