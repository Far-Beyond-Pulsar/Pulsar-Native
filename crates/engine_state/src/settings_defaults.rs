use crate::settings::{
    global_config, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
    NS_EDITOR, NS_PROJECT,
};

/// Register all default engine and project settings with the global [`ConfigManager`].
pub fn register_default_settings() {
    register_editor_settings();
    register_project_settings();
}

fn register_editor_settings() {
    register_appearance();
    register_editor_page();
    register_viewport_page();
    register_tooling_page();
    register_source_control_page();
    register_performance();
    register_advanced();
}

fn register_appearance() {
    let schema = NamespaceSchema::new("Appearance", "Visual appearance settings")
        .setting(
            "theme",
            SchemaEntry::new("Visual theme for the engine interface", "Default Dark")
                .label("Theme").page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::same("Default Dark"),
                        DropdownOption::same("Default Light"),
                    ],
                })
                .validator(Validator::string_one_of(["Default Dark", "Default Light"])),
        )
        .setting(
            "ui_scale",
            SchemaEntry::new("Scale factor for the user interface", 1.0_f64)
                .label("UI Scale").page("Appearance")
                .field_type(FieldType::Slider { min: 0.5, max: 2.0, step: 0.1 })
                .validator(Validator::float_range(0.5, 2.0)),
        )
        .setting(
            "accent_color",
            SchemaEntry::new("Primary accent color used throughout the interface", "#0ea5e9")
                .label("Accent Color").page("Appearance")
                .field_type(FieldType::ColorPicker),
        );
    let _ = global_config().register(NS_EDITOR, "appearance", schema);
}

fn register_editor_page() {
    let schema = NamespaceSchema::new("Editor", "Code editor settings")
        .setting(
            "font_size",
            SchemaEntry::new("Font size for code editor", 14_i64)
                .label("Font Size").page("Editor")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(8, 32)),
        )
        .setting(
            "show_line_numbers",
            SchemaEntry::new("Display line numbers in code editor", true)
                .label("Show Line Numbers").page("Editor")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "word_wrap",
            SchemaEntry::new("Enable word wrapping in code editor", false)
                .label("Word Wrap").page("Editor")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "tab_size",
            SchemaEntry::new("Number of spaces for tab indentation", 4_i64)
                .label("Tab Size").page("Editor")
                .field_type(FieldType::NumberInput { min: Some(2.0), max: Some(8.0), step: Some(1.0) })
                .validator(Validator::int_range(2, 8)),
        )
        .setting(
            "auto_save",
            SchemaEntry::new("Automatically save files when editing", true)
                .label("Auto Save").page("Editor")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_EDITOR, "editor", schema);
}

fn register_performance() {
    let schema = NamespaceSchema::new("Performance", "Performance settings")
        .setting(
            "max_viewport_fps",
            SchemaEntry::new("Maximum frame rate for viewport rendering", "60")
                .label("Max Viewport FPS").page("Performance")
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
            "optimization_level",
            SchemaEntry::new("Performance optimization level (higher = more aggressive)", 1.0_f64)
                .label("Performance Level").page("Performance")
                .field_type(FieldType::Slider { min: 0.0, max: 2.0, step: 1.0 })
                .validator(Validator::float_range(0.0, 2.0)),
        )
        .setting(
            "enable_vsync",
            SchemaEntry::new("Synchronize frame rate with display refresh rate", true)
                .label("Enable V-Sync").page("Performance")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_EDITOR, "performance", schema);
}

fn register_advanced() {
    let schema = NamespaceSchema::new("Advanced", "Advanced engine settings")
        .setting(
            "debug_logging",
            SchemaEntry::new("Enable detailed debug logging", false)
                .label("Debug Logging").page("Advanced")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "experimental_features",
            SchemaEntry::new("Enable experimental engine features (may be unstable)", false)
                .label("Experimental Features").page("Advanced")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "telemetry",
            SchemaEntry::new("Send anonymous usage data to help improve the engine", false)
                .label("Anonymous Telemetry").page("Advanced")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_EDITOR, "advanced", schema);
}

fn register_project_settings() {
    register_project_page();
    register_gameplay_page();
    register_window_page();
    register_world_page();
    register_graphics_page();
    register_physics_page();
    register_network_page();
    register_audio_page();
    register_input_page();
    register_paths_page();
    register_build_page();
    register_packaging_page();
}

fn register_viewport_page() {
    let schema = NamespaceSchema::new("Viewport", "Realtime viewport and scene camera settings")
        .setting(
            "camera_speed",
            SchemaEntry::new("Base speed used for editor camera movement", 4.0_f64)
                .label("Camera Speed").page("Viewport")
                .field_type(FieldType::Slider { min: 0.5, max: 12.0, step: 0.5 })
                .validator(Validator::float_range(0.5, 12.0)),
        )
        .setting(
            "fov_degrees",
            SchemaEntry::new("Perspective camera field of view in degrees", 70_i64)
                .label("Camera FOV").page("Viewport")
                .field_type(FieldType::NumberInput { min: Some(30.0), max: Some(120.0), step: Some(1.0) })
                .validator(Validator::int_range(30, 120)),
        )
        .setting(
            "show_grid",
            SchemaEntry::new("Show world grid in the viewport", true)
                .label("Show Grid").page("Viewport")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_gizmos",
            SchemaEntry::new("Show transform gizmos and helper widgets", true)
                .label("Show Gizmos").page("Viewport")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "realtime_rendering",
            SchemaEntry::new("Render viewport continuously instead of on-demand", true)
                .label("Realtime Rendering").page("Viewport")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "post_fx_quality",
            SchemaEntry::new("Editor viewport post-processing quality", "high")
                .label("Post FX Quality").page("Viewport")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Low", "low"),
                        DropdownOption::new("Medium", "medium"),
                        DropdownOption::new("High", "high"),
                        DropdownOption::new("Cinematic", "cinematic"),
                    ],
                })
                .validator(Validator::string_one_of(["low", "medium", "high", "cinematic"])),
        );
    let _ = global_config().register(NS_EDITOR, "viewport", schema);
}

fn register_tooling_page() {
    let schema = NamespaceSchema::new("Tooling", "Editor tooling and productivity settings")
        .setting(
            "autosave_interval_seconds",
            SchemaEntry::new("Seconds between editor autosave snapshots", 120_i64)
                .label("Autosave Interval (s)").page("Tooling")
                .field_type(FieldType::NumberInput { min: Some(15.0), max: Some(1800.0), step: Some(5.0) })
                .validator(Validator::int_range(15, 1800)),
        )
        .setting(
            "max_undo_steps",
            SchemaEntry::new("Maximum undo history depth", 256_i64)
                .label("Undo History Depth").page("Tooling")
                .field_type(FieldType::NumberInput { min: Some(32.0), max: Some(4096.0), step: Some(32.0) })
                .validator(Validator::int_range(32, 4096)),
        )
        .setting(
            "live_blueprint_compile",
            SchemaEntry::new("Compile visual scripts as they are edited", true)
                .label("Live Blueprint Compile").page("Tooling")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "enable_asset_thumbnails",
            SchemaEntry::new("Render thumbnails in asset browsers", true)
                .label("Asset Thumbnails").page("Tooling")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "diagnostics_level",
            SchemaEntry::new("Verbosity level for in-editor diagnostics", "standard")
                .label("Diagnostics Level").page("Tooling")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Quiet", "quiet"),
                        DropdownOption::new("Standard", "standard"),
                        DropdownOption::new("Verbose", "verbose"),
                    ],
                })
                .validator(Validator::string_one_of(["quiet", "standard", "verbose"])),
        );
    let _ = global_config().register(NS_EDITOR, "tooling", schema);
}

fn register_source_control_page() {
    let schema = NamespaceSchema::new("Source Control", "Integrated source control behavior")
        .setting(
            "provider",
            SchemaEntry::new("Source control backend", "git")
                .label("Provider").page("Source Control")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Git", "git"),
                        DropdownOption::new("Perforce", "perforce"),
                        DropdownOption::new("None", "none"),
                    ],
                })
                .validator(Validator::string_one_of(["git", "perforce", "none"])),
        )
        .setting(
            "auto_checkout_on_edit",
            SchemaEntry::new("Auto-checkout locked files when editing", false)
                .label("Auto Checkout on Edit").page("Source Control")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_changelists",
            SchemaEntry::new("Display changelists in content browser", true)
                .label("Show Changelists").page("Source Control")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "require_commit_message_template",
            SchemaEntry::new("Require commit message templates for check-ins", false)
                .label("Require Commit Template").page("Source Control")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_EDITOR, "source_control", schema);
}

fn register_gameplay_page() {
    let schema = NamespaceSchema::new("Gameplay", "Gameplay framework defaults")
        .setting(
            "target_tick_rate",
            SchemaEntry::new("Desired gameplay simulation tick rate", 60_i64)
                .label("Target Tick Rate").page("Gameplay")
                .field_type(FieldType::NumberInput { min: Some(15.0), max: Some(240.0), step: Some(1.0) })
                .validator(Validator::int_range(15, 240)),
        )
        .setting(
            "fixed_timestep",
            SchemaEntry::new("Use a fixed simulation timestep for deterministic behavior", false)
                .label("Fixed Timestep").page("Gameplay")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "pause_when_unfocused",
            SchemaEntry::new("Pause simulation when the game window loses focus", true)
                .label("Pause When Unfocused").page("Gameplay")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "default_game_mode",
            SchemaEntry::new("Path or identifier for the default game mode", "DefaultGameMode")
                .label("Default Game Mode").page("Gameplay")
                .field_type(FieldType::TextInput { placeholder: Some("DefaultGameMode".into()), multiline: false }),
        );
    let _ = global_config().register(NS_PROJECT, "gameplay", schema);
}

fn register_world_page() {
    let schema = NamespaceSchema::new("World", "World partitioning and streaming defaults")
        .setting(
            "world_partition_enabled",
            SchemaEntry::new("Enable world partition and region-based loading", true)
                .label("World Partition").page("World")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "streaming_distance_meters",
            SchemaEntry::new("Distance at which world cells stream in", 1200.0_f64)
                .label("Streaming Distance (m)").page("World")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(10000.0), step: Some(50.0) })
                .validator(Validator::float_range(100.0, 10000.0)),
        )
        .setting(
            "hlod_enabled",
            SchemaEntry::new("Enable hierarchical LOD generation", true)
                .label("Enable HLOD").page("World")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "origin_rebasing",
            SchemaEntry::new("Rebase world origin for very large worlds", true)
                .label("Origin Rebasing").page("World")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_PROJECT, "world", schema);
}

fn register_physics_page() {
    let schema = NamespaceSchema::new("Physics", "Physics simulation defaults")
        .setting(
            "solver_iterations",
            SchemaEntry::new("Constraint solver iterations per physics step", 8_i64)
                .label("Solver Iterations").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 64)),
        )
        .setting(
            "substepping",
            SchemaEntry::new("Enable physics substepping for stability at low frame rates", true)
                .label("Substepping").page("Physics")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "max_substeps",
            SchemaEntry::new("Maximum number of physics substeps per frame", 4_i64)
                .label("Max Substeps").page("Physics")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(16.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 16)),
        )
        .setting(
            "gravity_scale",
            SchemaEntry::new("Global gravity scalar multiplier", 1.0_f64)
                .label("Gravity Scale").page("Physics")
                .field_type(FieldType::Slider { min: 0.1, max: 3.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 3.0)),
        );
    let _ = global_config().register(NS_PROJECT, "physics", schema);
}

fn register_network_page() {
    let schema = NamespaceSchema::new("Network", "Multiplayer and replication settings")
        .setting(
            "enable_multiplayer",
            SchemaEntry::new("Enable networking systems for this project", false)
                .label("Enable Multiplayer").page("Network")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "default_server_port",
            SchemaEntry::new("Default listen/server port", 7777_i64)
                .label("Server Port").page("Network")
                .field_type(FieldType::NumberInput { min: Some(1024.0), max: Some(65535.0), step: Some(1.0) })
                .validator(Validator::int_range(1024, 65535)),
        )
        .setting(
            "replication_rate_hz",
            SchemaEntry::new("State replication update rate", 30_i64)
                .label("Replication Rate (Hz)").page("Network")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(120.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 120)),
        )
        .setting(
            "network_transport",
            SchemaEntry::new("Preferred network transport", "udp")
                .label("Transport").page("Network")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("UDP", "udp"),
                        DropdownOption::new("TCP", "tcp"),
                        DropdownOption::new("WebSocket", "ws"),
                    ],
                })
                .validator(Validator::string_one_of(["udp", "tcp", "ws"])),
        )
        .setting(
            "prediction_enabled",
            SchemaEntry::new("Use client-side prediction for responsive movement", true)
                .label("Client Prediction").page("Network")
                .field_type(FieldType::Checkbox),
        );
    let _ = global_config().register(NS_PROJECT, "network", schema);
}

fn register_packaging_page() {
    let schema = NamespaceSchema::new("Packaging", "Shipping and distribution settings")
        .setting(
            "configuration",
            SchemaEntry::new("Build configuration for packaged output", "development")
                .label("Build Configuration").page("Packaging")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Debug", "debug"),
                        DropdownOption::new("Development", "development"),
                        DropdownOption::new("Shipping", "shipping"),
                    ],
                })
                .validator(Validator::string_one_of(["debug", "development", "shipping"])),
        )
        .setting(
            "use_pak_files",
            SchemaEntry::new("Bundle cooked assets into package archives", true)
                .label("Use Pak Files").page("Packaging")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "compress_assets",
            SchemaEntry::new("Compress packaged assets for smaller download size", true)
                .label("Compress Assets").page("Packaging")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "strip_editor_content",
            SchemaEntry::new("Exclude editor-only assets from shipping builds", true)
                .label("Strip Editor Content").page("Packaging")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "staging_directory",
            SchemaEntry::new("Directory used for staged packaged builds", "build/staging")
                .label("Staging Directory").page("Packaging")
                .field_type(FieldType::PathSelector { directory: true }),
        );
    let _ = global_config().register(NS_PROJECT, "packaging", schema);
}

fn register_project_page() {
    let schema = NamespaceSchema::new("Project", "Project metadata settings")
        .setting("name",        SchemaEntry::new("Name of your game project", "MyGame").label("Project Name").page("Project").field_type(FieldType::TextInput { placeholder: Some("MyGame".into()), multiline: false }))
        .setting("version",     SchemaEntry::new("Project version", "0.1.0").label("Version").page("Project").field_type(FieldType::TextInput { placeholder: Some("0.1.0".into()), multiline: false }))
        .setting("author",      SchemaEntry::new("Author or studio name", "Your Name").label("Author").page("Project").field_type(FieldType::TextInput { placeholder: Some("Your Name".into()), multiline: false }))
        .setting("description", SchemaEntry::new("A brief description of your game", "A brief description.").label("Description").page("Project").field_type(FieldType::TextInput { placeholder: Some("A brief description.".into()), multiline: false }))
        .setting("company",     SchemaEntry::new("Studio or company name", "Your Studio Name").label("Company").page("Project").field_type(FieldType::TextInput { placeholder: Some("Your Studio Name".into()), multiline: false }))
        .setting(
            "license",
            SchemaEntry::new("License type", "MIT").label("License").page("Project")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::same("MIT"), DropdownOption::same("GPL"),
                        DropdownOption::new("Apache 2.0", "Apache 2.0"), DropdownOption::same("Proprietary"),
                    ],
                }),
        );
    let _ = global_config().register(NS_PROJECT, "project", schema);
}

fn register_window_page() {
    let schema = NamespaceSchema::new("Window", "Game window settings")
        .setting("title",      SchemaEntry::new("Title displayed in the game window", "My Game Window").label("Window Title").page("Window").field_type(FieldType::TextInput { placeholder: Some("My Game Window".into()), multiline: false }))
        .setting("width",      SchemaEntry::new("Window width in pixels", 1280_i64).label("Window Width").page("Window").field_type(FieldType::NumberInput { min: Some(320.0), max: Some(7680.0), step: Some(1.0) }).validator(Validator::int_range(320, 7680)))
        .setting("height",     SchemaEntry::new("Window height in pixels", 720_i64).label("Window Height").page("Window").field_type(FieldType::NumberInput { min: Some(240.0), max: Some(4320.0), step: Some(1.0) }).validator(Validator::int_range(240, 4320)))
        .setting("fullscreen", SchemaEntry::new("Start in fullscreen mode", false).label("Fullscreen").page("Window").field_type(FieldType::Checkbox))
        .setting("vsync",      SchemaEntry::new("Enable vertical sync", true).label("VSync").page("Window").field_type(FieldType::Checkbox))
        .setting("resizable",  SchemaEntry::new("Allow window resizing", true).label("Resizable").page("Window").field_type(FieldType::Checkbox))
        .setting("icon",       SchemaEntry::new("Path to window icon", "assets/icon.png").label("Window Icon").page("Window").field_type(FieldType::TextInput { placeholder: Some("assets/icon.png".into()), multiline: false }));
    let _ = global_config().register(NS_PROJECT, "window", schema);
}

fn register_graphics_page() {
    let schema = NamespaceSchema::new("Graphics", "Graphics rendering settings")
        .setting(
            "renderer",
            SchemaEntry::new("Graphics rendering backend", "OpenGL").label("Renderer").page("Graphics")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::same("OpenGL"), DropdownOption::same("Vulkan"),
                        DropdownOption::same("DirectX"), DropdownOption::same("Metal"),
                        DropdownOption::same("Software"),
                    ],
                }),
        )
        .setting(
            "msaa_samples",
            SchemaEntry::new("Multisample anti-aliasing samples (0 = off)", "4").label("MSAA Samples").page("Graphics")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Off", "0"), DropdownOption::new("2x", "2"),
                        DropdownOption::new("4x", "4"), DropdownOption::new("8x", "8"),
                    ],
                }),
        )
        .setting("max_fps",            SchemaEntry::new("Maximum frames per second (0 = unlimited)", 144_i64).label("Max FPS").page("Graphics").field_type(FieldType::NumberInput { min: Some(0.0), max: Some(360.0), step: Some(1.0) }).validator(Validator::int_range(0, 360)))
        .setting(
            "texture_filtering",
            SchemaEntry::new("Texture filtering method", "Anisotropic").label("Texture Filtering").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![DropdownOption::same("Nearest"), DropdownOption::same("Linear"), DropdownOption::same("Anisotropic")] }),
        )
        .setting(
            "shadow_quality",
            SchemaEntry::new("Quality of rendered shadows", "High").label("Shadow Quality").page("Graphics")
                .field_type(FieldType::Dropdown { options: vec![DropdownOption::same("Low"), DropdownOption::same("Medium"), DropdownOption::same("High"), DropdownOption::same("Ultra")] }),
        );
    let _ = global_config().register(NS_PROJECT, "graphics", schema);
}

fn register_audio_page() {
    let schema = NamespaceSchema::new("Audio", "Audio settings")
        .setting("master_volume",   SchemaEntry::new("Master audio volume (0.0 - 1.0)", 1.0_f64).label("Master Volume").page("Audio").field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 }).validator(Validator::float_range(0.0, 1.0)))
        .setting("music_volume",    SchemaEntry::new("Music volume (0.0 - 1.0)", 0.8_f64).label("Music Volume").page("Audio").field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 }).validator(Validator::float_range(0.0, 1.0)))
        .setting("sfx_volume",      SchemaEntry::new("Sound effects volume (0.0 - 1.0)", 0.8_f64).label("SFX Volume").page("Audio").field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.01 }).validator(Validator::float_range(0.0, 1.0)))
        .setting("enable_3d_audio", SchemaEntry::new("Enable spatial 3D audio", true).label("Enable 3D Audio").page("Audio").field_type(FieldType::Checkbox));
    let _ = global_config().register(NS_PROJECT, "audio", schema);
}

fn register_input_page() {
    let schema = NamespaceSchema::new("Input", "Input settings")
        .setting("mouse_sensitivity", SchemaEntry::new("Mouse sensitivity multiplier", 1.0_f64).label("Mouse Sensitivity").page("Input").field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.1 }).validator(Validator::float_range(0.1, 5.0)))
        .setting("invert_y_axis",     SchemaEntry::new("Invert mouse Y axis", false).label("Invert Y Axis").page("Input").field_type(FieldType::Checkbox));
    let _ = global_config().register(NS_PROJECT, "input", schema);
}

fn register_paths_page() {
    let schema = NamespaceSchema::new("Paths", "Project path settings")
        .setting("assets",    SchemaEntry::new("Path to assets directory", "assets/").label("Assets Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("assets/".into()), multiline: false }))
        .setting("shaders",   SchemaEntry::new("Path to shaders directory", "shaders/").label("Shaders Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("shaders/".into()), multiline: false }))
        .setting("scripts",   SchemaEntry::new("Path to scripts directory", "classes/").label("Scripts Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("classes/".into()), multiline: false }))
        .setting("savegames", SchemaEntry::new("Path to savegames directory", "saves/").label("Savegames Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("saves/".into()), multiline: false }))
        .setting("plugins",   SchemaEntry::new("Path to plugins directory", "plugins/").label("Plugins Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("plugins/".into()), multiline: false }))
        .setting("logs",      SchemaEntry::new("Path to logs directory", "logs/").label("Logs Path").page("Paths").field_type(FieldType::TextInput { placeholder: Some("logs/".into()), multiline: false }));
    let _ = global_config().register(NS_PROJECT, "paths", schema);
}

fn register_build_page() {
    let schema = NamespaceSchema::new("Build", "Build settings")
        .setting("debug",    SchemaEntry::new("Enable debug mode", true).label("Debug Mode").page("Build").field_type(FieldType::Checkbox))
        .setting("optimize", SchemaEntry::new("Enable optimizations", false).label("Optimize").page("Build").field_type(FieldType::Checkbox))
        .setting("hot_reload", SchemaEntry::new("Enable hot reload for faster iteration", true).label("Hot Reload").page("Build").field_type(FieldType::Checkbox))
        .setting(
            "target_platform",
            SchemaEntry::new("Platform to build for", "windows").label("Target Platform").page("Build")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Windows", "windows"), DropdownOption::new("Linux", "linux"),
                        DropdownOption::new("macOS", "macos"),     DropdownOption::new("Web (WASM)", "web"),
                    ],
                }),
        );
    let _ = global_config().register(NS_PROJECT, "build", schema);
}
