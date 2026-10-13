//! The two file types: input actions and input mappings.

use plugin_editor_api::{FileStructure, FileTypeDefinition, FileTypeId};

pub const ACTION_ID: &str = "input_action";
pub const ACTION_EXT: &str = crate::model::action::ACTION_EXTENSION;
pub const MAPPING_ID: &str = "input_mapping";
pub const MAPPING_EXT: &str = crate::model::mapping::MAPPING_EXTENSION;

pub fn file_types() -> Vec<FileTypeDefinition> {
    vec![
        FileTypeDefinition {
            id: FileTypeId::new(ACTION_ID),
            extension: ACTION_EXT.to_string(),
            display_name: "Input Action".to_string(),
            icon: ui::IconName::Flash,
            color: gpui::rgb(0x4ECDC4).into(),
            structure: FileStructure::Standalone,
            // An empty file: the editor gives it a stable id the first time
            // it is opened, so two actions made from this template never
            // share one.
            default_content: serde_json::Value::Null,
            categories: vec!["Input".to_string()],
            creation_directory: Some("input".to_string()),
        },
        FileTypeDefinition {
            id: FileTypeId::new(MAPPING_ID),
            extension: MAPPING_EXT.to_string(),
            display_name: "Input Mapping".to_string(),
            icon: ui::IconName::Key,
            color: gpui::rgb(0xF4C542).into(),
            structure: FileStructure::Standalone,
            default_content: serde_json::Value::Null,
            categories: vec!["Input".to_string()],
            creation_directory: Some("input".to_string()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_types_can_be_created_from_the_menu_and_dont_clash() {
        let types = file_types();
        assert_eq!(types.len(), 2);
        assert!(types.iter().all(|t| t.is_creatable()));
        assert_ne!(types[0].extension, types[1].extension);
        assert_ne!(types[0].id, types[1].id);
        assert_eq!(types[0].extension, "inputaction");
        assert_eq!(types[1].extension, "inputmap");
    }
}
