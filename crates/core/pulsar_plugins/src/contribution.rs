//! What a plugin contributes, read from its real registrations.
//!
//! Every registration type summarizes itself ([`Contribution`]): the
//! extension point it is for (its category), a stable id within its plugin
//! and category, a label and its origin. [`crate::registration!`] lists each
//! type as a [`ContributionSource`], so [`contributions`] reaches every
//! registration type linked into the binary without naming one, the way an
//! IntelliJ extension area lists every extension point and each extension's
//! plugin. The summaries come from the registrations' own fields, never from
//! a second list a plugin writes by hand, so they cannot drift from what is
//! registered.
//!
//! This module knows no registration type: each owner (the render features
//! in `engine_backend`, the settings groups in `pulsar_settings`, the level
//! editor's extension points in `ui_level_editor`...) implements
//! [`Contribution`] next to its type. The host's catalog
//! (`plugin_manager::catalog`) groups the summaries by plugin.

use crate::Registration;

/// What the catalog shows of one registration, derived from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// The extension point it is for ([`Contribution::CATEGORY`]):
    /// `render.features`, `settings.group`, `editor.create_entry`...
    pub category: &'static str,
    /// Stable within its plugin and category: a feature id, a class name, a
    /// settings group id, an entry id.
    pub id: String,
    /// What a person reads.
    pub label: String,
    /// The package that submitted it ([`crate::origin!`]): a plugin's root,
    /// or an engine crate. Its provenance, kept as it is.
    pub origin: &'static str,
    /// What it brings in turn (a render feature's nodes), with the same
    /// origin.
    pub children: Vec<Summary>,
}

impl Summary {
    /// A summary of `category` with no children.
    pub fn new(
        category: &'static str,
        id: impl Into<String>,
        label: impl Into<String>,
        origin: &'static str,
    ) -> Self {
        Self {
            category,
            id: id.into(),
            label: label.into(),
            origin,
            children: Vec::new(),
        }
    }

    /// This summary and its children, depth first.
    pub fn flatten(&self) -> Vec<&Summary> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.flatten());
        }
        out
    }
}

/// A registration's summary, from its own fields.
pub trait Contribution {
    /// The extension point the type registers into, as a stable dotted id
    /// (`render.features`). One per type.
    const CATEGORY: &'static str;

    /// The registration type's contract version: bumped when a change
    /// breaks plugin libraries built against the old one (its layout
    /// fingerprint, `crate::library::fingerprint`, carries it).
    const VERSION: u32 = 1;

    /// The package that submitted it ([`crate::origin!`]).
    fn origin(&self) -> &'static str;

    /// Stable within its plugin and category (a duplicate is a catalog
    /// problem).
    fn id(&self) -> String;

    /// What a person reads; the id by default.
    fn label(&self) -> String {
        self.id()
    }

    /// What it brings in turn (a render feature's nodes); none by default.
    fn children(&self) -> Vec<Summary> {
        Vec::new()
    }

    /// The summary of this registration.
    fn summary(&self) -> Summary {
        Summary {
            category: Self::CATEGORY,
            id: self.id(),
            label: self.label(),
            origin: self.origin(),
            children: self.children(),
        }
    }
}

/// One registration type, as the catalog reads it. Submitted by
/// [`crate::registration!`] for each type it declares (a registry with its
/// own gate, such as SceneDB's world components, submits one by hand).
pub struct ContributionSource {
    /// The type's [`Contribution::CATEGORY`].
    pub category: &'static str,
    /// Every linked registration of the type, whatever its plugin's state.
    pub all: fn() -> Vec<Summary>,
    /// The registrations its registry hands out: those passing the gate.
    pub active: fn() -> Vec<Summary>,
    /// The registrations linked at compile time (`inventory::submit!`),
    /// not added by a plugin's build: the engine's own. A plugin's here is
    /// one that bypasses its root's build (`plugins/installed`'s
    /// `installed_set` fails on it).
    pub linked: fn() -> Vec<Summary>,
}

inventory::collect!(ContributionSource);

/// Every linked registration of `T`, summarized.
pub fn all_summaries<T: Registration>() -> Vec<Summary> {
    crate::all_registered::<T>()
        .map(Contribution::summary)
        .collect()
}

/// The registrations of `T` linked at compile time (`inventory::submit!`).
pub fn linked_summaries<T: Registration>() -> Vec<Summary> {
    inventory::iter::<T>
        .into_iter()
        .map(Contribution::summary)
        .collect()
}

/// Every registration linked at compile time rather than added by a plugin
/// build, summarized: the engine's own (each origin `engine:<package>`).
pub fn linked_contributions() -> Vec<Summary> {
    inventory::iter::<ContributionSource>
        .into_iter()
        .flat_map(|source| (source.linked)())
        .collect()
}

/// The registrations of `T` its registry hands out ([`crate::registered`]),
/// summarized.
pub fn active_summaries<T: Registration>() -> Vec<Summary> {
    crate::registered::<T>()
        .map(Contribution::summary)
        .collect()
}

/// Every linked registration of every type, summarized, by category then
/// origin then id: what the binary's plugins and the engine contribute,
/// whether or not their plugin is enabled ([`crate::origin_enabled`] says
/// which count).
pub fn contributions() -> Vec<Summary> {
    let mut all: Vec<Summary> = inventory::iter::<ContributionSource>
        .into_iter()
        .flat_map(|source| (source.all)())
        .collect();
    all.sort_by(|a, b| (a.category, a.origin, &a.id).cmp(&(b.category, b.origin, &b.id)));
    all
}

/// The registrations every registry hands out now (those passing the
/// gate), summarized, in [`contributions`]' order: with a plugin disabled,
/// none of its registrations is here.
pub fn active_contributions() -> Vec<Summary> {
    let mut active: Vec<Summary> = inventory::iter::<ContributionSource>
        .into_iter()
        .flat_map(|source| (source.active)())
        .collect();
    active.sort_by(|a, b| (a.category, a.origin, &a.id).cmp(&(b.category, b.origin, &b.id)));
    active
}
