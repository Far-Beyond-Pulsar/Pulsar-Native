use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "streaming";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Streaming", "Asset and level streaming configuration")
        .setting(
            "texture_stream_pool_mb",
            SchemaEntry::new("VRAM budget for the texture streaming pool in MB", 512_i64)
                .label("Texture Pool (MB)")
                .page("Textures")
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
            .page("Textures")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(65536.0),
                step: Some(256.0),
            })
            .validator(Validator::int_range(0, 65536)),
        )
        .setting(
            "asset_bind_budget_ms",
            SchemaEntry::new(
                "Time a frame may spend applying finished asset loads and binding them to the scene, in ms; the rest waits for the next frame (0 = no time limit)",
                2_i64,
            )
            .label("Asset Bind Budget (ms)")
            .page("Asset Loading")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(50.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 50)),
        )
        .setting(
            "asset_bind_max_per_frame",
            SchemaEntry::new(
                "Most assets a frame applies and binds, however much time is left (0 = no limit)",
                8_i64,
            )
            .label("Asset Binds per Frame")
            .page("Asset Loading")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(1024.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 1024)),
        )
        .setting(
            "texture_upload_budget_kb",
            SchemaEntry::new(
                "Most texture data, in KB, a frame copies to the GPU; larger textures upload over several frames, nearest first, and keep showing their earlier quality meanwhile (0 = no limit)",
                4096_i64,
            )
            .label("Texture Upload per Frame (KB)")
            .page("Asset Loading")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(262144.0),
                step: Some(256.0),
            })
            .validator(Validator::int_range(0, 262144)),
        )
        .setting(
            "texture_upload_budget_ms",
            SchemaEntry::new(
                "Time a frame may spend copying textures to the GPU, in ms; the rest waits for the next frame (0 = no time limit)",
                1_i64,
            )
            .label("Texture Upload Budget (ms)")
            .page("Asset Loading")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(50.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 50)),
        )
        .setting(
            "coarse_texture_size",
            SchemaEntry::new(
                "Largest side, in texels, of the first version of a texture that streams in low resolution first; the full version follows, nearest first (0 = load textures in one step)",
                128_i64,
            )
            .label("Coarse Texture Size")
            .page("Textures")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(4096.0),
                step: Some(64.0),
            })
            .validator(Validator::int_range(0, 4096)),
        )
        .setting(
            "max_texture_size",
            SchemaEntry::new(
                "Largest side, in texels, a texture is kept at; larger images are scaled down as they load (0 = no cap)",
                2048_i64,
            )
            .label("Max Texture Size")
            .page("Textures")
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
                .page("Caches")
                .field_type(FieldType::NumberInput {
                    min: Some(64.0),
                    max: Some(65536.0),
                    step: Some(64.0),
                })
                .validator(Validator::int_range(64, 65536)),
        )
        .setting(
            "derived_cache_mb",
            SchemaEntry::new("Disk space for cooked assets under .pulsar/cache/derived, in MB", 32768_i64)
                .label("Cooked Asset Cache (MB)")
                .page("Caches")
                .field_type(FieldType::NumberInput {
                    min: Some(0.0),
                    max: Some(1048576.0),
                    step: Some(256.0),
                })
                .validator(Validator::int_range(0, 1048576)),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
