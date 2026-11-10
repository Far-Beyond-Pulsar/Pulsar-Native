/// One user-defined type alias (`.alias.json`) the drawer lists.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeInfo {
    pub uuid: uuid::Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    /// File the alias is defined in.
    pub file_path: std::path::PathBuf,
    /// The file type from the plugin registry.
    pub file_type_id: plugin_editor_api::FileTypeId,
    pub last_modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug)]
pub struct NavigateToType {
    pub file_path: Option<std::path::PathBuf>,
    pub type_name: String,
}
