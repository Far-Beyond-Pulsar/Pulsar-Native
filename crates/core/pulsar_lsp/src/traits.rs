//! LSP traits for Pulsar.
//!
//! `HoverProvider` and `DefinitionProvider` are now defined in the `ui` crate
//! so that the UI has zero dependency on any engine crate.  This module
//! re-exports them for backwards compatibility and so that `pulsar_lsp`
//! implementations can refer to a single canonical location.
//!
//! `LspBackend` remains here because it references engine-internal types.

use std::path::Path;

use anyhow::Result;

use crate::rust_analyzer::AnalyzerStatus;

// Re-export the UI-defined traits so engine code can `use pulsar_lsp::traits::*`
// (ui::input re-exports the lsp module publicly via `pub use lsp::*`)
pub use ui::input::{DefinitionProvider, HoverProvider};

/// Generic trait for an LSP backend (rust-analyzer, clangd, …).
pub trait LspBackend: Send + Sync {
    fn start(&mut self, workspace: &Path) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn status(&self) -> &AnalyzerStatus;
}
