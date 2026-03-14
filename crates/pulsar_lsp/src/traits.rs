//! Core LSP traits for Pulsar.
//!
//! `LspBackend`, `DefinitionProvider`, and `HoverProvider` are defined here.
//!
//! `CompletionProvider` and `CodeActionProvider` remain in `crates/ui` because they
//! reference `InputState`, which would create a circular dependency if moved here.

use std::path::Path;

use anyhow::Result;
use gpui::{App, Task, Window};
use ropey::Rope;

use crate::rust_analyzer::AnalyzerStatus;

/// Generic trait for an LSP backend (rust-analyzer, clangd, …).
///
/// The GPUI-aware methods on [`RustAnalyzerManager`] that require
/// `Window`/`Context<Self>` are intentionally separate; this trait covers the
/// minimal surface needed to query status and drive lifecycle from non-GPUI code.
pub trait LspBackend: Send + Sync {
    fn start(&mut self, workspace: &Path) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn status(&self) -> &AnalyzerStatus;
}

/// textDocument/definition provider.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition
pub trait DefinitionProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::LocationLink>>>;
}

/// textDocument/hover provider.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_hover
pub trait HoverProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>>;
}
