//! Core types used throughout the application

/// Editor type enumeration
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorType {
    Script,
    Level,
    Daw,
}

impl EditorType {
    pub fn display_name(&self) -> &'static str {
        match self {
            EditorType::Script => "Script Editor",
            EditorType::Level => "Level Editor",
            EditorType::Daw => "DAW Editor",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            EditorType::Script => "Code editor with LSP support",
            EditorType::Level => "3D level design and placement",
            EditorType::Daw => "Digital audio workstation",
        }
    }
}
