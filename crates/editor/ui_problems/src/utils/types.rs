use std::path::PathBuf;

use gpui::*;
use ui::ActiveTheme as _;
use ui::IconName;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLineType {
    Unchanged,
    Added,
    Deleted,
    Spacer,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    pub hints: Vec<Hint>,
    pub subitems: Vec<Diagnostic>,
    pub loading_actions: bool,
}

#[derive(Clone, Debug)]
pub struct Hint {
    pub message: String,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
    pub file_path: Option<String>,
    pub line: Option<usize>,
    pub loading: bool,
}

#[derive(Clone, Debug)]
pub struct NavigateToDiagnostic {
    pub file_path: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn icon(&self) -> IconName {
        match self {
            Self::Error => IconName::Close,
            Self::Warning => IconName::TriangleAlert,
            Self::Information => IconName::Info,
            Self::Hint => IconName::Info,
        }
    }

    pub fn color(&self, cx: &App) -> Hsla {
        match self {
            Self::Error => Hsla {
                h: 0.0,
                s: 0.85,
                l: 0.55,
                a: 1.0,
            },
            Self::Warning => Hsla {
                h: 38.0,
                s: 0.95,
                l: 0.55,
                a: 1.0,
            },
            Self::Information => Hsla {
                h: 210.0,
                s: 0.80,
                l: 0.60,
                a: 1.0,
            },
            Self::Hint => cx.theme().muted_foreground,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Information => "Info",
            Self::Hint => "Hint",
        }
    }
}
