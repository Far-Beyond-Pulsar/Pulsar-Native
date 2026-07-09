use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "build";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Build", "Compilation and development build settings")
        .setting(
            "configuration",
            SchemaEntry::new("Active build configuration", "debug")
                .label("Configuration")
                .page("Build")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Debug", "debug"),
                        DropdownOption::new("Development", "development"),
                        DropdownOption::new("Release", "release"),
                        DropdownOption::new("Shipping", "shipping"),
                    ],
                })
                .validator(Validator::string_one_of([
                    "debug",
                    "development",
                    "release",
                    "shipping",
                ])),
        )
        .setting(
            "target_platform",
            SchemaEntry::new("Platform to build and package for", "desktop")
                .label("Target Platform")
                .page("Build")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Desktop (current OS)", "desktop"),
                        DropdownOption::new("Windows (x64)", "win64"),
                        DropdownOption::new("macOS (arm64)", "macos_arm64"),
                        DropdownOption::new("macOS (x64)", "macos_x64"),
                        DropdownOption::new("Linux (x64)", "linux_x64"),
                        DropdownOption::new("Android (arm64)", "android_arm64"),
                        DropdownOption::new("iOS (arm64)", "ios_arm64"),
                        DropdownOption::new("WebAssembly", "wasm"),
                    ],
                }),
        )
        .setting(
            "optimize",
            SchemaEntry::new("Enable compiler optimizations", false)
                .label("Optimizations")
                .page("Build")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "debug_symbols",
            SchemaEntry::new("Include debug symbols in the build output", true)
                .label("Debug Symbols")
                .page("Build")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "hot_reload",
            SchemaEntry::new(
                "Enable hot reloading of scripts and assets during play-in-editor",
                true,
            )
            .label("Hot Reload")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "unity_build",
            SchemaEntry::new(
                "Combine translation units to speed up compilation (unity build)",
                false,
            )
            .label("Unity Build")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "parallel_jobs",
            SchemaEntry::new("Number of parallel compilation jobs (0 = auto)", 0_i64)
                .label("Parallel Jobs")
                .page("Build")
                .field_type(FieldType::NumberInput {
                    min: Some(0.0),
                    max: Some(128.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(0, 128)),
        )
        .setting(
            "asan",
            SchemaEntry::new(
                "Enable AddressSanitizer for memory error detection (debug only)",
                false,
            )
            .label("AddressSanitizer")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "tsan",
            SchemaEntry::new(
                "Enable ThreadSanitizer for data race detection (debug only)",
                false,
            )
            .label("ThreadSanitizer")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "output_dir",
            SchemaEntry::new("Directory where compiled binaries are placed", "bin/")
                .label("Output Directory")
                .page("Build")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "pre_build_script",
            SchemaEntry::new("Script to run before each build", "")
                .label("Pre-Build Script")
                .page("Build")
                .field_type(FieldType::TextInput {
                    placeholder: Some("scripts/pre_build.sh".into()),
                    multiline: false,
                }),
        )
        .setting(
            "post_build_script",
            SchemaEntry::new("Script to run after a successful build", "")
                .label("Post-Build Script")
                .page("Build")
                .field_type(FieldType::TextInput {
                    placeholder: Some("scripts/post_build.sh".into()),
                    multiline: false,
                }),
        )
        .setting(
            "build_threads",
            SchemaEntry::new(
                "Number of parallel compile threads (0 = auto-detect from CPU core count)",
                0_i64,
            )
            .label("Build Threads")
            .page("Build")
            .field_type(FieldType::NumberInput {
                min: Some(0.0),
                max: Some(256.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(0, 256)),
        )
        .setting(
            "incremental_compilation",
            SchemaEntry::new(
                "Only recompile translation units that have changed since the last build",
                true,
            )
            .label("Incremental Compilation")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "unity_builds",
            SchemaEntry::new(
                "Combine multiple .cpp files into single unity translation units for faster builds",
                false,
            )
            .label("Unity Builds")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "unity_batch_size",
            SchemaEntry::new("Number of source files to group per unity batch", 16_i64)
                .label("Unity Batch Size")
                .page("Build")
                .field_type(FieldType::NumberInput {
                    min: Some(4.0),
                    max: Some(64.0),
                    step: Some(4.0),
                })
                .validator(Validator::int_range(4, 64)),
        )
        .setting(
            "use_distributed_build",
            SchemaEntry::new(
                "Distribute compilation across remote build agents (icecc/distcc)",
                false,
            )
            .label("Distributed Build")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "build_server_address",
            SchemaEntry::new("Address of the build coordinator/scheduler server", "")
                .label("Build Server Address")
                .page("Build")
                .field_type(FieldType::TextInput {
                    placeholder: Some("build-server.local:8374".into()),
                    multiline: false,
                }),
        )
        .setting(
            "cache_build_artifacts",
            SchemaEntry::new("Cache compiled objects to a shared artifact store", false)
                .label("Artifact Cache")
                .page("Build")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "artifact_cache_url",
            SchemaEntry::new(
                "URL for the shared artifact cache (sccache, Bazel remote cache, etc.)",
                "",
            )
            .label("Artifact Cache URL")
            .page("Build")
            .field_type(FieldType::TextInput {
                placeholder: Some("http://cache.build.local:9000".into()),
                multiline: false,
            }),
        )
        .setting(
            "error_on_warning",
            SchemaEntry::new("Treat all compiler warnings as errors", false)
                .label("Warnings as Errors")
                .page("Build")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "lint_scripts",
            SchemaEntry::new(
                "Run the configured linter on scripting files before compilation",
                false,
            )
            .label("Lint Scripts")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "generate_compile_commands",
            SchemaEntry::new(
                "Generate compile_commands.json for clangd/LSP tooling",
                true,
            )
            .label("Generate compile_commands.json")
            .page("Build")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "hot_reload",
            SchemaEntry::new(
                "Rebuild and hot-reload modified modules while the game is running in the editor",
                false,
            )
            .label("Hot Reload")
            .page("Build")
            .field_type(FieldType::Checkbox),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
