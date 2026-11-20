//! Host-discovered component event metadata for editor plugins.
//!
//! The host fills this catalog from its component registrations before
//! creating editor views. Dynamic editor plugins read the GPUI global instead
//! of walking their own `inventory` copy, which may not contain registrations
//! linked into the host executable.

#[cfg(feature = "editor")]
use gpui::Global;
pub use pulsar_language::ComponentEventMetadata;

/// Host-owned catalog of component event declarations visible in the editor.
#[derive(Clone, Debug, Default)]
pub struct ComponentEventCatalog {
    pub events: Vec<ComponentEventMetadata>,
}

#[cfg(feature = "editor")]
impl Global for ComponentEventCatalog {}
