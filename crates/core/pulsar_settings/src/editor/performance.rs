use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "performance";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Performance", "Editor performance tuning")
        .setting(
            "max_viewport_fps",
            SchemaEntry::new("Maximum frame rate for editor viewports", "60")
                .label("Max Viewport FPS")
                .page("Performance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("30 FPS", "30"),
                        DropdownOption::new("60 FPS", "60"),
                        DropdownOption::new("120 FPS", "120"),
                        DropdownOption::new("144 FPS", "144"),
                        DropdownOption::new("240 FPS", "240"),
                        DropdownOption::new("Unlimited", "0"),
                    ],
                }),
        )
        .setting(
            "ui_fps",
            SchemaEntry::new(
                "Maximum frame rate for UI panels (lower = less CPU usage)",
                "60",
            )
            .label("UI Frame Rate")
            .page("Performance")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("30 FPS", "30"),
                    DropdownOption::new("60 FPS", "60"),
                    DropdownOption::new("120 FPS", "120"),
                    DropdownOption::new("Unlimited", "0"),
                ],
            }),
        )
        .setting(
            "enable_vsync",
            SchemaEntry::new("Sync editor frame rate to the monitor refresh rate", true)
                .label("Enable V-Sync")
                .page("Performance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "worker_threads",
            SchemaEntry::new("Number of background worker threads (0 = auto)", 0_i64)
                .label("Worker Threads")
                .page("Performance")
                .field_type(FieldType::NumberInput {
                    min: Some(0.0),
                    max: Some(64.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(0, 64)),
        )
        .setting(
            "asset_import_threads",
            SchemaEntry::new("Concurrent threads used when importing assets", 4_i64)
                .label("Import Threads")
                .page("Performance")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(32.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 32)),
        )
        .setting(
            "shader_compilation_threads",
            SchemaEntry::new("Concurrent threads for shader compilation", 4_i64)
                .label("Shader Compile Threads")
                .page("Performance")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(32.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 32)),
        )
        .setting(
            "texture_streaming_pool_mb",
            SchemaEntry::new("GPU texture streaming pool size in megabytes", 512_i64)
                .label("Texture Pool (MB)")
                .page("Performance")
                .field_type(FieldType::NumberInput {
                    min: Some(64.0),
                    max: Some(16384.0),
                    step: Some(64.0),
                })
                .validator(Validator::int_range(64, 16384)),
        )
        .setting(
            "enable_gpu_crash_diagnostics",
            SchemaEntry::new(
                "Capture GPU breadcrumbs for crash diagnostics (small overhead)",
                false,
            )
            .label("GPU Crash Diagnostics")
            .page("Performance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "suspend_when_unfocused",
            SchemaEntry::new(
                "Throttle the editor when its window loses focus to save resources",
                true,
            )
            .label("Suspend When Unfocused")
            .page("Performance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "background_fps_limit",
            SchemaEntry::new(
                "Maximum FPS for the editor when it is not in focus (0 = unlimited)",
                20_i64,
            )
            .label("Background FPS Limit")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(60.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 60)),
        )
        .setting(
            "foreground_fps_limit",
            SchemaEntry::new(
                "Maximum FPS for the editor when focused (0 = unlimited)",
                0_i64,
            )
            .label("Foreground FPS Limit")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(360.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 360)),
        )
        .setting(
            "gpu_memory_budget_mb",
            SchemaEntry::new(
                "GPU VRAM budget for the editor in megabytes (0 = auto)",
                0_i64,
            )
            .label("GPU Memory Budget (MB)")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(32768.0),
                step: Some(256.0),
            })
            .validator(Validator::int_range(0, 32768)),
        )
        .setting(
            "asset_thumbnail_threads",
            SchemaEntry::new(
                "Number of threads used to generate asset thumbnails in the background",
                2_i64,
            )
            .label("Thumbnail Threads")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(16.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 16)),
        )
        .setting(
            "shader_compilation_threads",
            SchemaEntry::new(
                "Threads used for background shader compilation (0 = auto)",
                0_i64,
            )
            .label("Shader Compilation Threads")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(64.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 64)),
        )
        .setting(
            "undo_memory_limit_mb",
            SchemaEntry::new(
                "Maximum memory to use for undo history (MB, 0 = unlimited)",
                256_i64,
            )
            .label("Undo Memory Limit (MB)")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(4096.0),
                step: Some(64.0),
            })
            .validator(Validator::int_range(0, 4096)),
        )
        .setting(
            "cache_assets_in_memory",
            SchemaEntry::new(
                "Keep recently used assets resident in RAM to speed up repeat access",
                true,
            )
            .label("Asset Memory Cache")
            .page("Performance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "asset_cache_size_mb",
            SchemaEntry::new(
                "Maximum RAM to use for the in-memory asset cache (MB)",
                512_i64,
            )
            .label("Asset Cache Size (MB)")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(64.0),
                max: Some(16384.0),
                step: Some(64.0),
            })
            .validator(Validator::int_range(64, 16384)),
        )
        .setting(
            "gc_interval_seconds",
            SchemaEntry::new(
                "Frequency at which the editor GCs unused loaded assets (seconds)",
                60_i64,
            )
            .label("GC Interval (s)")
            .page("Performance")
            .field_type(FieldType::NumberInput {
                min: Some(5.0),
                max: Some(600.0),
                step: Some(5.0),
            })
            .validator(Validator::int_range(5, 600)),
        )
        .setting(
            "disable_hardware_acceleration",
            SchemaEntry::new(
                "Run the editor UI in software rendering mode (for troubleshooting)",
                false,
            )
            .label("Disable GPU Acceleration")
            .page("Performance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "perf_stats_overlay",
            SchemaEntry::new(
                "Show a real-time CPU/GPU/memory stats overlay in the editor viewport",
                false,
            )
            .label("Perf Stats Overlay")
            .page("Performance")
            .field_type(FieldType::Checkbox),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
