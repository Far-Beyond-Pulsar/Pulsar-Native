//! Main Editor UI
//!
//! Main editor window with tabs and drawers

// Drawers and tabs are in the main editor module
pub mod drawers;
pub mod tabs;
pub mod editors;
pub mod registry_blueprint;
pub mod registry_script;
pub mod registry_daw;
pub mod registry_level;

// Re-export main types
pub use drawers::{FileManagerDrawer, TerminalDrawer, ProblemsDrawer};
pub use ui_file_manager::{FileSelected, FileType as DrawerFileType, PopoutFileManagerEvent};
pub use tabs::{
    ScriptEditorPanel, LevelEditorPanel, BlueprintEditorPanel, DawEditorPanel,
    TextEditorEvent,
};

// Re-export registry types
pub use registry_blueprint::{BlueprintEditorType, BlueprintClassAssetType, BlueprintFunctionAssetType};
pub use registry_script::{ScriptEditorType, RustScriptAssetType, LuaScriptAssetType, ShaderAssetType};
pub use registry_daw::{DawEditorType, DawProjectAssetType};
pub use registry_level::{LevelEditorType, LevelAssetType};
