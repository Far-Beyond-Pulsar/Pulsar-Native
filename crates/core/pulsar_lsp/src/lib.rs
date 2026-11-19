//! `pulsar_lsp`: the language-server layer.
//!
//! * [`LanguageServer`]: the generic client for one `(server, root)`:
//!   framing, request/response ids kept apart from the server's own, the
//!   server's requests answered, documents refcounted with incremental sync,
//!   progress forwarded, readiness by the spec's rule.
//! * [`LanguageServers`]: the service that routes a file to its server (the
//!   language registry says which language owns it, the language carries the
//!   server spec), creates and starts servers on demand, and owns the shared
//!   [`DiagnosticsStore`] (file by source).
//! * `pulsar_language::LanguageServerSpec`: what differs per server, carried
//!   by a `Language` registration. This crate names no server: each server's
//!   plugin brings its own spec.
//! * [`edit`]: the language-neutral `WorkspaceEdit` applier.
//! * [`path_utils`]: path and URI helpers.
//! * [`testing`]: a server scripted on a pipe, for tests of what talks to one.

pub mod capabilities;
pub mod code_actions;
pub mod diagnostics;
pub mod documents;
pub mod edit;
pub mod path_utils;
pub mod protocol;
pub mod server;
pub mod service;
pub mod session;
pub mod testing;

pub use diagnostics::{
    Counts, DiagnosticSource, DiagnosticsData, DiagnosticsEvent, DiagnosticsStore, ServerId,
};
pub use edit::{BufferEdits, EditHost, LspTextEdit, WorkspaceEdit};
pub use path_utils::{path_key, path_to_uri, uri_key, uri_to_path};
pub use server::{location_target, LanguageServer};
pub use service::{
    active_servers, compiler_diagnostics, LanguageServers, LanguageServersEvent, COMPILER_SOURCE,
};
pub use session::{ServerEvent, ServerStatus, StopReason};
