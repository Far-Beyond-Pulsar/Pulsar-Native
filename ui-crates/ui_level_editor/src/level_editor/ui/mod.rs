mod actions;
pub mod add_object_dialog;
mod bound_field;
mod component_fields_section;
mod field_bindings;
mod hierarchy;
mod material_section;
mod object_header_section;
mod object_type_fields_section;
mod panel;
pub mod properties; // New component renderer module
mod properties_panel; // Old properties panel (to be integrated)
mod state;
mod toolbar;
mod transform_section;
mod viewport;
mod world_settings;
mod world_settings_replicated;

pub use component_fields_section::ComponentFieldsSection;
pub use hierarchy::HierarchyPanel;
pub use object_header_section::ObjectHeaderSection;
pub use object_type_fields_section::ObjectTypeFieldsSection;
pub use panel::LevelEditorPanel;
pub use properties_panel::PropertiesPanel;
pub use state::*;
pub use toolbar::ToolbarPanel;
pub use transform_section::TransformSection;
pub use viewport::ViewportPanel;
pub use world_settings::WorldSettings;
pub use world_settings_replicated::WorldSettingsReplicated;
