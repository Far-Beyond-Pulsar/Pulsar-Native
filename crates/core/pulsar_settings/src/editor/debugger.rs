use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "editor";
pub const OWNER: &str = "debugger";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Debugger", "Integrated debugger and profiler settings")
        .setting(
            "pie_on_script_error",
            SchemaEntry::new(
                "What Play-in-Editor does when a script raises an error: keep running and report it in the Problems panel, or also pause the game there",
                "continue",
            )
            .label("On Script Error During Play")
            .page("Debugger")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("Continue and report", "continue"),
                    DropdownOption::new("Pause the game", "pause"),
                ],
            }),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
