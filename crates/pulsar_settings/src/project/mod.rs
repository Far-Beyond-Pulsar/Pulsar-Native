pub mod project_info;
pub mod window;
pub mod graphics;
pub mod rendering;
pub mod gameplay;
pub mod world;
pub mod physics;
pub mod network;
pub mod audio;
pub mod input;
pub mod paths;
pub mod build;
pub mod packaging;
pub mod ai;
pub mod accessibility;
pub mod localization;
pub mod plugins;
pub mod scripting;
pub mod animation;
pub mod streaming;
pub mod vr;

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
