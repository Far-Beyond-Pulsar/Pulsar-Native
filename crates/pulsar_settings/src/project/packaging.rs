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
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
