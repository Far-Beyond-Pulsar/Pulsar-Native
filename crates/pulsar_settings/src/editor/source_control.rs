use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "source_control";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Source Control", "Integrated version control settings")
        .setting("provider",
            SchemaEntry::new("Source control backend to use", "git")
                .label("Provider").page("Source Control")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Git", "git"),
                    DropdownOption::new("Perforce", "perforce"),
                    DropdownOption::new("Plastic SCM", "plastic"),
                    DropdownOption::new("None", "none"),
                ]})
                .validator(Validator::string_one_of(["git", "perforce", "plastic", "none"])))
        .setting("auto_checkout_on_edit",
            SchemaEntry::new("Automatically check out locked files when they are edited", false)
                .label("Auto Checkout on Edit").page("Source Control")
                .field_type(FieldType::Checkbox))
        .setting("show_changelists",
            SchemaEntry::new("Display changelists alongside files in the content browser", true)
                .label("Show Changelists").page("Source Control")
                .field_type(FieldType::Checkbox))
        .setting("require_commit_message_template",
            SchemaEntry::new("Require a template commit message when checking in", false)
                .label("Require Commit Template").page("Source Control")
                .field_type(FieldType::Checkbox))
        .setting("auto_fetch",
            SchemaEntry::new("Periodically fetch remote changes in the background", true)
                .label("Auto Fetch").page("Source Control")
                .field_type(FieldType::Checkbox))
        .setting("auto_fetch_interval_minutes",
            SchemaEntry::new("Minutes between background fetch operations", 5_i64)
                .label("Fetch Interval (min)").page("Source Control")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(60.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 60)))
        .setting("git_executable_path",
            SchemaEntry::new("Path to the git binary (leave blank to use PATH)", "")
                .label("Git Path").page("Source Control")
                .field_type(FieldType::TextInput { placeholder: Some("/usr/bin/git".into()), multiline: false }))
        .setting("perforce_server",
            SchemaEntry::new("Perforce server address (host:port)", "")
                .label("Perforce Server").page("Source Control")
                .field_type(FieldType::TextInput { placeholder: Some("localhost:1666".into()), multiline: false }))
        .setting("perforce_user",
            SchemaEntry::new("Perforce workspace user name", "")
                .label("Perforce User").page("Source Control")
                .field_type(FieldType::TextInput { placeholder: Some("user".into()), multiline: false }))
        .setting("show_gutter_blame",
            SchemaEntry::new("Show inline blame annotations in the code editor gutter", false)
                .label("Gutter Blame").page("Source Control")
                .field_type(FieldType::Checkbox))
        .setting("diff_tool",
            SchemaEntry::new("External diff/merge tool to launch for conflict resolution", "builtin")
                .label("Diff Tool").page("Source Control")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Built-in", "builtin"),
                    DropdownOption::new("VS Code", "vscode"),
                    DropdownOption::new("Meld", "meld"),
                    DropdownOption::new("KDiff3", "kdiff3"),
                    DropdownOption::new("P4Merge", "p4merge"),
                    DropdownOption::new("Custom", "custom"),
                ]}))
        .setting("custom_diff_tool_path",
            SchemaEntry::new("Path to custom diff/merge tool executable", "")
                .label("Custom Diff Tool Path").page("Source Control")
                .field_type(FieldType::TextInput { placeholder: Some("/usr/bin/meld".into()), multiline: false }))
        .setting("large_file_threshold_kb",
            SchemaEntry::new("Files above this size (KB) trigger a large-file warning", 10240_i64)
                .label("Large File Threshold (KB)").page("Source Control")
                .field_type(FieldType::NumberInput { min: Some(512.0), max: Some(1_048_576.0), step: Some(512.0) })
                .validator(Validator::int_range(512, 1_048_576)))
        .setting("use_git_lfs",
            SchemaEntry::new("Track large binary files with Git LFS", false)
                .label("Use Git LFS").page("Source Control")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
