use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "streaming";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Streaming", "Asset and level streaming configuration")
        .setting(
            "texture_stream_pool_mb",
            SchemaEntry::new("VRAM budget for the texture streaming pool in MB", 512_i64)
                .label("Texture Pool (MB)")
                .page("Streaming")
                .field_type(FieldType::NumberInput {
                    min: Some(64.0),
                    max: Some(16384.0),
                    step: Some(64.0),
                })
                .validator(Validator::int_range(64, 16384)),
        )
        .setting(
            "virtual_texturing_enabled",
            SchemaEntry::new(
                "Enable runtime virtual texturing (RVT) for large terrain surfaces",
                false,
            )
            .label("Virtual Texturing")
            .page("Streaming")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "virtual_texture_tile_size",
            SchemaEntry::new(
                "Size of each virtual texture tile in texels (power of 2)",
                "128",
            )
            .label("VT Tile Size")
            .page("Streaming")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("64", "64"),
                    DropdownOption::new("128", "128"),
                    DropdownOption::new("256", "256"),
                    DropdownOption::new("512", "512"),
                ],
            }),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
