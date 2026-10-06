use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "editor";
pub const OWNER: &str = "advanced";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Advanced", "Low-level engine and editor settings")
        .setting(
            "allow_unsafe_process",
            SchemaEntry::new(
                "WARNING: Enabling this allows blueprints to execute arbitrary system commands. Only enable if you trust all blueprint sources.",
                false,
            )
            .label("Allow Shell Execution")
            .page("Advanced")
            .field_type(FieldType::Checkbox),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
