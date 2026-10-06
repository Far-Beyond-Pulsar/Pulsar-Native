use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "build_notifications";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Build Notifications", "Audio feedback for completed builds")
        .setting("play_success_sound", SchemaEntry::new("Play a sound when a build succeeds", true).label("Success Sound").page("Build Notifications").field_type(FieldType::Checkbox))
        .setting("play_error_sound", SchemaEntry::new("Play a sound when a build fails", true).label("Error Sound").page("Build Notifications").field_type(FieldType::Checkbox))
        .setting("volume", SchemaEntry::new("Build notification sound volume", 1.0_f64).label("Volume").page("Build Notifications").field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.05 }).validator(Validator::float_range(0.0, 1.0)));
    let _ = cfg.register(NS, OWNER, schema);
}
