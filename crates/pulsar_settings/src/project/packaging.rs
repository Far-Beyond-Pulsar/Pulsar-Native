use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "packaging";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Packaging", "Distribution and packaging settings")
        .setting("staging_dir",
            SchemaEntry::new("Directory where the packaged build is assembled before archiving", "dist/staged/")
                .label("Staging Directory").page("Packaging")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("output_dir",
            SchemaEntry::new("Final output directory for the packaged archive", "dist/")
                .label("Package Output Directory").page("Packaging")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("use_pak_files",
            SchemaEntry::new("Pack assets into compressed .pak archive files", true)
                .label("Use .pak Files").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("compress_assets",
            SchemaEntry::new("Compress asset data inside pak files", true)
                .label("Compress Assets").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("encrypt_pak",
            SchemaEntry::new("Encrypt pak file contents", false)
                .label("Encrypt Pak Files").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("strip_editor_content",
            SchemaEntry::new("Remove editor-only assets and metadata from the package", true)
                .label("Strip Editor Content").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("include_debug_files",
            SchemaEntry::new("Include debug symbols and PDB files in the package", false)
                .label("Include Debug Files").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("installer_type",
            SchemaEntry::new("Type of installer to create", "zip")
                .label("Installer Type").page("Packaging")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("ZIP Archive", "zip"),
                    DropdownOption::new("NSIS Installer (Windows)", "nsis"),
                    DropdownOption::new("AppImage (Linux)", "appimage"),
                    DropdownOption::new("DMG (macOS)", "dmg"),
                ]}))
        .setting("code_sign",
            SchemaEntry::new("Sign the executable with a code signing certificate", false)
                .label("Code Signing").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("signing_identity",
            SchemaEntry::new("Code signing certificate identity or fingerprint", "")
                .label("Signing Identity").page("Packaging")
                .field_type(FieldType::TextInput { placeholder: Some("Developer ID Application: Your Name".into()), multiline: false }))
        .setting("bundle_id",
            SchemaEntry::new("macOS / iOS bundle identifier in reverse-DNS format", "com.example.mygame")
                .label("Bundle ID").page("Packaging")
                .field_type(FieldType::TextInput { placeholder: Some("com.example.mygame".into()), multiline: false }))
        .setting("version_in_filename",
            SchemaEntry::new("Append the project version to the output package filename", true)
                .label("Version in Filename").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("target_platform",
            SchemaEntry::new("Default target platform for packaging operations", "auto")
                .label("Target Platform").page("Packaging")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto (current OS)", "auto"),
                    DropdownOption::new("Linux x86-64", "linux-x64"),
                    DropdownOption::new("Linux ARM64", "linux-arm64"),
                    DropdownOption::new("Windows x86-64", "win-x64"),
                    DropdownOption::new("macOS Universal", "macos"),
                    DropdownOption::new("Android ARM64", "android-arm64"),
                    DropdownOption::new("iOS", "ios"),
                    DropdownOption::new("WebAssembly", "wasm"),
                ]}))
        .setting("build_configuration",
            SchemaEntry::new("Optimization level for the packaged build", "release")
                .label("Build Configuration").page("Packaging")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Debug", "debug"),
                    DropdownOption::new("Development (debug info + opts)", "development"),
                    DropdownOption::new("Release", "release"),
                    DropdownOption::new("Shipping (no dev tools)", "shipping"),
                ]})
                .validator(Validator::string_one_of(["debug", "development", "release", "shipping"])))
        .setting("include_debug_symbols",
            SchemaEntry::new("Package debug symbol files alongside the build", false)
                .label("Include Debug Symbols").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("compress_assets",
            SchemaEntry::new("Compress asset bundles to reduce distribution size", true)
                .label("Compress Assets").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("compression_algorithm",
            SchemaEntry::new("Compression algorithm to use for asset bundles", "zstd")
                .label("Compression Algorithm").page("Packaging")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("zstd", "zstd"),
                    DropdownOption::new("lz4 (faster, larger)", "lz4"),
                    DropdownOption::new("zlib", "zlib"),
                    DropdownOption::new("Brotli (smallest, slow)", "brotli"),
                ]})
                .validator(Validator::string_one_of(["zstd", "lz4", "zlib", "brotli"])))
        .setting("strip_unused_assets",
            SchemaEntry::new("Remove unreferenced assets from the package to save space", true)
                .label("Strip Unused Assets").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("cook_on_the_fly_server",
            SchemaEntry::new("Start a content cook-on-the-fly server for testing without full packaging", false)
                .label("Cook-on-the-Fly Server").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("notarize_macos",
            SchemaEntry::new("Submit macOS builds to Apple notarization after code signing", false)
                .label("Notarize (macOS)").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("installer_type",
            SchemaEntry::new("Type of installer/package to generate for desktop targets", "none")
                .label("Installer Type").page("Packaging")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None (directory)", "none"),
                    DropdownOption::new("ZIP archive", "zip"),
                    DropdownOption::new("Debian .deb", "deb"),
                    DropdownOption::new("AppImage", "appimage"),
                    DropdownOption::new("NSIS installer (Windows)", "nsis"),
                    DropdownOption::new("MSI (Windows)", "msi"),
                    DropdownOption::new("DMG (macOS)", "dmg"),
                ]}))
        .setting("run_after_package",
            SchemaEntry::new("Automatically launch the packaged build for quick smoke testing", false)
                .label("Run After Package").page("Packaging")
                .field_type(FieldType::Checkbox))
        .setting("packaging_threads",
            SchemaEntry::new("Number of parallel threads used for packaging and cooking (0 = auto)", 0_i64)
                .label("Packaging Threads").page("Packaging")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 64)));

    let _ = cfg.register(NS, OWNER, schema);
}
