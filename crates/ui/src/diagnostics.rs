//! Diagnostic types for LSP and code analysis

use serde::{Deserialize, Serialize};

/// A text edit representing a change to be made to a file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    /// The file path this edit applies to
    pub file_path: String,
    /// Start line (1-indexed)
    pub start_line: usize,
    /// Start column (1-indexed)
    pub start_column: usize,
    /// End line (1-indexed)
    pub end_line: usize,
    /// End column (1-indexed)
    pub end_column: usize,
    /// The new text to replace the range with
    pub new_text: String,
}

/// A code action / quick fix that can be applied
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeAction {
    /// Title of the action (e.g., "Remove unused import")
    pub title: String,
    /// The text edits to apply
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    /// End line of the diagnostic range (1-indexed)
    pub end_line: Option<usize>,
    /// End column of the diagnostic range (1-indexed)
    pub end_column: Option<usize>,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub code: Option<String>,
    pub source: Option<String>,
    /// Quick fixes / code actions associated with this diagnostic
    pub code_actions: Vec<CodeAction>,
    /// Raw JSON of the original LSP diagnostic (for code action requests)
    #[serde(skip)]
    pub raw_lsp_diagnostic: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl std::fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticSeverity::Error => write!(f, "Error"),
            DiagnosticSeverity::Warning => write!(f, "Warning"),
            DiagnosticSeverity::Information => write!(f, "Information"),
            DiagnosticSeverity::Hint => write!(f, "Hint"),
        }
    }
}
