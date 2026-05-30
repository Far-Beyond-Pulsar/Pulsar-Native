//! Shared diagnostic types for the Pulsar engine.
//!
//! Types are now canonical in `ui::diagnostics`.  This crate re-exports them
//! so that existing code that imports from `pulsar_diagnostics` continues to
//! work without changes.

pub use ui::diagnostics::{
    CodeAction, Diagnostic, DiagnosticSeverity, DiagnosticSink, DiagnosticSource, TextEdit,
};
