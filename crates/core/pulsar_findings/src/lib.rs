//! Findings: what the editor's Problems panel lists besides code
//! diagnostics. A finding is one actionable fact about the project, the
//! scene, the renderer or the editor (a budget that is full, a missing
//! import, a plugin whose contract is unmet, a stalled task), with where it
//! is and what the person can do about it.
//!
//! Sources publish their **complete** set under their own name
//! ([`publish`]); publishing an empty set removes the source. The panel
//! reads [`snapshot`] when [`generation`] moves. Anything can publish: the
//! editor's collectors (which poll the engine's existing signals) and
//! plugins, through a [`FindingSource`] registration the collector polls
//! while the plugin is enabled. The store names no plugin and no kind.

mod model;
mod source;
mod store;

pub use model::{Action, Category, Finding, FindingAction, Location, SettingsWindow, Severity};
pub use source::{FindingSource, collect_registered};
pub use store::{Published, generation, publish, snapshot};
