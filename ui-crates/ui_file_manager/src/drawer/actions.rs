use gpui::*;
use schemars::JsonSchema;
use serde::Deserialize;

// ============================================================================
// ACTIONS - All keyboard shortcuts and file manager operations
// ============================================================================

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewFolder {
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewFile {
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewClass {
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct DeleteItem {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct RenameItem {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct DuplicateItem {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CommitRename;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CancelRename;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct RefreshFileManager;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CollapseAllFolders;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct ExpandAllFolders;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct ToggleHiddenFiles;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct ToggleViewMode;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct Copy;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct Cut;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct Paste;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct SelectAll;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct Delete;
