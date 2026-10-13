//! The widget file type.

use plugin_editor_api::{FileStructure, FileTypeDefinition, FileTypeId};

pub const WIDGET_ID: &str = "widget";
pub const WIDGET_EXT: &str = crate::library::WIDGET_EXTENSION;

pub fn file_types() -> Vec<FileTypeDefinition> {
    vec![FileTypeDefinition {
        id: FileTypeId::new(WIDGET_ID),
        extension: WIDGET_EXT.to_string(),
        display_name: "Widget".to_string(),
        icon: ui::IconName::Component,
        color: gpui::rgb(0x9B7BFF).into(),
        structure: FileStructure::Standalone,
        // An empty file: the editor gives it a root and a stable id the first
        // time it is opened, so two widgets made from this template never
        // share an id.
        default_content: serde_json::Value::Null,
        categories: vec!["UI".to_string()],
        creation_directory: Some("ui".to_string()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_widget_type_can_be_created_from_the_menu() {
        let types = file_types();
        assert_eq!(types.len(), 1);
        assert!(types[0].is_creatable());
        assert_eq!(types[0].extension, "widget");
    }
}
