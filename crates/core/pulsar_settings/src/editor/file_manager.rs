use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "editor";
pub const OWNER: &str = "file_manager";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("File Manager", "Project file drawer preferences")
        .setting("view_mode", SchemaEntry::new("File layout", "grid").label("View Mode").page("File Manager").field_type(FieldType::Dropdown { options: vec![DropdownOption::new("Grid", "grid"), DropdownOption::new("List", "list")] }))
        .setting("sort_by", SchemaEntry::new("Sort files by", "name").label("Sort By").page("File Manager").field_type(FieldType::Dropdown { options: vec![DropdownOption::new("Name", "name"), DropdownOption::new("Modified", "modified"), DropdownOption::new("Size", "size"), DropdownOption::new("Type", "type")] }))
        .setting("sort_order", SchemaEntry::new("File sort direction", "ascending").label("Sort Order").page("File Manager").field_type(FieldType::Dropdown { options: vec![DropdownOption::new("Ascending", "ascending"), DropdownOption::new("Descending", "descending")] }))
        .setting("show_hidden_files", SchemaEntry::new("Show hidden files", false).label("Show Hidden Files").page("File Manager").field_type(FieldType::Checkbox));
    let _ = cfg.register(NS, OWNER, schema);
}
