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

#[derive(Action, Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct CreateAsset {
    #[serde(default)]
    pub file_type_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub extension: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub default_content: serde_json::Value,
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

// New context menu actions
#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct OpenInFileManager {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct OpenTerminalHere {
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct ValidateAsset {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct ToggleFavorite {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct ToggleGitignore {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct ToggleHidden {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct ShowHistory {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct CheckMultiuserSync {
    #[serde(default)]
    pub item_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct SetColorOverride {
    #[serde(default)]
    pub item_path: String,
    pub color: Option<ColorData>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct ColorData {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
