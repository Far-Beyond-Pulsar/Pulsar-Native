use super::settings_registry::{DropdownOption, FieldType, SettingDefinition, SettingScope, register_setting};

/// Register all default engine settings
pub fn register_default_settings() {
    register_global_settings();
    register_project_settings();
}

fn register_global_settings() {
    // Appearance page
    register_setting(
        SettingDefinition::builder("appearance.theme")
            .label("Theme")
            .description("Visual theme for the engine interface")
            .page("Appearance")
            .scope(SettingScope::Global)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Default Dark".to_string(),
                        value: "Default Dark".to_string(),
                    },
                    DropdownOption {
                        label: "Default Light".to_string(),
                        value: "Default Light".to_string(),
                    },
                ],
            })
            .default_value("Default Dark")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("appearance.ui_scale")
            .label("UI Scale")
            .description("Scale factor for the user interface")
            .page("Appearance")
            .scope(SettingScope::Global)
            .field_type(FieldType::Slider {
                min: 0.5,
                max: 2.0,
                step: 0.1,
            })
            .default_value(1.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("appearance.accent_color")
            .label("Accent Color")
            .description("Primary accent color used throughout the interface")
            .page("Appearance")
            .scope(SettingScope::Global)
            .field_type(FieldType::ColorPicker)
            .default_value("#0ea5e9")
            .build(),
    );

    // Editor page
    register_setting(
        SettingDefinition::builder("editor.font_size")
            .label("Font Size")
            .description("Font size for code editor")
            .page("Editor")
            .scope(SettingScope::Global)
            .field_type(FieldType::NumberInput {
                min: Some(8.0),
                max: Some(32.0),
                step: Some(1.0),
            })
            .default_value(14.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("editor.show_line_numbers")
            .label("Show Line Numbers")
            .description("Display line numbers in code editor")
            .page("Editor")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("editor.word_wrap")
            .label("Word Wrap")
            .description("Enable word wrapping in code editor")
            .page("Editor")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("editor.tab_size")
            .label("Tab Size")
            .description("Number of spaces for tab indentation")
            .page("Editor")
            .scope(SettingScope::Global)
            .field_type(FieldType::NumberInput {
                min: Some(2.0),
                max: Some(8.0),
                step: Some(1.0),
            })
            .default_value(4.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("editor.auto_save")
            .label("Auto Save")
            .description("Automatically save files when editing")
            .page("Editor")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    // Performance page
    register_setting(
        SettingDefinition::builder("performance.max_viewport_fps")
            .label("Max Viewport FPS")
            .description("Maximum frame rate for viewport rendering")
            .page("Performance")
            .scope(SettingScope::Global)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "30 FPS".to_string(),
                        value: "30".to_string(),
                    },
                    DropdownOption {
                        label: "60 FPS".to_string(),
                        value: "60".to_string(),
                    },
                    DropdownOption {
                        label: "120 FPS".to_string(),
                        value: "120".to_string(),
                    },
                    DropdownOption {
                        label: "144 FPS".to_string(),
                        value: "144".to_string(),
                    },
                    DropdownOption {
                        label: "240 FPS".to_string(),
                        value: "240".to_string(),
                    },
                    DropdownOption {
                        label: "Unlimited".to_string(),
                        value: "0".to_string(),
                    },
                ],
            })
            .default_value("60")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("performance.optimization_level")
            .label("Performance Level")
            .description("Performance optimization level (higher = more aggressive)")
            .page("Performance")
            .scope(SettingScope::Global)
            .field_type(FieldType::Slider {
                min: 0.0,
                max: 2.0,
                step: 1.0,
            })
            .default_value(1.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("performance.enable_vsync")
            .label("Enable V-Sync")
            .description("Synchronize frame rate with display refresh rate")
            .page("Performance")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    // Advanced page
    register_setting(
        SettingDefinition::builder("advanced.debug_logging")
            .label("Debug Logging")
            .description("Enable detailed debug logging")
            .page("Advanced")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("advanced.experimental_features")
            .label("Experimental Features")
            .description("Enable experimental engine features (may be unstable)")
            .page("Advanced")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("advanced.telemetry")
            .label("Anonymous Telemetry")
            .description("Send anonymous usage data to help improve the engine")
            .page("Advanced")
            .scope(SettingScope::Global)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );
}

fn register_project_settings() {
    // Project page
    register_setting(
        SettingDefinition::builder("project.name")
            .label("Project Name")
            .description("Name of your game project")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("MyGame".to_string()),
                multiline: false,
            })
            .default_value("MyGame")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.version")
            .label("Version")
            .description("Project version")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("0.1.0".to_string()),
                multiline: false,
            })
            .default_value("0.1.0")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.author")
            .label("Author")
            .description("Author or studio name")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("Your Name".to_string()),
                multiline: false,
            })
            .default_value("Your Name")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.description")
            .label("Description")
            .description("A brief description of your game")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("A brief description.".to_string()),
                multiline: false,
            })
            .default_value("A brief description.")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.company")
            .label("Company")
            .description("Studio or company name")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("Your Studio Name".to_string()),
                multiline: false,
            })
            .default_value("Your Studio Name")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.license")
            .label("License")
            .description("License type (MIT, GPL, Proprietary, etc.)")
            .page("Project")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "MIT".to_string(),
                        value: "MIT".to_string(),
                    },
                    DropdownOption {
                        label: "GPL".to_string(),
                        value: "GPL".to_string(),
                    },
                    DropdownOption {
                        label: "Apache 2.0".to_string(),
                        value: "Apache 2.0".to_string(),
                    },
                    DropdownOption {
                        label: "Proprietary".to_string(),
                        value: "Proprietary".to_string(),
                    },
                ],
            })
            .default_value("MIT")
            .build(),
    );

    // Window page
    register_setting(
        SettingDefinition::builder("window.title")
            .label("Window Title")
            .description("Title displayed in the game window")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("My Game Window".to_string()),
                multiline: false,
            })
            .default_value("My Game Window")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.width")
            .label("Window Width")
            .description("Window width in pixels")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(320.0),
                max: Some(7680.0),
                step: Some(1.0),
            })
            .default_value(1280.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.height")
            .label("Window Height")
            .description("Window height in pixels")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(240.0),
                max: Some(4320.0),
                step: Some(1.0),
            })
            .default_value(720.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.fullscreen")
            .label("Fullscreen")
            .description("Start in fullscreen mode")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.vsync")
            .label("VSync")
            .description("Enable vertical sync")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.resizable")
            .label("Resizable")
            .description("Allow window resizing")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("window.icon")
            .label("Window Icon")
            .description("Path to window icon")
            .page("Window")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("assets/icon.png".to_string()),
                multiline: false,
            })
            .default_value("assets/icon.png")
            .build(),
    );

    // Graphics page
    register_setting(
        SettingDefinition::builder("graphics.renderer")
            .label("Renderer")
            .description("Graphics rendering backend")
            .page("Graphics")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "OpenGL".to_string(),
                        value: "OpenGL".to_string(),
                    },
                    DropdownOption {
                        label: "Vulkan".to_string(),
                        value: "Vulkan".to_string(),
                    },
                    DropdownOption {
                        label: "DirectX".to_string(),
                        value: "DirectX".to_string(),
                    },
                    DropdownOption {
                        label: "Metal".to_string(),
                        value: "Metal".to_string(),
                    },
                    DropdownOption {
                        label: "Software".to_string(),
                        value: "Software".to_string(),
                    },
                ],
            })
            .default_value("OpenGL")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("graphics.msaa_samples")
            .label("MSAA Samples")
            .description("Multisample anti-aliasing samples (0 = off)")
            .page("Graphics")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Off".to_string(),
                        value: "0".to_string(),
                    },
                    DropdownOption {
                        label: "2x".to_string(),
                        value: "2".to_string(),
                    },
                    DropdownOption {
                        label: "4x".to_string(),
                        value: "4".to_string(),
                    },
                    DropdownOption {
                        label: "8x".to_string(),
                        value: "8".to_string(),
                    },
                ],
            })
            .default_value("4")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("graphics.max_fps")
            .label("Max FPS")
            .description("Maximum frames per second (0 = unlimited)")
            .page("Graphics")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(360.0),
                step: Some(1.0),
            })
            .default_value(144.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("graphics.texture_filtering")
            .label("Texture Filtering")
            .description("Texture filtering method")
            .page("Graphics")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Nearest".to_string(),
                        value: "Nearest".to_string(),
                    },
                    DropdownOption {
                        label: "Linear".to_string(),
                        value: "Linear".to_string(),
                    },
                    DropdownOption {
                        label: "Anisotropic".to_string(),
                        value: "Anisotropic".to_string(),
                    },
                ],
            })
            .default_value("Anisotropic")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("graphics.shadow_quality")
            .label("Shadow Quality")
            .description("Quality of rendered shadows")
            .page("Graphics")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Low".to_string(),
                        value: "Low".to_string(),
                    },
                    DropdownOption {
                        label: "Medium".to_string(),
                        value: "Medium".to_string(),
                    },
                    DropdownOption {
                        label: "High".to_string(),
                        value: "High".to_string(),
                    },
                    DropdownOption {
                        label: "Ultra".to_string(),
                        value: "Ultra".to_string(),
                    },
                ],
            })
            .default_value("High")
            .build(),
    );

    // Audio page
    register_setting(
        SettingDefinition::builder("audio.master_volume")
            .label("Master Volume")
            .description("Master audio volume (0.0 - 1.0)")
            .page("Audio")
            .scope(SettingScope::Project)
            .field_type(FieldType::Slider {
                min: 0.0,
                max: 1.0,
                step: 0.01,
            })
            .default_value(1.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("audio.music_volume")
            .label("Music Volume")
            .description("Music volume (0.0 - 1.0)")
            .page("Audio")
            .scope(SettingScope::Project)
            .field_type(FieldType::Slider {
                min: 0.0,
                max: 1.0,
                step: 0.01,
            })
            .default_value(0.8)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("audio.sfx_volume")
            .label("SFX Volume")
            .description("Sound effects volume (0.0 - 1.0)")
            .page("Audio")
            .scope(SettingScope::Project)
            .field_type(FieldType::Slider {
                min: 0.0,
                max: 1.0,
                step: 0.01,
            })
            .default_value(0.8)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("audio.enable_3d_audio")
            .label("Enable 3D Audio")
            .description("Enable spatial 3D audio")
            .page("Audio")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    // Input page
    register_setting(
        SettingDefinition::builder("input.mouse_sensitivity")
            .label("Mouse Sensitivity")
            .description("Mouse sensitivity multiplier")
            .page("Input")
            .scope(SettingScope::Project)
            .field_type(FieldType::Slider {
                min: 0.1,
                max: 5.0,
                step: 0.1,
            })
            .default_value(1.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("input.invert_y_axis")
            .label("Invert Y Axis")
            .description("Invert mouse Y axis")
            .page("Input")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    // Paths page
    register_setting(
        SettingDefinition::builder("paths.assets")
            .label("Assets Path")
            .description("Path to assets directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("assets/".to_string()),
                multiline: false,
            })
            .default_value("assets/")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("paths.shaders")
            .label("Shaders Path")
            .description("Path to shaders directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("shaders/".to_string()),
                multiline: false,
            })
            .default_value("shaders/")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("paths.scripts")
            .label("Scripts Path")
            .description("Path to scripts directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("classes/".to_string()),
                multiline: false,
            })
            .default_value("classes/")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("paths.savegames")
            .label("Savegames Path")
            .description("Path to savegames directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("saves/".to_string()),
                multiline: false,
            })
            .default_value("saves/")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("paths.plugins")
            .label("Plugins Path")
            .description("Path to plugins directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("plugins/".to_string()),
                multiline: false,
            })
            .default_value("plugins/")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("paths.logs")
            .label("Logs Path")
            .description("Path to logs directory")
            .page("Paths")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("logs/".to_string()),
                multiline: false,
            })
            .default_value("logs/")
            .build(),
    );

    // Build page
    register_setting(
        SettingDefinition::builder("build.debug")
            .label("Debug Mode")
            .description("Enable debug mode")
            .page("Build")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("build.optimize")
            .label("Optimize")
            .description("Enable optimizations")
            .page("Build")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(false)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("build.hot_reload")
            .label("Hot Reload")
            .description("Enable hot reload for faster iteration")
            .page("Build")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("build.target_platform")
            .label("Target Platform")
            .description("Platform to build for")
            .page("Build")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Windows".to_string(),
                        value: "windows".to_string(),
                    },
                    DropdownOption {
                        label: "Linux".to_string(),
                        value: "linux".to_string(),
                    },
                    DropdownOption {
                        label: "macOS".to_string(),
                        value: "macos".to_string(),
                    },
                    DropdownOption {
                        label: "Web (WASM)".to_string(),
                        value: "web".to_string(),
                    },
                ],
            })
            .default_value("windows")
            .build(),
    );
}
