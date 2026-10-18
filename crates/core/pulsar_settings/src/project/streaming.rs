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
            "texture_vram_budget_mb",
            SchemaEntry::new(
                "Video memory the scenes' textures may take, in MB. Over it, every texture is held with its largest mip level dropped (0 = no limit)",
                3072_i64,
            )
            .label("Texture VRAM Budget (MB)")
            .page("Streaming")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(65536.0),
                step: Some(256.0),
            })
            .validator(Validator::int_range(0, 65536)),
        )
        .setting(
            "max_texture_size",
            SchemaEntry::new(
                "Largest side, in texels, a texture is kept at; larger images are scaled down as they load (0 = no cap)",
                2048_i64,
            )
            .label("Max Texture Size")
            .page("Streaming")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(16384.0),
                step: Some(256.0),
            })
            .validator(Validator::int_range(0, 16384)),
        )
        .setting(
            "cpu_cache_mb",
            SchemaEntry::new("Memory for decoded asset data kept ready to rebuild GPU textures, in MB", 1024_i64)
                .label("CPU Asset Cache (MB)")
                .page("Streaming")
                .field_type(FieldType::NumberInput {
                    min: Some(64.0),
                    max: Some(65536.0),
                    step: Some(64.0),
                })
                .validator(Validator::int_range(64, 65536)),
        )
        .setting(
            "derived_cache_mb",
            SchemaEntry::new("Disk space for cooked assets under .pulsar/cache/derived, in MB", 4096_i64)
                .label("Cooked Asset Cache (MB)")
                .page("Streaming")
                .field_type(FieldType::NumberInput {
                    min: Some(0.0),
                    max: Some(1048576.0),
                    step: Some(256.0),
                })
                .validator(Validator::int_range(0, 1048576)),
        )
        .setting(
            "texture_filter",
            SchemaEntry::new("How material textures are filtered", "trilinear")
                .label("Texture Filtering")
                .page("Streaming")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("trilinear", "Trilinear (smooth, anisotropic)"),
                        DropdownOption::new("bilinear", "Bilinear"),
                        DropdownOption::new("nearest", "Nearest (pixel art)"),
                    ],
                }),
        )
        .setting(
            "texture_anisotropy",
            SchemaEntry::new("Anisotropic filtering level for trilinear texture filtering", 8_i64)
                .label("Anisotropy")
                .page("Streaming")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(16.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 16)),
        )
        .setting(
            "texture_wrap",
            SchemaEntry::new("What happens to texture coordinates outside 0 to 1", "repeat")
                .label("Texture Wrap")
                .page("Streaming")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("repeat", "Repeat"),
                        DropdownOption::new("clamp", "Clamp to edge"),
                    ],
                }),
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
