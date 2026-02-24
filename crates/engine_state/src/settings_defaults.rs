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
    // General page
    register_setting(
        SettingDefinition::builder("project.name")
            .label("Project Name")
            .description("Name of the project")
            .page("General")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("My Project".to_string()),
                multiline: false,
            })
            .default_value("Untitled Project")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.description")
            .label("Description")
            .description("Project description")
            .page("General")
            .scope(SettingScope::Project)
            .field_type(FieldType::TextInput {
                placeholder: Some("Describe your project...".to_string()),
                multiline: true,
            })
            .default_value("")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.auto_save_interval")
            .label("Auto-save Interval")
            .description("Interval in seconds for automatic project saves (0 = disabled)")
            .page("General")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(600.0),
                step: Some(30.0),
            })
            .default_value(300.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.enable_backups")
            .label("Enable Backups")
            .description("Create automatic backups of project files")
            .page("General")
            .scope(SettingScope::Project)
            .field_type(FieldType::Checkbox)
            .default_value(true)
            .build(),
    );

    // Build page
    register_setting(
        SettingDefinition::builder("project.build.target_platform")
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

    register_setting(
        SettingDefinition::builder("project.build.optimization")
            .label("Build Optimization")
            .description("Optimization level for builds")
            .page("Build")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Debug".to_string(),
                        value: "debug".to_string(),
                    },
                    DropdownOption {
                        label: "Release".to_string(),
                        value: "release".to_string(),
                    },
                    DropdownOption {
                        label: "Release with Debug Info".to_string(),
                        value: "release_debug".to_string(),
                    },
                ],
            })
            .default_value("debug")
            .build(),
    );

    // Rendering page
    register_setting(
        SettingDefinition::builder("project.rendering.default_resolution_width")
            .label("Default Width")
            .description("Default viewport width in pixels")
            .page("Rendering")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(320.0),
                max: Some(7680.0),
                step: Some(1.0),
            })
            .default_value(1920.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.rendering.default_resolution_height")
            .label("Default Height")
            .description("Default viewport height in pixels")
            .page("Rendering")
            .scope(SettingScope::Project)
            .field_type(FieldType::NumberInput {
                min: Some(240.0),
                max: Some(4320.0),
                step: Some(1.0),
            })
            .default_value(1080.0)
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.rendering.anti_aliasing")
            .label("Anti-aliasing")
            .description("Anti-aliasing method")
            .page("Rendering")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "None".to_string(),
                        value: "none".to_string(),
                    },
                    DropdownOption {
                        label: "FXAA".to_string(),
                        value: "fxaa".to_string(),
                    },
                    DropdownOption {
                        label: "MSAA 2x".to_string(),
                        value: "msaa2".to_string(),
                    },
                    DropdownOption {
                        label: "MSAA 4x".to_string(),
                        value: "msaa4".to_string(),
                    },
                    DropdownOption {
                        label: "MSAA 8x".to_string(),
                        value: "msaa8".to_string(),
                    },
                ],
            })
            .default_value("fxaa")
            .build(),
    );

    register_setting(
        SettingDefinition::builder("project.rendering.shadow_quality")
            .label("Shadow Quality")
            .description("Quality of rendered shadows")
            .page("Rendering")
            .scope(SettingScope::Project)
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption {
                        label: "Low".to_string(),
                        value: "low".to_string(),
                    },
                    DropdownOption {
                        label: "Medium".to_string(),
                        value: "medium".to_string(),
                    },
                    DropdownOption {
                        label: "High".to_string(),
                        value: "high".to_string(),
                    },
                    DropdownOption {
                        label: "Ultra".to_string(),
                        value: "ultra".to_string(),
                    },
                ],
            })
            .default_value("medium")
            .build(),
    );
}
