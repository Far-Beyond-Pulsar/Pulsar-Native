//! Host-discovered component event metadata for editor plugins.
//!
//! The host fills this catalog from its component registrations before
//! creating editor views. Dynamic editor plugins read the GPUI global instead
//! of walking their own `inventory` copy, which may not contain registrations
//! linked into the host executable.

use gpui::Global;
use pulsar_script_vm::EventDecl;

/// A component event exposed to visual scripting editors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentEventMetadata {
    /// Registered reflected component class name.
    pub component_class: String,
    /// Stable event name and unchanged local payload field types.
    pub event: EventDecl,
}

/// Host-owned catalog of component event declarations visible in the editor.
#[derive(Clone, Debug, Default)]
pub struct ComponentEventCatalog {
    pub events: Vec<ComponentEventMetadata>,
}

impl Global for ComponentEventCatalog {}
