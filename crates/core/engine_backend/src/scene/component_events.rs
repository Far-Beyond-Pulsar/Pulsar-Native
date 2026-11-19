//! What an asset's events look like to a scripting editor: the catalog an
//! [`AssetEventSource`](super::asset_event_sources::AssetEventSource) returns.
//!
//! The source knows what its events are and how to present them (the group
//! they are filed under, their text); the scripting editor draws a component
//! entry point from that, so it names neither the component nor the asset
//! kind. Which sources apply to a component, and the cache they go through,
//! are in [`asset_event_sources`](super::asset_event_sources).

use std::path::PathBuf;

use pulsar_events::gamma::EventDescriptor;

/// How a source wants one of its events presented.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventView {
    /// The palette group under the component, `Group/1a2b3c4d`.
    pub group: String,
    /// One line for the palette tooltip.
    pub description: String,
    /// The node's documentation.
    pub documentation: String,
    /// Extra node properties a compiler or a later read of the node needs
    /// (which asset the event belongs to, for example).
    pub properties: Vec<(String, String)>,
}

/// One event an asset raises.
#[derive(Clone, Debug)]
pub struct CatalogEvent {
    /// The entry's name: the node is titled "On <label>".
    pub label: String,
    pub descriptor: EventDescriptor,
    pub view: EventView,
}

/// The events an asset raises, with the files that could not be read.
#[derive(Debug, Default)]
pub struct ComponentEventCatalog {
    pub events: Vec<CatalogEvent>,
    /// Parametric events: one node standing for many events.
    pub families: Vec<EventFamily>,
    pub diagnostics: Vec<(PathBuf, String)>,
}

// ---- families: one parametric event node for many events -------------------

/// One value a parameter can take.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FamilyOption {
    /// What the node stores (stable: an asset id, never a display name).
    pub id: String,
    pub label: String,
    /// A short note the editor shows beside the label (a value shape).
    pub detail: String,
}

/// How a parameter is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Exactly one option.
    Choice,
    /// Any number of options, stored comma-separated.
    Multi,
}

/// A parameter of a family, shown as a node property.
#[derive(Clone, Debug)]
pub struct FamilyParam {
    pub name: String,
    pub label: String,
    pub kind: ParamKind,
    pub options: Vec<FamilyOption>,
    /// The value a new node starts with (option ids, comma-separated for
    /// `Multi`).
    pub default: String,
}

/// A node's parameter values by name.
pub type ParamValues = std::collections::BTreeMap<String, String>;

/// One data output of a resolved family, and how the compiler builds it from
/// the raw event's fields (`lowering`: `f64:value`, `bool:value`,
/// `vec2:x,y`, `vec3:x,y,z`, `entity:entity`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedOutput {
    pub pin: String,
    pub type_name: String,
    pub lowering: String,
}

/// What a family's parameters mean: an execution output per event it listens
/// to, and the typed data outputs they share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedFamily {
    /// `(execution pin, event name)`.
    pub arms: Vec<(String, String)>,
    pub outputs: Vec<ResolvedOutput>,
}

/// A parametric event: one node that stands for many events, chosen by its
/// parameters (a choice and the phases to hear). The palette offers one
/// entry per family however many choices it has; a compile resolves only the
/// nodes a graph uses.
#[derive(Clone, Debug)]
pub struct EventFamily {
    /// Stable id, `domain.name`; the node type is `event::family::<id>`.
    pub id: String,
    /// The node's title, "On <Label>".
    pub label: String,
    /// The palette group under the component.
    pub group: String,
    pub description: String,
    pub documentation: String,
    pub params: Vec<FamilyParam>,
    /// Every event the family can raise, so the compiler can check a node's
    /// events whatever its parameters.
    pub events: Vec<EventDescriptor>,
    /// Pure: the same family and values give the same answer.
    pub resolve: fn(&EventFamily, &ParamValues) -> Result<ResolvedFamily, String>,
}

impl EventFamily {
    /// The parameter values a new node starts with.
    pub fn defaults(&self) -> ParamValues {
        self.params
            .iter()
            .map(|param| (param.name.clone(), param.default.clone()))
            .collect()
    }

    pub fn param(&self, name: &str) -> Option<&FamilyParam> {
        self.params.iter().find(|param| param.name == name)
    }
}
