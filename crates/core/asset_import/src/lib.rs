//! Source-asset import pipeline.
//!
//! A project may hold raw source files (an FBX, an OBJ) next to the native
//! assets built from them. This crate decides what to do with them:
//!
//! - [`scan`] finds importable sources the project has no record of, and linked
//!   sources whose file changed since they were imported.
//! - [`db`] is the import database (`.pulsar/import_db.json`): for each *linked*
//!   source, its path, content hash and the native asset it produced.
//! - [`importer`] is the per-format conversion, selected by extension.
//! - [`service`] runs conversions as tasks in the editor task queue, in one of
//!   two modes: [`service::ImportMode::ConvertInPlace`] (the source is replaced
//!   by the native asset and no record is kept) or [`service::ImportMode::Link`]
//!   (the source stays, a record is added, and later edits to it are detected).

pub mod db;
pub mod importer;
pub mod scan;
pub mod service;

pub use db::{ImportDb, ImportRecord, LinkStatus};
pub use importer::{importer_for, Importer, OptionValues};
pub use scan::{scan_project, ScanReport};
pub use service::{ignore_sources, submit_import, submit_reimport, ImportMode};
