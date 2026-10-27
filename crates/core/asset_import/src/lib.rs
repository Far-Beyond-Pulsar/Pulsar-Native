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
//! - [`rig`] turns a rigged model into linked skeletal assets (a `.skeleton`, a
//!   `.skmesh` and one `.animseq` per animation) in place of the static `.mesh`.
//! - [`service`] runs conversions as tasks in the editor task queue, in one of
//!   two modes: [`service::ImportMode::ConvertInPlace`] (the source is replaced
//!   by the native asset and no record is kept) or [`service::ImportMode::Link`]
//!   (the source stays, a record is added, and later edits to it are detected).
//! - [`state`] is each file's relation to the database (linked, out of date,
//!   ignored, …), for UI that offers import actions per file.

pub mod companion;
pub mod db;
pub mod importer;
pub mod rig;
pub mod scan;
pub mod service;
pub mod state;

pub use companion::{nearby_images, offers_for, register_companion, CompanionOffer};
pub use db::{ImportDb, ImportOutput, ImportRecord, LinkStatus};
pub use importer::{importer_for, Imported, ImportedPart, Importer, OptionValues, Previous};
pub use scan::{affects_scan, scan_project, OfferedOnce, ScanReport};
pub use service::{
    ignore_sources, submit_import, submit_import_with_companions, submit_reimport, submit_scan,
    unignore_sources, unlink_source, ImportMode, TRASH_DIR,
};
pub use state::{db_revision, import_states, is_importable, FileImportState};
