use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "build";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Build", "Compilation and development build settings")
        .setting("configuration",
            SchemaEntry::new("Active build configuration", "debug")
                .label("Configuration").page("Build")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Debug", "debug"),
                    DropdownOption::new("Development", "development"),
                    DropdownOption::new("Release", "release"),
                    DropdownOption::new("Shipping", "shipping"),
                ]})
                .validator(Validator::string_one_of(["debug", "development", "release", "shipping"])))
        .setting("target_platform",
            SchemaEntry::new("Platform to build and package for", "desktop")
                .label("Target Platform").page("Build")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Desktop (current OS)", "desktop"),
                    DropdownOption::new("Windows (x64)", "win64"),
                    DropdownOption::new("macOS (arm64)", "macos_arm64"),
                    DropdownOption::new("macOS (x64)", "macos_x64"),
                    DropdownOption::new("Linux (x64)", "linux_x64"),
                    DropdownOption::new("Android (arm64)", "android_arm64"),
                    DropdownOption::new("iOS (arm64)", "ios_arm64"),
                    DropdownOption::new("WebAssembly", "wasm"),
                ]}))
        .setting("optimize",
            SchemaEntry::new("Enable compiler optimizations", false)
                .label("Optimizations").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("debug_symbols",
            SchemaEntry::new("Include debug symbols in the build output", true)
                .label("Debug Symbols").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("hot_reload",
            SchemaEntry::new("Enable hot reloading of scripts and assets during play-in-editor", true)
                .label("Hot Reload").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("unity_build",
            SchemaEntry::new("Combine translation units to speed up compilation (unity build)", false)
                .label("Unity Build").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("parallel_jobs",
            SchemaEntry::new("Number of parallel compilation jobs (0 = auto)", 0_i64)
                .label("Parallel Jobs").page("Build")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(128.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 128)))
        .setting("asan",
            SchemaEntry::new("Enable AddressSanitizer for memory error detection (debug only)", false)
                .label("AddressSanitizer").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("tsan",
            SchemaEntry::new("Enable ThreadSanitizer for data race detection (debug only)", false)
                .label("ThreadSanitizer").page("Build")
                .field_type(FieldType::Checkbox))
        .setting("output_dir",
            SchemaEntry::new("Directory where compiled binaries are placed", "bin/")
                .label("Output Directory").page("Build")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("pre_build_script",
            SchemaEntry::new("Script to run before each build", "")
                .label("Pre-Build Script").page("Build")
                .field_type(FieldType::TextInput { placeholder: Some("scripts/pre_build.sh".into()), multiline: false }))
        .setting("post_build_script",
            SchemaEntry::new("Script to run after a successful build", "")
                .label("Post-Build Script").page("Build")
                .field_type(FieldType::TextInput { placeholder: Some("scripts/post_build.sh".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
