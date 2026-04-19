use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "world";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("World", "World partition, streaming, and environment settings")
        .setting("world_partition_enabled",
            SchemaEntry::new("Enable world partition for large open worlds", false)
                .label("World Partition").page("World")
                .field_type(FieldType::Checkbox))
        .setting("cell_size",
            SchemaEntry::new("World partition cell size in meters", 512_i64)
                .label("Cell Size (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(8192.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 8192)))
        .setting("streaming_distance",
            SchemaEntry::new("Radius around the camera to stream in level content (m)", 2000.0_f64)
                .label("Streaming Distance (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(50000.0), step: Some(100.0) })
                .validator(Validator::float_range(100.0, 50000.0)))
        .setting("hlod_enabled",
            SchemaEntry::new("Enable hierarchical LOD for distant world cells", false)
                .label("HLOD").page("World")
                .field_type(FieldType::Checkbox))
        .setting("origin_rebasing",
            SchemaEntry::new("Shift the world origin near the camera to prevent float precision issues", true)
                .label("Origin Rebasing").page("World")
                .field_type(FieldType::Checkbox))
        .setting("gravity",
            SchemaEntry::new("World gravity vector Y component (m/s²)", -9.81_f64)
                .label("Gravity (m/s²)").page("World")
                .field_type(FieldType::NumberInput { min: Some(-100.0), max: Some(0.0), step: Some(0.1) })
                .validator(Validator::float_range(-100.0, 0.0)))
        .setting("sky_type",
            SchemaEntry::new("Sky rendering mode", "procedural")
                .label("Sky Type").page("World")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Solid Color", "solid"),
                    DropdownOption::new("Gradient", "gradient"),
                    DropdownOption::new("Skybox", "skybox"),
                    DropdownOption::new("Procedural (Atmospheric)", "procedural"),
                ]}))
        .setting("sun_angle",
            SchemaEntry::new("Angle of the directional sun light (degrees from horizon)", 45.0_f64)
                .label("Sun Angle (°)").page("World")
                .field_type(FieldType::Slider { min: 0.0, max: 90.0, step: 0.5 })
                .validator(Validator::float_range(0.0, 90.0)))
        .setting("enable_weather",
            SchemaEntry::new("Enable procedural weather system", false)
                .label("Weather System").page("World")
                .field_type(FieldType::Checkbox))
        .setting("occlusion_culling",
            SchemaEntry::new("Enable GPU-based occlusion culling for hidden geometry", true)
                .label("Occlusion Culling").page("World")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
