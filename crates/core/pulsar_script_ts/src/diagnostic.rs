//! Problems found compiling a class, with source positions.

use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

/// One problem. `line` and `column` are 1-based; `0` means "no position".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub line: u32,
    pub column: u32,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, line: u32, column: u32) -> Self {
        Self { severity: Severity::Error, message: message.into(), line, column }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// `line:column`, for editors and `CompileDiagnostic::location`.
    pub fn location(&self) -> Option<String> {
        (self.line > 0).then(|| format!("{}:{}", self.line, self.column))
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.location() {
            Some(at) => write!(f, "{kind} {at}: {}", self.message),
            None => write!(f, "{kind}: {}", self.message),
        }
    }
}

/// Byte offsets to line and column.
pub(crate) struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub(crate) fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(source.match_indices('\n').map(|(i, _)| i as u32 + 1));
        Self { starts }
    }

    /// 1-based `(line, column)`, the column counted in characters.
    pub(crate) fn position(&self, source: &str, offset: u32) -> (u32, u32) {
        let line = self.starts.partition_point(|&start| start <= offset).saturating_sub(1);
        let start = self.starts[line] as usize;
        let end = (offset as usize).min(source.len());
        let column = source.get(start..end).map_or(0, |text| text.chars().count());
        (line as u32 + 1, column as u32 + 1)
    }
}
