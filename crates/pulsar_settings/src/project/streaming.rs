use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "streaming";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Streaming", "Asset and level streaming configuration")
        .setting("async_loading",
            SchemaEntry::new("Load assets asynchronously on background threads", true)
                .label("Async Loading").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("max_concurrent_loads",
            SchemaEntry::new("Maximum number of assets loading simultaneously", 8_i64)
                .label("Max Concurrent Loads").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 64)))
        .setting("prefetch_radius",
            SchemaEntry::new("Radius around the player to pre-warm assets for (m)", 500.0_f64)
                .label("Prefetch Radius (m)").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(50.0), max: Some(5000.0), step: Some(50.0) })
                .validator(Validator::float_range(50.0, 5000.0)))
        .setting("priority_bias",
            SchemaEntry::new("Priority bias for visible vs background asset loading", "balanced")
                .label("Priority Bias").page("Streaming")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Foreground First", "foreground"),
                    DropdownOption::new("Balanced", "balanced"),
                    DropdownOption::new("Background First", "background"),
                ]})
                .validator(Validator::string_one_of(["foreground", "balanced", "background"])))
        .setting("texture_streaming",
            SchemaEntry::new("Stream texture mip levels progressively (reduces VRAM usage)", true)
                .label("Texture Streaming").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("texture_stream_pool_mb",
            SchemaEntry::new("VRAM budget for the texture streaming pool in MB", 512_i64)
                .label("Texture Pool (MB)").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(16384.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 16384)))
        .setting("mesh_streaming",
            SchemaEntry::new("Stream mesh vertex/index data on demand", false)
                .label("Mesh Streaming").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("audio_streaming_threshold_kb",
            SchemaEntry::new("Audio files larger than this (KB) are streamed rather than fully loaded", 512_i64)
                .label("Audio Stream Threshold (KB)").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(8192.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 8192)))
        .setting("gc_interval_seconds",
            SchemaEntry::new("How often the asset garbage collector runs to free unused assets (s)", 30_i64)
                .label("GC Interval (s)").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(5.0), max: Some(300.0), step: Some(5.0) })
                .validator(Validator::int_range(5, 300)))
        .setting("level_streaming_mode",
            SchemaEntry::new("How sub-levels are streamed in", "distance")
                .label("Level Streaming Mode").page("Streaming")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Distance-Based", "distance"),
                    DropdownOption::new("Always Loaded", "always"),
                    DropdownOption::new("Blueprint-Controlled", "blueprint"),
                ]})
                .validator(Validator::string_one_of(["distance", "always", "blueprint"])))
        .setting("virtual_texturing_enabled",
            SchemaEntry::new("Enable runtime virtual texturing (RVT) for large terrain surfaces", false)
                .label("Virtual Texturing").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("virtual_texture_tile_size",
            SchemaEntry::new("Size of each virtual texture tile in texels (power of 2)", "128")
                .label("VT Tile Size").page("Streaming")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("64", "64"),
                    DropdownOption::new("128", "128"),
                    DropdownOption::new("256", "256"),
                    DropdownOption::new("512", "512"),
                ]}))
        .setting("nanite_streaming",
            SchemaEntry::new("Enable Nanite virtualized geometry streaming", false)
                .label("Nanite Streaming").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("shader_cache_size_mb",
            SchemaEntry::new("Disk space budget for compiled shader cache in MB", 256_i64)
                .label("Shader Cache (MB)").page("Streaming")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(4096.0), step: Some(64.0) })
                .validator(Validator::int_range(0, 4096)))
        .setting("cooked_data_only",
            SchemaEntry::new("Load only pre-cooked binary asset formats in shipped builds", true)
                .label("Cooked Data Only").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("progressive_mesh_loading",
            SchemaEntry::new("Stream mesh LOD levels progressively as the player moves closer", false)
                .label("Progressive Mesh Loading").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("async_physics_cooking",
            SchemaEntry::new("Cook physics collision shapes asynchronously to avoid hitches", true)
                .label("Async Physics Cooking").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("streaming_install_enabled",
            SchemaEntry::new("Allow streaming installation where players can start playing before download is complete", false)
                .label("Streaming Install").page("Streaming")
                .field_type(FieldType::Checkbox))
        .setting("streaming_install_initial_chunk",
            SchemaEntry::new("Minimum content chunk required before streaming install allows gameplay", "chunk0")
                .label("Initial Chunk").page("Streaming")
                .field_type(FieldType::TextInput { placeholder: Some("chunk0".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
