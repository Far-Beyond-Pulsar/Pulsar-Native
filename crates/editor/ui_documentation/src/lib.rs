//! Documentation window: engine API docs, project docs and editable guides.
//!
//! Owns: the three documentation sources (`utils::engine_docs`,
//! `utils::project_docs`, `utils::manual_docs`), the flattening of each source's
//! tree into sidebar rows (`utils::rows`) and the `DocumentationWindow` view.
//!
//! Entry points: `create_documentation_window` and
//! `create_documentation_window_with_project`; `DocSource` is the trait a source
//! implements for its search box and initial content.
//!
//! Must not depend on: the editor shell or other `ui_*` feature crates. Doc
//! content comes from `pulsar_docs` (built-in and parsed project docs) and the
//! project's `docs/` folder; this crate never generates documentation itself.

pub mod components;
pub mod handlers;
mod screen;
pub mod utils;

pub use screen::{
    DocumentationWindow, create_documentation_window, create_documentation_window_with_project,
};
pub use utils::doc_source::DocSource;
