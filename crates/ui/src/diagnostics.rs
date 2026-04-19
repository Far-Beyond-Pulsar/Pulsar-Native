//! Diagnostic types for LSP and code analysis.
//!
//! All types are re-exported from `pulsar_diagnostics` so that downstream
//! crates share a single canonical definition and avoid type-mismatch errors
//! when passing diagnostics between `engine_backend`, `pulsar_lsp`, and the UI.

pub use pulsar_diagnostics::{CodeAction, Diagnostic, DiagnosticSeverity, TextEdit};
