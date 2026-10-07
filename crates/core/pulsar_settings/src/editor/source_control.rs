use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "source_control";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Source Control", "Integrated version control settings")
        .setting(
            "auto_fetch",
            SchemaEntry::new("Periodically fetch remote changes in the background", true)
                .label("Auto Fetch")
                .page("Source Control")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "auto_fetch_interval_minutes",
            SchemaEntry::new("Minutes between background fetch operations", 5_i64)
                .label("Fetch Interval (min)")
                .page("Source Control")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(60.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 60)),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
