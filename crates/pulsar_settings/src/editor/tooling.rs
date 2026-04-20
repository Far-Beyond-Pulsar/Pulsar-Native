use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "tooling";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Tooling", "Editor productivity and workflow settings")
        .setting("autosave_interval_seconds",
            SchemaEntry::new("Seconds between automatic editor saves", 120_i64)
                .label("Autosave Interval (s)").page("Tooling")
                .field_type(FieldType::NumberInput { min: Some(15.0), max: Some(3600.0), step: Some(15.0) })
                .validator(Validator::int_range(15, 3600)))
        .setting("max_undo_steps",
            SchemaEntry::new("Maximum depth of the undo history stack", 256_i64)
                .label("Undo History Depth").page("Tooling")
                .field_type(FieldType::NumberInput { min: Some(32.0), max: Some(8192.0), step: Some(32.0) })
                .validator(Validator::int_range(32, 8192)))
        .setting("live_blueprint_compile",
            SchemaEntry::new("Recompile visual scripts as you edit them", true)
                .label("Live Blueprint Compile").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("enable_asset_thumbnails",
            SchemaEntry::new("Render preview thumbnails in asset browsers", true)
                .label("Asset Thumbnails").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("thumbnail_size",
            SchemaEntry::new("Default thumbnail size in the asset browser (px)", 128_i64)
                .label("Thumbnail Size").page("Tooling")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Small (64px)", "64"),
                    DropdownOption::new("Medium (128px)", "128"),
                    DropdownOption::new("Large (256px)", "256"),
                    DropdownOption::new("Extra Large (512px)", "512"),
                ]}))
        .setting("diagnostics_level",
            SchemaEntry::new("Verbosity of in-editor diagnostics messages", "standard")
                .label("Diagnostics Level").page("Tooling")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Quiet", "quiet"),
                    DropdownOption::new("Standard", "standard"),
                    DropdownOption::new("Verbose", "verbose"),
                ]})
                .validator(Validator::string_one_of(["quiet", "standard", "verbose"])))
        .setting("show_breadcrumbs",
            SchemaEntry::new("Show navigation breadcrumbs above the code editor", true)
                .label("Show Breadcrumbs").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("explorer_auto_reveal",
            SchemaEntry::new("Auto-reveal active file in the file explorer", true)
                .label("Explorer Auto Reveal").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("confirm_delete",
            SchemaEntry::new("Ask for confirmation before deleting assets or files", true)
                .label("Confirm Delete").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("copy_relative_path",
            SchemaEntry::new("Copy paths relative to project root by default", true)
                .label("Copy Relative Path").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("smooth_scrolling",
            SchemaEntry::new("Use smooth momentum scrolling in lists and editors", true)
                .label("Smooth Scrolling").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("find_in_files_max_results",
            SchemaEntry::new("Maximum number of results shown in Find in Files", 5000_i64)
                .label("Max Find Results").page("Tooling")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(100_000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 100_000)))
        .setting("restore_layout_on_open",
            SchemaEntry::new("Restore the previous editor layout when opening a project", true)
                .label("Restore Layout").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("show_welcome_on_startup",
            SchemaEntry::new("Show the welcome screen when no project is open", true)
                .label("Welcome Screen").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("external_build_system",
            SchemaEntry::new("Build system used by the editor's build button", "cargo")
                .label("Build System").page("Tooling")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Cargo", "cargo"),
                    DropdownOption::new("Make", "make"),
                    DropdownOption::new("CMake", "cmake"),
                    DropdownOption::new("Ninja", "ninja"),
                    DropdownOption::new("Bazel", "bazel"),
                    DropdownOption::new("Custom", "custom"),
                ]}))
        .setting("custom_build_command",
            SchemaEntry::new("Custom build command run when build system is set to 'custom'", "")
                .label("Custom Build Command").page("Tooling")
                .field_type(FieldType::TextInput { placeholder: Some("./build.sh --release".into()), multiline: false }))
        .setting("lsp_enabled",
            SchemaEntry::new("Enable the built-in Language Server Protocol client for code intelligence", true)
                .label("LSP Client").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("rust_analyzer_path",
            SchemaEntry::new("Path to a custom rust-analyzer binary (empty = PATH lookup)", "")
                .label("rust-analyzer Binary").page("Tooling")
                .field_type(FieldType::TextInput { placeholder: Some("/usr/bin/rust-analyzer".into()), multiline: false }))
        .setting("formatter_enabled",
            SchemaEntry::new("Run the code formatter on save", true)
                .label("Format on Save").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("formatter_command",
            SchemaEntry::new("Custom formatter command run on save (empty = rustfmt for Rust files)", "")
                .label("Formatter Command").page("Tooling")
                .field_type(FieldType::TextInput { placeholder: Some("rustfmt --edition 2021".into()), multiline: false }))
        .setting("clippy_on_save",
            SchemaEntry::new("Run Clippy after each save and surface lints as diagnostics", false)
                .label("Clippy on Save").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("test_runner",
            SchemaEntry::new("Test framework used by the integrated test explorer", "cargo-test")
                .label("Test Runner").page("Tooling")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("cargo test", "cargo-test"),
                    DropdownOption::new("nextest", "nextest"),
                    DropdownOption::new("pytest", "pytest"),
                    DropdownOption::new("jest", "jest"),
                    DropdownOption::new("Custom", "custom"),
                ]}))
        .setting("custom_test_command",
            SchemaEntry::new("Custom test command when test runner is 'custom'", "")
                .label("Custom Test Command").page("Tooling")
                .field_type(FieldType::TextInput { placeholder: Some("./run_tests.sh".into()), multiline: false }))
        .setting("profiler_integration",
            SchemaEntry::new("Launch the external profiler alongside game-in-editor runs", false)
                .label("Profiler Integration").page("Tooling")
                .field_type(FieldType::Checkbox))
        .setting("task_shortcuts",
            SchemaEntry::new("Path to a JSON file defining custom task shortcuts shown in the toolbar", "")
                .label("Task Shortcuts File").page("Tooling")
                .field_type(FieldType::TextInput { placeholder: Some("config/tasks.json".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
