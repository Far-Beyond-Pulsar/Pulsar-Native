use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "graphics";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Graphics", "Real-time graphics feature toggles and quality")
        .setting(
            "bloom_enabled",
            SchemaEntry::new("Enable bloom glow effect on bright areas", true)
                .label("Bloom")
                .page("Graphics")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "bloom_intensity",
            SchemaEntry::new("Intensity of the bloom effect", 1.0_f64)
                .label("Bloom Intensity")
                .page("Graphics")
                .field_type(FieldType::Slider {
                    min: 0.0,
                    max: 5.0,
                    step: 0.1,
                })
                .validator(Validator::float_range(0.0, 5.0)),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
